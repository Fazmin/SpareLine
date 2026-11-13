//! MLS Delivery Service.
//!
//! The server never holds MLS state and never sees plaintext. It:
//! - orders handshake messages per conversation (a commit is accepted only for the
//!   current epoch: first come, first served),
//! - routes ciphertext to the devices that are leaves of each group,
//! - enforces group isolation on every add,
//! - tells members when the authoritative membership changes (`ConversationInfo.devices`)
//!   so that a member's client commits the matching MLS removals.
//!
//! Envelope storage is crate-private: the admin module only gets aggregate counts.

use std::collections::{BTreeSet, HashSet};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sl_proto::wire::{Event, PendingWelcome, Response, ServerFrame, SyncBatch};
use sl_proto::{
    Bytes, ClaimedKeyPackage, ConversationId, ConversationInfo, ConversationKind, DeviceId,
    DeviceRef, Envelope, EnvelopeKind, ErrorCode, GroupId, UserId,
};
use uuid::Uuid;

use crate::Engine;
use crate::db::now;
use crate::directory::Visibility;
use crate::error::{AppError, AppResult, api, api_detail};
use crate::hub::encode;

const MAX_GROUP_MEMBERS: usize = 500;
const MAX_KEY_PACKAGES: u32 = 200;
pub const KEY_PACKAGES_LOW: u32 = 20;
const MAX_BLOB: usize = 1024 * 1024;

/// The authenticated caller of a request.
#[derive(Debug, Clone, Copy)]
pub struct Caller {
    pub user: UserId,
    pub device: DeviceId,
}

fn kind_str(k: ConversationKind) -> &'static str {
    match k {
        ConversationKind::Direct => "direct",
        ConversationKind::Group => "group",
    }
}

fn env_kind(s: &str) -> EnvelopeKind {
    match s {
        "commit" => EnvelopeKind::Commit,
        "proposal" => EnvelopeKind::Proposal,
        _ => EnvelopeKind::Application,
    }
}

