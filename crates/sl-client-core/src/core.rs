//! The client core: a single-threaded actor that owns MLS state and the local store.
//! UI calls arrive as jobs; server events arrive from the transport.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use rand::Rng;
use sl_net::{PinMode, PinVerifier, WsStream};
use sl_proto::client_ipc::*;
use sl_proto::wire::*;
use sl_proto::*;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::error::{ClientError, ClientResult};
use crate::mls::{Mls, PendingCommit, Processed};
use crate::payload::{self, Payload};
use crate::profile::{Profile, SecretStore};
use crate::store::{LocalConv, LocalMessage, Store};
use crate::transport::{Incoming, Transport, recv_raw, send_raw};

const DIAL_TIMEOUT: Duration = Duration::from_secs(4);
const KP_TARGET: usize = 50;
const KP_LOW: u32 = 20;
const SECRET_SESSION: &str = "session";
const SECRET_DBKEY: &str = "dbkey";

pub struct ClientOptions {
    pub dir: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    pub device_name: String,
    pub platform: String,
    pub client_version: String,
    /// Use mDNS to find servers (tests turn this off).
    pub discovery: bool,
}

pub(crate) enum Internal {
    Dialed(Box<Result<DialOk, DialErr>>),
    Flush,
}

pub(crate) struct DialOk {
    ws: WsStream,
    auth: AuthOk,
    address: String,
    steps: Vec<ConnStep>,
}

pub(crate) struct DialErr {
    message: String,
    category: String,
    steps: Vec<ConnStep>,
    api: Option<ApiError>,
}

#[derive(Default)]
struct SeqState {
    own: BTreeSet<u64>,
    buffered: BTreeMap<u64, Envelope>,
    gap_since: Option<Instant>,
}

#[derive(Default)]
struct Diag {
    log: VecDeque<String>,
    last_steps: Vec<ConnStep>,
    failures: BTreeMap<String, u32>,
    window_start: i64,
    window_end: i64,
    last_error: String,
    discovered: Vec<DiscoveredServer>,
    discovery_method: String,
}

impl Diag {
    fn log(&mut self, s: impl Into<String>) {
        let line = format!("{} {}", sl_proto::now_ms(), s.into());
        tracing::info!(target: "sl_client", "{line}");
        if self.log.len() >= 200 {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }
    fn failure(&mut self, category: &str, msg: &str) {
        let now = sl_proto::now_ms();
        if self.failures.is_empty() {
            self.window_start = now;
        }
        self.window_end = now;
        *self.failures.entry(category.to_string()).or_default() += 1;
        self.last_error = msg.to_string();
    }
    fn take_report(&mut self, version: &str, platform: &str) -> Option<ClientHealthReport> {
        if self.failures.is_empty() {
            return None;
        }
        let r = ClientHealthReport {
            client_version: version.into(),
            platform: platform.into(),
            window_start: self.window_start,
            window_end: self.window_end,
            failures: std::mem::take(&mut self.failures).into_iter().collect(),
            last_error: std::mem::take(&mut self.last_error),
            discovery_method: self.discovery_method.clone(),
        };
        Some(r)
    }
}

fn steps_ui(steps: Vec<sl_net::Step>) -> Vec<ConnStep> {
    steps
        .into_iter()
        .map(|s| ConnStep {
            name: s.name.into(),
            ok: s.ok,
            detail: s.detail,
            millis: s.millis,
        })
        .collect()
}

pub struct Core {
    opts: ClientOptions,
    profile: Option<Profile>,
    mls: Mls,
    store: Store,
    me: Option<SelfUser>,
    conn: Option<Transport>,
    generation: u64,
    conn_state: ConnState,
    dialing: bool,
    attempt: u32,
    next_dial: Option<Instant>,
    broadcasts: Vec<Broadcast>,
    directory: Directory,
    presence: HashMap<UserId, Presence>,
    seqs: HashMap<ConversationId, SeqState>,
    dirty_acks: HashSet<ConversationId>,
    join_checks: HashMap<ConversationId, Instant>,
    diag: Diag,
    events: broadcast::Sender<ClientEvent>,
    state: Arc<RwLock<AppState>>,
    incoming_tx: mpsc::UnboundedSender<Incoming>,
    internal_tx: mpsc::UnboundedSender<Internal>,
    rx: Option<(
        mpsc::UnboundedReceiver<Incoming>,
        mpsc::UnboundedReceiver<Internal>,
    )>,
    key_packages_remaining: u32,
    /// The server's restore marker, if it was restored from a standby copy.
    server_restore: Option<RestoreMarker>,
}

/// The restore marker this device has already caught up with.
const KV_RESTORE_SEEN: &str = "server_restore_seen";

impl Core {
    pub(crate) fn open(
        opts: ClientOptions,
        events: broadcast::Sender<ClientEvent>,
        state: Arc<RwLock<AppState>>,
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&opts.dir)?;
        let key = match opts
            .secrets
            .get(SECRET_DBKEY)
            .and_then(|k| <[u8; 32]>::try_from(k.as_slice()).ok())
        {
            Some(k) => k,
            None => {
                let k: [u8; 32] = sl_proto::crypto::random_bytes();
                opts.secrets.set(SECRET_DBKEY, &k)?;
                // A new key means old encrypted history (if any) is unreadable: start fresh.
                Store::wipe(&opts.dir);
                k
            }
        };
        let store = Store::open(&opts.dir, &key)?;
        let profile = Profile::load(&opts.dir);
        let mut mls = Mls::open(&opts.dir.join("mls"))?;
        if let Some(p) = &profile
            && let (Some(u), Some(d)) = (p.user_id, p.device_id)
        {
            mls.set_identity(DeviceRef { user: u, device: d });
        }
        let (incoming_tx, incoming_rx) = mpsc::unbounded_channel();
        let (internal_tx, internal_rx) = mpsc::unbounded_channel();
        let me = profile.as_ref().and_then(|p| {
            Some(SelfUser {
                id: p.user_id?,
                username: p.username.clone(),
                display_name: p.display_name.clone(),
                groups: vec![],
            })
        });
        let mut core = Self {
            opts,
            profile,
            mls,
            store,
            me,
            conn: None,
            generation: 0,
            conn_state: ConnState::Offline,
            dialing: false,
            attempt: 0,
            next_dial: None,
            broadcasts: vec![],
            directory: Directory::default(),
            presence: HashMap::new(),
            seqs: HashMap::new(),
            dirty_acks: HashSet::new(),
            join_checks: HashMap::new(),
            diag: Diag::default(),
            events,
            state,
            incoming_tx,
            internal_tx,
            rx: Some((incoming_rx, internal_rx)),
            key_packages_remaining: 0,
            server_restore: None,
        };
        if core.phase() == Phase::Ready {
            core.next_dial = Some(Instant::now());
        }
        core.publish_state();
        Ok(core)
    }

    pub(crate) async fn run(mut self, mut jobs: mpsc::UnboundedReceiver<crate::Job>) {
        let Some((mut incoming, mut internal)) = self.rx.take() else {
            return;
        };
        let mut tick = tokio::time::interval(Duration::from_millis(500));
        loop {
            tokio::select! {
                job = jobs.recv() => match job {
                    Some(job) => job(&mut self).await,
                    None => break,
                },
                Some(inc) = incoming.recv() => self.on_incoming(inc).await,
                Some(msg) = internal.recv() => self.on_internal(msg).await,
                _ = tick.tick() => self.on_tick().await,
            }
        }
    }

    // =====================================================================================
    // State
    // =====================================================================================

    fn token(&self) -> Option<String> {
        self.opts
            .secrets
            .get(SECRET_SESSION)
            .and_then(|b| String::from_utf8(b).ok())
    }

    pub(crate) fn phase(&self) -> Phase {
        match &self.profile {
            None => Phase::Onboarding,
            Some(p) if p.user_id.is_none() || self.token().is_none() => Phase::SignedOut,
            Some(_) => Phase::Ready,
        }
    }

    pub(crate) fn app_state(&self) -> AppState {
        AppState {
            phase: self.phase(),
            me: self.me.clone(),
            server: self.profile.as_ref().map(|p| PinnedServerView {
                name: p.server_name.clone(),
                server_id: p.server_id,
                fingerprint: p.fingerprint(),
                last_address: p.addresses.first().cloned().unwrap_or_default(),
            }),
            conn: self.conn_state.clone(),
            broadcasts: self.broadcasts.clone(),
        }
    }

    fn publish_state(&self) {
        let s = self.app_state();
        *self.state.write() = s.clone();
        let _ = self.events.send(ClientEvent::State(s));
    }

    fn set_conn(&mut self, c: ConnState) {
        self.conn_state = c.clone();
        *self.state.write() = self.app_state();
        let _ = self.events.send(ClientEvent::Conn(c));
    }

    fn emit(&self, e: ClientEvent) {
        let _ = self.events.send(e);
    }

    fn notice(&self, level: Severity, text: impl Into<String>) {
        self.emit(ClientEvent::Notice {
            level,
            text: text.into(),
        });
    }

    fn my_device(&self) -> Option<DeviceRef> {
        self.mls.identity()
    }

    fn me_id(&self) -> Option<UserId> {
        self.me.as_ref().map(|m| m.id)
    }

    fn device_info(&self) -> DeviceInfo {
        DeviceInfo {
            name: self.opts.device_name.clone(),
            platform: self.opts.platform.clone(),
            client_version: self.opts.client_version.clone(),
            signature_key: Bytes(self.mls.signature_public()),
        }
    }

