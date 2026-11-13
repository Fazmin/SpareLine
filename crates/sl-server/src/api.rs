//! Authenticated request dispatch.

use rusqlite::params;
use sl_proto::wire::{Event, Request, Response};
use sl_proto::{ClientHealthReport, Directory, DirectoryUser, ErrorCode, Presence, SelfUser};

use crate::Engine;
use crate::db::now;
use crate::delivery::{self, Caller};
use crate::error::{AppResult, api};
use crate::hub::ConnId;

/// Per-connection session context.
#[derive(Debug, Clone)]
pub struct Session {
    pub conn_id: ConnId,
    pub caller: Caller,
    pub token_hash: String,
}

pub async fn dispatch(engine: &Engine, s: &Session, req: Request) -> AppResult<Response> {
    let me = s.caller;
    match req {
        Request::Directory => directory(engine, me),
        Request::Conversations => delivery::conversations(engine, me).await,
        Request::Sync { limit } => delivery::sync(engine, me, limit).await,
        Request::Ack { conversation, seq } => delivery::ack(engine, me, conversation, seq).await,
        Request::UploadKeyPackages {
            key_packages,
            last_resort,
        } => delivery::upload_key_packages(engine, me, key_packages, last_resort).await,
        Request::ClaimKeyPackages { users } => {
            delivery::claim_key_packages(engine, me, users).await
        }
        Request::CreateConversation {
            kind,
            scope_group,
            members,
            mls_group_id,
            epoch,
            welcome,
            welcome_devices,
            group_info,
        } => {
            delivery::create_conversation(
                engine,
                me,
                kind,
                scope_group,
                members,
                mls_group_id,
                epoch,
                welcome,
                welcome_devices,
                group_info,
            )
            .await
        }
        Request::Commit {
            conversation,
            epoch,
            commit,
            welcome,
            welcome_devices,
            added_users,
            removed_devices,
            group_info,
        } => {
            delivery::commit(
                engine,
                me,
                conversation,
                epoch,
                commit,
                welcome,
                welcome_devices,
                added_users,
                removed_devices,
                group_info,
            )
            .await
        }
        Request::Send {
            conversation,
            epoch,
            ciphertext,
        } => delivery::send(engine, me, conversation, epoch, ciphertext).await,
        Request::Fetch {
            conversation,
            after_seq,
            limit,
        } => delivery::fetch(engine, me, conversation, after_seq, limit).await,
        Request::GroupInfo { conversation } => delivery::group_info(engine, me, conversation).await,
        Request::ExternalJoin {
            conversation,
            epoch,
            commit,
            group_info,
        } => delivery::external_join(engine, me, conversation, epoch, commit, group_info).await,
        Request::Purge { conversation, seq } => {
            delivery::purge(engine, me, conversation, seq).await
        }
        Request::Leave { conversation } => delivery::leave(engine, me, conversation).await,
        Request::MarkRead { conversation, seq } => {
            delivery::mark_read(engine, me, conversation, seq).await
        }
        Request::SetPresence { presence } => set_presence(engine, s, presence),
        Request::SetDisplayName { display_name } => {
            set_display_name(engine, me, display_name).await
        }
        Request::ChangePassword { current, new } => change_password(engine, s, current, new).await,
        Request::ReportHealth(report) => report_health(engine, me, report).await,
        Request::Logout => logout(engine, s).await,
    }
}

fn directory(engine: &Engine, me: Caller) -> AppResult<Response> {
    let vis = engine.dir().get();
    let groups = vis.group_refs_of(&me.user);
    let mine = vis.groups_of(&me.user);
    let mut users: Vec<DirectoryUser> = vis
        .visible_users(&me.user)
        .into_iter()
        .filter_map(|u| {
            let b = vis.users.get(&u)?;
            Some(DirectoryUser {
                id: u,
                username: b.username.clone(),
                display_name: b.display_name.clone(),
                groups: vis.groups_of(&u).intersection(&mine).copied().collect(),
                presence: engine.hub().presence(&u),
            })
        })
        .collect();
    users.sort_by_key(|a| a.display_name.to_lowercase());
    Ok(Response::Directory(Directory { groups, users }))
}

