//! Admin API used by the control panel (in-process only; never exposed on the network).
//!
//! Privacy boundary: this module has no access to envelope storage and offers no way to
//! read, list or count individual conversations. It sees accounts, groups, devices,
//! connection health and aggregate counters.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use sl_proto::panel::*;
use sl_proto::wire::Event;
use sl_proto::{ApiError, DeviceId, ErrorCode, GroupId, GroupRef, UserId};
use uuid::Uuid;

use crate::Engine;
use crate::db::now;
use crate::error::{AppError, AppResult, api, api_detail};

#[derive(Clone)]
pub struct AdminApi {
    engine: Engine,
}

impl AdminApi {
    pub(crate) fn new(engine: Engine) -> Self {
        Self { engine }
    }

    fn e(&self) -> &Engine {
        &self.engine
    }

    // -----------------------------------------------------------------------------------
    // Overview
    // -----------------------------------------------------------------------------------

    pub async fn overview(&self) -> AppResult<Overview> {
        let e = self.e();
        let week_ago = now() - 7 * 86_400_000;
        let (users_total, devices_total, devices_ready, active_broadcasts) = e
            .db()
            .read(move |c| {
                let users: u32 = c.query_row("SELECT COUNT(*) FROM users WHERE disabled = 0", [], |r| r.get(0))?;
                let devices: u32 = c.query_row("SELECT COUNT(*) FROM devices WHERE revoked_at IS NULL", [], |r| r.get(0))?;
                let ready: u32 = c.query_row(
                    "SELECT COUNT(*) FROM devices WHERE revoked_at IS NULL AND last_seen >= ?1",
                    [week_ago],
                    |r| r.get(0),
                )?;
                let b: u32 = c.query_row(
                    "SELECT COUNT(*) FROM broadcasts WHERE ended_at IS NULL AND (expires_at IS NULL OR expires_at > ?1)",
                    [now()],
                    |r| r.get(0),
                )?;
                Ok((users, devices, ready, b))
            })
            .await?;
        let health = e.cached_health().await;
        let running = e.local_addr().is_some();
        let status = if !running {
            ServerStatus::Stopped
        } else if health.iter().any(|h| h.status == CheckStatus::Fail) {
            ServerStatus::Degraded
        } else {
            ServerStatus::Running
        };
        let cfg = e.config();
        let now = now();
        let m = &e.tel().metrics;
        Ok(Overview {
            status,
            server_name: cfg.server_name.clone(),
            addresses: sl_net::lan_ipv4()
                .iter()
                .map(|ip| format!("{ip}:{}", cfg.port))
                .collect(),
            port: cfg.port,
            fingerprint: e.tls().fingerprint.clone(),
            started_at: e.started_at(),
            users_total,
            users_online: e.hub().online_users().len() as u32,
            devices_total,
            devices_ready,
            connections: e.hub().count() as u32,
            messages_last_hour: m.messages_since(now - 3_600_000),
            messages_last_24h: m.messages_since(now - 86_400_000),
            series: m.series(),
            system: e.system_stats(),
            cert_not_after: e.tls().not_after_ms,
            health,
            debug_mode_until: e.tel().logs.debug_until(),
            mdns_active: e.mdns_instance().is_some(),
            active_broadcasts,
        })
    }

    // -----------------------------------------------------------------------------------
    // Users
    // -----------------------------------------------------------------------------------