    async fn rpc(&self, req: Request) -> ClientResult<Response> {
        match &self.conn {
            Some(t) => t.call(req).await,
            None => Err(ClientError::Offline),
        }
    }

    fn require_conn(&self) -> ClientResult<()> {
        if self.conn.is_some() {
            Ok(())
        } else {
            Err(ClientError::Offline)
        }
    }

    // =====================================================================================
    // Connecting
    // =====================================================================================

    fn schedule_reconnect(&mut self, last_error: String) {
        self.attempt += 1;
        let base = [1u64, 2, 4, 8, 15, 30][(self.attempt as usize - 1).min(5)];
        let jitter = rand::thread_rng().gen_range(0..=base * 250);
        let delay = Duration::from_millis(base * 1000 + jitter);
        self.next_dial = Some(Instant::now() + delay);
        self.set_conn(ConnState::Reconnecting {
            attempt: self.attempt,
            next_in_ms: delay.as_millis() as u64,
            last_error,
        });
    }

    fn start_dial(&mut self) {
        let Some(p) = self.profile.clone() else {
            return;
        };
        let (Some(token), Some(device_id), Some(spki)) = (self.token(), p.device_id, p.spki())
        else {
            return;
        };
        self.dialing = true;
        self.next_dial = None;
        if !matches!(self.conn_state, ConnState::Reconnecting { .. }) {
            self.set_conn(ConnState::Connecting);
        }
        let tx = self.internal_tx.clone();
        let discovery = self.opts.discovery;
        let version = self.opts.client_version.clone();
        tokio::spawn(async move {
            let res = dial_resume(p, spki, token, device_id, version, discovery).await;
            let _ = tx.send(Internal::Dialed(Box::new(res)));
        });
    }

    async fn on_internal(&mut self, msg: Internal) {
        match msg {
            Internal::Flush => self.flush_outbox().await,
            Internal::Dialed(res) => {
                self.dialing = false;
                if self.phase() != Phase::Ready {
                    return; // signed out while dialing
                }
                match *res {
                    Ok(ok) => {
                        self.diag.last_steps = ok.steps.clone();
                        self.diag.log(format!("connected to {}", ok.address));
                        self.attach(ok.ws, ok.auth, &ok.address).await;
                    }
                    Err(e) => {
                        self.diag.last_steps = e.steps.clone();
                        self.diag.failure(&e.category, &e.message);
                        self.diag.log(format!("connect failed: {}", e.message));
                        if let Some(api) = e.api {
                            match api.code {
                                ErrorCode::SessionRevoked
                                | ErrorCode::SessionExpired
                                | ErrorCode::AccountDisabled => {
                                    self.opts.secrets.delete(SECRET_SESSION);
                                    self.set_conn(ConnState::Offline);
                                    self.publish_state();
                                    self.notice(Severity::Warning, api.user_message());
                                    return;
                                }
                                _ => {}
                            }
                        }
                        self.schedule_reconnect(e.message);
                    }
                }
            }
        }
    }

    /// Adopt an authenticated socket and run the post-connect sync.
    async fn attach(&mut self, ws: WsStream, auth: AuthOk, address: &str) {
        self.generation += 1;
        let t = Transport::start(ws, self.generation, self.incoming_tx.clone());
        self.conn = Some(t);
        self.attempt = 0;
        self.me = Some(auth.user.clone());
        self.broadcasts = auth.broadcasts.clone();
        self.key_packages_remaining = auth.key_packages_remaining;
        self.server_restore = auth.server.restore;
        if let Some(p) = self.profile.as_mut() {
            p.remember_address(address);
            p.server_name = auth.server.name.clone();
            p.display_name = auth.user.display_name.clone();
            p.username = auth.user.username.clone();
            let _ = p.save(&self.opts.dir);
        }
        let _ = self
            .store
            .upsert_user(auth.user.id, &auth.user.username, &auth.user.display_name);
        self.set_conn(ConnState::Connected { rtt_ms: None });
        self.publish_state();
        if let Err(e) = self.after_connect().await {
            self.diag.log(format!("sync after connect failed: {e}"));
        }
    }

    async fn after_connect(&mut self) -> ClientResult<()> {
        if self.key_packages_remaining < KP_LOW {
            self.upload_key_packages().await?;
        }
        self.refresh_directory().await?;
        // Conversations: the server's list is authoritative for membership.
        if let Response::Conversations(list) = self.rpc(Request::Conversations).await? {
            self.apply_restore(&list);
            let ids: HashSet<ConversationId> = list.iter().map(|c| c.id).collect();
            for info in &list {
                self.store.upsert_conversation(info)?;
            }
            for c in self.store.conversations() {
                if !ids.contains(&c.id) && !c.removed {
                    self.store.set_removed(c.id, true)?;
                    self.mls.delete_group(&c.mls_group_id);
                }
            }
        }
        self.sync_all().await?;
        // Join anything we should be in but aren't; reconcile membership everywhere.
        let me = self.my_device();
        for c in self.store.conversations() {
            if c.removed {
                continue;
            }
            let allowed = me.is_some_and(|d| c.devices.contains(&d));
            if allowed && !self.mls.has_group(&c.mls_group_id) {
                if let Err(e) = self.external_join(c.id).await {
                    self.diag.log(format!("could not join {}: {e}", c.id));
                }
            } else if self.mls.has_group(&c.mls_group_id) {
                let _ = self.reconcile(c.id).await;
            }
        }
        self.flush_outbox().await;
        self.send_acks();
        if let Some(r) = self
            .diag
            .take_report(&self.opts.client_version, &self.opts.platform)
        {
            let _ = self.rpc(Request::ReportHealth(r)).await;
        }
        for c in self.store.conversations() {
            self.emit_conversation(c.id);
        }
        Ok(())
    }

    /// The server was restored from a standby copy since this device last saw it.
    /// Anything after the copy in our local view is history the restored server no
    /// longer has. Rewind each chat to where the copy left it, so the restored server's
    /// new envelopes, which reuse those sequence numbers, aren't skipped as already
    /// seen. If our MLS group moved past the copy's epoch, the restored server can't
    /// follow it, so drop the group; the caller rejoins by external commit.
    fn apply_restore(&mut self, list: &[ConversationInfo]) {
        let Some(marker) = self.server_restore else {
            return;
        };
        let seen = marker.id.to_string();
        if self.store.kv_get(KV_RESTORE_SEEN).as_deref() == Some(seen.as_str()) {
            return;
        }
        for info in list {
            let Some(point) = info.restored else {
                continue;
            };
            let Some(c) = self.store.conversation(info.id) else {
                continue;
            };
            if c.wm <= point.seq {
                continue;
            }
            let _ = self.store.rewind(info.id, point.seq);
            self.seqs.remove(&info.id);
            if self
                .mls
                .epoch(&c.mls_group_id)
                .is_some_and(|e| e > point.epoch)
            {
                self.mls.delete_group(&c.mls_group_id);
            }
            self.diag.log(format!(
                "server restored from a copy: rewound {} to {}",
                info.id, point.seq
            ));
            self.system_message(
                info.id,
                "SpareLine Server was restored from a standby copy. Messages sent shortly before it stopped may not have reached everyone.".into(),
                sl_proto::now_ms(),
            );
        }
        let _ = self.store.kv_set(KV_RESTORE_SEEN, &seen);
    }

    /// The server said our epoch is stale. Usually we're behind and catch up. If the
    /// server is behind us, it was restored from an earlier copy and can't follow our
    /// group state, so rejoin from what it has.
    async fn on_stale_epoch(
        &mut self,
        conv: ConversationId,
        server_epoch: Option<u64>,
    ) -> ClientResult<()> {
        if let Some(se) = server_epoch
            && let Some(c) = self.store.conversation(conv)
            && self
                .mls
                .epoch(&c.mls_group_id)
                .is_some_and(|mine| mine > se)
        {
            self.diag
                .log(format!("server is behind in {conv}; rejoining"));
            self.mls.delete_group(&c.mls_group_id);
            return self.external_join(conv).await;
        }
        self.catch_up(conv).await
    }

    async fn upload_key_packages(&mut self) -> ClientResult<()> {
        let (kps, lr) = self.mls.key_packages(KP_TARGET)?;
        self.rpc(Request::UploadKeyPackages {
            key_packages: kps.into_iter().map(Bytes).collect(),
            last_resort: Some(Bytes(lr)),
        })
        .await?;
        self.key_packages_remaining += KP_TARGET as u32;
        Ok(())
    }

    async fn refresh_directory(&mut self) -> ClientResult<Directory> {
        if let Response::Directory(d) = self.rpc(Request::Directory).await? {
            self.store.upsert_users(&d.users)?;
            for u in &d.users {
                self.presence.insert(u.id, u.presence);
            }
            if let Some(me) = self.me.as_mut() {
                me.groups = d.groups.clone();
            }
            self.directory = d.clone();
            self.emit(ClientEvent::Directory(d.clone()));
            return Ok(d);
        }
        Ok(self.directory.clone())
    }

    async fn sync_all(&mut self) -> ClientResult<()> {
        for _ in 0..50 {
            let Response::Sync(batch) = self.rpc(Request::Sync { limit: 500 }).await? else {
                break;
            };
            for w in batch.welcomes {
                self.on_welcome(w).await;
            }
            for env in batch.envelopes {
                self.on_envelope(env).await;
            }
            if !batch.more {
                break;
            }
        }
        Ok(())
    }