fn check_blob(b: &Bytes) -> AppResult<()> {
    if b.is_empty() || b.len() > MAX_BLOB {
        api_detail(ErrorCode::BadRequest, "payload size")
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Conversation views
// ---------------------------------------------------------------------------------------

struct ConvRow {
    kind: ConversationKind,
    scope_group: Option<GroupId>,
    mls_group_id: Vec<u8>,
    epoch: u64,
    last_seq: u64,
    created_by: UserId,
    created_at: i64,
}

fn conv_row(c: &Connection, id: ConversationId) -> AppResult<Option<ConvRow>> {
    Ok(c.query_row(
        "SELECT kind, scope_group, mls_group_id, epoch, last_seq, created_by, created_at FROM conversations WHERE id = ?1",
        [id],
        |r| {
            Ok(ConvRow {
                kind: if r.get::<_, String>(0)? == "group" { ConversationKind::Group } else { ConversationKind::Direct },
                scope_group: r.get(1)?,
                mls_group_id: r.get(2)?,
                epoch: r.get::<_, i64>(3)? as u64,
                last_seq: r.get::<_, i64>(4)? as u64,
                created_by: r.get(5)?,
                created_at: r.get(6)?,
            })
        },
    )
    .optional()?)
}

pub(crate) fn members_of(c: &Connection, id: ConversationId) -> AppResult<Vec<UserId>> {
    let mut st = c.prepare_cached(
        "SELECT user_id FROM conversation_members WHERE conversation = ?1 ORDER BY joined_at",
    )?;
    let rows = st.query_map([id], |r| r.get::<_, UserId>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Active devices of active members: who is *allowed* in the MLS group.
fn allowed_devices(
    c: &Connection,
    vis: &Visibility,
    id: ConversationId,
) -> AppResult<Vec<DeviceRef>> {
    let mut st = c.prepare_cached(
        "SELECT d.user_id, d.id FROM devices d
         JOIN conversation_members m ON m.user_id = d.user_id
         WHERE m.conversation = ?1 AND d.revoked_at IS NULL ORDER BY d.created_at",
    )?;
    let rows = st.query_map([id], |r| {
        Ok(DeviceRef {
            user: r.get(0)?,
            device: r.get(1)?,
        })
    })?;
    let mut out = Vec::new();
    for r in rows {
        let d = r?;
        if vis.is_active(&d.user) {
            out.push(d);
        }
    }
    Ok(out)
}

/// Devices that are currently leaves AND allowed: where ciphertext gets routed.
fn routable_devices(
    c: &Connection,
    vis: &Visibility,
    id: ConversationId,
) -> AppResult<Vec<DeviceId>> {
    let allowed: HashSet<DeviceId> = allowed_devices(c, vis, id)?
        .into_iter()
        .map(|d| d.device)
        .collect();
    let mut st =
        c.prepare_cached("SELECT device_id FROM conversation_devices WHERE conversation = ?1")?;
    let rows = st.query_map([id], |r| r.get::<_, DeviceId>(0))?;
    let mut out = Vec::new();
    for r in rows {
        let d = r?;
        if allowed.contains(&d) {
            out.push(d);
        }
    }
    Ok(out)
}

fn in_tree(c: &Connection, id: ConversationId, device: DeviceId) -> AppResult<bool> {
    Ok(c.query_row(
        "SELECT 1 FROM conversation_devices WHERE conversation = ?1 AND device_id = ?2",
        params![id, device],
        |_| Ok(true),
    )
    .optional()?
    .unwrap_or(false))
}

fn is_member(c: &Connection, id: ConversationId, user: UserId) -> AppResult<bool> {
    Ok(c.query_row(
        "SELECT 1 FROM conversation_members WHERE conversation = ?1 AND user_id = ?2",
        params![id, user],
        |_| Ok(true),
    )
    .optional()?
    .unwrap_or(false))
}

fn read_only(vis: &Visibility, kind: ConversationKind, members: &[UserId]) -> bool {
    match kind {
        ConversationKind::Direct => {
            if members.len() != 2 {
                return true;
            }
            !vis.is_active(&members[0])
                || !vis.is_active(&members[1])
                || vis.shared_groups(&members[0], &members[1]).is_empty()
        }
        ConversationKind::Group => false,
    }
}

pub(crate) fn conversation_info(
    c: &Connection,
    vis: &Visibility,
    id: ConversationId,
) -> AppResult<Option<ConversationInfo>> {
    let Some(row) = conv_row(c, id)? else {
        return Ok(None);
    };
    let members = members_of(c, id)?;
    let devices = allowed_devices(c, vis, id)?;
    Ok(Some(ConversationInfo {
        id,
        kind: row.kind,
        scope_group: row.scope_group,
        read_only: read_only(vis, row.kind, &members),
        members,
        devices,
        mls_group_id: Bytes(row.mls_group_id),
        epoch: row.epoch,
        last_seq: row.last_seq,
        created_by: row.created_by,
        created_at: row.created_at,
    }))
}

pub async fn conversations(engine: &Engine, me: Caller) -> AppResult<Response> {
    let vis = engine.dir().get();
    let list = engine
        .db()
        .read(move |c| {
            let mut st =
                c.prepare("SELECT conversation FROM conversation_members WHERE user_id = ?1")?;
            let ids: Vec<ConversationId> = st
                .query_map([me.user], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let mut out = Vec::with_capacity(ids.len());
            for id in ids {
                if let Some(info) = conversation_info(c, &vis, id)? {
                    out.push(info);
                }
            }
            Ok(out)
        })
        .await?;
    Ok(Response::Conversations(list))
}

// ---------------------------------------------------------------------------------------
// Key packages
// ---------------------------------------------------------------------------------------

pub async fn upload_key_packages(
    engine: &Engine,
    me: Caller,
    kps: Vec<Bytes>,
    last_resort: Option<Bytes>,
) -> AppResult<Response> {
    for k in kps.iter().chain(last_resort.iter()) {
        check_blob(k)?;
    }
    engine
        .db()
        .write(move |c| {
            let tx = c.transaction()?;
            let have: u32 = tx.query_row(
                "SELECT COUNT(*) FROM key_packages WHERE device_id = ?1 AND last_resort = 0",
                [me.device],
                |r| r.get(0),
            )?;
            let room = MAX_KEY_PACKAGES.saturating_sub(have) as usize;
            let now = now();
            for k in kps.into_iter().take(room) {
                tx.execute(
                    "INSERT INTO key_packages (device_id, data, last_resort, created_at) VALUES (?1, ?2, 0, ?3)",
                    params![me.device, k.0, now],
                )?;
            }
            if let Some(lr) = last_resort {
                tx.execute("DELETE FROM key_packages WHERE device_id = ?1 AND last_resort = 1", [me.device])?;
                tx.execute(
                    "INSERT INTO key_packages (device_id, data, last_resort, created_at) VALUES (?1, ?2, 1, ?3)",
                    params![me.device, lr.0, now],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(Response::Ok)
}

pub async fn claim_key_packages(
    engine: &Engine,
    me: Caller,
    users: Vec<UserId>,
) -> AppResult<Response> {
    let vis = engine.dir().get();
    let users: BTreeSet<UserId> = users.into_iter().collect();
    if users.len() > MAX_GROUP_MEMBERS {
        return api(ErrorCode::BadRequest);
    }
    for u in &users {
        if !vis.can_see(&me.user, u) || !vis.is_active(u) {
            return api(ErrorCode::NotAuthorized);
        }
    }
    let (claimed, low) = engine
        .db()
        .write(move |c| {
            let tx = c.transaction()?;
            let mut claimed = Vec::new();
            let mut low = Vec::new();
            for u in &users {
                let devices: Vec<DeviceId> = {
                    let mut st = tx.prepare(
                        "SELECT id FROM devices WHERE user_id = ?1 AND revoked_at IS NULL",
                    )?;
                    st.query_map([u], |r| r.get(0))?.collect::<Result<_, _>>()?
                };
                for d in devices {
                    if d == me.device {
                        continue;
                    }
                    let kp: Option<(i64, Vec<u8>, bool)> = tx
                        .query_row(
                            "SELECT id, data, last_resort FROM key_packages WHERE device_id = ?1
                             ORDER BY last_resort ASC, id ASC LIMIT 1",
                            [d],
                            |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
                        )
                        .optional()?;
                    let Some((id, data, last_resort)) = kp else {
                        continue;
                    };
                    if !last_resort {
                        tx.execute("DELETE FROM key_packages WHERE id = ?1", [id])?;
                    }
                    claimed.push(ClaimedKeyPackage {
                        user: *u,
                        device: d,
                        key_package: Bytes(data),
                    });
                    let remaining = crate::auth::key_packages_remaining(&tx, d)?;
                    if remaining < KEY_PACKAGES_LOW {
                        low.push((d, remaining));
                    }
                }
            }
            tx.commit()?;
            Ok((claimed, low))
        })
        .await?;
    for (d, remaining) in low {
        engine.hub().send_device(
            &d,
            encode(&ServerFrame::Event(Event::KeyPackagesLow { remaining })),
        );
    }
    Ok(Response::KeyPackages(claimed))
}

// ---------------------------------------------------------------------------------------
// Conversation lifecycle
// ---------------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub async fn create_conversation(
    engine: &Engine,
    me: Caller,
    kind: ConversationKind,
    scope_group: Option<GroupId>,
    members: Vec<UserId>,
    mls_group_id: Bytes,
    epoch: u64,
    welcome: Option<Bytes>,
    welcome_devices: Vec<DeviceId>,
    group_info: Bytes,
) -> AppResult<Response> {
    check_blob(&group_info)?;
    if let Some(w) = &welcome {
        check_blob(w)?;
    }
    if mls_group_id.is_empty() || mls_group_id.len() > 64 {
        return api_detail(ErrorCode::BadRequest, "group id");
    }
    let vis = engine.dir().get();
    let mut set: BTreeSet<UserId> = members.into_iter().collect();
    set.insert(me.user);
    if set.len() > MAX_GROUP_MEMBERS {
        return api_detail(ErrorCode::BadRequest, "too many members");
    }
    for u in &set {
        if !vis.is_active(u) {
            return api(ErrorCode::NotAuthorized);
        }
    }
    let dm_key = match kind {
        ConversationKind::Direct => {
            if set.len() != 2 || scope_group.is_some() {
                return api_detail(
                    ErrorCode::BadRequest,
                    "a direct chat has exactly two people",
                );
            }
            let v: Vec<UserId> = set.iter().copied().collect();
            if vis.shared_groups(&v[0], &v[1]).is_empty() {
                return api(ErrorCode::NotAuthorized);
            }
            Some(format!("{}:{}", v[0], v[1]))
        }
        ConversationKind::Group => {
            let Some(g) = scope_group else {
                return api_detail(ErrorCode::BadRequest, "group chats need a group");
            };
            if !set.iter().all(|u| vis.is_member(u, &g)) {
                return api(ErrorCode::NotAuthorized);
            }
            None
        }
    };
    if welcome.is_some() == welcome_devices.is_empty() {
        return api_detail(
            ErrorCode::BadRequest,
            "welcome and welcome_devices must be given together",
        );
    }
    let id = Uuid::now_v7();
    let set_c = set.clone();
    let vis_c = vis.clone();
    let wd = welcome_devices.clone();
    let welcome_c = welcome.clone();
    let info = engine
        .db()
        .write(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some(k) = &dm_key {
                let existing: Option<ConversationId> =
                    tx.query_row("SELECT id FROM conversations WHERE dm_key = ?1", [k], |r| r.get(0)).optional()?;
                if let Some(e) = existing {
                    return api_detail(ErrorCode::AlreadyExists, e.to_string());
                }
            }
            // Welcome targets must be active devices of members (never the caller's own device).
            for d in &wd {
                let owner: Option<UserId> = tx
                    .query_row("SELECT user_id FROM devices WHERE id = ?1 AND revoked_at IS NULL", [d], |r| r.get(0))
                    .optional()?;
                match owner {
                    Some(u) if set_c.contains(&u) && *d != me.device => {}
                    _ => return api(ErrorCode::NotAuthorized),
                }
            }
            let now = now();
            tx.execute(
                "INSERT INTO conversations (id, kind, scope_group, mls_group_id, epoch, last_seq, created_by, created_at, group_info, dm_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9)",
                params![id, kind_str(kind), scope_group, mls_group_id.0, epoch as i64, me.user, now, group_info.0, dm_key],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                    AppError::Api(sl_proto::ApiError::with_detail(ErrorCode::AlreadyExists, "mls group id"))
                }
                other => other.into(),
            })?;
            for u in &set_c {
                tx.execute(
                    "INSERT INTO conversation_members (conversation, user_id, joined_at) VALUES (?1, ?2, ?3)",
                    params![id, u, now],
                )?;
            }
            for d in std::iter::once(&me.device).chain(wd.iter()) {
                tx.execute(
                    "INSERT OR IGNORE INTO conversation_devices (conversation, device_id, joined_at) VALUES (?1, ?2, ?3)",
                    params![id, d, now],
                )?;
                tx.execute(
                    "INSERT OR REPLACE INTO device_cursors (device_id, conversation, seq) VALUES (?1, ?2, 0)",
                    params![d, id],
                )?;
            }
            if let Some(w) = &welcome_c {
                for d in &wd {
                    tx.execute(
                        "INSERT INTO welcomes (device_id, conversation, data, created_at) VALUES (?1, ?2, ?3, ?4)",
                        params![d, id, w.0, now],
                    )?;
                }
            }
            let info = conversation_info(&tx, &vis_c, id)?.ok_or(AppError::Api(ErrorCode::Internal.into()))?;
            tx.commit()?;
            Ok(info)
        })
        .await?;
    tracing::debug!(target: "sl_server::delivery", conversation = %id, members = info.members.len(), "conversation created");
    if let Some(w) = welcome {
        let frame = encode(&ServerFrame::Event(Event::Welcome(PendingWelcome {
            conversation: info.clone(),
            welcome: w,
            start_seq: 0,
        })));
        engine.hub().send_devices(welcome_devices.iter(), frame);
    }
    let others: Vec<UserId> = info
        .members
        .iter()
        .copied()
        .filter(|u| *u != me.user)
        .collect();
    engine
        .hub()
        .event_users(others.iter(), Event::Conversation(info.clone()));
    Ok(Response::Conversation(info))
}

/// A member commit (adds, removes, updates).
#[allow(clippy::too_many_arguments)]
pub async fn commit(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
    epoch: u64,
    commit: Bytes,
    welcome: Option<Bytes>,
    welcome_devices: Vec<DeviceId>,
    added_users: Vec<UserId>,
    removed_devices: Vec<DeviceId>,
    group_info: Bytes,
) -> AppResult<Response> {
    check_blob(&commit)?;
    check_blob(&group_info)?;
    if let Some(w) = &welcome {
        check_blob(w)?;
    }
    if welcome.is_some() == welcome_devices.is_empty() {
        return api_detail(
            ErrorCode::BadRequest,
            "welcome and welcome_devices must be given together",
        );
    }
    let vis = engine.dir().get();
    let vis_c = vis.clone();
    let wd = welcome_devices.clone();
    let welcome_c = welcome.clone();
    let (info, seq, ts, targets, removed_users) = engine
        .db()
        .write(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let Some(row) = conv_row(&tx, conversation)? else { return api(ErrorCode::NotFound) };
            if !is_member(&tx, conversation, me.user)? || !in_tree(&tx, conversation, me.device)? {
                return api(ErrorCode::NotAuthorized);
            }
            if row.epoch != epoch {
                let mut e = sl_proto::ApiError::new(ErrorCode::StaleEpoch);
                e.epoch = Some(row.epoch);
                return Err(AppError::Api(e));
            }
            let members: BTreeSet<UserId> = members_of(&tx, conversation)?.into_iter().collect();
            let added: BTreeSet<UserId> = added_users.iter().copied().filter(|u| !members.contains(u)).collect();
            if !added.is_empty() {
                match row.kind {
                    ConversationKind::Direct => return api_detail(ErrorCode::BadRequest, "cannot add people to a direct chat"),
                    ConversationKind::Group => {
                        let Some(g) = row.scope_group else { return api(ErrorCode::ReadOnly) };
                        if members.len() + added.len() > MAX_GROUP_MEMBERS {
                            return api_detail(ErrorCode::BadRequest, "too many members");
                        }
                        for u in &added {
                            if !vis_c.is_member(u, &g) || !vis_c.is_active(u) || !vis_c.can_see(&me.user, u) {
                                return api(ErrorCode::NotAuthorized);
                            }
                        }
                    }
                }
            }
            for d in &wd {
                let owner: Option<UserId> = tx
                    .query_row("SELECT user_id FROM devices WHERE id = ?1 AND revoked_at IS NULL", [d], |r| r.get(0))
                    .optional()?;
                match owner {
                    Some(u) if members.contains(&u) || added.contains(&u) => {}
                    _ => return api(ErrorCode::NotAuthorized),
                }
            }
            if read_only(&vis_c, row.kind, &members.iter().copied().collect::<Vec<_>>()) && !added.is_empty() {
                return api(ErrorCode::ReadOnly);
            }
            // Route the commit to the group as it was before this commit.
            let targets_before: Vec<DeviceId> = routable_devices(&tx, &vis_c, conversation)?
                .into_iter()
                .filter(|d| *d != me.device)
                .collect();
            let now = now();
            let seq = row.last_seq + 1;
            for u in &added {
                tx.execute(
                    "INSERT OR IGNORE INTO conversation_members (conversation, user_id, joined_at) VALUES (?1, ?2, ?3)",
                    params![conversation, u, now],
                )?;
            }
            let mut removed_users = Vec::new();
            for d in &removed_devices {
                if *d == me.device {
                    return api_detail(ErrorCode::BadRequest, "use Leave to remove yourself");
                }
                tx.execute("DELETE FROM conversation_devices WHERE conversation = ?1 AND device_id = ?2", params![conversation, d])?;
                tx.execute("DELETE FROM device_cursors WHERE conversation = ?2 AND device_id = ?1", params![d, conversation])?;
                // Track owners: in a group chat, removing every device of a member who is
                // still allowed means "remove this person from the chat".
                let owner: Option<UserId> =
                    tx.query_row("SELECT user_id FROM devices WHERE id = ?1", [d], |r| r.get(0)).optional()?;
                if let Some(u) = owner {
                    removed_users.push(u);
                }
            }
            let mut gone = Vec::new();
            if row.kind == ConversationKind::Group {
                for u in removed_users.iter().copied().collect::<BTreeSet<_>>() {
                    if added.contains(&u) || u == me.user {
                        continue;
                    }
                    let left: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM conversation_devices cd JOIN devices d ON d.id = cd.device_id
                         WHERE cd.conversation = ?1 AND d.user_id = ?2",
                        params![conversation, u],
                        |r| r.get(0),
                    )?;
                    let still_allowed = vis_c.is_active(&u) && members.contains(&u);
                    if left == 0 && still_allowed {
                        tx.execute("DELETE FROM conversation_members WHERE conversation = ?1 AND user_id = ?2", params![conversation, u])?;
                        gone.push(u);
                    }
                }
            }
            for d in &wd {
                tx.execute(
                    "INSERT OR IGNORE INTO conversation_devices (conversation, device_id, joined_at) VALUES (?1, ?2, ?3)",
                    params![conversation, d, now],
                )?;
                tx.execute(
                    "INSERT OR REPLACE INTO device_cursors (device_id, conversation, seq) VALUES (?1, ?2, ?3)",
                    params![d, conversation, seq as i64],
                )?;
            }
            tx.execute(
                "INSERT INTO envelopes (conversation, seq, sender_user, sender_device, kind, epoch, data, ts)
                 VALUES (?1, ?2, ?3, ?4, 'commit', ?5, ?6, ?7)",
                params![conversation, seq as i64, me.user, me.device, epoch as i64, commit.0, now],
            )?;
            tx.execute(
                "UPDATE conversations SET epoch = ?1, last_seq = ?2, group_info = ?3 WHERE id = ?4",
                params![(epoch + 1) as i64, seq as i64, group_info.0, conversation],
            )?;
            if let Some(w) = &welcome_c {
                for d in &wd {
                    tx.execute(
                        "INSERT INTO welcomes (device_id, conversation, data, created_at) VALUES (?1, ?2, ?3, ?4)",
                        params![d, conversation, w.0, now],
                    )?;
                }
            }
            let info = conversation_info(&tx, &vis_c, conversation)?.ok_or(AppError::Api(ErrorCode::Internal.into()))?;
            tx.commit()?;
            Ok((info, seq, now, targets_before, gone))
        })
        .await?;
    let env = Envelope {
        conversation,
        seq,
        sender_user: Some(me.user),
        sender_device: Some(me.device),
        kind: EnvelopeKind::Commit,
        epoch,
        data: Bytes(Vec::new()),
        ts,
    };
    fan_out_envelope(engine, conversation, seq, env, &targets).await;
    if let Some(w) = welcome {
        let frame = encode(&ServerFrame::Event(Event::Welcome(PendingWelcome {
            conversation: info.clone(),
            welcome: w,
            start_seq: seq,
        })));
        engine.hub().send_devices(welcome_devices.iter(), frame);
    }
    for u in &removed_users {
        engine.hub().event_users(
            std::iter::once(u),
            Event::ConversationRemoved { conversation },
        );
    }
    engine
        .hub()
        .event_users(info.members.iter(), Event::Conversation(info.clone()));
    tracing::debug!(target: "sl_server::delivery", %conversation, epoch = epoch + 1, "commit accepted");
    Ok(Response::Committed {
        seq,
        epoch: epoch + 1,
    })
}

/// Push an envelope to devices. The payload is loaded once from the DB (the caller
/// passes an envelope with empty data to avoid holding big buffers twice).
async fn fan_out_envelope(
    engine: &Engine,
    conversation: ConversationId,
    seq: u64,
    mut env: Envelope,
    targets: &[DeviceId],
) {
    if targets.is_empty() {
        return;
    }
    if env.data.is_empty() {
        let data = engine
            .db()
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT data FROM envelopes WHERE conversation = ?1 AND seq = ?2",
                    params![conversation, seq as i64],
                    |r| r.get::<_, Vec<u8>>(0),
                )?)
            })
            .await;
        match data {
            Ok(d) => env.data = Bytes(d),
            Err(e) => {
                tracing::warn!(target: "sl_server::delivery", error = %e, "envelope vanished before fan-out");
                return;
            }
        }
    }
    let frame = encode(&ServerFrame::Event(Event::Envelope(env)));
    engine.hub().send_devices(targets.iter(), frame);
}

