//! Registry of live, authenticated connections and fan-out of frames.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};
use sl_proto::wire::{Event, ServerFrame};
use sl_proto::{ApiError, DeviceId, Presence, Timestamp, UserId};
use tokio::sync::mpsc;

pub type ConnId = u64;

/// Outbound queue capacity per connection. A client that falls this far behind is
/// disconnected (it will reconnect and resync) rather than slowing everyone down.
pub const OUTBOUND_CAPACITY: usize = 1024;

#[derive(Debug)]
pub enum Outbound {
    Text(Arc<str>),
    Close(ApiError),
}

#[derive(Debug)]
pub struct Conn {
    pub id: ConnId,
    pub user_id: UserId,
    pub device_id: DeviceId,
    pub username: String,
    pub display_name: String,
    pub device_name: String,
    pub platform: String,
    pub client_version: String,
    pub ip: String,
    pub connected_at: Timestamp,
    /// 0 = unknown.
    pub rtt_ms: AtomicU32,
    pub presence: Mutex<Presence>,
    pub tx: mpsc::Sender<Outbound>,
}

impl Conn {
    pub fn rtt(&self) -> Option<u32> {
        match self.rtt_ms.load(Ordering::Relaxed) {
            0 => None,
            v => Some(v),
        }
    }
}

#[derive(Default)]
struct Inner {
    conns: HashMap<ConnId, Arc<Conn>>,
    by_device: HashMap<DeviceId, ConnId>,
    by_user: HashMap<UserId, HashSet<ConnId>>,
}

#[derive(Default)]
pub struct Hub {
    inner: RwLock<Inner>,
    next_id: AtomicU64,
}

pub fn encode(frame: &ServerFrame) -> Arc<str> {
    match serde_json::to_string(frame) {
        Ok(s) => Arc::from(s),
        Err(e) => {
            tracing::error!(error = %e, "failed to encode frame");
            Arc::from("{}")
        }
    }
}

impl Hub {
    pub fn next_conn_id(&self) -> ConnId {
        self.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Register an authenticated connection. Returns a previous connection of the same
    /// device, which the caller should close (one live connection per device).
    pub fn register(&self, conn: Arc<Conn>) -> Option<Arc<Conn>> {
        let mut g = self.inner.write();
        let old = g
            .by_device
            .insert(conn.device_id, conn.id)
            .and_then(|old| g.conns.get(&old).cloned());
        g.by_user.entry(conn.user_id).or_default().insert(conn.id);
        g.conns.insert(conn.id, conn);
        old
    }

    pub fn unregister(&self, id: ConnId) -> Option<Arc<Conn>> {
        let mut g = self.inner.write();
        let conn = g.conns.remove(&id)?;
        if g.by_device.get(&conn.device_id) == Some(&id) {
            g.by_device.remove(&conn.device_id);
        }
        if let Some(set) = g.by_user.get_mut(&conn.user_id) {
            set.remove(&id);
            if set.is_empty() {
                g.by_user.remove(&conn.user_id);
            }
        }
        Some(conn)
    }

    pub fn count(&self) -> usize {
        self.inner.read().conns.len()
    }

    pub fn list(&self) -> Vec<Arc<Conn>> {
        self.inner.read().conns.values().cloned().collect()
    }

    pub fn is_user_online(&self, user: &UserId) -> bool {
        self.inner.read().by_user.contains_key(user)
    }

    pub fn is_device_online(&self, device: &DeviceId) -> bool {
        self.inner.read().by_device.contains_key(device)
    }

    pub fn online_users(&self) -> HashSet<UserId> {
        self.inner.read().by_user.keys().copied().collect()
    }

    /// Aggregate presence: Online if any connection is online, Away if all are away.
    pub fn presence(&self, user: &UserId) -> Presence {
        let g = self.inner.read();
        let Some(ids) = g.by_user.get(user) else {
            return Presence::Offline;
        };
        let mut any_away = false;
        for id in ids {
            if let Some(c) = g.conns.get(id) {
                match *c.presence.lock() {
                    Presence::Online => return Presence::Online,
                    Presence::Away => any_away = true,
                    Presence::Offline => {}
                }
            }
        }
        if any_away {
            Presence::Away
        } else {
            Presence::Offline
        }
    }

    fn deliver(conn: &Conn, msg: Outbound) {
        match conn.tx.try_send(msg) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(target: "sl_server::ws", conn = conn.id, user = %conn.username, "client too slow; disconnecting");
                // Best effort: the writer will notice the closed channel.
                let _ = conn.tx.try_send(Outbound::Close(ApiError::with_detail(
                    sl_proto::ErrorCode::Internal,
                    "outbound queue full",
                )));
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    pub fn send_conn(&self, id: ConnId, text: Arc<str>) {
        if let Some(c) = self.inner.read().conns.get(&id) {
            Self::deliver(c, Outbound::Text(text));
        }
    }

    pub fn send_device(&self, device: &DeviceId, text: Arc<str>) {
        let g = self.inner.read();
        if let Some(c) = g.by_device.get(device).and_then(|id| g.conns.get(id)) {
            Self::deliver(c, Outbound::Text(text));
        }
    }

    pub fn send_devices<'a>(
        &self,
        devices: impl IntoIterator<Item = &'a DeviceId>,
        text: Arc<str>,
    ) {
        let g = self.inner.read();
        for d in devices {
            if let Some(c) = g.by_device.get(d).and_then(|id| g.conns.get(id)) {
                Self::deliver(c, Outbound::Text(text.clone()));
            }
        }
    }

    pub fn send_user(&self, user: &UserId, text: Arc<str>) {
        self.send_users(std::iter::once(user), text);
    }

    pub fn send_users<'a>(&self, users: impl IntoIterator<Item = &'a UserId>, text: Arc<str>) {
        let g = self.inner.read();
        for u in users {
            if let Some(ids) = g.by_user.get(u) {
                for id in ids {
                    if let Some(c) = g.conns.get(id) {
                        Self::deliver(c, Outbound::Text(text.clone()));
                    }
                }
            }
        }
    }

    pub fn send_all(&self, text: Arc<str>) {
        let g = self.inner.read();
        for c in g.conns.values() {
            Self::deliver(c, Outbound::Text(text.clone()));
        }
    }

    pub fn event_users<'a>(&self, users: impl IntoIterator<Item = &'a UserId>, event: Event) {
        self.send_users(users, encode(&ServerFrame::Event(event)));
    }

    pub fn event_all(&self, event: Event) {
        self.send_all(encode(&ServerFrame::Event(event)));
    }

    /// Close every connection of a user (disabled, deleted, sessions revoked).
    pub fn close_user(&self, user: &UserId, err: ApiError) {
        let g = self.inner.read();
        if let Some(ids) = g.by_user.get(user) {
            for id in ids {
                if let Some(c) = g.conns.get(id) {
                    Self::deliver(c, Outbound::Close(err.clone()));
                }
            }
        }
    }

    pub fn close_device(&self, device: &DeviceId, err: ApiError) {
        let g = self.inner.read();
        if let Some(c) = g.by_device.get(device).and_then(|id| g.conns.get(id)) {
            Self::deliver(c, Outbound::Close(err));
        }
    }

    pub fn close_all(&self, err: ApiError) {
        let g = self.inner.read();
        for c in g.conns.values() {
            Self::deliver(c, Outbound::Close(err.clone()));
        }
    }
}
