//! Standby copies: a self-contained, encrypted copy of this server (database,
//! certificate key and settings) kept somewhere else, such as a USB drive or a shared
//! folder on another computer. If this computer fails, SpareLine Server on another
//! computer restores the copy and takes over with the same identity, so staff apps
//! trust it and reconnect by themselves.
//!
//! File layout (`.slstandby`):
//!
//! ```text
//! header: "SLSTBY01" | m_cost u32 | t_cost u32 | p_cost u32 | salt [16] | nonce prefix [19]
//! chunks: last u8 | len u32 | ciphertext (len bytes, including the 16-byte tag)
//! ```
//!
//! Each chunk is XChaCha20-Poly1305 under a key derived from the standby password with
//! Argon2id. Its nonce is `prefix ‖ index (u32) ‖ last`, and the header is the
//! associated data, so a truncated, reordered or edited file fails to open. Numbers
//! are little-endian.
//!
//! The plaintext is a list of entries, `name_len u16 | name | len u64 | bytes`, ending
//! with `name_len = 0`. The first entry is `manifest.json`.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sl_proto::panel::{CheckStatus, HealthCheck, StandbyCopyRow, StandbyInfo, StandbyStatus};
use sl_proto::{ErrorCode, RestoreMarker};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::Engine;
use crate::config::{Config, Paths};
use crate::db::now;
use crate::error::{AppError, AppResult, api_detail};
use crate::health::{human_age, lan_addresses};
use crate::tls::{TlsIdentity, write_private};

const MAGIC: &[u8; 8] = b"SLSTBY01";
const HEADER_LEN: usize = 8 + 12 + 16 + 19;
const CHUNK: usize = 1 << 20;
const TAG: usize = 16;
const PREFIX: &str = "spareline-standby-";
const EXT: &str = "slstandby";

// Argon2id for new standby passwords: 64 MiB, 3 passes.
const M_COST: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 1;
// A crafted copy could ask for huge parameters to make opening it hang.
const MAX_M_COST: u32 = 1024 * 1024;
const MAX_T_COST: u32 = 16;
const MAX_P_COST: u32 = 16;

pub const MIN_PASSWORD_CHARS: usize = 10;

const E_MANIFEST: &str = "manifest.json";
const E_CONFIG: &str = "config.toml";
const E_CERT: &str = "tls/server.crt.der";
const E_KEY: &str = "tls/server.key.der";
const E_STANDBY_KEY: &str = "standby.key";
const E_DB: &str = "spareline.db";
const ENTRIES: [&str; 6] = [E_MANIFEST, E_CONFIG, E_CERT, E_KEY, E_STANDBY_KEY, E_DB];
const REQUIRED: [&str; 5] = [E_MANIFEST, E_CONFIG, E_CERT, E_KEY, E_DB];

// ---------------------------------------------------------------------------------------
// Keys
// ---------------------------------------------------------------------------------------

#[derive(Clone)]
struct StandbyKey {
    m: u32,
    t: u32,
    p: u32,
    salt: [u8; 16],
    key: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    m: u32,
    t: u32,
    p: u32,
    salt: String,
    key: String,
}

fn derive(password: &str, salt: &[u8; 16], m: u32, t: u32, p: u32) -> AppResult<[u8; 32]> {
    let params = Params::new(m, t, p, Some(32)).map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
    let mut out = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|e| anyhow::anyhow!("argon2: {e}"))?;
    Ok(out)
}

fn load_key(paths: &Paths) -> Option<StandbyKey> {
    let text = std::fs::read_to_string(&paths.standby_key).ok()?;
    let f: KeyFile = serde_json::from_str(&text).ok()?;
    let b64 = data_encoding::BASE64;
    Some(StandbyKey {
        m: f.m,
        t: f.t,
        p: f.p,
        salt: b64.decode(f.salt.as_bytes()).ok()?.try_into().ok()?,
        key: b64.decode(f.key.as_bytes()).ok()?.try_into().ok()?,
    })
}