/// Broadcast a user's aggregate presence to everyone who can see them.
pub fn broadcast_presence(engine: &Engine, user: sl_proto::UserId) {
    let presence = engine.hub().presence(&user);
    let vis = engine.dir().get();
    let viewers = vis.visible_users(&user);
    engine
        .hub()
        .event_users(viewers.iter(), Event::Presence { user, presence });
}

fn set_presence(engine: &Engine, s: &Session, presence: Presence) -> AppResult<Response> {
    let presence = if presence == Presence::Offline {
        Presence::Away
    } else {
        presence
    };
    let before = engine.hub().presence(&s.caller.user);
    if let Some(c) = engine.hub().list().into_iter().find(|c| c.id == s.conn_id) {
        *c.presence.lock() = presence;
    }
    if engine.hub().presence(&s.caller.user) != before {
        broadcast_presence(engine, s.caller.user);
    }
    Ok(Response::Ok)
}

async fn set_display_name(
    engine: &Engine,
    me: Caller,
    display_name: String,
) -> AppResult<Response> {
    let name = crate::auth::validate_display_name(&display_name)?;
    let n2 = name.clone();
    engine
        .db()
        .write(move |c| {
            c.execute(
                "UPDATE users SET display_name = ?1 WHERE id = ?2",
                params![n2, me.user],
            )?;
            Ok(())
        })
        .await?;
    engine.reload_directory().await?;
    let vis = engine.dir().get();
    let viewers = vis.visible_users(&me.user);
    engine
        .hub()
        .event_users(viewers.iter(), Event::DirectoryChanged);
    if let Some(b) = vis.users.get(&me.user) {
        let profile = SelfUser {
            id: me.user,
            username: b.username.clone(),
            display_name: name,
            groups: vis.group_refs_of(&me.user),
        };
        engine
            .hub()
            .event_users(std::iter::once(&me.user), Event::Profile(profile));
    }
    Ok(Response::Ok)
}

async fn change_password(
    engine: &Engine,
    s: &Session,
    current: String,
    new: String,
) -> AppResult<Response> {
    crate::auth::validate_password(&new)?;
    let me = s.caller;
    let hash: String = engine
        .db()
        .read(move |c| {
            Ok(c.query_row(
                "SELECT password_hash FROM users WHERE id = ?1",
                [me.user],
                |r| r.get(0),
            )?)
        })
        .await?;
    if !crate::auth::verify_password_async(hash, current).await? {
        engine.tel().metrics.record_auth_failure();
        return api(ErrorCode::InvalidCredentials);
    }
    let new_hash = crate::auth::hash_password_async(new).await?;
    let keep = s.token_hash.clone();
    engine
        .db()
        .write(move |c| {
            c.execute(
                "UPDATE users SET password_hash = ?1 WHERE id = ?2",
                params![new_hash, me.user],
            )?;
            // Sign out other sessions; keep this one.
            c.execute(
                "DELETE FROM sessions WHERE user_id = ?1 AND token_hash != ?2",
                params![me.user, keep],
            )?;
            Ok(())
        })
        .await?;
    for c in engine.hub().list() {
        if c.user_id == me.user && c.id != s.conn_id {
            let _ =
                c.tx.try_send(crate::hub::Outbound::Close(sl_proto::ApiError::new(
                    ErrorCode::SessionRevoked,
                )));
        }
    }
    tracing::info!(target: "sl_server::auth", user = %me.user, "password changed");
    Ok(Response::Ok)
}

async fn report_health(
    engine: &Engine,
    me: Caller,
    report: ClientHealthReport,
) -> AppResult<Response> {
    // Bound the size; these are small summaries.
    let mut report = report;
    report.failures.truncate(20);
    report.last_error.truncate(500);
    let text = serde_json::to_string(&report)?;
    engine
        .db()
        .write(move |c| {
            c.execute(
                "INSERT INTO health_reports (received_at, user_id, device_id, report) VALUES (?1, ?2, ?3, ?4)",
                params![now(), me.user, me.device, text],
            )?;
            Ok(())
        })
        .await?;
    tracing::info!(target: "sl_server::ws", user = %me.user, "client health report received");
    Ok(Response::Ok)
}

async fn logout(engine: &Engine, s: &Session) -> AppResult<Response> {
    let h = s.token_hash.clone();
    engine
        .db()
        .write(move |c| {
            c.execute("DELETE FROM sessions WHERE token_hash = ?1", [h])?;
            Ok(())
        })
        .await?;
    Ok(Response::Ok)
}