    pub async fn users(&self) -> AppResult<Vec<UserRow>> {
        let vis = self.e().dir().get();
        let online = self.e().hub().online_users();
        self.e()
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT u.id, u.username, u.display_name, u.disabled, u.created_at, u.last_seen,
                            (SELECT COUNT(*) FROM devices d WHERE d.user_id = u.id AND d.revoked_at IS NULL)
                     FROM users u ORDER BY u.display_name COLLATE NOCASE",
                )?;
                let rows = st.query_map([], |r| {
                    let id: UserId = r.get(0)?;
                    Ok(UserRow {
                        id,
                        username: r.get(1)?,
                        display_name: r.get(2)?,
                        status: if r.get::<_, i64>(3)? != 0 { UserStatus::Disabled } else { UserStatus::Active },
                        created_at: r.get(4)?,
                        last_seen: r.get(5)?,
                        devices: r.get(6)?,
                        groups: vis.group_refs_of(&id),
                        online: online.contains(&id),
                    })
                })?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .await
    }

    pub async fn user_detail(&self, id: UserId) -> AppResult<UserDetail> {
        let user = self
            .users()
            .await?
            .into_iter()
            .find(|u| u.id == id)
            .ok_or(AppError::from(ErrorCode::NotFound))?;
        let hub_devices: BTreeSet<DeviceId> = self
            .e()
            .hub()
            .list()
            .iter()
            .filter(|c| c.user_id == id)
            .map(|c| c.device_id)
            .collect();
        let vis = self.e().dir().get();
        let (devices, sessions, events, reports) = self
            .e()
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT id, name, platform, client_version, created_at, last_seen, revoked_at FROM devices WHERE user_id = ?1 ORDER BY created_at DESC",
                )?;
                let devices: Vec<DeviceRow> = st
                    .query_map([id], |r| {
                        let did: DeviceId = r.get(0)?;
                        Ok(DeviceRow {
                            id: did,
                            name: r.get(1)?,
                            platform: r.get(2)?,
                            client_version: r.get(3)?,
                            created_at: r.get(4)?,
                            last_seen: r.get(5)?,
                            revoked: r.get::<_, Option<i64>>(6)?.is_some(),
                            online: hub_devices.contains(&did),
                        })
                    })?
                    .collect::<Result<_, _>>()?;
                let mut st = c.prepare(
                    "SELECT token_hash, device_id, created_at, last_used, expires_at, ip FROM sessions WHERE user_id = ?1 ORDER BY last_used DESC",
                )?;
                let sessions: Vec<SessionRow> = st
                    .query_map([id], |r| {
                        Ok(SessionRow {
                            id: r.get(0)?,
                            device_id: r.get(1)?,
                            created_at: r.get(2)?,
                            last_used: r.get(3)?,
                            expires_at: r.get(4)?,
                            ip: r.get(5)?,
                        })
                    })?
                    .collect::<Result<_, _>>()?;
                let events = load_events(c, &vis, Some(id), 100)?;
                let mut st = c.prepare(
                    "SELECT received_at, device_id, report FROM health_reports WHERE user_id = ?1 ORDER BY received_at DESC LIMIT 20",
                )?;
                let reports: Vec<HealthReportRow> = st
                    .query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, DeviceId>(1)?, r.get::<_, String>(2)?)))?
                    .filter_map(|r| r.ok())
                    .filter_map(|(received_at, device_id, text)| {
                        serde_json::from_str(&text).ok().map(|report| HealthReportRow { received_at, device_id, report })
                    })
                    .collect();
                Ok((devices, sessions, events, reports))
            })
            .await?;
        Ok(UserDetail {
            user,
            devices,
            sessions,
            recent_events: events,
            health_reports: reports,
        })
    }

    pub async fn set_user_disabled(&self, id: UserId, disabled: bool) -> AppResult<()> {
        let n = self
            .e()
            .db()
            .write(move |c| {
                Ok(c.execute(
                    "UPDATE users SET disabled = ?1 WHERE id = ?2",
                    params![disabled as i64, id],
                )?)
            })
            .await?;
        if n == 0 {
            return api(ErrorCode::NotFound);
        }
        self.e().reload_directory().await?;
        if disabled {
            self.e()
                .hub()
                .close_user(&id, ApiError::new(ErrorCode::AccountDisabled));
        }
        crate::delivery::reconcile(self.e(), &[id]).await?;
        self.e().audit(
            if disabled {
                "user.disable"
            } else {
                "user.enable"
            },
            &self.user_label(id),
            "",
        );
        Ok(())
    }

    pub async fn delete_user(&self, id: UserId) -> AppResult<()> {
        let label = self.user_label(id);
        let convs: Vec<Uuid> = self
            .e()
            .db()
            .write(move |c| {
                let tx = c.transaction()?;
                let convs: Vec<Uuid> = {
                    let mut st = tx.prepare(
                        "SELECT conversation FROM conversation_members WHERE user_id = ?1",
                    )?;
                    st.query_map([id], |r| r.get(0))?
                        .collect::<Result<_, _>>()?
                };
                let n = tx.execute("DELETE FROM users WHERE id = ?1", [id])?;
                if n == 0 {
                    return api(ErrorCode::NotFound);
                }
                tx.commit()?;
                Ok(convs)
            })
            .await?;
        self.e().hub().close_user(
            &id,
            ApiError::with_detail(ErrorCode::AccountDisabled, "account deleted"),
        );
        self.e().reload_directory().await?;
        self.notify_conversations(convs).await?;
        self.e().hub().event_all(Event::DirectoryChanged);
        self.e().audit("user.delete", &label, "");
        Ok(())
    }

    async fn notify_conversations(&self, convs: Vec<Uuid>) -> AppResult<()> {
        let vis = self.e().dir().get();
        let infos = self
            .e()
            .db()
            .read(move |c| {
                let mut out = Vec::new();
                for id in convs {
                    if let Some(i) = crate::delivery::conversation_info(c, &vis, id)? {
                        out.push(i);
                    }
                }
                Ok(out)
            })
            .await?;
        for info in infos {
            self.e()
                .hub()
                .event_users(info.members.iter(), Event::Conversation(info.clone()));
        }
        Ok(())
    }

    pub async fn revoke_device(&self, device: DeviceId) -> AppResult<()> {
        let owner: UserId = self
            .e()
            .db()
            .write(move |c| {
                let tx = c.transaction()?;
                let owner: Option<UserId> = tx
                    .query_row("SELECT user_id FROM devices WHERE id = ?1", [device], |r| {
                        r.get(0)
                    })
                    .optional()?;
                let Some(owner) = owner else {
                    return api(ErrorCode::NotFound);
                };
                tx.execute(
                    "UPDATE devices SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
                    params![now(), device],
                )?;
                tx.execute("DELETE FROM sessions WHERE device_id = ?1", [device])?;
                tx.execute("DELETE FROM key_packages WHERE device_id = ?1", [device])?;
                tx.execute("DELETE FROM welcomes WHERE device_id = ?1", [device])?;
                tx.commit()?;
                Ok(owner)
            })
            .await?;
        self.e().hub().close_device(
            &device,
            ApiError::with_detail(ErrorCode::SessionRevoked, "device removed by admin"),
        );
        crate::delivery::reconcile(self.e(), &[owner]).await?;
        self.e()
            .audit("device.revoke", &self.user_label(owner), device.to_string());
        Ok(())
    }

    pub async fn revoke_session(&self, token_hash: String) -> AppResult<()> {
        let h = token_hash.clone();
        let row: Option<(UserId, DeviceId)> = self
            .e()
            .db()
            .write(move |c| {
                let row = c
                    .query_row(
                        "SELECT user_id, device_id FROM sessions WHERE token_hash = ?1",
                        [&h],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?;
                c.execute("DELETE FROM sessions WHERE token_hash = ?1", [&h])?;
                Ok(row)
            })
            .await?;
        let Some((user, device)) = row else {
            return api(ErrorCode::NotFound);
        };
        self.e()
            .hub()
            .close_device(&device, ApiError::new(ErrorCode::SessionRevoked));
        self.e().audit("session.revoke", &self.user_label(user), "");
        Ok(())
    }

    pub async fn sign_out_everywhere(&self, user: UserId) -> AppResult<()> {
        self.e()
            .db()
            .write(move |c| {
                c.execute("DELETE FROM sessions WHERE user_id = ?1", [user])?;
                Ok(())
            })
            .await?;
        self.e()
            .hub()
            .close_user(&user, ApiError::new(ErrorCode::SessionRevoked));
        self.e()
            .audit("user.sign_out_everywhere", &self.user_label(user), "");
        Ok(())
    }

    pub async fn set_user_groups(&self, user: UserId, groups: Vec<GroupId>) -> AppResult<()> {
        let groups: BTreeSet<GroupId> = groups.into_iter().collect();
        self.e()
            .db()
            .write(move |c| {
                let tx = c.transaction()?;
                tx.execute("DELETE FROM group_members WHERE user_id = ?1", [user])?;
                for g in &groups {
                    tx.execute(
                        "INSERT INTO group_members (group_id, user_id, added_at) VALUES (?1, ?2, ?3)",
                        params![g, user, now()],
                    )?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        self.after_membership_change(&[user]).await?;
        self.e()
            .audit("user.set_groups", &self.user_label(user), "");
        Ok(())
    }

    fn user_label(&self, id: UserId) -> String {
        self.e()
            .dir()
            .get()
            .users
            .get(&id)
            .map(|u| u.username.clone())
            .unwrap_or_else(|| id.to_string())
    }

    async fn after_membership_change(&self, users: &[UserId]) -> AppResult<()> {
        self.e().reload_directory().await?;
        crate::delivery::reconcile(self.e(), users).await?;
        // Each affected user's own profile (group list) changed.
        let vis = self.e().dir().get();
        for u in users {
            if let Some(b) = vis.users.get(u) {
                let profile = sl_proto::SelfUser {
                    id: *u,
                    username: b.username.clone(),
                    display_name: b.display_name.clone(),
                    groups: vis.group_refs_of(u),
                };
                self.e()
                    .hub()
                    .event_users(std::iter::once(u), Event::Profile(profile));
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------------------
    // Groups
    // -----------------------------------------------------------------------------------

    pub async fn groups(&self) -> AppResult<Vec<GroupRow>> {
        self.e()
            .db()
            .read(|c| {
                let mut st = c.prepare(
                    "SELECT g.id, g.name, g.description, g.created_at,
                            (SELECT COUNT(*) FROM group_members m WHERE m.group_id = g.id)
                     FROM groups g ORDER BY g.name COLLATE NOCASE",
                )?;
                let rows = st.query_map([], |r| {
                    Ok(GroupRow {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        description: r.get(2)?,
                        created_at: r.get(3)?,
                        member_count: r.get(4)?,
                    })
                })?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .await
    }

    pub async fn group_detail(&self, id: GroupId) -> AppResult<GroupDetail> {
        let group = self
            .groups()
            .await?
            .into_iter()
            .find(|g| g.id == id)
            .ok_or(AppError::from(ErrorCode::NotFound))?;
        let members = self
            .e()
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT u.id, u.username, u.display_name FROM users u JOIN group_members m ON m.user_id = u.id
                     WHERE m.group_id = ?1 ORDER BY u.display_name COLLATE NOCASE",
                )?;
                let rows = st.query_map([id], |r| Ok(UserRef { id: r.get(0)?, username: r.get(1)?, display_name: r.get(2)? }))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .await?;
        Ok(GroupDetail { group, members })
    }

    pub async fn create_group(&self, name: String, description: String) -> AppResult<GroupRow> {
        let name = name.trim().to_string();
        if name.is_empty() || name.chars().count() > 64 {
            return api_detail(ErrorCode::BadRequest, "group names are 1–64 characters");
        }
        let id = Uuid::now_v7();
        let (n, d) = (name.clone(), description.trim().to_string());
        self.e()
            .db()
            .write(move |c| {
                c.execute(
                    "INSERT INTO groups (id, name, description, created_at) VALUES (?1, ?2, ?3, ?4)",
                    params![id, n, d, now()],
                )
                .map_err(unique_to_exists)?;
                Ok(())
            })
            .await?;
        self.e().reload_directory().await?;
        self.e().audit("group.create", &name, "");
        self.groups()
            .await?
            .into_iter()
            .find(|g| g.id == id)
            .ok_or(AppError::from(ErrorCode::Internal))
    }

    pub async fn update_group(
        &self,
        id: GroupId,
        name: String,
        description: String,
    ) -> AppResult<()> {
        let name = name.trim().to_string();
        if name.is_empty() || name.chars().count() > 64 {
            return api_detail(ErrorCode::BadRequest, "group names are 1–64 characters");
        }
        let (n, d) = (name.clone(), description.trim().to_string());
        let changed = self
            .e()
            .db()
            .write(move |c| {
                c.execute(
                    "UPDATE groups SET name = ?1, description = ?2 WHERE id = ?3",
                    params![n, d, id],
                )
                .map_err(unique_to_exists)
            })
            .await?;
        if changed == 0 {
            return api(ErrorCode::NotFound);
        }
        self.e().reload_directory().await?;
        let members: Vec<UserId> = self
            .e()
            .dir()
            .get()
            .group_users
            .get(&id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        self.after_membership_change(&members).await?;
        self.e().audit("group.update", &name, "");
        Ok(())
    }

    pub async fn delete_group(&self, id: GroupId) -> AppResult<()> {
        let members: Vec<UserId> = self
            .e()
            .dir()
            .get()
            .group_users
            .get(&id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        let name = self
            .e()
            .dir()
            .get()
            .group_names
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let n = self
            .e()
            .db()
            .write(move |c| Ok(c.execute("DELETE FROM groups WHERE id = ?1", [id])?))
            .await?;
        if n == 0 {
            return api(ErrorCode::NotFound);
        }
        self.after_membership_change(&members).await?;
        self.e().audit("group.delete", &name, "");
        Ok(())
    }

    pub async fn set_group_members(&self, id: GroupId, users: Vec<UserId>) -> AppResult<()> {
        let new: BTreeSet<UserId> = users.into_iter().collect();
        let old: BTreeSet<UserId> = self
            .e()
            .dir()
            .get()
            .group_users
            .get(&id)
            .cloned()
            .unwrap_or_default();
        let new_c = new.clone();
        self.e()
            .db()
            .write(move |c| {
                let tx = c.transaction()?;
                let exists: Option<i64> = tx.query_row("SELECT 1 FROM groups WHERE id = ?1", [id], |r| r.get(0)).optional()?;
                if exists.is_none() {
                    return api(ErrorCode::NotFound);
                }
                tx.execute("DELETE FROM group_members WHERE group_id = ?1", [id])?;
                for u in &new_c {
                    tx.execute("INSERT INTO group_members (group_id, user_id, added_at) VALUES (?1, ?2, ?3)", params![id, u, now()])?;
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        let changed: Vec<UserId> = old.symmetric_difference(&new).copied().collect();
        self.after_membership_change(&changed).await?;
        let name = self
            .e()
            .dir()
            .get()
            .group_names
            .get(&id)
            .cloned()
            .unwrap_or_default();
        self.e()
            .audit("group.set_members", &name, format!("{} members", new.len()));
        Ok(())
    }

    // -----------------------------------------------------------------------------------
    // Invites
    // -----------------------------------------------------------------------------------

    pub async fn invites(&self) -> AppResult<Vec<InviteRow>> {
        let vis = self.e().dir().get();
        self.e()
            .db()
            .read(move |c| {
                let now = now();
                let mut st = c.prepare(
                    "SELECT id, kind, target_user, used_by, note, created_at, expires_at, used_at, revoked_at
                     FROM invites ORDER BY created_at DESC LIMIT 1000",
                )?;
                let rows: Vec<InviteDbRow> = st
                    .query_map([], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?))
                    })?
                    .collect::<Result<_, _>>()?;
                let mut gst = c.prepare("SELECT group_id FROM invite_groups WHERE invite = ?1")?;
                let user_ref = |u: UserId| {
                    vis.users.get(&u).map(|b| UserRef { id: u, username: b.username.clone(), display_name: b.display_name.clone() })
                };
                let mut out = Vec::with_capacity(rows.len());
                for (id, kind, target, used_by, note, created_at, expires_at, used_at, revoked_at) in rows {
                    let gids: Vec<GroupId> = gst.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
                    out.push(InviteRow {
                        id,
                        kind: crate::auth::kind_from_str(&kind),
                        groups: gids
                            .into_iter()
                            .map(|g| GroupRef { id: g, name: vis.group_names.get(&g).cloned().unwrap_or_default() })
                            .collect(),
                        target_user: target.and_then(user_ref),
                        used_by: used_by.and_then(user_ref),
                        note,
                        created_at,
                        expires_at,
                        status: crate::auth::invite_status(expires_at, used_at, revoked_at, now),
                    });
                }
                Ok(out)
            })
            .await
    }

    fn invite_message(
        &self,
        display_code: &str,
        link: &str,
        expires_in_secs: i64,
        setup: bool,
    ) -> String {
        let cfg = self.e().config();
        let hosts: Vec<String> = sl_net::lan_ipv4()
            .iter()
            .map(|ip| format!("{ip}:{}", cfg.port))
            .collect();
        let expiry = describe_duration(expires_in_secs);
        let intro = if setup {
            "Here is your SpareLine setup code. Use it to sign in on a new device or to choose a new password."
        } else {
            "You're invited to SpareLine, our backup chat for when our usual chat, email or other cloud services are down."
        };
        let step2 = if setup {
            "Open SpareLine and choose \"Use a setup code\"."
        } else {
            "Open SpareLine and choose \"I have an invite\"."
        };
        format!(
            "{intro}\n\n1. Install SpareLine (ask IT if it isn't on your computer).\n2. {step2}\n3. Enter this code: {display_code}\n\nServer: {name}\nIf SpareLine doesn't find the server automatically, use: {hosts}\n\nOr open this link on your work computer: {link}\n\nThe code works once and expires in {expiry}. Keep it private.",
            name = cfg.server_name,
            hosts = if hosts.is_empty() {
                "(ask your admin)".to_string()
            } else {
                hosts.join(" or ")
            },
        )
    }

    pub async fn create_invites(&self, input: CreateInvitesInput) -> AppResult<Vec<CreatedInvite>> {
        if input.count == 0 || input.count > 200 {
            return api_detail(
                ErrorCode::BadRequest,
                "create between 1 and 200 invites at a time",
            );
        }
        if !(300..=30 * 86_400).contains(&input.expires_in_secs) {
            return api_detail(
                ErrorCode::BadRequest,
                "expiry must be between 5 minutes and 30 days",
            );
        }
        let vis = self.e().dir().get();
        for g in &input.group_ids {
            if !vis.group_names.contains_key(g) {
                return api(ErrorCode::NotFound);
            }
        }
        let note = input.note.trim().chars().take(200).collect::<String>();
        let mut codes = Vec::with_capacity(input.count as usize);
        for _ in 0..input.count {
            let code = sl_proto::crypto::generate_code();
            let keys = sl_proto::crypto::derive_invite(&code);
            codes.push((Uuid::now_v7(), code, keys));
        }
        let created_at = now();
        let expires_at = created_at + input.expires_in_secs * 1000;
        let rows: Vec<(Uuid, String, String)> = codes
            .iter()
            .map(|(id, c, k)| (*id, c.clone(), k.invite_id.clone()))
            .collect();
        let keys: Vec<(Uuid, String, [u8; 32])> = codes
            .iter()
            .map(|(id, _, k)| (*id, k.invite_id.clone(), k.proof_key))
            .collect();
        let groups = input.group_ids.clone();
        let note_c = note.clone();
        self.e()
            .db()
            .write(move |c| {
                let tx = c.transaction()?;
                for (id, invite_id, proof_key) in &keys {
                    tx.execute(
                        "INSERT INTO invites (id, invite_id, proof_key, kind, note, created_at, expires_at) VALUES (?1, ?2, ?3, 'new_user', ?4, ?5, ?6)",
                        params![id, invite_id, proof_key.as_slice(), note_c, created_at, expires_at],
                    )?;
                    for g in &groups {
                        tx.execute("INSERT INTO invite_groups (invite, group_id) VALUES (?1, ?2)", params![id, g])?;
                    }
                }
                tx.commit()?;
                Ok(())
            })
            .await?;
        let group_refs: Vec<GroupRef> = input
            .group_ids
            .iter()
            .map(|g| GroupRef {
                id: *g,
                name: vis.group_names.get(g).cloned().unwrap_or_default(),
            })
            .collect();
        let hosts: Vec<String> = sl_net::lan_ipv4().iter().map(|ip| ip.to_string()).collect();
        let port = self.e().config().port;
        let out = rows
            .into_iter()
            .map(|(id, code, _)| {
                let display = sl_proto::code::display(&code);
                let link = sl_proto::code::join_link(&code, &hosts, port);
                CreatedInvite {
                    row: InviteRow {
                        id,
                        kind: InviteKind::NewUser,
                        groups: group_refs.clone(),
                        target_user: None,
                        used_by: None,
                        note: note.clone(),
                        created_at,
                        expires_at,
                        status: InviteStatus::Pending,
                    },
                    message: self.invite_message(&display, &link, input.expires_in_secs, false),
                    code: display,
                    link,
                }
            })
            .collect();
        self.e().audit(
            "invite.create",
            &format!("{} invite(s)", input.count),
            group_refs
                .iter()
                .map(|g| g.name.clone())
                .collect::<Vec<_>>()
                .join(", "),
        );
        Ok(out)
    }

    pub async fn create_setup_code(
        &self,
        user: UserId,
        expires_in_secs: i64,
    ) -> AppResult<CreatedInvite> {
        if !(300..=7 * 86_400).contains(&expires_in_secs) {
            return api_detail(
                ErrorCode::BadRequest,
                "expiry must be between 5 minutes and 7 days",
            );
        }
        let vis = self.e().dir().get();
        let Some(b) = vis.users.get(&user).cloned() else {
            return api(ErrorCode::NotFound);
        };
        let code = sl_proto::crypto::generate_code();
        let keys = sl_proto::crypto::derive_invite(&code);
        let id = Uuid::now_v7();
        let created_at = now();
        let expires_at = created_at + expires_in_secs * 1000;
        self.e()
            .db()
            .write(move |c| {
                // Only one live setup code per user.
                c.execute(
                    "UPDATE invites SET revoked_at = ?1 WHERE kind = 'setup' AND target_user = ?2 AND used_at IS NULL AND revoked_at IS NULL",
                    params![created_at, user],
                )?;
                c.execute(
                    "INSERT INTO invites (id, invite_id, proof_key, kind, target_user, created_at, expires_at) VALUES (?1, ?2, ?3, 'setup', ?4, ?5, ?6)",
                    params![id, keys.invite_id, keys.proof_key.as_slice(), user, created_at, expires_at],
                )?;
                Ok(())
            })
            .await?;
        let hosts: Vec<String> = sl_net::lan_ipv4().iter().map(|ip| ip.to_string()).collect();
        let display = sl_proto::code::display(&code);
        let link = sl_proto::code::join_link(&code, &hosts, self.e().config().port);
        self.e().audit("invite.setup_code", &b.username, "");
        Ok(CreatedInvite {
            row: InviteRow {
                id,
                kind: InviteKind::Setup,
                groups: vec![],
                target_user: Some(UserRef {
                    id: user,
                    username: b.username.clone(),
                    display_name: b.display_name.clone(),
                }),
                used_by: None,
                note: String::new(),
                created_at,
                expires_at,
                status: InviteStatus::Pending,
            },
            message: self.invite_message(&display, &link, expires_in_secs, true),
            code: display,
            link,
        })
    }

    pub async fn revoke_invite(&self, id: Uuid) -> AppResult<()> {
        let n = self
            .e()
            .db()
            .write(move |c| {
                Ok(c.execute(
                    "UPDATE invites SET revoked_at = ?1 WHERE id = ?2 AND used_at IS NULL AND revoked_at IS NULL",
                    params![now(), id],
                )?)
            })
            .await?;
        if n == 0 {
            return api(ErrorCode::NotFound);
        }
        self.e().audit("invite.revoke", &id.to_string(), "");
        Ok(())
    }

    // -----------------------------------------------------------------------------------
    // Broadcasts
    // -----------------------------------------------------------------------------------

    pub async fn broadcasts(&self) -> AppResult<Vec<BroadcastRow>> {
        crate::broadcast::list(self.e()).await
    }
    pub async fn create_broadcast(&self, input: BroadcastInput) -> AppResult<()> {
        crate::broadcast::create(self.e(), input).await.map(|_| ())
    }
    pub async fn end_broadcast(&self, id: Uuid) -> AppResult<()> {
        crate::broadcast::end(self.e(), id).await
    }

    // -----------------------------------------------------------------------------------
    // Monitoring
    // -----------------------------------------------------------------------------------

    pub fn logs(&self, q: &LogQuery) -> Vec<LogEntry> {
        self.e().tel().logs.query(q)
    }

    pub fn subscribe_logs(&self) -> tokio::sync::broadcast::Receiver<LogEntry> {
        self.e().tel().logs.subscribe()
    }

    pub fn connections(&self) -> Vec<ConnectionRow> {
        let mut rows: Vec<ConnectionRow> = self
            .e()
            .hub()
            .list()
            .into_iter()
            .map(|c| ConnectionRow {
                conn_id: c.id,
                user: UserRef {
                    id: c.user_id,
                    username: c.username.clone(),
                    display_name: c.display_name.clone(),
                },
                device_id: c.device_id,
                device_name: c.device_name.clone(),
                ip: c.ip.clone(),
                connected_at: c.connected_at,
                rtt_ms: c.rtt(),
                client_version: c.client_version.clone(),
                platform: c.platform.clone(),
            })
            .collect();
        rows.sort_by_key(|a| a.user.display_name.to_lowercase());
        rows
    }

    pub async fn connection_events(
        &self,
        user: Option<UserId>,
        limit: u32,
    ) -> AppResult<Vec<ConnectionEvent>> {
        let vis = self.e().dir().get();
        self.e()
            .db()
            .read(move |c| load_events(c, &vis, user, limit.clamp(1, 1000)))
            .await
    }

    pub async fn health(&self) -> Vec<HealthCheck> {
        let checks = crate::health::run_checks(self.e()).await;
        self.e().set_cached_health(checks.clone());
        checks
    }

    pub async fn run_fix(&self, id: &str) -> AppResult<String> {
        crate::health::run_fix(self.e(), id).await
    }

    pub async fn self_test(&self) -> Vec<SelfTestStep> {
        self.e().audit("diagnostics.self_test", "", "");
        crate::health::self_test(self.e()).await
    }

    /// Turn on debug logging for `minutes` (1–240), or off with 0.
    pub fn set_debug_mode(&self, minutes: u32) {
        let until = (minutes > 0).then(|| now() + i64::from(minutes.min(240)) * 60_000);
        self.e().tel().logs.set_debug_until(until);
        self.e().audit(
            "diagnostics.debug_mode",
            if until.is_some() { "on" } else { "off" },
            format!("{minutes} min"),
        );
    }

    pub async fn audit_log(&self, limit: u32) -> AppResult<Vec<AuditRow>> {
        self.e()
            .db()
            .read(move |c| {
                let mut st = c.prepare(
                    "SELECT ts, action, target, detail FROM audit_log ORDER BY id DESC LIMIT ?1",
                )?;
                let rows = st.query_map([limit.clamp(1, 1000)], |r| {
                    Ok(AuditRow {
                        ts: r.get(0)?,
                        action: r.get(1)?,
                        target: r.get(2)?,
                        detail: r.get(3)?,
                    })
                })?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .await
    }

    /// Write a plain-text diagnostics report (no message content exists on the server).
    pub async fn diagnostics_report(&self) -> AppResult<PathBuf> {
        let e = self.e();
        let overview = self.overview().await?;
        let events = self.connection_events(None, 300).await?;
        let cfg = e.config();
        let mut out = String::new();
        use std::fmt::Write as _;
        let _ = writeln!(out, "SpareLine Server diagnostics report");
        let _ = writeln!(out, "Generated: {} (unix ms)", now());
        let _ = writeln!(
            out,
            "Version: {} · protocol {}",
            env!("CARGO_PKG_VERSION"),
            sl_proto::PROTOCOL_VERSION
        );
        let _ = writeln!(
            out,
            "OS: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        let _ = writeln!(out, "\n== Configuration (redacted) ==");
        let _ = writeln!(
            out,
            "server_name = {}\nport = {}\nbind = {}\nmdns_enabled = {}\nenvelope_retention_days = {}\nsession_idle_days = {}",
            cfg.server_name,
            cfg.port,
            cfg.bind,
            cfg.mdns_enabled,
            cfg.envelope_retention_days,
            cfg.session_idle_days
        );
        let _ = writeln!(
            out,
            "\n== Status ==\n{:?} · {} connection(s) · {} user(s) online · fingerprint {}",
            overview.status, overview.connections, overview.users_online, overview.fingerprint
        );
        let _ = writeln!(out, "Addresses: {}", overview.addresses.join(", "));
        let _ = writeln!(out, "\n== Health checks ==");
        for h in &overview.health {
            let _ = writeln!(out, "[{:?}] {}: {}", h.status, h.name, h.detail);
        }
        let _ = writeln!(out, "\n== Recent connection events ==");
        for ev in &events {
            let who = ev.user.as_ref().map(|u| u.username.as_str()).unwrap_or("-");
            let _ = writeln!(
                out,
                "{} {:?} ip={} user={} {}",
                ev.ts, ev.kind, ev.ip, who, ev.detail
            );
        }
        let _ = writeln!(out, "\n== Recent log ==\n{}", e.tel().logs.dump(3000));
        let path = e
            .paths()
            .reports
            .join(format!("spareline-diagnostics-{}.txt", now()));
        tokio::fs::write(&path, out).await?;
        e.audit("diagnostics.report", &path.to_string_lossy(), "");
        Ok(path)
    }

    // -----------------------------------------------------------------------------------
    // Settings, backups, admin password
    // -----------------------------------------------------------------------------------

    pub fn settings(&self) -> ServerSettings {
        let c = self.e().config();
        ServerSettings {
            server_name: c.server_name.clone(),
            port: c.port,
            envelope_retention_days: c.envelope_retention_days,
            log_retention_days: c.log_retention_days,
            backup_keep: c.backup_keep,
            session_idle_days: c.session_idle_days,
        }
    }

    /// Returns true if a restart is needed (port change).
    pub async fn update_settings(&self, s: ServerSettings) -> AppResult<bool> {
        let name = s.server_name.trim().to_string();
        if name.is_empty() || name.chars().count() > 64 {
            return api_detail(ErrorCode::BadRequest, "server name is 1–64 characters");
        }
        if s.port < 1024 {
            return api_detail(ErrorCode::BadRequest, "use a port of 1024 or higher");
        }
        let restart = self.e().update_config(|c| {
            let restart = c.port != s.port;
            c.server_name = name.clone();
            c.port = s.port;
            c.envelope_retention_days = s.envelope_retention_days.clamp(1, 365);
            c.log_retention_days = s.log_retention_days.clamp(1, 365);
            c.backup_keep = s.backup_keep.clamp(1, 60);
            c.session_idle_days = s.session_idle_days.clamp(1, 365);
            restart
        })?;
        self.e().restart_mdns();
        self.e().audit(
            "settings.update",
            "",
            if restart {
                "port changed (restart required)"
            } else {
                ""
            },
        );
        Ok(restart)
    }

    pub fn backups(&self) -> Vec<BackupRow> {
        list_backup_files(&self.e().paths().backups)
    }

    pub async fn backup_now(&self) -> AppResult<BackupRow> {
        let row = crate::retention::backup(self.e()).await?;
        self.e().audit("backup.create", &row.file_name, "");
        Ok(row)
    }

    // ---- standby copies -----------------------------------------------------------

    pub fn standby_status(&self) -> StandbyStatus {
        crate::standby::status(self.e())
    }

    /// Turn on (or change) standby copies, then make the first copy straight away.
    pub async fn standby_setup(&self, input: StandbySetupInput) -> AppResult<StandbyStatus> {
        let folder = input.folder.trim().to_string();
        if folder.is_empty() {
            return api_detail(ErrorCode::BadRequest, "choose a folder for standby copies");
        }
        let path = PathBuf::from(&folder);
        if !path.is_dir() {
            return api_detail(
                ErrorCode::BadRequest,
                "that folder doesn't exist or can't be reached",
            );
        }
        let probe = path.join(".spareline-write-test");
        if let Err(e) = std::fs::write(&probe, b"ok") {
            return api_detail(
                ErrorCode::BadRequest,
                format!("SpareLine Server can't save files in that folder ({e})"),
            );
        }
        let _ = std::fs::remove_file(&probe);
        if !input.password.is_empty() {
            crate::standby::set_password(self.e().paths(), input.password).await?;
        } else if !crate::standby::has_password(self.e().paths()) {
            return api_detail(
                ErrorCode::WeakPassword,
                "choose a standby password. You'll need it to restore a copy",
            );
        }
        let f = folder.clone();
        self.e().update_config(|c| {
            c.standby_folder = f;
            c.standby_interval_minutes = input.interval_minutes.clamp(5, 24 * 60);
            c.standby_keep = input.keep.clamp(1, 50);
        })?;
        self.e().audit("standby.setup", "", folder);
        // A failure here is shown in the status rather than failing the setup.
        let _ = crate::standby::take_copy(self.e()).await;
        Ok(self.standby_status())
    }

    pub async fn standby_copy_now(&self) -> AppResult<StandbyCopyRow> {
        crate::standby::take_copy(self.e()).await
    }

    pub fn standby_turn_off(&self) -> AppResult<()> {
        self.e().update_config(|c| c.standby_folder.clear())?;
        self.e().audit("standby.off", "", "");
        Ok(())
    }

    pub async fn change_admin_password(&self, current: String, new: String) -> AppResult<()> {
        if !self.e().verify_admin(&current).await {
            return api(ErrorCode::InvalidCredentials);
        }
        self.e().set_admin_password(&new).await?;
        self.e().audit("admin.password_change", "", "");
        Ok(())
    }

    /// Reset the panel password with the recovery key. Returns a new recovery key.
    pub async fn reset_admin_password(
        &self,
        recovery_key: String,
        new: String,
    ) -> AppResult<String> {
        self.e()
            .reset_admin_with_recovery(&recovery_key, &new)
            .await
    }
}

/// (id, kind, target_user, used_by, note, created_at, expires_at, used_at, revoked_at)
type InviteDbRow = (
    Uuid,
    String,
    Option<UserId>,
    Option<UserId>,
    String,
    i64,
    i64,
    Option<i64>,
    Option<i64>,
);

fn unique_to_exists(e: rusqlite::Error) -> AppError {
    match e {
        rusqlite::Error::SqliteFailure(f, _)
            if f.code == rusqlite::ErrorCode::ConstraintViolation =>
        {
            AppError::Api(ApiError::with_detail(
                ErrorCode::AlreadyExists,
                "a group with that name already exists",
            ))
        }
        other => other.into(),
    }
}

fn load_events(
    c: &rusqlite::Connection,
    vis: &crate::directory::Visibility,
    user: Option<UserId>,
    limit: u32,
) -> AppResult<Vec<ConnectionEvent>> {
    let sql = if user.is_some() {
        "SELECT ts, ip, user_id, device_id, kind, detail FROM connection_events WHERE user_id = ?1 ORDER BY id DESC LIMIT ?2"
    } else {
        "SELECT ts, ip, user_id, device_id, kind, detail FROM connection_events WHERE ?1 IS NULL ORDER BY id DESC LIMIT ?2"
    };
    let mut st = c.prepare(sql)?;
    let rows = st.query_map(params![user, limit], |r| {
        let uid: Option<UserId> = r.get(2)?;
        Ok(ConnectionEvent {
            ts: r.get(0)?,
            ip: r.get(1)?,
            user: uid.and_then(|u| {
                vis.users.get(&u).map(|b| UserRef {
                    id: u,
                    username: b.username.clone(),
                    display_name: b.display_name.clone(),
                })
            }),
            device_id: r.get(3)?,
            kind: parse_event_kind(&r.get::<_, String>(4)?),
            detail: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn event_kind_str(k: ConnEventKind) -> &'static str {
    match k {
        ConnEventKind::TcpAccepted => "tcp_accepted",
        ConnEventKind::TlsFailed => "tls_failed",
        ConnEventKind::Connected => "connected",
        ConnEventKind::Disconnected => "disconnected",
        ConnEventKind::AuthFailed => "auth_failed",
        ConnEventKind::Registered => "registered",
        ConnEventKind::ProtocolMismatch => "protocol_mismatch",
        ConnEventKind::RateLimited => "rate_limited",
        ConnEventKind::SessionRevoked => "session_revoked",
    }
}

fn parse_event_kind(s: &str) -> ConnEventKind {
    match s {
        "tcp_accepted" => ConnEventKind::TcpAccepted,
        "tls_failed" => ConnEventKind::TlsFailed,
        "connected" => ConnEventKind::Connected,
        "disconnected" => ConnEventKind::Disconnected,
        "registered" => ConnEventKind::Registered,
        "protocol_mismatch" => ConnEventKind::ProtocolMismatch,
        "rate_limited" => ConnEventKind::RateLimited,
        "session_revoked" => ConnEventKind::SessionRevoked,
        _ => ConnEventKind::AuthFailed,
    }
}

pub fn list_backup_files(dir: &Path) -> Vec<BackupRow> {
    let mut out: Vec<BackupRow> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".db"))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            let created = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Some(BackupRow {
                file_name: e.file_name().to_string_lossy().to_string(),
                created_at: created,
                size: meta.len(),
            })
        })
        .collect();
    out.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    out
}

fn describe_duration(secs: i64) -> String {
    if secs >= 2 * 86_400 {
        format!("{} days", secs / 86_400)
    } else if secs >= 7200 {
        format!("{} hours", secs / 3600)
    } else {
        format!("{} minutes", (secs / 60).max(1))
    }
}
