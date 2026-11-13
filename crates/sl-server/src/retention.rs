//! Background jobs: retention, backups, broadcast expiry, debug-mode timeout.

use std::time::Duration;

use sl_proto::panel::BackupRow;

use crate::Engine;
use crate::db::now;
use crate::error::AppResult;

pub fn spawn(engine: Engine) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        let mut minutes = 0u64;
        loop {
            tick.tick().await;
            // Every 30 s: debug-mode timeout, broadcast expiry.
            engine.tel().logs.expire_debug(now());
            if let Err(e) = crate::broadcast::expire(&engine).await {
                tracing::warn!(error = %e, "broadcast expiry failed");
            }
            minutes += 1;
            // Every ~10 minutes: retention and backup scheduling.
            if minutes % 20 == 1 {
                if let Err(e) = cleanup(&engine).await {
                    tracing::warn!(error = %e, "retention cleanup failed");
                }
                if let Err(e) = maybe_backup(&engine).await {
                    tracing::warn!(error = %e, "scheduled backup failed");
                }
            }
        }
    })
}

async fn cleanup(engine: &Engine) -> AppResult<()> {
    let cfg = engine.config();
    let env_cut = now() - i64::from(cfg.envelope_retention_days) * 86_400_000;
    let ev_cut = now() - i64::from(cfg.connection_event_retention_days) * 86_400_000;
    let report_cut = now() - 30 * 86_400_000;
    let (envs, events, sessions) = engine
        .db()
        .write(move |c| {
            let envs = crate::delivery::purge_old(c, env_cut)?;
            let events = c.execute("DELETE FROM connection_events WHERE ts < ?1", [ev_cut])?;
            let sessions = c.execute("DELETE FROM sessions WHERE expires_at < ?1", [now()])?;
            c.execute(
                "DELETE FROM health_reports WHERE received_at < ?1",
                [report_cut],
            )?;
            c.execute("DELETE FROM welcomes WHERE created_at < ?1", [env_cut])?;
            Ok((envs, events, sessions))
        })
        .await?;
    if envs + events + sessions > 0 {
        tracing::info!(
            envelopes = envs,
            connection_events = events,
            sessions,
            "retention cleanup"
        );
    }
    Ok(())
}

async fn maybe_backup(engine: &Engine) -> AppResult<()> {
    let cfg = engine.config();
    // Skip the first hour after start so a crash-looping server doesn't churn backups.
    if now() - engine.started_at() < 3_600_000 {
        return Ok(());
    }
    let interval = i64::from(cfg.backup_interval_hours.max(1)) * 3_600_000;
    let last = crate::admin::list_backup_files(&engine.paths().backups)
        .first()
        .map(|b| b.created_at)
        .unwrap_or(0);
    if now() - last >= interval {
        backup(engine).await?;
    }
    Ok(())
}

/// Take a consistent backup now and prune old ones.
pub async fn backup(engine: &Engine) -> AppResult<BackupRow> {
    let dir = engine.paths().backups.clone();
    std::fs::create_dir_all(&dir)?;
    let name = format!("spareline-{}.db", now());
    let path = dir.join(&name);
    engine.db().backup_to(path.clone()).await?;
    let keep = engine.config().backup_keep.max(1) as usize;
    let all = crate::admin::list_backup_files(&dir);
    for old in all.iter().skip(keep) {
        let _ = std::fs::remove_file(dir.join(&old.file_name));
    }
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    tracing::info!(file = %name, size, "backup written");
    Ok(BackupRow {
        file_name: name,
        created_at: now(),
        size,
    })
}