    fn send_acks(&mut self) {
        let Some(t) = self.conn.clone() else { return };
        for conv in self.dirty_acks.drain() {
            let Some(c) = self.store.conversation(conv) else {
                continue;
            };
            let t = t.clone();
            tokio::spawn(async move {
                let _ = t
                    .call(Request::Ack {
                        conversation: conv,
                        seq: c.wm,
                    })
                    .await;
            });
        }
    }

    async fn on_tick(&mut self) {
        if self.phase() == Phase::Ready
            && self.conn.is_none()
            && !self.dialing
            && self.next_dial.is_some_and(|t| Instant::now() >= t)
        {
            self.start_dial();
        }
        if self.conn.is_none() {
            return;
        }
        // Gaps: fetch what's missing.
        let gaps: Vec<ConversationId> = self
            .seqs
            .iter()
            .filter(|(_, s)| {
                s.gap_since
                    .is_some_and(|t| t.elapsed() > Duration::from_secs(2))
            })
            .map(|(c, _)| *c)
            .collect();
        for c in gaps {
            if let Some(s) = self.seqs.get_mut(&c) {
                s.gap_since = None;
            }
            let _ = self.catch_up(c).await;
        }
        // Pending joins.
        let due: Vec<ConversationId> = self
            .join_checks
            .iter()
            .filter(|(_, t)| Instant::now() >= **t)
            .map(|(c, _)| *c)
            .collect();
        if !due.is_empty() {
            for c in &due {
                self.join_checks.remove(c);
            }
            let _ = self.sync_all().await;
            for c in due {
                if let Some(lc) = self.store.conversation(c)
                    && !lc.removed
                    && !self.mls.has_group(&lc.mls_group_id)
                    && let Err(e) = self.external_join(c).await
                {
                    self.diag.log(format!("join failed: {e}"));
                }
            }
        }
        self.send_acks();
    }

    // =====================================================================================
    // Incoming
    // =====================================================================================

    async fn on_incoming(&mut self, inc: Incoming) {
        match inc {
            Incoming::Rtt(ms) => {
                if matches!(self.conn_state, ConnState::Connected { .. }) {
                    self.set_conn(ConnState::Connected { rtt_ms: Some(ms) });
                }
            }
            Incoming::Closed { generation, reason } => {
                if generation != self.generation || self.conn.is_none() {
                    return;
                }
                self.conn = None;
                let msg = reason
                    .as_ref()
                    .map(|r| r.user_message().to_string())
                    .unwrap_or_else(|| "Connection lost".into());
                self.diag.failure("disconnected", &msg);
                self.diag.log(format!("disconnected: {msg}"));
                if let Some(r) = &reason {
                    match r.code {
                        ErrorCode::SessionRevoked
                        | ErrorCode::AccountDisabled
                        | ErrorCode::SessionExpired => {
                            if r.detail.contains("replaced by a newer connection") {
                                // Another connection of ours took over; just reconnect.
                            } else {
                                self.opts.secrets.delete(SECRET_SESSION);
                                self.set_conn(ConnState::Offline);
                                self.publish_state();
                                self.notice(Severity::Warning, r.user_message());
                                return;
                            }
                        }
                        _ => {}
                    }
                }
                self.schedule_reconnect(msg);
            }
            Incoming::Event(e) => self.on_event(e).await,
        }
    }

    async fn on_event(&mut self, e: Event) {
        match e {
            Event::Envelope(env) => self.on_envelope(env).await,
            Event::Welcome(w) => self.on_welcome(w).await,
            Event::Conversation(info) => self.on_conversation(info).await,
            Event::ConversationRemoved { conversation } => {
                if let Some(c) = self.store.conversation(conversation) {
                    let _ = self.store.set_removed(conversation, true);
                    self.mls.delete_group(&c.mls_group_id);
                    self.system_message(
                        conversation,
                        "You're no longer in this chat.".into(),
                        sl_proto::now_ms(),
                    );
                    self.emit(ClientEvent::ConversationRemoved { id: conversation });
                }
            }
            Event::DirectoryChanged => {
                let _ = self.refresh_directory().await;
            }
            Event::Presence { user, presence } => {
                self.presence.insert(user, presence);
                self.emit(ClientEvent::Presence { user, presence });
            }
            Event::Typing { conversation, user } => {
                let name = self.name_of(user);
                self.emit(ClientEvent::Typing {
                    conversation,
                    user,
                    name,
                });
            }
            Event::ReadMarker(m) => {
                let _ = self.store.set_marker(m.conversation, m.user, m.seq);
                if Some(m.user) == self.me_id() {
                    let _ = self.store.set_read_seq(m.conversation, m.seq);
                }
                self.emit_conversation(m.conversation);
                self.emit_recent_statuses(m.conversation);
            }
            Event::Broadcast(b) => {
                self.broadcasts.retain(|x| x.id != b.id);
                self.broadcasts.insert(0, b);
                self.emit(ClientEvent::Broadcasts(self.broadcasts.clone()));
                *self.state.write() = self.app_state();
            }
            Event::BroadcastEnded { id } => {
                self.broadcasts.retain(|x| x.id != id);
                self.emit(ClientEvent::Broadcasts(self.broadcasts.clone()));
                *self.state.write() = self.app_state();
            }
            Event::KeyPackagesLow { remaining } => {
                self.key_packages_remaining = remaining;
                let _ = self.upload_key_packages().await;
            }
            Event::Profile(u) => {
                self.me = Some(u.clone());
                if let Some(p) = self.profile.as_mut() {
                    p.display_name = u.display_name.clone();
                    let _ = p.save(&self.opts.dir);
                }
                self.publish_state();
            }
        }
    }

    async fn on_conversation(&mut self, info: ConversationInfo) {
        let id = info.id;
        if self.store.upsert_conversation(&info).is_err() {
            return;
        }
        let Some(c) = self.store.conversation(id) else {
            return;
        };
        let me = self.my_device();
        let allowed = me.is_some_and(|d| info.devices.contains(&d));
        if allowed && !self.mls.has_group(&c.mls_group_id) {
            // A welcome may be on its way; check shortly, then join by external commit.
            self.join_checks
                .entry(id)
                .or_insert(Instant::now() + Duration::from_secs(2));
        } else if self.mls.has_group(&c.mls_group_id) {
            // Small random delay spreads out members racing to commit the same removal.
            let ms = rand::thread_rng().gen_range(0..400);
            tokio::time::sleep(Duration::from_millis(ms)).await;
            let _ = self.reconcile(id).await;
        }
        self.emit_conversation(id);
    }

    async fn on_welcome(&mut self, w: PendingWelcome) {
        let info = w.conversation;
        let id = info.id;
        if self.store.upsert_conversation(&info).is_err() {
            return;
        }
        if !self.mls.has_group(info.mls_group_id.as_slice()) {
            match self.mls.join_welcome(w.welcome.as_slice()) {
                Ok(gid) if gid == info.mls_group_id.as_slice() => {
                    let _ = self.store.set_wm(id, w.start_seq);
                    self.seqs.remove(&id);
                    self.diag
                        .log(format!("joined conversation {id} from welcome"));
                }
                Ok(_) => self.diag.log("welcome group id mismatch".to_string()),
                Err(e) => {
                    self.diag.log(format!(
                        "welcome failed ({e}); will join by external commit"
                    ));
                    self.join_checks
                        .insert(id, Instant::now() + Duration::from_millis(500));
                }
            }
        }
        // Clears the welcome on the server.
        if let Some(t) = self.conn.clone() {
            let seq = self
                .store
                .conversation(id)
                .map(|c| c.wm)
                .unwrap_or(w.start_seq);
            tokio::spawn(async move {
                let _ = t
                    .call(Request::Ack {
                        conversation: id,
                        seq,
                    })
                    .await;
            });
        }
        self.emit_conversation(id);
    }

    /// Queue an envelope and process everything that is now contiguous.
    async fn on_envelope(&mut self, env: Envelope) {
        let conv = env.conversation;
        let Some(c) = self.store.conversation(conv) else {
            return;
        };
        if env.seq <= c.wm {
            return;
        }
        let mine =
            env.sender_device.is_some() && env.sender_device == self.my_device().map(|d| d.device);
        let st = self.seqs.entry(conv).or_default();
        if mine {
            st.own.insert(env.seq);
        } else {
            st.buffered.insert(env.seq, env);
        }
        self.drain(conv).await;
    }

    async fn drain(&mut self, conv: ConversationId) {
        loop {
            let Some(c) = self.store.conversation(conv) else {
                return;
            };
            let next = c.wm + 1;
            let st = self.seqs.entry(conv).or_default();
            // Drop anything at or below the watermark.
            st.own.retain(|s| *s >= next);
            st.buffered.retain(|s, _| *s >= next);
            if st.own.remove(&next) {
                let _ = self.store.set_wm(conv, next);
                self.dirty_acks.insert(conv);
                continue;
            }
            let Some(env) = st.buffered.remove(&next) else {
                break;
            };
            self.process_envelope(&c, env).await;
            let _ = self.store.set_wm(conv, next);
            self.dirty_acks.insert(conv);
        }
        let st = self.seqs.entry(conv).or_default();
        if st.buffered.is_empty() && st.own.is_empty() {
            st.gap_since = None;
        } else if st.gap_since.is_none() {
            st.gap_since = Some(Instant::now());
        }
    }

    async fn catch_up(&mut self, conv: ConversationId) -> ClientResult<()> {
        for _ in 0..20 {
            let Some(c) = self.store.conversation(conv) else {
                return Ok(());
            };
            let Response::Envelopes(envs) = self
                .rpc(Request::Fetch {
                    conversation: conv,
                    after_seq: c.wm,
                    limit: 500,
                })
                .await?
            else {
                return Ok(());
            };
            let n = envs.len();
            for e in envs {
                self.on_envelope(e).await;
            }
            if n < 500 {
                break;
            }
        }
        Ok(())
    }

