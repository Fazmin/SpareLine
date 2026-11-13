//! Server configuration (`config.toml`) and data-directory layout.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server_id: Uuid,
    pub server_name: String,
    /// Address to bind. `0.0.0.0` listens on every interface.
    pub bind: String,
    pub port: u16,
    pub mdns_enabled: bool,
    /// How long relayed ciphertext stays on the server for offline devices.
    pub envelope_retention_days: u32,
    pub log_retention_days: u32,
    pub connection_event_retention_days: u32,
    pub backup_keep: u32,
    pub backup_interval_hours: u32,
    /// Sessions expire after this many days without use.
    pub session_idle_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_id: Uuid::now_v7(),
            server_name: String::new(),
            bind: "0.0.0.0".into(),
            port: sl_proto::DEFAULT_PORT,
            mdns_enabled: true,
            envelope_retention_days: 30,
            log_retention_days: 14,
            connection_event_retention_days: 7,
            backup_keep: 7,
            backup_interval_hours: 24,
            session_idle_days: 30,
        }
    }
}

impl Config {
    pub fn load_or_create(paths: &Paths) -> anyhow::Result<Self> {
        if paths.config.exists() {
            let text = std::fs::read_to_string(&paths.config)?;
            let cfg: Config = toml::from_str(&text)?;
            Ok(cfg)
        } else {
            let cfg = Config {
                server_name: default_server_name(),
                ..Config::default()
            };
            cfg.save(paths)?;
            Ok(cfg)
        }
    }

    pub fn save(&self, paths: &Paths) -> anyhow::Result<()> {
        let text = toml::to_string_pretty(self)?;
        let tmp = paths.config.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &paths.config)?;
        Ok(())
    }
}

/// "SpareLine on OFFICE-PC"
pub fn default_server_name() -> String {
    format!("SpareLine on {}", sl_net::hostname())
}

/// Every file the server keeps, under one root directory.
#[derive(Debug, Clone)]
pub struct Paths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub db: PathBuf,
    pub tls_dir: PathBuf,
    pub cert: PathBuf,
    pub key: PathBuf,
    pub logs: PathBuf,
    pub backups: PathBuf,
    pub reports: PathBuf,
}

impl Paths {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let tls_dir = root.join("tls");
        Self {
            config: root.join("config.toml"),
            db: root.join("spareline.db"),
            cert: tls_dir.join("server.crt.der"),
            key: tls_dir.join("server.key.der"),
            tls_dir,
            logs: root.join("logs"),
            backups: root.join("backups"),
            reports: root.join("reports"),
            root,
        }
    }

    /// Platform default:
    /// - Windows: `%LOCALAPPDATA%\SpareLine\Server`
    /// - macOS: `~/Library/Application Support/SpareLine/Server`
    /// - other: `~/.local/share/spareline/server`
    pub fn default_root() -> PathBuf {
        if cfg!(windows) {
            if let Ok(p) = std::env::var("LOCALAPPDATA") {
                return PathBuf::from(p).join("SpareLine").join("Server");
            }
        } else if cfg!(target_os = "macos") {
            if let Ok(h) = std::env::var("HOME") {
                return PathBuf::from(h).join("Library/Application Support/SpareLine/Server");
            }
        } else if let Ok(h) = std::env::var("HOME") {
            return PathBuf::from(h).join(".local/share/spareline/server");
        }
        PathBuf::from("spareline-data").join("server")
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [
            &self.root,
            &self.tls_dir,
            &self.logs,
            &self.backups,
            &self.reports,
        ] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}
