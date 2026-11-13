//! Accounts, invites, sessions.

use std::net::SocketAddr;
use std::sync::OnceLock;

use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sl_proto::panel::{ConnEventKind, InviteKind, InviteStatus};
use sl_proto::wire::{AuthOk, LoginRequest, RedeemRequest, RegisterRequest, ResumeRequest};
use sl_proto::{Bytes, DeviceId, DeviceInfo, ErrorCode, SelfUser, UserId};
use uuid::Uuid;

use crate::Engine;
use crate::db::now;
use crate::error::{AppError, AppResult, api, api_detail};

/// Everything the WebSocket layer needs to know about an authenticated connection.
#[derive(Debug, Clone)]
pub struct Authed {
    pub user_id: UserId,
    pub device_id: DeviceId,
    pub username: String,
    pub display_name: String,
    pub device_name: String,
    pub platform: String,
    pub client_version: String,
    pub token_hash: String,
    pub ok: AuthOk,
}

// ---------------------------------------------------------------------------------------
// Password helpers
// ---------------------------------------------------------------------------------------

pub fn hash_password(pw: &str) -> anyhow::Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))
}

pub fn verify_password(hash: &str, pw: &str) -> bool {
    PasswordHash::new(hash)
        .is_ok_and(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok())
}

/// Verify against a fixed dummy hash so unknown usernames take as long as real ones.
fn burn_dummy_verify(pw: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let h = DUMMY.get_or_init(|| hash_password("spareline-dummy-password").unwrap_or_default());
    let _ = verify_password(h, pw);
}

pub async fn hash_password_async(pw: String) -> AppResult<String> {
    Ok(tokio::task::spawn_blocking(move || hash_password(&pw)).await??)
}

pub async fn verify_password_async(hash: String, pw: String) -> AppResult<bool> {
    Ok(tokio::task::spawn_blocking(move || verify_password(&hash, &pw)).await?)
}

// ---------------------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------------------

pub fn normalize_username(u: &str) -> AppResult<String> {
    let u = u.trim().to_lowercase();
    let ok_len = (3..=32).contains(&u.chars().count());
    let ok_chars = u
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
    let ok_start = u.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    if ok_len && ok_chars && ok_start {
        Ok(u)
    } else {
        api(ErrorCode::InvalidUsername)
    }
}

pub fn validate_display_name(d: &str) -> AppResult<String> {
    let d = d.trim();
    if d.is_empty() || d.chars().count() > 64 || d.chars().any(char::is_control) {
        return api_detail(
            ErrorCode::BadRequest,
            "display name must be 1–64 characters",
        );
    }
    Ok(d.to_string())
}

pub fn validate_password(p: &str) -> AppResult<()> {
    let n = p.chars().count();
    if !(10..=512).contains(&n) {
        api(ErrorCode::WeakPassword)
    } else {
        Ok(())
    }
}

fn validate_device(d: &DeviceInfo) -> AppResult<()> {
    if d.name.trim().is_empty() || d.name.chars().count() > 64 || d.platform.chars().count() > 32 {
        return api_detail(ErrorCode::BadRequest, "invalid device description");
    }
    if d.signature_key.is_empty() || d.signature_key.len() > 512 {
        return api_detail(ErrorCode::BadRequest, "invalid device signature key");
    }
    Ok(())
}