pub async fn send(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
    epoch: u64,
    ciphertext: Bytes,
) -> AppResult<Response> {
    check_blob(&ciphertext)?;
    let vis = engine.dir().get();
    let len = ciphertext.len();
    let data = ciphertext.clone();
    let (seq, ts, targets) = engine
        .db()
        .write(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let Some(row) = conv_row(&tx, conversation)? else { return api(ErrorCode::NotFound) };
            if !is_member(&tx, conversation, me.user)? || !in_tree(&tx, conversation, me.device)? {
                return api(ErrorCode::NotAuthorized);
            }
            let members = members_of(&tx, conversation)?;
            if read_only(&vis, row.kind, &members) {
                return api(ErrorCode::ReadOnly);
            }
            if row.epoch != epoch {
                let mut e = sl_proto::ApiError::new(ErrorCode::StaleEpoch);
                e.epoch = Some(row.epoch);
                return Err(AppError::Api(e));
            }
            let seq = row.last_seq + 1;
            let now = now();
            tx.execute(
                "INSERT INTO envelopes (conversation, seq, sender_user, sender_device, kind, epoch, data, ts)
                 VALUES (?1, ?2, ?3, ?4, 'application', ?5, ?6, ?7)",
                params![conversation, seq as i64, me.user, me.device, epoch as i64, ciphertext.0, now],
            )?;
            tx.execute("UPDATE conversations SET last_seq = ?1 WHERE id = ?2", params![seq as i64, conversation])?;
            tx.execute(
                "INSERT INTO device_cursors (device_id, conversation, seq) VALUES (?1, ?2, ?3)
                 ON CONFLICT(device_id, conversation) DO UPDATE SET seq = MAX(seq, excluded.seq)",
                params![me.device, conversation, seq as i64],
            )?;
            let targets: Vec<DeviceId> =
                routable_devices(&tx, &vis, conversation)?.into_iter().filter(|d| *d != me.device).collect();
            tx.commit()?;
            Ok((seq, now, targets))
        })
        .await?;
    engine.tel().metrics.record_message(len);
    let env = Envelope {
        conversation,
        seq,
        sender_user: Some(me.user),
        sender_device: Some(me.device),
        kind: EnvelopeKind::Application,
        epoch,
        data,
        ts,
    };
    fan_out_envelope(engine, conversation, seq, env, &targets).await;
    Ok(Response::Sent { seq, ts })
}

