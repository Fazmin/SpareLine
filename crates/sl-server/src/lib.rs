//! SpareLine server engine.
//!
//! ```text
//! Engine::open(options) → setup (first run) → start() → [serving] → stop()
//! ```
//!
//! The engine is embedded in the SpareLine Server tray app (control panel) or run
//! headless. The admin surface ([`admin::AdminApi`]) is in-process only.

pub mod admin;
mod api;
pub mod auth;
mod broadcast;
pub mod config;
pub mod db;
mod delivery;
pub mod directory;
pub mod error;
pub mod health;
pub mod hub;
mod mdns;
pub mod net;
mod retention;
pub mod telemetry;
pub mod tls;
mod ws;

use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Router;
use axum::routing::get;
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use parking_lot::{Mutex, RwLock};
use rusqlite::{OptionalExtension, params};
use sl_proto::panel::{ConnEventKind, HealthCheck};
use sl_proto::{ApiError, ErrorCode, ServerInfo, UserId};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub use config::{Config, Paths};
pub use error::{AppError, AppResult};
pub use telemetry::Telemetry;

use crate::db::{Db, now};
use crate::directory::{DirectoryCache, Visibility};
use crate::error::api;
use crate::hub::Hub;
use crate::tls::TlsIdentity;

/// Options for opening an engine.
pub struct EngineOptions {
    pub data_dir: PathBuf,
    pub telemetry: Telemetry,
    /// Override the bind address (tests use `127.0.0.1:0`).
    pub bind_override: Option<SocketAddr>,
    /// Advertise over mDNS (subject to config). Tests turn this off.
    pub mdns: bool,
}

impl EngineOptions {
    pub fn new(data_dir: impl Into<PathBuf>, telemetry: Telemetry) -> Self {
        Self {
            data_dir: data_dir.into(),
            telemetry,
            bind_override: None,
            mdns: true,
        }
    }
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

struct Inner {
    paths: Paths,
    config: RwLock<Config>,
    db: Db,
    tls: RwLock<TlsIdentity>,
    hub: Hub,
    dir: DirectoryCache,
    tel: Telemetry,
    started_at: i64,
    /// Per source address (generous: many staff can share one address behind NAT,
    /// terminal servers or a VPN gateway).
    auth_limiter: DefaultKeyedRateLimiter<IpAddr>,
    /// Per (address, username) password attempts (strict).
    login_limiter: DefaultKeyedRateLimiter<String>,
    runtime: Mutex<Runtime>,
    bind_override: Option<SocketAddr>,
    mdns_allowed: bool,
    sys: Mutex<(
        sysinfo::System,
        Option<(Instant, sl_proto::panel::SystemStats)>,
    )>,
    health_cache: Mutex<Option<(Instant, Vec<HealthCheck>)>>,
}

#[derive(Default)]
struct Runtime {
    server: Option<JoinHandle<()>>,
    shutdown: Option<oneshot::Sender<()>>,
    local_addr: Option<SocketAddr>,
    mdns: Option<mdns::Advertiser>,
    mdns_instance: Option<String>,
    mdns_error: Option<String>,
    jobs: Vec<JoinHandle<()>>,
}

impl Engine {
    /// Open (or create) the data directory, config, database and TLS identity.
    pub fn open(opts: EngineOptions) -> anyhow::Result<Self> {
        let paths = Paths::new(&opts.data_dir);
        paths.ensure()?;
        let config = Config::load_or_create(&paths)?;
        let db = Db::open(&paths.db)?;
        let tls = TlsIdentity::load_or_create(&paths)?;
        let vis = db
            .write_sync(|c| Visibility::load(c))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let dir = DirectoryCache::default();
        dir.set(vis);
        // Per address: 120 attempts a minute, bursts of 60.
        let quota = Quota::per_minute(NonZeroU32::new(120).unwrap_or(NonZeroU32::MIN))
            .allow_burst(NonZeroU32::new(60).unwrap_or(NonZeroU32::MIN));
        // Per username and address: 10 password attempts a minute, bursts of 5.
        let login_quota = Quota::per_minute(NonZeroU32::new(10).unwrap_or(NonZeroU32::MIN))
            .allow_burst(NonZeroU32::new(5).unwrap_or(NonZeroU32::MIN));
        tracing::info!(data_dir = %paths.root.display(), fingerprint = %tls.fingerprint, "engine opened");
        Ok(Self {
            inner: Arc::new(Inner {
                paths,
                config: RwLock::new(config),
                db,
                tls: RwLock::new(tls),
                hub: Hub::default(),
                dir,
                tel: opts.telemetry,
                started_at: now(),
                auth_limiter: RateLimiter::keyed(quota),
                login_limiter: RateLimiter::keyed(login_quota),
                runtime: Mutex::new(Runtime::default()),
                bind_override: opts.bind_override,
                mdns_allowed: opts.mdns,
                sys: Mutex::new((sysinfo::System::new(), None)),
                health_cache: Mutex::new(None),
            }),
        })
    }