pub fn check_proto(proto: u16) -> AppResult<()> {
    if proto < sl_proto::MIN_PROTOCOL_VERSION {
        api(ErrorCode::ProtocolTooOld)
    } else if proto > sl_proto::PROTOCOL_VERSION {
        api(ErrorCode::ProtocolTooNew)
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Invites
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct InviteRec {
    pub id: Uuid,
    pub proof_key: Vec<u8>,
    pub kind: InviteKind,
    pub target_user: Option<UserId>,
    pub status: InviteStatus,
}

pub fn invite_status(
    expires_at: i64,
    used_at: Option<i64>,
    revoked_at: Option<i64>,
    now: i64,
) -> InviteStatus {
    if revoked_at.is_some() {
        InviteStatus::Revoked
    } else if used_at.is_some() {
        InviteStatus::Used
    } else if expires_at <= now {
        InviteStatus::Expired
    } else {
        InviteStatus::Pending
    }
}

pub fn kind_from_str(s: &str) -> InviteKind {
    if s == "setup" {
        InviteKind::Setup
    } else {
        InviteKind::NewUser
    }
}

pub fn lookup_invite(conn: &Connection, invite_id: &str) -> AppResult<Option<InviteRec>> {
    let now = now();
    Ok(conn
        .query_row(
            "SELECT id, proof_key, kind, target_user, expires_at, used_at, revoked_at FROM invites WHERE invite_id = ?1",
            [invite_id],
            |r| {
                Ok(InviteRec {
                    id: r.get(0)?,
                    proof_key: r.get(1)?,
                    kind: kind_from_str(&r.get::<_, String>(2)?),
                    target_user: r.get(3)?,
                    status: invite_status(r.get(4)?, r.get(5)?, r.get(6)?, now),
                })
            },
        )
        .optional()?)
}

fn status_error(s: InviteStatus) -> AppResult<()> {
    match s {
        InviteStatus::Pending => Ok(()),
        InviteStatus::Used => api(ErrorCode::InviteUsed),
        InviteStatus::Expired => api(ErrorCode::InviteExpired),
        InviteStatus::Revoked => api(ErrorCode::InvalidInvite),
    }
}

/// Server identity proof for an invite (see `sl_proto::crypto`).
pub async fn prove(
    engine: &Engine,
    invite_id: String,
    nonce: Bytes,
    peer: SocketAddr,
) -> AppResult<Bytes> {
    if !engine.allow_auth_attempt(peer) {
        engine.conn_event(
            peer,
            None,
            None,
            ConnEventKind::RateLimited,
            "too many invite attempts",
        );
        return api(ErrorCode::RateLimited);
    }
    if invite_id.len() != 32 || nonce.len() < 16 || nonce.len() > 64 {
        return api(ErrorCode::BadRequest);
    }
    let rec = engine
        .db()
        .read(move |c| lookup_invite(c, &invite_id))
        .await?;
    let Some(rec) = rec else {
        engine.tel().metrics.record_auth_failure();
        engine.conn_event(
            peer,
            None,
            None,
            ConnEventKind::AuthFailed,
            "unknown invite code",
        );
        return api(ErrorCode::InvalidInvite);
    };
    status_error(rec.status)?;
    let key: [u8; 32] = rec
        .proof_key
        .as_slice()
        .try_into()
        .map_err(|_| AppError::Internal(anyhow::anyhow!("bad proof key")))?;
    let spki = engine.tls().spki_sha256;
    Ok(Bytes(
        sl_proto::crypto::compute_proof(&key, nonce.as_slice(), &spki).to_vec(),
    ))
}

/// Check a presented code against an invite and return it if usable.
fn verify_code(conn: &Connection, code: &str, expected: InviteKind) -> AppResult<InviteRec> {
    let Some(canon) = sl_proto::code::normalize(code) else {
        return api(ErrorCode::InvalidInvite);
    };
    let keys = sl_proto::crypto::derive_invite(&canon);
    let Some(rec) = lookup_invite(conn, &keys.invite_id)? else {
        return api(ErrorCode::InvalidInvite);
    };
    if !sl_proto::crypto::ct_eq(&rec.proof_key, &keys.proof_key) || rec.kind != expected {
        return api(ErrorCode::InvalidInvite);
    }
    status_error(rec.status)?;
    Ok(rec)
}

// ---------------------------------------------------------------------------------------
// Devices and sessions
// ---------------------------------------------------------------------------------------

fn insert_device(conn: &Connection, user: UserId, d: &DeviceInfo) -> AppResult<DeviceId> {
    let id = Uuid::now_v7();
    conn.execute(
        "INSERT INTO devices (id, user_id, name, platform, client_version, signature_key, created_at, last_seen)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        params![id, user, d.name.trim(), d.platform, d.client_version, d.signature_key.as_slice(), now()],
    )?;
    Ok(id)
}

fn insert_session(
    conn: &Connection,
    user: UserId,
    device: DeviceId,
    ip: &str,
    idle_days: u32,
) -> AppResult<(String, String)> {
    let token = sl_proto::crypto::generate_token();
    let hash = sl_proto::crypto::sha256_hex(token.as_bytes());
    let now = now();
    let expires = now + i64::from(idle_days.max(1)) * 86_400_000;
    conn.execute(
        "INSERT INTO sessions (token_hash, user_id, device_id, created_at, last_used, expires_at, ip) VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6)",
        params![hash, user, device, now, expires, ip],
    )?;
    Ok((token, hash))
}

struct UserRow {
    id: UserId,
    username: String,
    password_hash: String,
    disabled: bool,
}

fn user_by_name(conn: &Connection, username: &str) -> AppResult<Option<UserRow>> {
    Ok(conn
        .query_row(
            "SELECT id, username, password_hash, disabled FROM users WHERE username = ?1",
            [username],
            |r| {
                Ok(UserRow {
                    id: r.get(0)?,
                    username: r.get(1)?,
                    password_hash: r.get(2)?,
                    disabled: r.get::<_, i64>(3)? != 0,
                })
            },
        )
        .optional()?)
}

pub fn key_packages_remaining(conn: &Connection, device: DeviceId) -> AppResult<u32> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM key_packages WHERE device_id = ?1 AND last_resort = 0",
        [device],
        |r| r.get::<_, u32>(0),
    )?)
}