pub async fn external_join(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
    epoch: u64,
    commit: Bytes,
    group_info: Bytes,
) -> AppResult<Response> {
    check_blob(&commit)?;
    check_blob(&group_info)?;
    let vis = engine.dir().get();
    if !vis.is_active(&me.user) {
        return api(ErrorCode::NotAuthorized);
    }
    let vis_c = vis.clone();
    let (info, seq, ts, targets) = engine
        .db()
        .write(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let Some(row) = conv_row(&tx, conversation)? else { return api(ErrorCode::NotFound) };
            if !is_member(&tx, conversation, me.user)? {
                return api(ErrorCode::NotAuthorized);
            }
            if row.epoch != epoch {
                let mut e = sl_proto::ApiError::new(ErrorCode::StaleEpoch);
                e.epoch = Some(row.epoch);
                return Err(AppError::Api(e));
            }
            let targets: Vec<DeviceId> =
                routable_devices(&tx, &vis_c, conversation)?.into_iter().filter(|d| *d != me.device).collect();
            let now = now();
            let seq = row.last_seq + 1;
            tx.execute(
                "INSERT OR IGNORE INTO conversation_devices (conversation, device_id, joined_at) VALUES (?1, ?2, ?3)",
                params![conversation, me.device, now],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO device_cursors (device_id, conversation, seq) VALUES (?1, ?2, ?3)",
                params![me.device, conversation, seq as i64],
            )?;
            tx.execute(
                "INSERT INTO envelopes (conversation, seq, sender_user, sender_device, kind, epoch, data, ts)
                 VALUES (?1, ?2, ?3, ?4, 'commit', ?5, ?6, ?7)",
                params![conversation, seq as i64, me.user, me.device, epoch as i64, commit.0, now],
            )?;
            tx.execute(
                "UPDATE conversations SET epoch = ?1, last_seq = ?2, group_info = ?3 WHERE id = ?4",
                params![(epoch + 1) as i64, seq as i64, group_info.0, conversation],
            )?;
            tx.execute("DELETE FROM welcomes WHERE device_id = ?1 AND conversation = ?2", params![me.device, conversation])?;
            let info = conversation_info(&tx, &vis_c, conversation)?.ok_or(AppError::Api(ErrorCode::Internal.into()))?;
            tx.commit()?;
            Ok((info, seq, now, targets))
        })
        .await?;
    let env = Envelope {
        conversation,
        seq,
        sender_user: Some(me.user),
        sender_device: Some(me.device),
        kind: EnvelopeKind::Commit,
        epoch,
        data: Bytes(Vec::new()),
        ts,
    };
    fan_out_envelope(engine, conversation, seq, env, &targets).await;
    engine
        .hub()
        .event_users(info.members.iter(), Event::Conversation(info.clone()));
    tracing::debug!(target: "sl_server::delivery", %conversation, "device joined by external commit");
    Ok(Response::Committed {
        seq,
        epoch: epoch + 1,
    })
}

