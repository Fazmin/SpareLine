//! Local client state on disk: the pinned server, the signed-in account, and secrets.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sl_proto::{DeviceId, UserId};
use uuid::Uuid;

/// Platform default data directory. `SPARELINE_CLIENT_DIR` overrides it (handy for
/// running two clients on one machine during development).
pub fn default_dir() -> PathBuf {
    if let Ok(p) = std::env::var("SPARELINE_CLIENT_DIR")
        && !p.is_empty()
    {
        return PathBuf::from(p);
    }
    if cfg!(windows) {
        if let Ok(p) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(p).join("SpareLine").join("Client");
        }
    } else if cfg!(target_os = "macos") {
        if let Ok(h) = std::env::var("HOME") {
            return PathBuf::from(h).join("Library/Application Support/SpareLine/Client");
        }
    } else if let Ok(h) = std::env::var("HOME") {
        return PathBuf::from(h).join(".local/share/spareline/client");
    }
    PathBuf::from("spareline-data").join("client")
}

/// The pinned server and signed-in account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Profile {
    pub server_id: Uuid,
    pub server_name: String,
    /// Hex SHA-256 of the server's SPKI (the pin).
    pub spki_hex: String,
    /// Known-good addresses, most recent first.
    pub addresses: Vec<String>,
    pub user_id: Option<UserId>,
    pub device_id: Option<DeviceId>,
    pub username: String,
    pub display_name: String,
}

impl Profile {
    pub fn spki(&self) -> Option<[u8; 32]> {
        let v = data_encoding::HEXLOWER
            .decode(self.spki_hex.as_bytes())
            .ok()?;
        v.try_into().ok()
    }

    pub fn fingerprint(&self) -> String {
        self.spki()
            .map(|s| sl_proto::crypto::fingerprint_display(&s))
            .unwrap_or_default()
    }

    pub fn remember_address(&mut self, addr: &str) {
        self.addresses.retain(|a| a != addr);
        self.addresses.insert(0, addr.to_string());
        self.addresses.truncate(5);
    }

    pub fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join("profile.json")).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join("profile.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, dir.join("profile.json"))
    }

    pub fn delete(dir: &Path) {
        let _ = std::fs::remove_file(dir.join("profile.json"));
    }
}

/// Where session tokens and the local database key are kept.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    fn set(&self, key: &str, value: &[u8]) -> anyhow::Result<()>;
    fn delete(&self, key: &str);
}

/// OS keychain (Windows Credential Manager, macOS Keychain).
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    /// One keychain namespace per data directory.
    pub fn for_dir(dir: &Path) -> Self {
        let h = sl_proto::crypto::sha256_hex(dir.to_string_lossy().as_bytes());
        Self {
            service: format!("SpareLine/{}", &h[..12]),
        }
    }

    /// Check the keychain works on this machine.
    pub fn probe(&self) -> bool {
        let k = "probe";
        self.set(k, b"ok").is_ok() && self.get(k).as_deref() == Some(b"ok".as_slice()) && {
            self.delete(k);
            true
        }
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        keyring::Entry::new(&self.service, key)
            .ok()?
            .get_secret()
            .ok()
    }
    fn set(&self, key: &str, value: &[u8]) -> anyhow::Result<()> {
        keyring::Entry::new(&self.service, key)?.set_secret(value)?;
        Ok(())
    }
    fn delete(&self, key: &str) {
        if let Ok(e) = keyring::Entry::new(&self.service, key) {
            let _ = e.delete_credential();
        }
    }
}

/// In-memory store for tests and headless tools.
#[derive(Default, Clone)]
pub struct MemoryStore(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.0.lock().get(key).cloned()
    }
    fn set(&self, key: &str, value: &[u8]) -> anyhow::Result<()> {
        self.0.lock().insert(key.to_string(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) {
        self.0.lock().remove(key);
    }
}

/// File fallback when no keychain is available (owner-only file in the data dir).
pub struct FileStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("secrets.json"),
            lock: Mutex::new(()),
        }
    }
    fn read(&self) -> HashMap<String, String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }
}

impl SecretStore for FileStore {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let _g = self.lock.lock();
        self.read()
            .get(key)
            .and_then(|v| data_encoding::BASE64.decode(v.as_bytes()).ok())
    }
    fn set(&self, key: &str, value: &[u8]) -> anyhow::Result<()> {
        let _g = self.lock.lock();
        let mut m = self.read();
        m.insert(key.into(), data_encoding::BASE64.encode(value));
        std::fs::write(&self.path, serde_json::to_vec(&m)?)?;
        Ok(())
    }
    fn delete(&self, key: &str) {
        let _g = self.lock.lock();
        let mut m = self.read();
        m.remove(key);
        let _ = std::fs::write(&self.path, serde_json::to_vec(&m).unwrap_or_default());
    }
}