/// Assemble the `Authed` result after a successful sign-in.
async fn finish(
    engine: &Engine,
    user_id: UserId,
    device_id: DeviceId,
    token: Option<String>,
    token_hash: String,
    device: (String, String, String),
) -> AppResult<Authed> {
    let vis = engine.dir().get();
    let (username, display_name) = engine
        .db()
        .read(move |c| {
            Ok(c.query_row(
                "SELECT username, display_name FROM users WHERE id = ?1",
                [user_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?)
        })
        .await?;
    let remaining = engine
        .db()
        .read(move |c| key_packages_remaining(c, device_id))
        .await?;
    let broadcasts = crate::broadcast::active(engine).await?;
    let ok = AuthOk {
        user: SelfUser {
            id: user_id,
            username: username.clone(),
            display_name: display_name.clone(),
            groups: vis.group_refs_of(&user_id),
        },
        device_id,
        session_token: token,
        server: engine.server_info(),
        broadcasts,
        key_packages_remaining: remaining,
    };
    Ok(Authed {
        user_id,
        device_id,
        username,
        display_name,
        device_name: device.0,
        platform: device.1,
        client_version: device.2,
        token_hash,
        ok,
    })
}

// ---------------------------------------------------------------------------------------
// Flows
// ---------------------------------------------------------------------------------------

pub async fn register(
    engine: &Engine,
    req: RegisterRequest,
    peer: SocketAddr,
) -> AppResult<Authed> {
    check_proto(req.proto)?;
    if !engine.allow_auth_attempt(peer) {
        engine.conn_event(
            peer,
            None,
            None,
            ConnEventKind::RateLimited,
            "too many sign-up attempts",
        );
        return api(ErrorCode::RateLimited);
    }
    let username = normalize_username(&req.username)?;
    let display_name = validate_display_name(&req.display_name)?;
    validate_password(&req.password)?;
    validate_device(&req.device)?;
    // Validate the code before spending time on hashing.
    let code = req.code.clone();
    engine
        .db()
        .read(move |c| verify_code(c, &code, InviteKind::NewUser).map(|_| ()))
        .await?;
    let password_hash = hash_password_async(req.password.clone()).await?;
    let ip = peer.ip().to_string();
    let idle = engine.config().session_idle_days;
    let device = req.device.clone();
    let code = req.code.clone();
    let res = engine
        .db()
        .write(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let invite = verify_code(&tx, &code, InviteKind::NewUser)?;
            let taken: bool = tx
                .query_row("SELECT 1 FROM users WHERE username = ?1", [&username], |_| Ok(true))
                .optional()?
                .unwrap_or(false);
            if taken {
                return api(ErrorCode::UsernameTaken);
            }
            let user_id = Uuid::now_v7();
            let now = now();
            tx.execute(
                "INSERT INTO users (id, username, display_name, password_hash, created_at, last_seen) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![user_id, username, display_name, password_hash, now],
            )?;
            tx.execute(
                "INSERT INTO group_members (group_id, user_id, added_at) SELECT group_id, ?1, ?2 FROM invite_groups WHERE invite = ?3",
                params![user_id, now, invite.id],
            )?;
            let n = tx.execute(
                "UPDATE invites SET used_at = ?1, used_by = ?2 WHERE id = ?3 AND used_at IS NULL",
                params![now, user_id, invite.id],
            )?;
            if n != 1 {
                return api(ErrorCode::InviteUsed);
            }
            let device_id = insert_device(&tx, user_id, &device)?;
            let (token, hash) = insert_session(&tx, user_id, device_id, &ip, idle)?;
            tx.commit()?;
            Ok((user_id, device_id, token, hash))
        })
        .await;
    let (user_id, device_id, token, hash) = match res {
        Ok(v) => v,
        Err(e) => {
            engine.tel().metrics.record_auth_failure();
            engine.conn_event(
                peer,
                None,
                None,
                ConnEventKind::AuthFailed,
                format!("sign-up failed: {e}"),
            );
            return Err(e);
        }
    };
    engine.reload_directory().await?;
    let visible = engine.dir().get().visible_users(&user_id);
    engine
        .hub()
        .event_users(visible.iter(), sl_proto::wire::Event::DirectoryChanged);
    tracing::info!(target: "sl_server::auth", user = %user_id, "new account registered");
    engine.conn_event(
        peer,
        Some(user_id),
        Some(device_id),
        ConnEventKind::Registered,
        "account created with invite",
    );
    finish(
        engine,
        user_id,
        device_id,
        Some(token),
        hash,
        (
            req.device.name,
            req.device.platform,
            req.device.client_version,
        ),
    )
    .await
}

pub async fn redeem(engine: &Engine, req: RedeemRequest, peer: SocketAddr) -> AppResult<Authed> {
    check_proto(req.proto)?;
    if !engine.allow_auth_attempt(peer) {
        engine.conn_event(
            peer,
            None,
            None,
            ConnEventKind::RateLimited,
            "too many setup-code attempts",
        );
        return api(ErrorCode::RateLimited);
    }
    validate_password(&req.new_password)?;
    validate_device(&req.device)?;
    let code = req.code.clone();
    engine
        .db()
        .read(move |c| verify_code(c, &code, InviteKind::Setup).map(|_| ()))
        .await?;
    let password_hash = hash_password_async(req.new_password.clone()).await?;
    let ip = peer.ip().to_string();
    let idle = engine.config().session_idle_days;
    let device = req.device.clone();
    let code = req.code.clone();
    let (user_id, device_id, token, hash) = engine
        .db()
        .write(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let invite = verify_code(&tx, &code, InviteKind::Setup)?;
            let Some(user_id) = invite.target_user else {
                return api(ErrorCode::InvalidInvite);
            };
            let disabled: i64 =
                tx.query_row("SELECT disabled FROM users WHERE id = ?1", [user_id], |r| {
                    r.get(0)
                })?;
            if disabled != 0 {
                return api(ErrorCode::AccountDisabled);
            }
            let now = now();
            tx.execute(
                "UPDATE users SET password_hash = ?1, last_seen = ?2 WHERE id = ?3",
                params![password_hash, now, user_id],
            )?;
            // A reset signs the account out everywhere.
            tx.execute("DELETE FROM sessions WHERE user_id = ?1", [user_id])?;
            let n = tx.execute(
                "UPDATE invites SET used_at = ?1, used_by = ?2 WHERE id = ?3 AND used_at IS NULL",
                params![now, user_id, invite.id],
            )?;
            if n != 1 {
                return api(ErrorCode::InviteUsed);
            }
            let device_id = insert_device(&tx, user_id, &device)?;
            let (token, hash) = insert_session(&tx, user_id, device_id, &ip, idle)?;
            tx.commit()?;
            Ok((user_id, device_id, token, hash))
        })
        .await?;
    // Other devices were signed out.
    engine
        .hub()
        .close_user(&user_id, sl_proto::ApiError::new(ErrorCode::SessionRevoked));
    tracing::info!(target: "sl_server::auth", user = %user_id, "setup code redeemed");
    engine.conn_event(
        peer,
        Some(user_id),
        Some(device_id),
        ConnEventKind::Registered,
        "setup code redeemed (new device / password reset)",
    );
    finish(
        engine,
        user_id,
        device_id,
        Some(token),
        hash,
        (
            req.device.name,
            req.device.platform,
            req.device.client_version,
        ),
    )
    .await
}