pub async fn group_info(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
) -> AppResult<Response> {
    let (info, epoch) = engine
        .db()
        .read(move |c| {
            if !is_member(c, conversation, me.user)? {
                return api(ErrorCode::NotAuthorized);
            }
            let row: Option<(Option<Vec<u8>>, i64)> = c
                .query_row(
                    "SELECT group_info, epoch FROM conversations WHERE id = ?1",
                    [conversation],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match row {
                Some((Some(gi), epoch)) => Ok((gi, epoch as u64)),
                _ => api(ErrorCode::NotFound),
            }
        })
        .await?;
    Ok(Response::GroupInfo {
        info: Bytes(info),
        epoch,
    })
}

// ---------------------------------------------------------------------------------------
// Delivery to a device
// ---------------------------------------------------------------------------------------

/// Envelopes after `after`. The caller's own envelopes are returned without payload so
/// the client sees a contiguous sequence (it never needs to decrypt its own messages).
fn load_envelopes(
    c: &Connection,
    conversation: ConversationId,
    after: u64,
    own_device: DeviceId,
    limit: u32,
) -> AppResult<Vec<Envelope>> {
    let mut st = c.prepare_cached(
        "SELECT seq, sender_user, sender_device, kind, epoch,
                CASE WHEN sender_device = ?3 THEN x'' ELSE data END, ts
         FROM envelopes WHERE conversation = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?4",
    )?;
    let rows = st.query_map(
        params![conversation, after as i64, own_device, limit],
        |r| {
            Ok(Envelope {
                conversation,
                seq: r.get::<_, i64>(0)? as u64,
                sender_user: r.get(1)?,
                sender_device: r.get(2)?,
                kind: env_kind(&r.get::<_, String>(3)?),
                epoch: r.get::<_, i64>(4)? as u64,
                data: Bytes(r.get(5)?),
                ts: r.get(6)?,
            })
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub async fn sync(engine: &Engine, me: Caller, limit: u32) -> AppResult<Response> {
    let limit = limit.clamp(1, 1000);
    let vis = engine.dir().get();
    let batch = engine
        .db()
        .read(move |c| {
            let mut batch = SyncBatch::default();
            // Welcomes first: they create groups that envelopes may refer to.
            {
                let mut st = c.prepare(
                    "SELECT w.conversation, w.data, COALESCE(dc.seq, 0) FROM welcomes w
                     LEFT JOIN device_cursors dc ON dc.device_id = w.device_id AND dc.conversation = w.conversation
                     WHERE w.device_id = ?1 ORDER BY w.id",
                )?;
                let rows: Vec<(ConversationId, Vec<u8>, i64)> =
                    st.query_map([me.device], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
                for (conv, data, start) in rows {
                    if let Some(info) = conversation_info(c, &vis, conv)?
                        && info.members.contains(&me.user)
                    {
                        batch.welcomes.push(PendingWelcome { conversation: info, welcome: Bytes(data), start_seq: start as u64 });
                    }
                }
            }
            let mut st = c.prepare(
                "SELECT cd.conversation, COALESCE(dc.seq, 0) FROM conversation_devices cd
                 JOIN conversation_members m ON m.conversation = cd.conversation AND m.user_id = ?2
                 LEFT JOIN device_cursors dc ON dc.device_id = cd.device_id AND dc.conversation = cd.conversation
                 JOIN conversations cv ON cv.id = cd.conversation
                 WHERE cd.device_id = ?1 AND cv.last_seq > COALESCE(dc.seq, 0)",
            )?;
            let convs: Vec<(ConversationId, i64)> =
                st.query_map(params![me.device, me.user], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
            let mut budget = limit;
            for (conv, cursor) in convs {
                if budget == 0 {
                    batch.more = true;
                    break;
                }
                let envs = load_envelopes(c, conv, cursor as u64, me.device, budget)?;
                budget = budget.saturating_sub(envs.len() as u32);
                if budget == 0 {
                    batch.more = true;
                }
                batch.envelopes.extend(envs);
            }
            Ok(batch)
        })
        .await?;
    Ok(Response::Sync(batch))
}

pub async fn ack(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
    seq: u64,
) -> AppResult<Response> {
    engine
        .db()
        .write(move |c| {
            c.execute(
                "INSERT INTO device_cursors (device_id, conversation, seq)
                 SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM conversation_devices WHERE device_id = ?1 AND conversation = ?2)
                 ON CONFLICT(device_id, conversation) DO UPDATE SET seq = MAX(seq, excluded.seq)",
                params![me.device, conversation, seq as i64],
            )?;
            c.execute("DELETE FROM welcomes WHERE device_id = ?1 AND conversation = ?2", params![me.device, conversation])?;
            Ok(())
        })
        .await?;
    Ok(Response::Ok)
}

pub async fn fetch(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
    after_seq: u64,
    limit: u32,
) -> AppResult<Response> {
    let limit = limit.clamp(1, 500);
    let envs = engine
        .db()
        .read(move |c| {
            if !is_member(c, conversation, me.user)? || !in_tree(c, conversation, me.device)? {
                return api(ErrorCode::NotAuthorized);
            }
            load_envelopes(c, conversation, after_seq, me.device, limit)
        })
        .await?;
    Ok(Response::Envelopes(envs))
}

pub async fn purge(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
    seq: u64,
) -> AppResult<Response> {
    let n = engine
        .db()
        .write(move |c| {
            Ok(c.execute(
                "DELETE FROM envelopes WHERE conversation = ?1 AND seq = ?2 AND sender_user = ?3 AND kind = 'application'",
                params![conversation, seq as i64, me.user],
            )?)
        })
        .await?;
    if n == 0 {
        api(ErrorCode::NotFound)
    } else {
        Ok(Response::Ok)
    }
}

pub async fn leave(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
) -> AppResult<Response> {
    let vis = engine.dir().get();
    let info = engine
        .db()
        .write(move |c| {
            let tx = c.transaction()?;
            let Some(row) = conv_row(&tx, conversation)? else { return api(ErrorCode::NotFound) };
            if row.kind == ConversationKind::Direct {
                return api_detail(ErrorCode::BadRequest, "you can't leave a direct chat");
            }
            tx.execute("DELETE FROM conversation_members WHERE conversation = ?1 AND user_id = ?2", params![conversation, me.user])?;
            tx.execute(
                "DELETE FROM conversation_devices WHERE conversation = ?1 AND device_id IN (SELECT id FROM devices WHERE user_id = ?2)",
                params![conversation, me.user],
            )?;
            let info = conversation_info(&tx, &vis, conversation)?;
            tx.commit()?;
            Ok(info)
        })
        .await?;
    engine.hub().event_users(
        std::iter::once(&me.user),
        Event::ConversationRemoved { conversation },
    );
    if let Some(info) = info {
        engine
            .hub()
            .event_users(info.members.iter(), Event::Conversation(info.clone()));
    }
    Ok(Response::Ok)
}

pub async fn mark_read(
    engine: &Engine,
    me: Caller,
    conversation: ConversationId,
    seq: u64,
) -> AppResult<Response> {
    let members = engine
        .db()
        .write(move |c| {
            if !is_member(c, conversation, me.user)? {
                return api(ErrorCode::NotAuthorized);
            }
            c.execute(
                "INSERT INTO read_markers (user_id, conversation, seq) VALUES (?1, ?2, ?3)
                 ON CONFLICT(user_id, conversation) DO UPDATE SET seq = MAX(seq, excluded.seq)",
                params![me.user, conversation, seq as i64],
            )?;
            members_of(c, conversation)
        })
        .await?;
    engine.hub().event_users(
        members.iter(),
        Event::ReadMarker(sl_proto::ReadMarker {
            conversation,
            user: me.user,
            seq,
        }),
    );
    Ok(Response::Ok)
}

pub async fn typing(engine: &Engine, me: Caller, conversation: ConversationId) -> AppResult<()> {
    let members = engine
        .db()
        .read(move |c| {
            if !is_member(c, conversation, me.user)? {
                return api(ErrorCode::NotAuthorized);
            }
            members_of(c, conversation)
        })
        .await?;
    let others: Vec<UserId> = members.into_iter().filter(|u| *u != me.user).collect();
    engine.hub().event_users(
        others.iter(),
        Event::Typing {
            conversation,
            user: me.user,
        },
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Membership changes made by the admin
// ---------------------------------------------------------------------------------------

/// Re-apply isolation after Group membership, user status or device changes:
/// - group chats drop members who left the scope Group (or whose Group was deleted),
/// - every affected conversation's members get the new `ConversationInfo`, so a member's
///   client commits the MLS removals,
/// - removed people get `ConversationRemoved`.
pub async fn reconcile(engine: &Engine, affected_users: &[UserId]) -> AppResult<()> {
    let vis = engine.dir().get();
    let affected: Vec<UserId> = affected_users.to_vec();
    let (removed, updated) = engine
        .db()
        .write(move |c| {
            let tx = c.transaction()?;
            let mut removed: Vec<(UserId, ConversationId)> = Vec::new();
            let mut touched: BTreeSet<ConversationId> = BTreeSet::new();
            for u in &affected {
                let convs: Vec<(ConversationId, String, Option<GroupId>)> = {
                    let mut st = tx.prepare(
                        "SELECT c.id, c.kind, c.scope_group FROM conversations c
                         JOIN conversation_members m ON m.conversation = c.id WHERE m.user_id = ?1",
                    )?;
                    st.query_map([u], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?
                };
                for (conv, kind, scope) in convs {
                    touched.insert(conv);
                    if kind == "group" {
                        let allowed = scope.is_some_and(|g| vis.is_member(u, &g));
                        if !allowed {
                            tx.execute("DELETE FROM conversation_members WHERE conversation = ?1 AND user_id = ?2", params![conv, u])?;
                            tx.execute(
                                "DELETE FROM conversation_devices WHERE conversation = ?1 AND device_id IN (SELECT id FROM devices WHERE user_id = ?2)",
                                params![conv, u],
                            )?;
                            removed.push((*u, conv));
                        }
                    }
                }
            }
            // Revoked devices leave every tree.
            tx.execute(
                "DELETE FROM conversation_devices WHERE device_id IN (SELECT id FROM devices WHERE revoked_at IS NOT NULL)",
                [],
            )?;
            let mut updated = Vec::new();
            for conv in touched {
                if let Some(info) = conversation_info(&tx, &vis, conv)? {
                    updated.push(info);
                }
            }
            tx.commit()?;
            Ok((removed, updated))
        })
        .await?;
    for (u, conv) in removed {
        engine.hub().event_users(
            std::iter::once(&u),
            Event::ConversationRemoved { conversation: conv },
        );
    }
    for info in updated {
        engine
            .hub()
            .event_users(info.members.iter(), Event::Conversation(info.clone()));
    }
    engine.hub().event_all(Event::DirectoryChanged);
    Ok(())
}

/// Retention: drop relayed ciphertext older than the window.
pub(crate) fn purge_old(c: &Connection, older_than: i64) -> AppResult<usize> {
    Ok(c.execute("DELETE FROM envelopes WHERE ts < ?1", [older_than])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_rules() {
        let vis = Visibility::default();
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        assert!(
            read_only(&vis, ConversationKind::Direct, &[a, b]),
            "no shared group → read-only"
        );
        assert!(!read_only(&vis, ConversationKind::Group, &[a, b]));
    }
}