/// Set (or change) the standby password. Copies made from now on use it; older
/// copies still open with the password they were made with.
pub async fn set_password(paths: &Paths, password: String) -> AppResult<()> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return api_detail(
            ErrorCode::WeakPassword,
            format!("use at least {MIN_PASSWORD_CHARS} characters for the standby password"),
        );
    }
    let path = paths.standby_key.clone();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let salt: [u8; 16] = sl_proto::crypto::random_bytes();
        let key = derive(&password, &salt, M_COST, T_COST, P_COST)?;
        let b64 = data_encoding::BASE64;
        let f = KeyFile {
            m: M_COST,
            t: T_COST,
            p: P_COST,
            salt: b64.encode(&salt),
            key: b64.encode(&key),
        };
        write_private(&path, &serde_json::to_vec(&f)?)?;
        Ok(())
    })
    .await?
}

pub fn has_password(paths: &Paths) -> bool {
    load_key(paths).is_some()
}

// ---------------------------------------------------------------------------------------
// Sealed stream
// ---------------------------------------------------------------------------------------

fn nonce(prefix: &[u8; 19], index: u32, last: bool) -> [u8; 24] {
    let mut n = [0u8; 24];
    n[..19].copy_from_slice(prefix);
    n[19..23].copy_from_slice(&index.to_be_bytes());
    n[23] = u8::from(last);
    n
}

struct SealWriter<W: Write> {
    out: W,
    cipher: XChaCha20Poly1305,
    header: Vec<u8>,
    prefix: [u8; 19],
    index: u32,
    buf: Vec<u8>,
}