    async fn process_envelope(&mut self, c: &LocalConv, env: Envelope) {
        if env.data.is_empty() || !self.mls.has_group(&c.mls_group_id) {
            return;
        }
        let gid = c.mls_group_id.clone();
        match self.mls.process(&gid, env.data.as_slice()) {
            Ok(Processed::Application { sender, bytes }) => {
                // Only accept content from devices the server says belong in this chat.
                if !c.devices.contains(&sender) && !c.members.contains(&sender.user) {
                    self.diag
                        .log("ignored message from a device not in the chat".to_string());
                    return;
                }
                if let Some(p) = payload::decode(&bytes) {
                    self.apply_payload(c, sender.user, p, Some(env.seq), env.ts);
                }
            }
            Ok(Processed::Commit {
                sender,
                added,
                removed,
                self_removed,
            }) => {
                self.describe_commit(c, sender, &added, &removed, env.ts);
                if self_removed {
                    self.mls.delete_group(&gid);
                    let allowed = self.my_device().is_some_and(|d| c.devices.contains(&d));
                    if allowed && !c.removed {
                        self.join_checks
                            .insert(c.id, Instant::now() + Duration::from_secs(1));
                    }
                }
            }
            Ok(Processed::Proposal) | Ok(Processed::Own) => {}
            Err(e) => {
                self.diag.log(format!(
                    "could not process message {} in {}: {e}",
                    env.seq, c.id
                ));
                if env.kind == EnvelopeKind::Commit {
                    // We're stuck in an old epoch: rejoin with an external commit.
                    self.mls.delete_group(&gid);
                    self.join_checks
                        .insert(c.id, Instant::now() + Duration::from_millis(500));
                }
            }
        }
    }

    fn describe_commit(
        &mut self,
        c: &LocalConv,
        sender: Option<DeviceRef>,
        added: &[DeviceRef],
        removed: &[DeviceRef],
        ts: i64,
    ) {
        let me = self.me_id();
        let actor = sender
            .map(|s| self.name_of(s.user))
            .unwrap_or_else(|| "Someone".into());
        // External join (no adds/removes): a member's new device.
        if added.is_empty() && removed.is_empty() {
            if let Some(s) = sender
                && self.my_device() != Some(s)
            {
                let text = if Some(s.user) == me {
                    "You signed in on another device.".to_string()
                } else {
                    format!("{actor} joined from a new device.")
                };
                self.system_message(c.id, text, ts);
            }
            return;
        }
        let added_users: BTreeSet<UserId> = added.iter().map(|d| d.user).collect();
        for u in added_users {
            if Some(u) == sender.map(|s| s.user) {
                continue;
            }
            let who = if Some(u) == me {
                "you".to_string()
            } else {
                self.name_of(u)
            };
            self.system_message(c.id, format!("{actor} added {who}."), ts);
        }
        let removed_users: BTreeSet<UserId> = removed.iter().map(|d| d.user).collect();
        for u in removed_users {
            if c.members.contains(&u) {
                continue; // only a device was removed
            }
            let who = self.name_of(u);
            self.system_message(c.id, format!("{who} left the chat."), ts);
        }
    }

    fn apply_payload(
        &mut self,
        c: &LocalConv,
        sender: UserId,
        p: Payload,
        seq: Option<u64>,
        ts: i64,
    ) {
        let me = self.me_id();
        match p {
            Payload::Text { id, body, reply_to } => {
                let body: String = body.chars().take(payload::MAX_BODY_CHARS).collect();
                let m = LocalMessage {
                    id,
                    conversation: c.id,
                    seq,
                    sender: Some(sender),
                    kind: MessageKind::Text,
                    body,
                    reply_to,
                    ts,
                    edited: false,
                    deleted: false,
                    status: SendStatus::Sent,
                    mine: Some(sender) == me,
                };
                if self.store.insert_message(&m).unwrap_or(false) {
                    self.emit_message(id);
                    self.emit_conversation(c.id);
                }
            }
            Payload::Edit { target, body } => {
                if self
                    .store
                    .apply_edit(target, sender, &body)
                    .unwrap_or(false)
                {
                    self.emit_message(target);
                    self.emit_conversation(c.id);
                }
            }
            Payload::Delete { target } => {
                if self.store.apply_delete(target, sender).unwrap_or(false) {
                    self.emit_message(target);
                    self.emit_conversation(c.id);
                }
            }
            Payload::Reaction { target, emoji, add } => {
                if payload::valid_emoji(&emoji)
                    && self
                        .store
                        .apply_reaction(target, sender, &emoji, add)
                        .unwrap_or(false)
                {
                    self.emit_message(target);
                }
            }
            Payload::ChatMeta { title } => {
                if c.kind == ConversationKind::Group {
                    let title: String = title.trim().chars().take(80).collect();
                    let _ = self.store.set_title(c.id, &title);
                    let who = if Some(sender) == me {
                        "You".to_string()
                    } else {
                        self.name_of(sender)
                    };
                    if !title.is_empty() {
                        self.system_message(c.id, format!("{who} named the chat \"{title}\"."), ts);
                    }
                    self.emit_conversation(c.id);
                }
            }
        }
    }

    fn system_message(&mut self, conv: ConversationId, text: String, ts: i64) {
        let m = LocalMessage {
            id: Uuid::now_v7(),
            conversation: conv,
            seq: None,
            sender: None,
            kind: MessageKind::System,
            body: text,
            reply_to: None,
            ts,
            edited: false,
            deleted: false,
            status: SendStatus::Sent,
            mine: false,
        };
        if self.store.insert_message(&m).unwrap_or(false) {
            self.emit_message(m.id);
        }
    }

    // =====================================================================================
    // MLS membership
    // =====================================================================================