pub async fn login(engine: &Engine, req: LoginRequest, peer: SocketAddr) -> AppResult<Authed> {
    check_proto(req.proto)?;
    if !engine.allow_login_attempt(peer, &req.username) {
        engine.conn_event(
            peer,
            None,
            None,
            ConnEventKind::RateLimited,
            "too many sign-in attempts",
        );
        return api(ErrorCode::RateLimited);
    }
    validate_device(&req.device)?;
    let username = req.username.trim().to_lowercase();
    let user = {
        let u = username.clone();
        engine.db().read(move |c| user_by_name(c, &u)).await?
    };
    let Some(user) = user else {
        let pw = req.password.clone();
        let _ = tokio::task::spawn_blocking(move || burn_dummy_verify(&pw)).await;
        engine.tel().metrics.record_auth_failure();
        engine.conn_event(
            peer,
            None,
            None,
            ConnEventKind::AuthFailed,
            "sign-in with unknown username",
        );
        return api(ErrorCode::InvalidCredentials);
    };
    if !verify_password_async(user.password_hash.clone(), req.password.clone()).await? {
        engine.tel().metrics.record_auth_failure();
        engine.conn_event(
            peer,
            Some(user.id),
            None,
            ConnEventKind::AuthFailed,
            "wrong password",
        );
        return api(ErrorCode::InvalidCredentials);
    }
    if user.disabled {
        engine.conn_event(
            peer,
            Some(user.id),
            None,
            ConnEventKind::AuthFailed,
            "account disabled",
        );
        return api(ErrorCode::AccountDisabled);
    }
    let ip = peer.ip().to_string();
    let idle = engine.config().session_idle_days;
    let device = req.device.clone();
    let user_id = user.id;
    let want_device = req.device_id;
    let (device_id, token, hash) = engine
        .db()
        .write(move |c| {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let reuse = match want_device {
                Some(d) => tx
                    .query_row(
                        "SELECT 1 FROM devices WHERE id = ?1 AND user_id = ?2 AND revoked_at IS NULL",
                        params![d, user_id],
                        |_| Ok(true),
                    )
                    .optional()?
                    .map(|_| d),
                None => None,
            };
            let device_id = match reuse {
                Some(d) => {
                    tx.execute(
                        "UPDATE devices SET name = ?1, platform = ?2, client_version = ?3, last_seen = ?4 WHERE id = ?5",
                        params![device.name.trim(), device.platform, device.client_version, now(), d],
                    )?;
                    d
                }
                None => insert_device(&tx, user_id, &device)?,
            };
            tx.execute("UPDATE users SET last_seen = ?1 WHERE id = ?2", params![now(), user_id])?;
            let (token, hash) = insert_session(&tx, user_id, device_id, &ip, idle)?;
            tx.commit()?;
            Ok((device_id, token, hash))
        })
        .await?;
    tracing::info!(target: "sl_server::auth", user = %user.username, "signed in");
    finish(
        engine,
        user_id,
        device_id,
        Some(token),
        hash,
        (
            req.device.name,
            req.device.platform,
            req.device.client_version,
        ),
    )
    .await
}