    // ---- accessors -------------------------------------------------------------------

    pub fn admin(&self) -> admin::AdminApi {
        admin::AdminApi::new(self.clone())
    }
    pub fn paths(&self) -> &Paths {
        &self.inner.paths
    }
    pub fn config(&self) -> Config {
        self.inner.config.read().clone()
    }
    pub(crate) fn db(&self) -> &Db {
        &self.inner.db
    }
    pub fn tls(&self) -> TlsIdentity {
        self.inner.tls.read().clone()
    }
    pub fn hub(&self) -> &Hub {
        &self.inner.hub
    }
    pub fn dir(&self) -> &DirectoryCache {
        &self.inner.dir
    }
    pub fn tel(&self) -> &Telemetry {
        &self.inner.tel
    }
    pub fn started_at(&self) -> i64 {
        self.inner.started_at
    }
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.inner.runtime.lock().local_addr
    }
    pub fn mdns_instance(&self) -> Option<String> {
        self.inner.runtime.lock().mdns_instance.clone()
    }
    pub fn mdns_error(&self) -> Option<String> {
        self.inner.runtime.lock().mdns_error.clone()
    }

    pub fn server_info(&self) -> ServerInfo {
        let c = self.inner.config.read();
        ServerInfo {
            server_id: c.server_id,
            name: c.server_name.clone(),
            proto: sl_proto::PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub(crate) fn update_config<T>(&self, f: impl FnOnce(&mut Config) -> T) -> AppResult<T> {
        let mut c = self.inner.config.write();
        let out = f(&mut c);
        c.save(&self.inner.paths)?;
        Ok(out)
    }

    pub(crate) fn allow_auth_attempt(&self, peer: SocketAddr) -> bool {
        self.inner.auth_limiter.check_key(&peer.ip()).is_ok()
    }

    /// Password attempts for one username from one address.
    pub(crate) fn allow_login_attempt(&self, peer: SocketAddr, username: &str) -> bool {
        let key = format!("{}|{}", peer.ip(), username.trim().to_lowercase());
        self.inner.login_limiter.check_key(&key).is_ok() && self.allow_auth_attempt(peer)
    }

    pub async fn reload_directory(&self) -> AppResult<()> {
        let v = self.db().read(Visibility::load).await?;
        self.inner.dir.set(v);
        Ok(())
    }

    /// Record a connection event (fire-and-forget).
    pub fn conn_event(
        &self,
        peer: SocketAddr,
        user: Option<UserId>,
        device: Option<sl_proto::DeviceId>,
        kind: ConnEventKind,
        detail: impl Into<String>,
    ) {
        let db = self.inner.db.clone();
        let detail: String = detail.into();
        let ip = peer.ip().to_string();
        if matches!(
            kind,
            ConnEventKind::TlsFailed
                | ConnEventKind::AuthFailed
                | ConnEventKind::RateLimited
                | ConnEventKind::ProtocolMismatch
        ) {
            tracing::info!(target: "sl_server::ws", %ip, kind = admin::event_kind_str(kind), %detail, "connection problem");
        }
        tokio::spawn(async move {
            let _ = db
                .write(move |c| {
                    c.execute(
                        "INSERT INTO connection_events (ts, ip, user_id, device_id, kind, detail) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![now(), ip, user, device, admin::event_kind_str(kind), detail],
                    )?;
                    Ok(())
                })
                .await;
        });
    }

    /// Append to the admin audit log (fire-and-forget). Never pass content here.
    pub fn audit(&self, action: &str, target: &str, detail: impl Into<String>) {
        let db = self.inner.db.clone();
        let (action, target, detail) = (action.to_string(), target.to_string(), detail.into());
        tracing::info!(target: "sl_server::admin", %action, %target, "admin action");
        tokio::spawn(async move {
            let _ = db
                .write(move |c| {
                    c.execute(
                        "INSERT INTO audit_log (ts, action, target, detail) VALUES (?1, ?2, ?3, ?4)",
                        params![now(), action, target, detail],
                    )?;
                    Ok(())
                })
                .await;
        });
    }

    pub fn system_stats(&self) -> sl_proto::panel::SystemStats {
        let mut g = self.inner.sys.lock();
        if let Some((t, s)) = &g.1
            && t.elapsed() < Duration::from_secs(2)
        {
            return s.clone();
        }
        let stats =
            health::collect_system(&mut g.0, &self.inner.paths.root, self.inner.db.size_bytes());
        g.1 = Some((Instant::now(), stats.clone()));
        stats
    }

    pub(crate) async fn cached_health(&self) -> Vec<HealthCheck> {
        if let Some((t, v)) = self.inner.health_cache.lock().as_ref()
            && t.elapsed() < Duration::from_secs(30)
        {
            return v.clone();
        }
        let v = health::run_checks(self).await;
        self.set_cached_health(v.clone());
        v
    }

    pub(crate) fn set_cached_health(&self, v: Vec<HealthCheck>) {
        *self.inner.health_cache.lock() = Some((Instant::now(), v));
    }

    // ---- admin account (control panel) -------------------------------------------------

    pub fn needs_setup(&self) -> bool {
        self.inner
            .db
            .write_sync(|c| {
                Ok(
                    c.query_row("SELECT 1 FROM admin WHERE id = 1", [], |_| Ok(()))
                        .optional()?,
                )
            })
            .map(|r| r.is_none())
            .unwrap_or(true)
    }

    /// First-run setup: name the server and set the panel password.
    /// Returns a recovery key to show the admin once.
    pub async fn setup(&self, server_name: &str, admin_password: &str) -> AppResult<String> {
        if !self.needs_setup() {
            return api(ErrorCode::AlreadyExists);
        }
        auth::validate_password(admin_password)?;
        let name = server_name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return error::api_detail(ErrorCode::BadRequest, "server name is 1–64 characters");
        }
        let recovery = recovery_key();
        let pw_hash = auth::hash_password_async(admin_password.to_string()).await?;
        let rk_hash = auth::hash_password_async(normalize_recovery(&recovery)).await?;
        self.db()
            .write(move |c| {
                c.execute(
                    "INSERT INTO admin (id, password_hash, recovery_hash, created_at, updated_at) VALUES (1, ?1, ?2, ?3, ?3)",
                    params![pw_hash, rk_hash, now()],
                )?;
                Ok(())
            })
            .await?;
        let name = name.to_string();
        self.update_config(|c| c.server_name = name)?;
        self.audit("admin.setup", "", "first-run setup completed");
        Ok(recovery)
    }

    pub async fn verify_admin(&self, password: &str) -> bool {
        let hash = self
            .db()
            .read(|c| {
                Ok(
                    c.query_row("SELECT password_hash FROM admin WHERE id = 1", [], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()?,
                )
            })
            .await
            .ok()
            .flatten();
        match hash {
            Some(h) => auth::verify_password_async(h, password.to_string())
                .await
                .unwrap_or(false),
            None => false,
        }
    }

    pub(crate) async fn set_admin_password(&self, new: &str) -> AppResult<()> {
        auth::validate_password(new)?;
        let h = auth::hash_password_async(new.to_string()).await?;
        self.db()
            .write(move |c| {
                c.execute(
                    "UPDATE admin SET password_hash = ?1, updated_at = ?2 WHERE id = 1",
                    params![h, now()],
                )?;
                Ok(())
            })
            .await
    }

    pub(crate) async fn reset_admin_with_recovery(
        &self,
        recovery: &str,
        new: &str,
    ) -> AppResult<String> {
        auth::validate_password(new)?;
        let hash = self
            .db()
            .read(|c| {
                Ok(
                    c.query_row("SELECT recovery_hash FROM admin WHERE id = 1", [], |r| {
                        r.get::<_, String>(0)
                    })
                    .optional()?,
                )
            })
            .await?;
        let Some(hash) = hash else {
            return api(ErrorCode::NotFound);
        };
        if !auth::verify_password_async(hash, normalize_recovery(recovery)).await? {
            return api(ErrorCode::InvalidCredentials);
        }
        let new_recovery = recovery_key();
        let pw = auth::hash_password_async(new.to_string()).await?;
        let rk = auth::hash_password_async(normalize_recovery(&new_recovery)).await?;
        self.db()
            .write(move |c| {
                c.execute(
                    "UPDATE admin SET password_hash = ?1, recovery_hash = ?2, updated_at = ?3 WHERE id = 1",
                    params![pw, rk, now()],
                )?;
                Ok(())
            })
            .await?;
        self.audit("admin.password_reset", "", "reset with recovery key");
        Ok(new_recovery)
    }

    // ---- lifecycle -------------------------------------------------------------------

    /// Bind the TLS listener, start serving, advertise over mDNS and start background jobs.
    pub async fn start(&self) -> anyhow::Result<SocketAddr> {
        if let Some(addr) = self.local_addr() {
            return Ok(addr);
        }
        let cfg = self.config();
        let addr: SocketAddr = match self.inner.bind_override {
            Some(a) => a,
            None => format!("{}:{}", cfg.bind, cfg.port).parse()?,
        };
        let tls_cfg = self.tls().server_config()?;
        let me = self.clone();
        let hook: net::TlsFailureHook = Arc::new(move |peer, reason| {
            me.conn_event(peer, None, None, ConnEventKind::TlsFailed, reason);
        });
        let listener = net::TlsListener::bind(addr, tls_cfg, hook)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    anyhow::anyhow!(
                        "port {} is already in use (is another SpareLine Server running?)",
                        addr.port()
                    )
                } else {
                    anyhow::anyhow!("could not listen on {addr}: {e}")
                }
            })?;
        let local = axum::serve::Listener::local_addr(&listener)?;
        let app = Router::new()
            .route(sl_proto::WS_PATH, get(ws::route))
            .route(sl_proto::HEALTH_PATH, get(health_route))
            .with_state(self.clone())
            .into_make_service_with_connect_info::<net::PeerAddr>();
        let (tx, rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let res = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
            if let Err(e) = res {
                tracing::error!(error = %e, "server stopped with an error");
            }
        });
        {
            let mut rt = self.inner.runtime.lock();
            rt.server = Some(server);
            rt.shutdown = Some(tx);
            rt.local_addr = Some(local);
            rt.jobs.push(retention::spawn(self.clone()));
        }
        self.restart_mdns();
        tracing::info!(%local, name = %cfg.server_name, "SpareLine Server is running");
        Ok(local)
    }

    /// (Re)start the mDNS advertisement with the current name and port.
    pub(crate) fn restart_mdns(&self) {
        let mut rt = self.inner.runtime.lock();
        if let Some(a) = rt.mdns.take() {
            a.stop();
        }
        rt.mdns_instance = None;
        rt.mdns_error = None;
        let cfg = self.config();
        let Some(local) = rt.local_addr else { return };
        if !(cfg.mdns_enabled && self.inner.mdns_allowed) {
            return;
        }
        match mdns::Advertiser::start(
            cfg.server_id,
            &cfg.server_name,
            local.port(),
            &self.tls().fingerprint_hint,
        ) {
            Ok(a) => {
                rt.mdns_instance = Some(a.instance.clone());
                rt.mdns = Some(a);
            }
            Err(e) => {
                tracing::warn!(target: "sl_server::mdns", error = %e, "mDNS advertising failed");
                rt.mdns_error = Some(format!(
                    "mDNS could not start: {e}. Clients can still connect by address."
                ));
            }
        }
    }

    /// Stop serving: tell clients, stop mDNS and background jobs.
    pub async fn stop(&self) {
        self.hub().close_all(ApiError::new(ErrorCode::ShuttingDown));
        let (server, shutdown, mdns, jobs) = {
            let mut rt = self.inner.runtime.lock();
            rt.local_addr = None;
            rt.mdns_instance = None;
            (
                rt.server.take(),
                rt.shutdown.take(),
                rt.mdns.take(),
                std::mem::take(&mut rt.jobs),
            )
        };
        if let Some(a) = mdns {
            a.stop();
        }
        for j in jobs {
            j.abort();
        }
        if let Some(tx) = shutdown {
            let _ = tx.send(());
        }
        if let Some(s) = server {
            let _ = tokio::time::timeout(Duration::from_secs(5), s).await;
        }
        tracing::info!("SpareLine Server stopped");
    }
}

async fn health_route(
    axum::extract::State(engine): axum::extract::State<Engine>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "status": "ok", "server": engine.server_info() }))
}

/// 24-symbol recovery key, e.g. `RK-ABCD-EFGH-JKMN-PQRS-TVWX-YZ01`.
fn recovery_key() -> String {
    let a = sl_proto::crypto::generate_code();
    let b = sl_proto::crypto::generate_code();
    let s: String = format!("{a}{b}").chars().take(24).collect();
    let groups: Vec<String> = s
        .as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect();
    format!("RK-{}", groups.join("-"))
}

fn normalize_recovery(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect::<String>()
        .trim_start_matches("RK")
        .to_string()
}
