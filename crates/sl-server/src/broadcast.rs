//! Admin broadcasts: one-way announcements shown as a banner on every client.

use rusqlite::params;
use sl_proto::panel::{BroadcastInput, BroadcastRow};
use sl_proto::wire::Event;
use sl_proto::{Broadcast, ErrorCode, Severity};
use uuid::Uuid;

use crate::Engine;
use crate::db::now;
use crate::error::{AppResult, api, api_detail};

fn sev_str(s: Severity) -> &'static str {
    match s {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Critical => "critical",
    }
}

fn sev(s: &str) -> Severity {
    match s {
        "warning" => Severity::Warning,
        "critical" => Severity::Critical,
        _ => Severity::Info,
    }
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Broadcast, Option<i64>)> {
    Ok((
        Broadcast {
            id: r.get(0)?,
            title: r.get(1)?,
            body: r.get(2)?,
            severity: sev(&r.get::<_, String>(3)?),
            created_at: r.get(4)?,
            expires_at: r.get(5)?,
        },
        r.get(6)?,
    ))
}

/// Broadcasts currently live.
pub async fn active(engine: &Engine) -> AppResult<Vec<Broadcast>> {
    engine
        .db()
        .read(|c| {
            let now = now();
            let mut st = c.prepare(
                "SELECT id, title, body, severity, created_at, expires_at, ended_at FROM broadcasts
                 WHERE ended_at IS NULL AND (expires_at IS NULL OR expires_at > ?1) ORDER BY created_at DESC",
            )?;
            let rows = st.query_map([now], row)?;
            Ok(rows.map(|r| r.map(|(b, _)| b)).collect::<Result<_, _>>()?)
        })
        .await
}

pub async fn list(engine: &Engine) -> AppResult<Vec<BroadcastRow>> {
    engine
        .db()
        .read(|c| {
            let now = now();
            let mut st = c.prepare(
                "SELECT id, title, body, severity, created_at, expires_at, ended_at FROM broadcasts ORDER BY created_at DESC LIMIT 200",
            )?;
            let rows = st.query_map([], row)?;
            let mut out = Vec::new();
            for r in rows {
                let (b, ended_at) = r?;
                let active = ended_at.is_none() && b.expires_at.is_none_or(|e| e > now);
                out.push(BroadcastRow { broadcast: b, active, ended_at });
            }
            Ok(out)
        })
        .await
}

pub async fn create(engine: &Engine, input: BroadcastInput) -> AppResult<Broadcast> {
    let title = input.title.trim().to_string();
    let body = input.body.trim().to_string();
    if title.is_empty() || title.chars().count() > 120 || body.chars().count() > 2000 {
        return api_detail(
            ErrorCode::BadRequest,
            "title 1–120 characters, message up to 2000",
        );
    }
    let now = now();
    let b = Broadcast {
        id: Uuid::now_v7(),
        title,
        body,
        severity: input.severity,
        created_at: now,
        expires_at: input
            .expires_in_secs
            .filter(|s| *s > 0)
            .map(|s| now + s * 1000),
    };
    let b2 = b.clone();
    engine
        .db()
        .write(move |c| {
            c.execute(
                "INSERT INTO broadcasts (id, title, body, severity, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![b2.id, b2.title, b2.body, sev_str(b2.severity), b2.created_at, b2.expires_at],
            )?;
            Ok(())
        })
        .await?;
    engine.hub().event_all(Event::Broadcast(b.clone()));
    engine.audit(
        "broadcast.create",
        &b.title,
        format!("severity={}", sev_str(b.severity)),
    );
    Ok(b)
}

pub async fn end(engine: &Engine, id: Uuid) -> AppResult<()> {
    let n = engine
        .db()
        .write(move |c| {
            Ok(c.execute(
                "UPDATE broadcasts SET ended_at = ?1 WHERE id = ?2 AND ended_at IS NULL",
                params![now(), id],
            )?)
        })
        .await?;
    if n == 0 {
        return api(ErrorCode::NotFound);
    }
    engine.hub().event_all(Event::BroadcastEnded { id });
    engine.audit("broadcast.end", &id.to_string(), "");
    Ok(())
}

/// Close broadcasts whose expiry has passed and tell clients.
pub async fn expire(engine: &Engine) -> AppResult<()> {
    let ids: Vec<Uuid> = engine
        .db()
        .write(|c| {
            let now = now();
            let mut st = c.prepare("SELECT id FROM broadcasts WHERE ended_at IS NULL AND expires_at IS NOT NULL AND expires_at <= ?1")?;
            let ids: Vec<Uuid> = st.query_map([now], |r| r.get(0))?.collect::<Result<_, _>>()?;
            for id in &ids {
                c.execute("UPDATE broadcasts SET ended_at = ?1 WHERE id = ?2", params![now, id])?;
            }
            Ok(ids)
        })
        .await?;
    for id in ids {
        engine.hub().event_all(Event::BroadcastEnded { id });
    }
    Ok(())
}