pub async fn resume(engine: &Engine, req: ResumeRequest, peer: SocketAddr) -> AppResult<Authed> {
    check_proto(req.proto)?;
    let hash = sl_proto::crypto::sha256_hex(req.token.as_bytes());
    let h = hash.clone();
    let row = engine
        .db()
        .read(move |c| {
            Ok(c.query_row(
                "SELECT s.user_id, s.device_id, s.expires_at, u.disabled, d.revoked_at, d.name, d.platform
                 FROM sessions s JOIN users u ON u.id = s.user_id JOIN devices d ON d.id = s.device_id
                 WHERE s.token_hash = ?1",
                [h],
                |r| {
                    Ok((
                        r.get::<_, UserId>(0)?,
                        r.get::<_, DeviceId>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)? != 0,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, String>(5)?,
                        r.get::<_, String>(6)?,
                    ))
                },
            )
            .optional()?)
        })
        .await?;
    let Some((user_id, device_id, expires_at, disabled, revoked, device_name, platform)) = row
    else {
        engine.tel().metrics.record_auth_failure();
        engine.conn_event(
            peer,
            None,
            Some(req.device_id),
            ConnEventKind::AuthFailed,
            "unknown or revoked session",
        );
        return api(ErrorCode::SessionRevoked);
    };
    if device_id != req.device_id {
        engine.conn_event(
            peer,
            Some(user_id),
            Some(req.device_id),
            ConnEventKind::AuthFailed,
            "session used from a different device",
        );
        return api(ErrorCode::SessionRevoked);
    }
    if disabled {
        engine.conn_event(
            peer,
            Some(user_id),
            Some(device_id),
            ConnEventKind::AuthFailed,
            "account disabled",
        );
        return api(ErrorCode::AccountDisabled);
    }
    if revoked.is_some() {
        engine.conn_event(
            peer,
            Some(user_id),
            Some(device_id),
            ConnEventKind::AuthFailed,
            "device was removed by admin",
        );
        return api(ErrorCode::SessionRevoked);
    }
    if expires_at <= now() {
        engine.conn_event(
            peer,
            Some(user_id),
            Some(device_id),
            ConnEventKind::AuthFailed,
            "session expired",
        );
        return api(ErrorCode::SessionExpired);
    }
    let idle = engine.config().session_idle_days;
    let ip = peer.ip().to_string();
    let h = hash.clone();
    let version = req.client_version.clone();
    engine
        .db()
        .write(move |c| {
            let now = now();
            let expires = now + i64::from(idle.max(1)) * 86_400_000;
            c.execute(
                "UPDATE sessions SET last_used = ?1, expires_at = ?2, ip = ?3 WHERE token_hash = ?4",
                params![now, expires, ip, h],
            )?;
            c.execute("UPDATE devices SET last_seen = ?1, client_version = ?2 WHERE id = ?3", params![now, version, device_id])?;
            c.execute("UPDATE users SET last_seen = ?1 WHERE id = ?2", params![now, user_id])?;
            Ok(())
        })
        .await?;
    finish(
        engine,
        user_id,
        device_id,
        None,
        hash,
        (device_name, platform, req.client_version),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames() {
        assert_eq!(
            normalize_username("  Alex.Smith ").ok().as_deref(),
            Some("alex.smith")
        );
        assert!(normalize_username("ab").is_err());
        assert!(normalize_username("_alex").is_err());
        assert!(normalize_username("alex smith").is_err());
        assert!(normalize_username(&"a".repeat(33)).is_err());
    }

    #[test]
    fn passwords() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("long enough pw").is_ok());
        let h = hash_password("correct horse battery").expect("hash");
        assert!(verify_password(&h, "correct horse battery"));
        assert!(!verify_password(&h, "wrong"));
        assert!(!verify_password("not-a-hash", "x"));
    }

    #[test]
    fn statuses() {
        assert_eq!(invite_status(10, None, None, 5), InviteStatus::Pending);
        assert_eq!(invite_status(10, None, None, 10), InviteStatus::Expired);
        assert_eq!(invite_status(10, Some(1), None, 5), InviteStatus::Used);
        assert_eq!(
            invite_status(10, Some(1), Some(2), 5),
            InviteStatus::Revoked
        );
    }
}