    /// Remove leaves the server no longer allows (revoked devices, removed people).
    async fn reconcile(&mut self, conv: ConversationId) -> ClientResult<()> {
        for _ in 0..3 {
            let Some(c) = self.store.conversation(conv) else {
                return Ok(());
            };
            if c.removed || !self.mls.has_group(&c.mls_group_id) {
                return Ok(());
            }
            let Some((pc, devices)) = self.mls.remove_disallowed(&c.mls_group_id, &c.devices)?
            else {
                return Ok(());
            };
            match self.commit(conv, pc, vec![], devices, vec![]).await {
                Ok(()) => return Ok(()),
                Err(ClientError::Api(e)) if e.code == ErrorCode::StaleEpoch => {
                    self.on_stale_epoch(conv, e.epoch).await?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Send a pending commit; merge it if accepted, discard it otherwise.
    async fn commit(
        &mut self,
        conv: ConversationId,
        pc: PendingCommit,
        added_users: Vec<UserId>,
        removed_devices: Vec<DeviceId>,
        welcome_devices: Vec<DeviceId>,
    ) -> ClientResult<()> {
        let gid = pc.group_id.clone();
        let req = Request::Commit {
            conversation: conv,
            epoch: pc.epoch,
            commit: Bytes(pc.commit),
            welcome: pc.welcome.map(Bytes),
            welcome_devices,
            added_users,
            removed_devices,
            group_info: Bytes(pc.group_info),
        };
        match self.rpc(req).await {
            Ok(Response::Committed { seq, .. }) => {
                self.mls.merge_pending(&gid)?;
                self.seqs.entry(conv).or_default().own.insert(seq);
                self.drain(conv).await;
                Ok(())
            }
            Ok(_) => {
                self.mls.clear_pending(&gid);
                Err(ClientError::Internal("unexpected response".into()))
            }
            Err(e) => {
                self.mls.clear_pending(&gid);
                Err(e)
            }
        }
    }

    async fn external_join(&mut self, conv: ConversationId) -> ClientResult<()> {
        for _ in 0..3 {
            let Some(c) = self.store.conversation(conv) else {
                return Ok(());
            };
            let Response::GroupInfo { info, .. } =
                self.rpc(Request::GroupInfo { conversation: conv }).await?
            else {
                return Err(ClientError::Internal("unexpected response".into()));
            };
            let pc = self.mls.external_join(info.as_slice())?;
            let req = Request::ExternalJoin {
                conversation: conv,
                epoch: pc.epoch,
                commit: Bytes(pc.commit),
                group_info: Bytes(pc.group_info),
            };
            match self.rpc(req).await {
                Ok(Response::Committed { seq, .. }) => {
                    let _ = self.store.set_wm(conv, seq);
                    self.seqs.remove(&conv);
                    self.diag.log(format!("joined {conv} by external commit"));
                    return Ok(());
                }
                Err(ClientError::Api(e)) if e.code == ErrorCode::StaleEpoch => {
                    self.mls.delete_group(&c.mls_group_id);
                    continue;
                }
                Err(e) => {
                    self.mls.delete_group(&c.mls_group_id);
                    return Err(e);
                }
                Ok(_) => {
                    self.mls.delete_group(&c.mls_group_id);
                    return Err(ClientError::Internal("unexpected response".into()));
                }
            }
        }
        Ok(())
    }

    // =====================================================================================
    // Sending
    // =====================================================================================

    async fn send_payload(
        &mut self,
        conv: ConversationId,
        p: &Payload,
    ) -> ClientResult<(u64, i64)> {
        let bytes = payload::encode(p);
        for _ in 0..3 {
            let c = self
                .store
                .conversation(conv)
                .ok_or(ClientError::Invalid("Unknown chat".into()))?;
            if c.removed {
                return Err(ClientError::Api(ApiError::new(ErrorCode::NotAuthorized)));
            }
            if !self.mls.has_group(&c.mls_group_id) {
                self.external_join(conv).await?;
            }
            let (ct, epoch) = self.mls.encrypt(&c.mls_group_id, &bytes)?;
            match self
                .rpc(Request::Send {
                    conversation: conv,
                    epoch,
                    ciphertext: Bytes(ct),
                })
                .await
            {
                Ok(Response::Sent { seq, ts }) => {
                    self.seqs.entry(conv).or_default().own.insert(seq);
                    self.drain(conv).await;
                    return Ok((seq, ts));
                }
                Ok(_) => return Err(ClientError::Internal("unexpected response".into())),
                Err(ClientError::Api(e)) if e.code == ErrorCode::StaleEpoch => {
                    self.on_stale_epoch(conv, e.epoch).await?;
                }
                Err(e) => return Err(e),
            }
        }
        Err(ClientError::Connect(
            "The chat kept changing while sending. Try again.".into(),
        ))
    }

    async fn flush_outbox(&mut self) {
        if self.conn.is_none() {
            return;
        }
        for row in self.store.outbox() {
            if self.conn.is_none() {
                break;
            }
            let msg_id = match &row.payload {
                Payload::Text { id, .. } => Some(*id),
                _ => None,
            };
            match self.send_payload(row.conversation, &row.payload).await {
                Ok((seq, ts)) => {
                    let _ = self.store.outbox_done(row.id);
                    if let Some(id) = msg_id {
                        let _ = self
                            .store
                            .update_status(id, SendStatus::Sent, Some(seq), Some(ts));
                        self.emit_message(id);
                        self.emit_conversation(row.conversation);
                    }
                    if let Payload::Delete { target } = &row.payload
                        && let Some(m) = self.store.message(*target)
                        && let Some(s) = m.seq
                    {
                        let _ = self
                            .rpc(Request::Purge {
                                conversation: row.conversation,
                                seq: s,
                            })
                            .await;
                    }
                }
                Err(ClientError::Offline) => break,
                Err(ClientError::Api(e))
                    if matches!(
                        e.code,
                        ErrorCode::ReadOnly | ErrorCode::NotAuthorized | ErrorCode::NotFound
                    ) =>
                {
                    let _ = self.store.outbox_done(row.id);
                    if let Some(id) = msg_id {
                        let _ = self.store.update_status(id, SendStatus::Failed, None, None);
                        self.emit_message(id);
                    }
                    self.notice(Severity::Warning, e.user_message());
                }
                Err(e) => {
                    let _ = self.store.outbox_attempt(row.id);
                    self.diag.log(format!("send failed: {e}"));
                    if row.attempts >= 4 {
                        let _ = self.store.outbox_done(row.id);
                        if let Some(id) = msg_id {
                            let _ = self.store.update_status(id, SendStatus::Failed, None, None);
                            self.emit_message(id);
                        }
                    }
                    break;
                }
            }
        }
    }

    fn queue(&self, conv: ConversationId, p: &Payload) -> ClientResult<()> {
        self.store.outbox_push(conv, p)?;
        let _ = self.internal_tx.send(Internal::Flush);
        Ok(())
    }

    // =====================================================================================
    // Views
    // =====================================================================================

    fn name_of(&self, u: UserId) -> String {
        if Some(u) == self.me_id() {
            return self
                .me
                .as_ref()
                .map(|m| m.display_name.clone())
                .unwrap_or_else(|| "You".into());
        }
        self.store
            .user_name(u)
            .map(|(_, d)| d)
            .unwrap_or_else(|| "Unknown user".into())
    }

    fn user_lite(&self, u: UserId) -> UserLite {
        let (username, display_name) = if Some(u) == self.me_id() {
            self.me
                .as_ref()
                .map(|m| (m.username.clone(), m.display_name.clone()))
                .unwrap_or_default()
        } else {
            self.store
                .user_name(u)
                .unwrap_or_else(|| (String::new(), "Unknown user".into()))
        };
        let presence = if Some(u) == self.me_id() && self.conn.is_some() {
            Presence::Online
        } else {
            self.presence.get(&u).copied().unwrap_or_default()
        };
        UserLite {
            id: u,
            display_name,
            username,
            presence,
        }
    }

    pub(crate) fn conversation_view(&self, c: &LocalConv) -> ConversationView {
        let me = self.me_id();
        let others: Vec<UserId> = c
            .members
            .iter()
            .copied()
            .filter(|u| Some(*u) != me)
            .collect();
        let title = match c.kind {
            ConversationKind::Direct => others
                .first()
                .map(|u| self.name_of(*u))
                .unwrap_or_else(|| "Direct chat".into()),
            ConversationKind::Group => c.title.clone().unwrap_or_else(|| {
                let mut names: Vec<String> =
                    others.iter().take(3).map(|u| self.name_of(*u)).collect();
                if others.len() > 3 {
                    names.push(format!("+{}", others.len() - 3));
                }
                if names.is_empty() {
                    "Group chat".into()
                } else {
                    names.join(", ")
                }
            }),
        };
        let last = self.store.last_message(c.id).map(|m| MessagePreview {
            sender_name: match (m.kind, m.sender) {
                (MessageKind::System, _) => String::new(),
                (_, Some(s)) if Some(s) == me => "You".into(),
                (_, Some(s)) => self.name_of(s),
                _ => String::new(),
            },
            text: if m.deleted {
                "Message deleted".into()
            } else {
                m.body.chars().take(120).collect()
            },
            ts: m.ts,
        });
        let scope_group = c.scope_group.and_then(|g| {
            self.me
                .as_ref()
                .and_then(|m| m.groups.iter().find(|x| x.id == g).cloned())
                .or(Some(GroupRef {
                    id: g,
                    name: String::new(),
                }))
        });
        ConversationView {
            id: c.id,
            kind: c.kind,
            title,
            scope_group,
            members: c.members.iter().map(|u| self.user_lite(*u)).collect(),
            last_message: last,
            unread: self.store.unread_count(c.id, c.read_seq),
            read_only: c.read_only || c.removed,
            updated_at: c.updated_at,
        }
    }

    fn message_view(&self, m: &LocalMessage) -> MessageView {
        let me = self.me_id();
        let mut reactions: Vec<ReactionView> = Vec::new();
        for (emoji, user) in self.store.reactions(m.id) {
            let name = if Some(user) == me {
                "You".to_string()
            } else {
                self.name_of(user)
            };
            match reactions.iter_mut().find(|r| r.emoji == emoji) {
                Some(r) => {
                    r.count += 1;
                    r.mine |= Some(user) == me;
                    r.names.push(name);
                }
                None => reactions.push(ReactionView {
                    emoji,
                    count: 1,
                    mine: Some(user) == me,
                    names: vec![name],
                }),
            }
        }
        let reply_to = m
            .reply_to
            .and_then(|r| self.store.message(r))
            .map(|r| ReplyPreview {
                message_id: r.id,
                sender_name: r
                    .sender
                    .map(|s| {
                        if Some(s) == me {
                            "You".into()
                        } else {
                            self.name_of(s)
                        }
                    })
                    .unwrap_or_default(),
                text: if r.deleted {
                    "Message deleted".into()
                } else {
                    r.body.chars().take(140).collect()
                },
            });
        let status = if m.mine && m.status == SendStatus::Sent {
            match m.seq {
                Some(s)
                    if self
                        .store
                        .others_read(m.conversation, me.unwrap_or_default())
                        >= s =>
                {
                    SendStatus::Read
                }
                _ => SendStatus::Sent,
            }
        } else {
            m.status
        };
        MessageView {
            id: m.id,
            conversation: m.conversation,
            kind: m.kind,
            sender: m.sender,
            sender_name: m.sender.map(|s| self.name_of(s)).unwrap_or_default(),
            body: if m.deleted {
                String::new()
            } else {
                m.body.clone()
            },
            ts: m.ts,
            edited: m.edited,
            deleted: m.deleted,
            mine: m.mine,
            reply_to,
            reactions,
            status,
        }
    }

    fn emit_conversation(&self, id: ConversationId) {
        if let Some(c) = self.store.conversation(id) {
            self.emit(ClientEvent::Conversation(self.conversation_view(&c)));
        }
    }

    fn emit_message(&self, id: Uuid) {
        if let Some(m) = self.store.message(id) {
            self.emit(ClientEvent::Message(self.message_view(&m)));
        }
    }

    fn emit_recent_statuses(&self, conv: ConversationId) {
        for m in self.store.messages(conv, None, 30) {
            if m.mine {
                self.emit(ClientEvent::Message(self.message_view(&m)));
            }
        }
    }

    // =====================================================================================
    // UI operations (called via jobs)
    // =====================================================================================

    fn reset_local(&mut self, keep_history: bool) -> ClientResult<()> {
        self.conn = None;
        self.seqs.clear();
        Mls::wipe(&self.opts.dir.join("mls"));
        self.mls = Mls::open(&self.opts.dir.join("mls"))?;
        if !keep_history {
            let key: [u8; 32] = sl_proto::crypto::random_bytes();
            self.opts
                .secrets
                .set(SECRET_DBKEY, &key)
                .map_err(ClientError::from)?;
            Store::wipe(&self.opts.dir);
            self.store = Store::open(&self.opts.dir, &key)?;
        }
        Ok(())
    }

    /// Candidate addresses: explicit, then invite hints, then mDNS.
    async fn candidates(&mut self, explicit: &str, hints: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |a: &str| {
            let a = a.trim();
            if a.is_empty() {
                return;
            }
            let a = if a.contains(':') {
                a.to_string()
            } else {
                format!("{a}:{}", sl_proto::DEFAULT_PORT)
            };
            if !out.contains(&a) {
                out.push(a);
            }
        };
        push(explicit);
        for h in hints {
            push(h);
        }
        if self.opts.discovery {
            let found = crate::discovery::browse(Duration::from_secs(2)).await;
            self.diag.discovered = found.clone();
            for s in &found {
                for a in &s.addresses {
                    push(a);
                }
            }
        }
        out
    }

    pub(crate) async fn discover(&mut self) -> Vec<DiscoveredServer> {
        let mut found = if self.opts.discovery {
            crate::discovery::browse(Duration::from_secs(3)).await
        } else {
            vec![]
        };
        if let Some(p) = &self.profile {
            for s in found.iter_mut() {
                if s.server_id == Some(p.server_id) {
                    s.name = p.server_name.clone();
                }
            }
        }
        self.diag.discovered = found.clone();
        found
    }

    /// New account with an invite code.
    pub(crate) async fn join(&mut self, input: JoinInput) -> ClientResult<AppState> {
        let parsed = sl_proto::code::parse_invite_text(&input.invite_text);
        let code = parsed.code.ok_or_else(|| {
            ClientError::Invalid(
                "That doesn't look like an invite code. It looks like SPL-XXXX-XXXX-XXXX-XXXX."
                    .into(),
            )
        })?;
        let ws_auth = self
            .prove_and(
                &code,
                &input.address,
                &parsed.hosts,
                AuthKind::Register(input.clone()),
            )
            .await?;
        Ok(ws_auth)
    }

    /// Existing account with a setup code (new device or password reset).
    pub(crate) async fn redeem(&mut self, input: RedeemInput) -> ClientResult<AppState> {
        let parsed = sl_proto::code::parse_invite_text(&input.invite_text);
        let code = parsed
            .code
            .ok_or_else(|| ClientError::Invalid("That doesn't look like a setup code.".into()))?;
        self.prove_and(
            &code,
            &input.address,
            &parsed.hosts,
            AuthKind::Redeem(input.clone()),
        )
        .await
    }

    async fn prove_and(
        &mut self,
        code: &str,
        explicit: &str,
        hints: &[String],
        kind: AuthKind,
    ) -> ClientResult<AppState> {
        let keys = sl_proto::crypto::derive_invite(code);
        let candidates = self.candidates(explicit, hints).await;
        if candidates.is_empty() {
            return Err(ClientError::Connect(
                "Couldn't find a SpareLine server on this network. Enter the server address from your invite (for example 192.168.1.10).".into(),
            ));
        }
        let mut last_err = ClientError::Connect("Couldn't reach the server.".into());
        for addr in candidates {
            let verifier = PinVerifier::new(PinMode::Capture);
            let mut d = match sl_net::dial(&addr, verifier, DIAL_TIMEOUT).await {
                Ok(d) => d,
                Err((e, steps)) => {
                    self.diag.last_steps = steps_ui(steps);
                    self.diag.failure(e.category(), &e.to_string());
                    last_err = ClientError::Connect(format!("Couldn't connect to {addr}: {e}"));
                    continue;
                }
            };
            self.diag.last_steps = steps_ui(std::mem::take(&mut d.steps));
            let ServerFrame::Hello { server } = recv_raw(&mut d.ws, Duration::from_secs(5)).await?
            else {
                last_err = ClientError::Connect(format!("{addr} is not a SpareLine server."));
                continue;
            };
            let nonce: [u8; 32] = sl_proto::crypto::random_bytes();
            send_raw(
                &mut d.ws,
                &ClientFrame::Prove {
                    invite_id: keys.invite_id.clone(),
                    nonce: Bytes(nonce.to_vec()),
                },
            )
            .await?;
            match recv_raw(&mut d.ws, Duration::from_secs(10)).await? {
                ServerFrame::Proof { proof } => {
                    if !sl_proto::crypto::verify_proof(
                        &keys.proof_key,
                        &nonce,
                        &d.spki_sha256,
                        proof.as_slice(),
                    ) {
                        self.diag.log(format!(
                            "{addr} failed the identity proof (possible impostor)"
                        ));
                        last_err = ClientError::Api(ApiError::new(ErrorCode::PinMismatch));
                        continue;
                    }
                }
                ServerFrame::AuthError(e) if e.code == ErrorCode::InvalidInvite => {
                    last_err = ClientError::Api(e);
                    continue; // maybe another server on the LAN knows this code
                }
                ServerFrame::AuthError(e) => return Err(ClientError::Api(e)),
                _ => continue,
            }
            self.diag.log(format!(
                "server {} proved its identity; pinning {}",
                server.name,
                sl_proto::crypto::fingerprint_display(&d.spki_sha256)
            ));
            // Pinned. Now create the account / redeem over this same connection.
            let same_server = self
                .profile
                .as_ref()
                .is_some_and(|p| p.server_id == server.server_id);
            match &kind {
                AuthKind::Register(_) => self.reset_local(false)?,
                AuthKind::Redeem(_) => self.reset_local(same_server)?,
            }
            let frame = match &kind {
                AuthKind::Register(i) => ClientFrame::Register(RegisterRequest {
                    proto: PROTOCOL_VERSION,
                    code: code.to_string(),
                    username: i.username.clone(),
                    display_name: i.display_name.clone(),
                    password: i.password.clone(),
                    device: self.device_info(),
                }),
                AuthKind::Redeem(i) => ClientFrame::Redeem(RedeemRequest {
                    proto: PROTOCOL_VERSION,
                    code: code.to_string(),
                    new_password: i.new_password.clone(),
                    device: self.device_info(),
                }),
            };
            send_raw(&mut d.ws, &frame).await?;
            match recv_raw(&mut d.ws, Duration::from_secs(20)).await? {
                ServerFrame::Authed(ok) => {
                    let ok = *ok;
                    if !same_server
                        || self.profile.as_ref().and_then(|p| p.user_id) != Some(ok.user.id)
                    {
                        // Different account on this device: clear old history.
                        if matches!(kind, AuthKind::Redeem(_)) && same_server {
                            let key: [u8; 32] = sl_proto::crypto::random_bytes();
                            self.opts
                                .secrets
                                .set(SECRET_DBKEY, &key)
                                .map_err(ClientError::from)?;
                            Store::wipe(&self.opts.dir);
                            self.store = Store::open(&self.opts.dir, &key)?;
                        }
                    }
                    let profile = Profile {
                        server_id: server.server_id,
                        server_name: server.name.clone(),
                        spki_hex: sl_proto::crypto::hex(&d.spki_sha256),
                        addresses: vec![addr.clone()],
                        user_id: Some(ok.user.id),
                        device_id: Some(ok.device_id),
                        username: ok.user.username.clone(),
                        display_name: ok.user.display_name.clone(),
                    };
                    profile.save(&self.opts.dir)?;
                    self.profile = Some(profile);
                    if let Some(t) = &ok.session_token {
                        self.opts
                            .secrets
                            .set(SECRET_SESSION, t.as_bytes())
                            .map_err(ClientError::from)?;
                    }
                    self.mls.set_identity(DeviceRef {
                        user: ok.user.id,
                        device: ok.device_id,
                    });
                    self.attach(d.ws, ok, &addr).await;
                    return Ok(self.app_state());
                }
                ServerFrame::AuthError(e) => return Err(ClientError::Api(e)),
                _ => {
                    return Err(ClientError::Connect(
                        "Unexpected reply from the server.".into(),
                    ));
                }
            }
        }
        Err(last_err)
    }

    /// Sign in on a device that already pinned the server.
    pub(crate) async fn login(&mut self, input: LoginInput) -> ClientResult<AppState> {
        let p = self
            .profile
            .clone()
            .ok_or_else(|| ClientError::Invalid("Join with an invite first.".into()))?;
        let spki = p
            .spki()
            .ok_or_else(|| ClientError::Internal("missing server pin".into()))?;
        let same_user =
            p.username.eq_ignore_ascii_case(input.username.trim()) && p.device_id.is_some();
        if !same_user {
            self.reset_local(false)?;
        }
        let mut addrs = p.addresses.clone();
        if self.opts.discovery {
            for s in crate::discovery::browse(Duration::from_secs(2)).await {
                if s.server_id == Some(p.server_id) {
                    addrs.extend(s.addresses);
                }
            }
        }
        let mut last_err = ClientError::Connect("Can't reach the SpareLine server.".into());
        for addr in addrs {
            let mut d =
                match sl_net::dial(&addr, PinVerifier::new(PinMode::Pinned(spki)), DIAL_TIMEOUT)
                    .await
                {
                    Ok(d) => d,
                    Err((e, steps)) => {
                        self.diag.last_steps = steps_ui(steps);
                        last_err = if matches!(e, sl_net::ConnectError::PinMismatch) {
                            ClientError::Api(ApiError::new(ErrorCode::PinMismatch))
                        } else {
                            ClientError::Connect(format!("Couldn't connect to {addr}: {e}"))
                        };
                        continue;
                    }
                };
            let ServerFrame::Hello { server } = recv_raw(&mut d.ws, Duration::from_secs(5)).await?
            else {
                continue;
            };
            if server.server_id != p.server_id {
                continue;
            }
            send_raw(
                &mut d.ws,
                &ClientFrame::Login(LoginRequest {
                    proto: PROTOCOL_VERSION,
                    username: input.username.trim().to_string(),
                    password: input.password.clone(),
                    device_id: if same_user { p.device_id } else { None },
                    device: self.device_info(),
                }),
            )
            .await?;
            match recv_raw(&mut d.ws, Duration::from_secs(20)).await? {
                ServerFrame::Authed(ok) => {
                    let ok = *ok;
                    let mut np = p.clone();
                    np.user_id = Some(ok.user.id);
                    np.device_id = Some(ok.device_id);
                    np.username = ok.user.username.clone();
                    np.display_name = ok.user.display_name.clone();
                    np.save(&self.opts.dir)?;
                    self.profile = Some(np);
                    if let Some(t) = &ok.session_token {
                        self.opts
                            .secrets
                            .set(SECRET_SESSION, t.as_bytes())
                            .map_err(ClientError::from)?;
                    }
                    self.mls.set_identity(DeviceRef {
                        user: ok.user.id,
                        device: ok.device_id,
                    });
                    self.attach(d.ws, ok, &addr).await;
                    return Ok(self.app_state());
                }
                ServerFrame::AuthError(e) => return Err(ClientError::Api(e)),
                _ => continue,
            }
        }
        Err(last_err)
    }

    pub(crate) async fn logout(&mut self, forget: bool) -> ClientResult<AppState> {
        let _ = self.rpc(Request::Logout).await;
        self.opts.secrets.delete(SECRET_SESSION);
        self.conn = None;
        self.generation += 1;
        self.set_conn(ConnState::Offline);
        if forget {
            Profile::delete(&self.opts.dir);
            self.profile = None;
            self.me = None;
            self.reset_local(false)?;
        }
        self.publish_state();
        Ok(self.app_state())
    }

    pub(crate) fn reconnect_now(&mut self) {
        if self.conn.is_none() && self.phase() == Phase::Ready {
            self.attempt = 0;
            self.next_dial = Some(Instant::now());
        }
    }

    pub(crate) fn list_conversations(&self) -> Vec<ConversationView> {
        let mut v: Vec<ConversationView> = self
            .store
            .conversations()
            .iter()
            .map(|c| self.conversation_view(c))
            .collect();
        v.sort_by(|a, b| {
            let ta = a
                .last_message
                .as_ref()
                .map(|m| m.ts)
                .unwrap_or(a.updated_at);
            let tb = b
                .last_message
                .as_ref()
                .map(|m| m.ts)
                .unwrap_or(b.updated_at);
            tb.cmp(&ta)
        });
        v
    }

    pub(crate) fn messages(
        &self,
        conv: ConversationId,
        before: Option<i64>,
        limit: u32,
    ) -> Vec<MessageView> {
        self.store
            .messages(conv, before, limit.clamp(1, 200))
            .iter()
            .map(|m| self.message_view(m))
            .collect()
    }

    pub(crate) fn send_text(&mut self, input: SendInput) -> ClientResult<MessageView> {
        let body = input.body.trim_end().to_string();
        if body.trim().is_empty() {
            return Err(ClientError::Invalid("Type a message first.".into()));
        }
        if body.chars().count() > payload::MAX_BODY_CHARS {
            return Err(ClientError::Invalid(format!(
                "Messages can be up to {} characters.",
                payload::MAX_BODY_CHARS
            )));
        }
        let c = self
            .store
            .conversation(input.conversation)
            .ok_or_else(|| ClientError::Invalid("Unknown chat".into()))?;
        if c.read_only || c.removed {
            return Err(ClientError::Api(ApiError::new(ErrorCode::ReadOnly)));
        }
        let id = Uuid::now_v7();
        let m = LocalMessage {
            id,
            conversation: c.id,
            seq: None,
            sender: self.me_id(),
            kind: MessageKind::Text,
            body: body.clone(),
            reply_to: input.reply_to,
            ts: sl_proto::now_ms(),
            edited: false,
            deleted: false,
            status: SendStatus::Pending,
            mine: true,
        };
        self.store.insert_message(&m)?;
        self.queue(
            c.id,
            &Payload::Text {
                id,
                body,
                reply_to: input.reply_to,
            },
        )?;
        self.emit_conversation(c.id);
        Ok(self.message_view(&m))
    }

    pub(crate) fn edit(
        &mut self,
        conv: ConversationId,
        id: Uuid,
        body: String,
    ) -> ClientResult<()> {
        let me = self.me_id().ok_or(ClientError::Offline)?;
        let body = body.trim_end().to_string();
        if body.trim().is_empty() {
            return Err(ClientError::Invalid(
                "A message can't be empty. Delete it instead.".into(),
            ));
        }
        if !self.store.apply_edit(id, me, &body)? {
            return Err(ClientError::Invalid(
                "You can only edit your own messages.".into(),
            ));
        }
        self.queue(conv, &Payload::Edit { target: id, body })?;
        self.emit_message(id);
        self.emit_conversation(conv);
        Ok(())
    }

    pub(crate) fn delete(&mut self, conv: ConversationId, id: Uuid) -> ClientResult<()> {
        let me = self.me_id().ok_or(ClientError::Offline)?;
        let pending = self
            .store
            .message(id)
            .is_some_and(|m| m.status == SendStatus::Pending);
        if !self.store.apply_delete(id, me)? {
            return Err(ClientError::Invalid(
                "You can only delete your own messages.".into(),
            ));
        }
        if pending {
            // Never sent: just drop it from the outbox.
            for r in self.store.outbox() {
                if matches!(&r.payload, Payload::Text { id: pid, .. } if *pid == id) {
                    let _ = self.store.outbox_done(r.id);
                }
            }
        } else {
            self.queue(conv, &Payload::Delete { target: id })?;
        }
        self.emit_message(id);
        self.emit_conversation(conv);
        Ok(())
    }

    pub(crate) fn react(
        &mut self,
        conv: ConversationId,
        id: Uuid,
        emoji: String,
    ) -> ClientResult<()> {
        let me = self.me_id().ok_or(ClientError::Offline)?;
        if !payload::valid_emoji(&emoji) {
            return Err(ClientError::Invalid("Pick an emoji.".into()));
        }
        let add = !self.store.has_reaction(id, me, &emoji);
        if !self.store.apply_reaction(id, me, &emoji, add)? {
            return Err(ClientError::Invalid("That message is gone.".into()));
        }
        self.queue(
            conv,
            &Payload::Reaction {
                target: id,
                emoji,
                add,
            },
        )?;
        self.emit_message(id);
        Ok(())
    }

    pub(crate) async fn mark_read(&mut self, conv: ConversationId) -> ClientResult<()> {
        let c = self
            .store
            .conversation(conv)
            .ok_or_else(|| ClientError::Invalid("Unknown chat".into()))?;
        if c.read_seq >= c.wm {
            return Ok(());
        }
        self.store.set_read_seq(conv, c.wm)?;
        self.emit_conversation(conv);
        let receipts = self.store.kv_get("read_receipts").as_deref() != Some("off");
        if receipts && self.conn.is_some() {
            let _ = self
                .rpc(Request::MarkRead {
                    conversation: conv,
                    seq: c.wm,
                })
                .await;
        }
        Ok(())
    }

    pub(crate) fn typing(&mut self, conv: ConversationId) {
        if let Some(t) = &self.conn {
            t.send(ClientFrame::Typing { conversation: conv });
        }
    }

    pub(crate) async fn start_chat(
        &mut self,
        input: StartChatInput,
    ) -> ClientResult<ConversationView> {
        self.require_conn()?;
        let me = self.me_id().ok_or(ClientError::Offline)?;
        let my_dev = self.my_device().ok_or(ClientError::Offline)?;
        let members: BTreeSet<UserId> =
            input.members.iter().copied().filter(|u| *u != me).collect();
        if members.is_empty() {
            return Err(ClientError::Invalid("Choose at least one person.".into()));
        }
        if input.kind == ConversationKind::Direct {
            if members.len() != 1 {
                return Err(ClientError::Invalid(
                    "A direct chat is with one person.".into(),
                ));
            }
            let other = *members.iter().next().unwrap_or(&me);
            if let Some(existing) = self.store.conversations().into_iter().find(|c| {
                c.kind == ConversationKind::Direct && !c.removed && c.members.contains(&other)
            }) {
                return Ok(self.conversation_view(&existing));
            }
        } else if input.scope_group.is_none() {
            return Err(ClientError::Invalid(
                "Choose which group this chat belongs to.".into(),
            ));
        }
        let mut claim: Vec<UserId> = members.iter().copied().collect();
        claim.push(me);
        let Response::KeyPackages(kps) =
            self.rpc(Request::ClaimKeyPackages { users: claim }).await?
        else {
            return Err(ClientError::Internal("unexpected response".into()));
        };
        let mut valid = Vec::new();
        let mut devices = Vec::new();
        for k in kps {
            let expected = DeviceRef {
                user: k.user,
                device: k.device,
            };
            if expected == my_dev {
                continue;
            }
            match self
                .mls
                .validate_key_package(k.key_package.as_slice(), expected)
            {
                Ok(kp) => {
                    valid.push(kp);
                    devices.push(k.device);
                }
                Err(e) => self
                    .diag
                    .log(format!("skipped an invalid key package: {e}")),
            }
        }
        let created = self.mls.create_group(&valid)?;
        let req = Request::CreateConversation {
            kind: input.kind,
            scope_group: input.scope_group,
            members: members.iter().copied().collect(),
            mls_group_id: Bytes(created.group_id.clone()),
            epoch: created.epoch,
            welcome_devices: if created.welcome.is_some() {
                devices
            } else {
                vec![]
            },
            welcome: created.welcome.map(Bytes),
            group_info: Bytes(created.group_info),
        };
        match self.rpc(req).await {
            Ok(Response::Conversation(info)) => {
                self.store.upsert_conversation(&info)?;
                self.store.set_wm(info.id, 0)?;
                let title = input.title.trim();
                if input.kind == ConversationKind::Group && !title.is_empty() {
                    self.store.set_title(info.id, title)?;
                    self.queue(
                        info.id,
                        &Payload::ChatMeta {
                            title: title.to_string(),
                        },
                    )?;
                }
                let c = self
                    .store
                    .conversation(info.id)
                    .ok_or_else(|| ClientError::Internal("lost conversation".into()))?;
                let v = self.conversation_view(&c);
                self.emit(ClientEvent::Conversation(v.clone()));
                Ok(v)
            }
            Err(ClientError::Api(e)) if e.code == ErrorCode::AlreadyExists => {
                self.mls.delete_group(&created.group_id);
                if let Response::Conversations(list) = self.rpc(Request::Conversations).await? {
                    for info in &list {
                        self.store.upsert_conversation(info)?;
                    }
                }
                let id: Option<ConversationId> = e.detail.parse().ok();
                let c = id
                    .and_then(|i| self.store.conversation(i))
                    .ok_or(ClientError::Api(e))?;
                if !self.mls.has_group(&c.mls_group_id) {
                    let _ = self.sync_all().await;
                    if !self.mls.has_group(&c.mls_group_id) {
                        let _ = self.external_join(c.id).await;
                    }
                }
                Ok(self.conversation_view(&c))
            }
            Err(e) => {
                self.mls.delete_group(&created.group_id);
                Err(e)
            }
            Ok(_) => Err(ClientError::Internal("unexpected response".into())),
        }
    }

    pub(crate) async fn add_people(
        &mut self,
        conv: ConversationId,
        users: Vec<UserId>,
    ) -> ClientResult<()> {
        self.require_conn()?;
        let c = self
            .store
            .conversation(conv)
            .ok_or_else(|| ClientError::Invalid("Unknown chat".into()))?;
        if c.kind != ConversationKind::Group {
            return Err(ClientError::Invalid(
                "You can only add people to group chats.".into(),
            ));
        }
        let new: Vec<UserId> = users
            .into_iter()
            .filter(|u| !c.members.contains(u))
            .collect();
        if new.is_empty() {
            return Ok(());
        }
        for _ in 0..3 {
            let Response::KeyPackages(kps) = self
                .rpc(Request::ClaimKeyPackages { users: new.clone() })
                .await?
            else {
                return Err(ClientError::Internal("unexpected response".into()));
            };
            let mut valid = Vec::new();
            let mut devices = Vec::new();
            for k in kps {
                if let Ok(kp) = self.mls.validate_key_package(
                    k.key_package.as_slice(),
                    DeviceRef {
                        user: k.user,
                        device: k.device,
                    },
                ) {
                    valid.push(kp);
                    devices.push(k.device);
                }
            }
            if valid.is_empty() {
                // Nobody has a device ready; add them on the server so they join later.
                return Err(ClientError::Invalid(
                    "They haven't signed in to SpareLine yet. Try again once they have.".into(),
                ));
            }
            let pc = self.mls.add(&c.mls_group_id, &valid)?;
            match self.commit(conv, pc, new.clone(), vec![], devices).await {
                Ok(()) => return Ok(()),
                Err(ClientError::Api(e)) if e.code == ErrorCode::StaleEpoch => {
                    self.on_stale_epoch(conv, e.epoch).await?;
                }
                Err(e) => return Err(e),
            }
        }
        Err(ClientError::Connect(
            "The chat kept changing. Try again.".into(),
        ))
    }

    pub(crate) async fn leave(&mut self, conv: ConversationId) -> ClientResult<()> {
        self.rpc(Request::Leave { conversation: conv }).await?;
        if let Some(c) = self.store.conversation(conv) {
            self.mls.delete_group(&c.mls_group_id);
            self.store.set_removed(conv, true)?;
            self.store.outbox_drop_conversation(conv)?;
        }
        self.emit(ClientEvent::ConversationRemoved { id: conv });
        Ok(())
    }

    pub(crate) async fn directory(&mut self) -> ClientResult<Directory> {
        if self.conn.is_some() {
            self.refresh_directory().await
        } else {
            Ok(self.directory.clone())
        }
    }

    pub(crate) async fn set_presence(&mut self, p: Presence) -> ClientResult<()> {
        self.rpc(Request::SetPresence { presence: p })
            .await
            .map(|_| ())
    }

    pub(crate) async fn set_display_name(&mut self, name: String) -> ClientResult<()> {
        self.rpc(Request::SetDisplayName { display_name: name })
            .await
            .map(|_| ())
    }

    pub(crate) async fn change_password(
        &mut self,
        current: String,
        new: String,
    ) -> ClientResult<()> {
        self.rpc(Request::ChangePassword { current, new })
            .await
            .map(|_| ())
    }

    pub(crate) fn set_read_receipts(&mut self, on: bool) -> ClientResult<()> {
        self.store
            .kv_set("read_receipts", if on { "on" } else { "off" })
    }

    pub(crate) fn read_receipts(&self) -> bool {
        self.store.kv_get("read_receipts").as_deref() != Some("off")
    }

    pub(crate) fn search(&self, q: &str) -> Vec<SearchHit> {
        let q = q.trim();
        if q.chars().count() < 2 {
            return vec![];
        }
        self.store
            .search(q, 50)
            .iter()
            .filter_map(|m| {
                let c = self.store.conversation(m.conversation)?;
                Some(SearchHit {
                    conversation: c.id,
                    conversation_title: self.conversation_view(&c).title,
                    message: self.message_view(m),
                })
            })
            .collect()
    }

    pub(crate) fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            app_version: self.opts.client_version.clone(),
            platform: self.opts.platform.clone(),
            server: self.app_state().server,
            conn: self.conn_state.clone(),
            last_attempt: self.diag.last_steps.clone(),
            discovered: self.diag.discovered.clone(),
            recent_failures: self
                .diag
                .failures
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            log: self.diag.log.iter().cloned().collect(),
        }
    }
}

enum AuthKind {
    Register(JoinInput),
    Redeem(RedeemInput),
}

/// Background dial + resume (runs outside the core so the UI stays responsive).
async fn dial_resume(
    p: Profile,
    spki: [u8; 32],
    token: String,
    device_id: DeviceId,
    version: String,
    discovery: bool,
) -> Result<DialOk, DialErr> {
    let mut tried: Vec<String> = Vec::new();
    let mut last = DialErr {
        message: "No known server address".into(),
        category: "no_address".into(),
        steps: vec![],
        api: None,
    };
    for round in 0..2 {
        let mut addrs: Vec<String> = Vec::new();
        if round == 0 {
            addrs = p.addresses.clone();
        } else if discovery {
            for s in crate::discovery::browse(Duration::from_millis(1800)).await {
                if s.server_id == Some(p.server_id) {
                    addrs.extend(s.addresses);
                }
            }
            if addrs.is_empty() {
                last.message = format!(
                    "{} (and the server wasn't found on the network)",
                    last.message
                );
                last.category = if last.category == "no_address" {
                    "not_discovered".into()
                } else {
                    last.category
                };
            }
        }
        for addr in addrs {
            if tried.contains(&addr) {
                continue;
            }
            tried.push(addr.clone());
            let mut d =
                match sl_net::dial(&addr, PinVerifier::new(PinMode::Pinned(spki)), DIAL_TIMEOUT)
                    .await
                {
                    Ok(d) => d,
                    Err((e, steps)) => {
                        last = DialErr {
                            message: e.to_string(),
                            category: e.category().into(),
                            steps: steps_ui(steps),
                            api: None,
                        };
                        continue;
                    }
                };
            let steps = steps_ui(std::mem::take(&mut d.steps));
            let fail = |m: String, cat: &str, api: Option<ApiError>| DialErr {
                message: m,
                category: cat.into(),
                steps: steps.clone(),
                api,
            };
            match recv_raw(&mut d.ws, Duration::from_secs(5)).await {
                Ok(ServerFrame::Hello { server }) if server.server_id == p.server_id => {}
                Ok(_) => {
                    last = fail(
                        "A different server answered at this address".into(),
                        "wrong_server",
                        None,
                    );
                    continue;
                }
                Err(e) => {
                    last = fail(e.to_string(), "hello", None);
                    continue;
                }
            }
            let resume = ClientFrame::Resume(ResumeRequest {
                proto: PROTOCOL_VERSION,
                token: token.clone(),
                device_id,
                client_version: version.clone(),
            });
            if let Err(e) = send_raw(&mut d.ws, &resume).await {
                last = fail(e.to_string(), "resume", None);
                continue;
            }
            match recv_raw(&mut d.ws, Duration::from_secs(10)).await {
                Ok(ServerFrame::Authed(ok)) => {
                    return Ok(DialOk {
                        ws: d.ws,
                        auth: *ok,
                        address: addr,
                        steps,
                    });
                }
                Ok(ServerFrame::AuthError(e)) => {
                    return Err(fail(e.user_message().into(), "auth", Some(e)));
                }
                Ok(_) => last = fail("Unexpected reply".into(), "resume", None),
                Err(e) => last = fail(e.to_string(), "resume", None),
            }
        }
    }
    Err(last)
}