impl<W: Write> SealWriter<W> {
    fn new(mut out: W, key: &StandbyKey) -> io::Result<Self> {
        let prefix: [u8; 19] = sl_proto::crypto::random_bytes();
        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&key.m.to_le_bytes());
        header.extend_from_slice(&key.t.to_le_bytes());
        header.extend_from_slice(&key.p.to_le_bytes());
        header.extend_from_slice(&key.salt);
        header.extend_from_slice(&prefix);
        out.write_all(&header)?;
        Ok(Self {
            out,
            cipher: XChaCha20Poly1305::new((&key.key).into()),
            header,
            prefix,
            index: 0,
            buf: Vec::with_capacity(CHUNK),
        })
    }

    fn seal(&mut self, last: bool) -> io::Result<()> {
        let n = nonce(&self.prefix, self.index, last);
        let ct = self
            .cipher
            .encrypt(
                XNonce::from_slice(&n),
                Payload {
                    msg: &self.buf,
                    aad: &self.header,
                },
            )
            .map_err(|_| io::Error::other("encryption failed"))?;
        let len = u32::try_from(ct.len()).map_err(|_| io::Error::other("chunk too large"))?;
        self.out.write_all(&[u8::from(last)])?;
        self.out.write_all(&len.to_le_bytes())?;
        self.out.write_all(&ct)?;
        self.buf.clear();
        self.index = self
            .index
            .checked_add(1)
            .ok_or_else(|| io::Error::other("the copy is too large"))?;
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        self.seal(true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for SealWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = (CHUNK - self.buf.len()).min(data.len());
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() == CHUNK {
            self.seal(false)?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Why a copy didn't open, in words for the admin.
fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

const WRONG_PASSWORD: &str = "That password doesn't open this standby copy.";
const DAMAGED: &str = "This standby copy is damaged or incomplete. Try an older copy.";
const NOT_A_COPY: &str = "This file isn't a SpareLine standby copy.";

struct OpenReader<R: Read> {
    inp: R,
    cipher: XChaCha20Poly1305,
    header: Vec<u8>,
    prefix: [u8; 19],
    index: u32,
    buf: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> OpenReader<R> {
    fn open(mut inp: R, password: &str) -> io::Result<Self> {
        let mut header = vec![0u8; HEADER_LEN];
        inp.read_exact(&mut header).map_err(|_| bad(NOT_A_COPY))?;
        if &header[..8] != MAGIC {
            return Err(bad(NOT_A_COPY));
        }
        let u32_at = |i: usize| {
            let mut b = [0u8; 4];
            b.copy_from_slice(&header[i..i + 4]);
            u32::from_le_bytes(b)
        };
        let (m, t, p) = (u32_at(8), u32_at(12), u32_at(16));
        if m > MAX_M_COST || t > MAX_T_COST || p > MAX_P_COST {
            return Err(bad(NOT_A_COPY));
        }
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&header[20..36]);
        let mut prefix = [0u8; 19];
        prefix.copy_from_slice(&header[36..55]);
        let key = derive(password, &salt, m, t, p).map_err(|_| bad(NOT_A_COPY))?;
        Ok(Self {
            inp,
            cipher: XChaCha20Poly1305::new((&key).into()),
            header,
            prefix,
            index: 0,
            buf: Vec::new(),
            pos: 0,
            done: false,
        })
    }

    /// Decrypt the next chunk. `false` once the final chunk has been read.
    fn next_chunk(&mut self) -> io::Result<bool> {
        if self.done {
            return Ok(false);
        }
        let mut head = [0u8; 5];
        self.inp.read_exact(&mut head).map_err(|_| bad(DAMAGED))?;
        let last = match head[0] {
            0 => false,
            1 => true,
            _ => return Err(bad(DAMAGED)),
        };
        let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
        if !(TAG..=CHUNK + TAG).contains(&len) {
            return Err(bad(DAMAGED));
        }
        let mut ct = vec![0u8; len];
        self.inp.read_exact(&mut ct).map_err(|_| bad(DAMAGED))?;
        let n = nonce(&self.prefix, self.index, last);
        self.buf = self
            .cipher
            .decrypt(
                XNonce::from_slice(&n),
                Payload {
                    msg: &ct,
                    aad: &self.header,
                },
            )
            .map_err(|_| {
                bad(if self.index == 0 {
                    WRONG_PASSWORD
                } else {
                    DAMAGED
                })
            })?;
        self.pos = 0;
        self.index = self.index.checked_add(1).ok_or_else(|| bad(DAMAGED))?;
        if last {
            self.done = true;
            let mut extra = [0u8; 1];
            if self.inp.read(&mut extra)? != 0 {
                return Err(bad(DAMAGED));
            }
        }
        Ok(true)
    }

    /// Read to the end, so every chunk has been checked.
    fn finish(mut self) -> io::Result<()> {
        if self.pos < self.buf.len() {
            return Err(bad(DAMAGED));
        }
        while self.next_chunk()? {
            if !self.buf.is_empty() {
                return Err(bad(DAMAGED));
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for OpenReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.buf.len() {
            if !self.next_chunk()? {
                return Ok(0);
            }
        }
        let n = (self.buf.len() - self.pos).min(out.len());
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn put_entry<W: Write>(w: &mut W, name: &str, len: u64, data: &mut impl Read) -> io::Result<()> {
    let n = u16::try_from(name.len()).map_err(|_| io::Error::other("name too long"))?;
    w.write_all(&n.to_le_bytes())?;
    w.write_all(name.as_bytes())?;
    w.write_all(&len.to_le_bytes())?;
    let copied = io::copy(&mut data.take(len), w)?;
    if copied != len {
        return Err(io::Error::other(format!("{name} changed while copying")));
    }
    Ok(())
}

fn put_bytes<W: Write>(w: &mut W, name: &str, data: &[u8]) -> io::Result<()> {
    put_entry(w, name, data.len() as u64, &mut &data[..])
}

/// A short read inside the stream means the copy was cut off; other errors (a wrong
/// password, a damaged chunk) already say what went wrong.
fn eof_damaged(e: io::Error) -> io::Error {
    if e.kind() == io::ErrorKind::UnexpectedEof {
        bad(DAMAGED)
    } else {
        e
    }
}

/// The next entry's name and length, or `None` after the last one.
fn next_entry<R: Read>(r: &mut R) -> io::Result<Option<(String, u64)>> {
    let mut b2 = [0u8; 2];
    r.read_exact(&mut b2).map_err(eof_damaged)?;
    let n = u16::from_le_bytes(b2) as usize;
    if n == 0 {
        return Ok(None);
    }
    let mut name = vec![0u8; n];
    r.read_exact(&mut name).map_err(eof_damaged)?;
    let name = String::from_utf8(name).map_err(|_| bad(DAMAGED))?;
    let mut b8 = [0u8; 8];
    r.read_exact(&mut b8).map_err(eof_damaged)?;
    Ok(Some((name, u64::from_le_bytes(b8))))
}

// ---------------------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Manifest {
    format: u32,
    server_id: Uuid,
    server_name: String,
    taken_at: i64,
    fingerprint: String,
    port: u16,
    people: u32,
    addresses: Vec<String>,
    app_version: String,
}

impl Manifest {
    fn info(&self) -> StandbyInfo {
        StandbyInfo {
            server_name: self.server_name.clone(),
            server_id: self.server_id,
            taken_at: self.taken_at,
            fingerprint: self.fingerprint.clone(),
            port: self.port,
            people: self.people,
            addresses: self.addresses.clone(),
            app_version: self.app_version.clone(),
        }
    }
}

fn read_manifest<R: Read>(r: &mut R) -> io::Result<Manifest> {
    let Some((name, len)) = next_entry(r)? else {
        return Err(bad(DAMAGED));
    };
    if name != E_MANIFEST || len > 64 * 1024 {
        return Err(bad(DAMAGED));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).map_err(eof_damaged)?;
    let m: Manifest = serde_json::from_slice(&buf).map_err(|_| bad(DAMAGED))?;
    if m.format != 1 {
        return Err(bad(
            "This standby copy was made by a newer SpareLine Server. Update this one first.",
        ));
    }
    Ok(m)
}

// ---------------------------------------------------------------------------------------
// File names
// ---------------------------------------------------------------------------------------

/// `20261114-104200` (UTC) for a Unix time in milliseconds.
fn utc_stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn short_id(server_id: Uuid) -> String {
    server_id.simple().to_string()[..6].to_string()
}

fn copy_name(taken_at: i64, server_id: Uuid) -> String {
    format!(
        "{PREFIX}{}-{}.{EXT}",
        utc_stamp(taken_at),
        short_id(server_id)
    )
}

/// Copies of this server in `folder`, newest first.
pub fn list_copies(folder: &Path, server_id: Uuid) -> Vec<StandbyCopyRow> {
    let suffix = format!("-{}.{EXT}", short_id(server_id));
    let mut out: Vec<StandbyCopyRow> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !(name.starts_with(PREFIX) && name.ends_with(&suffix)) {
                return None;
            }
            let meta = e.metadata().ok()?;
            let created_at = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Some(StandbyCopyRow {
                file_name: name,
                created_at,
                size: meta.len(),
            })
        })
        .collect();
    // Names sort by the time the copy was taken.
    out.sort_by(|a, b| b.file_name.cmp(&a.file_name));
    out
}

fn prune(folder: &Path, server_id: Uuid, keep: usize) {
    for old in list_copies(folder, server_id).iter().skip(keep) {
        let _ = std::fs::remove_file(folder.join(&old.file_name));
    }
    // Copies cut short (power loss, a drive pulled out) never got their final name.
    let partial = format!("-{}.partial", short_id(server_id));
    for e in std::fs::read_dir(folder).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with(PREFIX) && name.ends_with(&partial) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Is `a` on the same disk as `b`? Then a copy there wouldn't survive a disk failure.
pub fn same_disk(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        fn volume(p: &Path) -> Option<String> {
            let c = std::fs::canonicalize(p).ok()?;
            match c.components().next()? {
                std::path::Component::Prefix(pre) => {
                    Some(pre.as_os_str().to_string_lossy().to_uppercase())
                }
                _ => None,
            }
        }
        matches!((volume(a), volume(b)), (Some(x), Some(y)) if x == y)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (a, b);
        false
    }
}

// ---------------------------------------------------------------------------------------
// Taking copies
// ---------------------------------------------------------------------------------------

/// Progress and the last outcome, for the panel and the health check.
#[derive(Default)]
pub(crate) struct State {
    last_ok: Option<StandbyCopyRow>,
    last_error: Option<String>,
    last_attempt: i64,
    /// An admin change since the last copy. Copy again soon, so removals and new
    /// accounts reach the standby quickly.
    dirty: bool,
    busy: bool,
}

impl State {
    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// An error as a phrase for the admin ("the standby folder isn't available …").
fn describe(e: &AppError) -> String {
    match e {
        AppError::Api(a) if !a.detail.is_empty() => a.detail.clone(),
        AppError::Api(a) => a.code.user_message().to_string(),
        AppError::Internal(e) => format!("{e:#}"),
    }
}

fn sentence(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn folder_of(cfg: &Config) -> Option<PathBuf> {
    let f = cfg.standby_folder.trim();
    (!f.is_empty()).then(|| PathBuf::from(f))
}

fn unavailable(folder: &Path) -> String {
    format!(
        "the standby folder isn't available ({}). Check that the drive is connected or the shared folder can be reached",
        folder.display()
    )
}

/// Make a standby copy now.
pub async fn take_copy(engine: &Engine) -> AppResult<StandbyCopyRow> {
    {
        let mut st = engine.standby_state();
        if st.busy {
            return api_detail(
                ErrorCode::BadRequest,
                "a standby copy is already being made",
            );
        }
        st.busy = true;
        st.dirty = false;
        st.last_attempt = now();
    }
    let res = take_copy_inner(engine).await;
    let mut st = engine.standby_state();
    st.busy = false;
    match &res {
        Ok(row) => {
            st.last_ok = Some(row.clone());
            st.last_error = None;
        }
        Err(e) => {
            let msg = describe(e);
            tracing::warn!(error = %msg, "standby copy failed");
            st.last_error = Some(msg);
        }
    }
    res
}

async fn take_copy_inner(engine: &Engine) -> AppResult<StandbyCopyRow> {
    let cfg = engine.config();
    let Some(folder) = folder_of(&cfg) else {
        return api_detail(ErrorCode::BadRequest, "standby copies are off");
    };
    let Some(key) = load_key(engine.paths()) else {
        return api_detail(ErrorCode::BadRequest, "set a standby password first");
    };
    if !folder.is_dir() {
        return api_detail(ErrorCode::BadRequest, unavailable(&folder));
    }
    let tmp = engine.paths().standby_tmp.clone();
    std::fs::create_dir_all(&tmp)?;
    let snap = tmp.join("snapshot.db");
    let _ = std::fs::remove_file(&snap);
    engine.db().backup_to(snap.clone()).await?;

    let people: u32 = engine
        .db()
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, i64>(0))? as u32))
        .await?;
    let tls = engine.tls();
    let manifest = Manifest {
        format: 1,
        server_id: cfg.server_id,
        server_name: cfg.server_name.clone(),
        taken_at: now(),
        fingerprint: tls.fingerprint.clone(),
        port: cfg.port,
        people,
        addresses: lan_addresses(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let config_toml = toml::to_string_pretty(&cfg).map_err(anyhow::Error::from)?;
    let standby_key = std::fs::read(&engine.paths().standby_key).ok();
    let name = copy_name(manifest.taken_at, cfg.server_id);
    let taken_at = manifest.taken_at;
    let keep = cfg.standby_keep.max(1) as usize;
    let dest = folder.join(&name);
    let cert = tls.cert_der.clone();
    let tls_key = tls.key_der().to_vec();
    let server_id = cfg.server_id;
    let size = tokio::task::spawn_blocking(move || -> io::Result<u64> {
        let partial = dest.with_extension("partial");
        let written = (|| -> io::Result<()> {
            let file = File::create(&partial)?;
            let mut w = SealWriter::new(BufWriter::new(file), &key)?;
            put_bytes(&mut w, E_MANIFEST, &serde_json::to_vec(&manifest)?)?;
            put_bytes(&mut w, E_CONFIG, config_toml.as_bytes())?;
            put_bytes(&mut w, E_CERT, &cert)?;
            put_bytes(&mut w, E_KEY, &tls_key)?;
            if let Some(k) = &standby_key {
                put_bytes(&mut w, E_STANDBY_KEY, k)?;
            }
            let db = File::open(&snap)?;
            let len = db.metadata()?.len();
            put_entry(&mut w, E_DB, len, &mut BufReader::new(db))?;
            w.write_all(&0u16.to_le_bytes())?;
            let file = w.finish()?.into_inner().map_err(|e| e.into_error())?;
            file.sync_all()?;
            Ok(())
        })();
        let _ = std::fs::remove_file(&snap);
        if let Err(e) = written {
            let _ = std::fs::remove_file(&partial);
            return Err(e);
        }
        std::fs::rename(&partial, &dest)?;
        prune(dest.parent().unwrap_or(Path::new(".")), server_id, keep);
        Ok(std::fs::metadata(&dest)?.len())
    })
    .await?
    .map_err(|e| {
        AppError::from(sl_proto::ApiError::with_detail(
            ErrorCode::BadRequest,
            format!("couldn't write the standby copy: {e}"),
        ))
    })?;
    tracing::info!(file = %name, size, "standby copy written");
    Ok(StandbyCopyRow {
        file_name: name,
        created_at: taken_at,
        size,
    })
}

fn due(engine: &Engine) -> bool {
    let cfg = engine.config();
    if folder_of(&cfg).is_none() || !engine.paths().standby_key.exists() {
        return false;
    }
    let st = engine.standby_state();
    if st.busy {
        return false;
    }
    let since = now() - st.last_attempt;
    let interval = i64::from(cfg.standby_interval_minutes.max(5)) * 60_000;
    since >= interval || (st.dirty && since >= 60_000)
}

/// Background job: copy on schedule, and soon after admin changes.
pub(crate) fn spawn(engine: Engine) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            if due(&engine) {
                let _ = take_copy(&engine).await;
            }
        }
    })
}

pub(crate) fn status(engine: &Engine) -> StandbyStatus {
    let cfg = engine.config();
    let folder = folder_of(&cfg);
    let copies = folder
        .as_deref()
        .map(|f| list_copies(f, cfg.server_id))
        .unwrap_or_default();
    let st = engine.standby_state();
    StandbyStatus {
        enabled: folder.is_some(),
        folder: cfg.standby_folder.trim().to_string(),
        interval_minutes: cfg.standby_interval_minutes,
        keep: cfg.standby_keep,
        has_password: has_password(engine.paths()),
        last_copy: st.last_ok.clone().or_else(|| copies.first().cloned()),
        last_error: st.last_error.as_deref().map(sentence),
        same_disk: folder
            .as_deref()
            .is_some_and(|f| same_disk(f, &engine.paths().root)),
        copies,
    }
}

pub(crate) fn health(engine: &Engine) -> HealthCheck {
    let s = status(engine);
    let (status, detail) = if !s.enabled {
        (
            CheckStatus::Unknown,
            "Off. Turn on standby copies in Settings so another computer can take over if this one fails.".to_string(),
        )
    } else if let Some(e) = engine.standby_state().last_error.clone() {
        (CheckStatus::Fail, format!("The last copy failed: {e}"))
    } else {
        match &s.last_copy {
            None => (
                CheckStatus::Unknown,
                "The first copy will be made shortly".into(),
            ),
            Some(c) => {
                let age = now() - c.created_at;
                let interval = i64::from(s.interval_minutes.max(5)) * 60_000;
                if age > 2 * interval + 60_000 {
                    (
                        CheckStatus::Warn,
                        format!(
                            "The last copy is {} old. Copies should be made every {} minutes.",
                            human_age(age),
                            s.interval_minutes
                        ),
                    )
                } else if s.same_disk {
                    (
                        CheckStatus::Warn,
                        "Copies are on the same disk as this server, so they wouldn't survive a disk failure. Choose a USB drive or a shared folder on another computer.".into(),
                    )
                } else {
                    (
                        CheckStatus::Ok,
                        format!("Last copy {} ago in {}", human_age(age), s.folder),
                    )
                }
            }
        }
    };
    HealthCheck {
        id: "standby".into(),
        name: "Standby copy".into(),
        status,
        detail,
        fix: None,
    }
}

// ---------------------------------------------------------------------------------------
// Restoring
// ---------------------------------------------------------------------------------------

/// Open a copy and describe it, checking the password. Doesn't change anything.
pub fn inspect(path: &Path, password: &str) -> AppResult<StandbyInfo> {
    let file = File::open(path).map_err(|e| user_err(format!("couldn't open that file: {e}")))?;
    let mut r = OpenReader::open(BufReader::new(file), password).map_err(io_user_err)?;
    let m = read_manifest(&mut r).map_err(io_user_err)?;
    Ok(m.info())
}

/// Decrypt a copy into `dest` (emptied first) and check every byte of it.
fn extract(path: &Path, password: &str, dest: &Path) -> AppResult<Manifest> {
    let file = File::open(path).map_err(|e| user_err(format!("couldn't open that file: {e}")))?;
    let mut r = OpenReader::open(BufReader::new(file), password).map_err(io_user_err)?;
    let manifest = read_manifest(&mut r).map_err(io_user_err)?;
    let _ = std::fs::remove_dir_all(dest);
    std::fs::create_dir_all(dest.join("tls"))?;
    let mut seen: Vec<String> = vec![E_MANIFEST.into()];
    while let Some((name, len)) = next_entry(&mut r).map_err(io_user_err)? {
        if !ENTRIES.contains(&name.as_str()) || seen.contains(&name) {
            return Err(io_user_err(bad(DAMAGED)));
        }
        let mut out = BufWriter::new(File::create(dest.join(&name))?);
        let copied = io::copy(&mut (&mut r).take(len), &mut out).map_err(io_user_err)?;
        if copied != len {
            return Err(io_user_err(bad(DAMAGED)));
        }
        out.flush()?;
        seen.push(name);
    }
    r.finish().map_err(io_user_err)?;
    if REQUIRED.iter().any(|n| !seen.iter().any(|s| s == n)) {
        return Err(io_user_err(bad(DAMAGED)));
    }
    Ok(manifest)
}

fn user_err(msg: String) -> AppError {
    AppError::from(sl_proto::ApiError::with_detail(ErrorCode::BadRequest, msg))
}

fn io_user_err(e: io::Error) -> AppError {
    if e.kind() == io::ErrorKind::InvalidData {
        user_err(e.to_string())
    } else {
        AppError::from(e)
    }
}

/// Take over from a failed server: restore its standby copy into this engine, which
/// must be freshly installed and not yet serving.
pub async fn restore(engine: &Engine, path: &Path, password: &str) -> AppResult<StandbyInfo> {
    if !engine.needs_setup() {
        return api_detail(
            ErrorCode::AlreadyExists,
            "this server is already set up. Restore a standby copy on a newly installed SpareLine Server, or into an empty data folder",
        );
    }
    if engine.local_addr().is_some() {
        return api_detail(ErrorCode::BadRequest, "stop the server before restoring");
    }
    let paths = engine.paths().clone();
    let work = paths.standby_tmp.join("restore");
    let (src, pw, dst) = (path.to_path_buf(), password.to_string(), work.clone());
    let manifest = tokio::task::spawn_blocking(move || extract(&src, &pw, &dst)).await??;

    let result = async {
        let text = std::fs::read_to_string(work.join(E_CONFIG))?;
        let config: Config = toml::from_str(&text)
            .map_err(|e| user_err(format!("the copy's settings couldn't be read: {e}")))?;
        if config.server_id != manifest.server_id {
            return Err(io_user_err(bad(DAMAGED)));
        }
        let cert = std::fs::read(work.join(E_CERT))?;
        let key = std::fs::read(work.join(E_KEY))?;
        let tls = TlsIdentity::install(&paths, cert, key)
            .map_err(|e| user_err(format!("the copy's certificate couldn't be used: {e}")))?;
        let db = engine.db().clone();
        let db_file = work.join(E_DB);
        tokio::task::spawn_blocking(move || db.replace_with(&db_file)).await??;
        let marker = RestoreMarker {
            id: Uuid::now_v7(),
            copy_taken_at: manifest.taken_at,
        };
        let json = serde_json::to_string(&marker)?;
        engine
            .db()
            .write(move |c| {
                let tx = c.transaction()?;
                tx.execute(
                    "UPDATE conversations SET restore_seq = last_seq, restore_epoch = epoch",
                    [],
                )?;
                tx.execute(
                    "INSERT OR REPLACE INTO server_meta (key, value) VALUES ('restore', ?1)",
                    [json],
                )?;
                tx.commit()?;
                Ok(())
            })
            .await?;
        if let Ok(k) = std::fs::read(work.join(E_STANDBY_KEY)) {
            write_private(&paths.standby_key, &k)?;
        }
        engine.adopt_restored(config, tls, marker)?;
        engine.reload_directory().await?;
        Ok::<_, AppError>(())
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result?;
    engine.audit(
        "standby.restore",
        &manifest.server_name,
        format!("copy taken {} UTC", utc_stamp(manifest.taken_at)),
    );
    tracing::info!(server = %manifest.server_name, taken_at = manifest.taken_at, "restored from a standby copy");
    Ok(manifest.info())
}

/// The restore marker saved by [`restore`], if any.
pub(crate) fn load_marker(c: &rusqlite::Connection) -> Option<RestoreMarker> {
    let json: String = c
        .query_row(
            "SELECT value FROM server_meta WHERE key = 'restore'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    serde_json::from_str(&json).ok()
}

/// "2026-11-14 10:42 UTC", for messages.
pub fn utc_display(ms: i64) -> String {
    let s = utc_stamp(ms);
    format!(
        "{}-{}-{} {}:{} UTC",
        &s[0..4],
        &s[4..6],
        &s[6..8],
        &s[9..11],
        &s[11..13]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_key() -> StandbyKey {
        let salt = [7u8; 16];
        StandbyKey {
            m: 8,
            t: 1,
            p: 1,
            salt,
            key: derive("standby password", &salt, 8, 1, 1).expect("derive"),
        }
    }

    fn seal(data: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut w = SealWriter::new(Vec::new(), &test_key()).expect("writer");
        for (name, bytes) in data {
            put_bytes(&mut w, name, bytes).expect("entry");
        }
        w.write_all(&0u16.to_le_bytes()).expect("end");
        w.finish().expect("finish")
    }

    fn read_all(file: &[u8], password: &str) -> io::Result<Vec<(String, Vec<u8>)>> {
        let mut r = OpenReader::open(file, password)?;
        let mut out = Vec::new();
        while let Some((name, len)) = next_entry(&mut r)? {
            let mut b = vec![0u8; len as usize];
            r.read_exact(&mut b)?;
            out.push((name, b));
        }
        r.finish()?;
        Ok(out)
    }

    #[test]
    fn round_trip_across_chunks() {
        let big: Vec<u8> = (0..(CHUNK * 2 + 123)).map(|i| (i % 251) as u8).collect();
        let file = seal(&[("a", b"hello".to_vec()), ("b", big.clone())]);
        let back = read_all(&file, "standby password").expect("opens");
        assert_eq!(back[0], ("a".to_string(), b"hello".to_vec()));
        assert_eq!(back[1].1, big);
    }

    #[test]
    fn wrong_password_is_reported() {
        let file = seal(&[("a", b"x".to_vec())]);
        let e = read_all(&file, "not the password").expect_err("must fail");
        assert_eq!(e.to_string(), WRONG_PASSWORD);
    }

    #[test]
    fn truncation_and_tampering_are_detected() {
        let big = vec![1u8; CHUNK + 10];
        let file = seal(&[("a", big)]);
        // Drop the final chunk.
        let cut = &file[..HEADER_LEN + 5 + CHUNK + TAG];
        assert!(read_all(cut, "standby password").is_err());
        // Flip a byte in the second chunk.
        let mut bad_file = file.clone();
        let i = file.len() - 3;
        bad_file[i] ^= 1;
        assert!(read_all(&bad_file, "standby password").is_err());
        // Trailing bytes after the final chunk.
        let mut extra = file.clone();
        extra.push(0);
        assert!(read_all(&extra, "standby password").is_err());
    }

    #[test]
    fn utc_stamps() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(1_794_652_920_000), "20261114-104200");
        assert_eq!(utc_display(1_794_652_920_000), "2026-11-14 10:42 UTC");
    }
}
