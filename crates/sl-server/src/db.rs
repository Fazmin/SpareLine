//! SQLite access: one writer connection plus a small pool of readers (WAL mode).
//! All calls run on Tokio's blocking pool.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use crate::error::{AppError, AppResult};

const READERS: usize = 3;

#[derive(Clone)]
pub struct Db {
    inner: Arc<DbInner>,
}

struct DbInner {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

fn migrations() -> Migrations<'static> {
    Migrations::new(vec![
        M::up(include_str!("schema/001_init.sql")),
        M::up(include_str!("schema/002_standby.sql")),
    ])
}

fn open_conn(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(conn)
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mut writer = open_conn(path)?;
        migrations().to_latest(&mut writer)?;
        let mut readers = Vec::with_capacity(READERS);
        for _ in 0..READERS {
            readers.push(Mutex::new(open_conn(path)?));
        }
        Ok(Self {
            inner: Arc::new(DbInner {
                path: path.to_path_buf(),
                writer: Mutex::new(writer),
                readers,
                next: AtomicUsize::new(0),
            }),
        })
    }

    /// Run `f` on the writer connection.
    pub async fn write<T, F>(&self, f: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> AppResult<T> + Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.writer.lock();
            f(&mut conn)
        })
        .await
        .map_err(AppError::from)?
    }

    /// Run `f` on a reader connection.
    pub async fn read<T, F>(&self, f: F) -> AppResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> AppResult<T> + Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let i = inner.next.fetch_add(1, Ordering::Relaxed) % inner.readers.len();
            let conn = inner.readers[i].lock();
            f(&conn)
        })
        .await
        .map_err(AppError::from)?
    }

    /// Synchronous access for startup code.
    pub fn write_sync<T>(&self, f: impl FnOnce(&mut Connection) -> AppResult<T>) -> AppResult<T> {
        let mut conn = self.inner.writer.lock();
        f(&mut conn)
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Database file size including WAL.
    pub fn size_bytes(&self) -> u64 {
        let main = std::fs::metadata(&self.inner.path)
            .map(|m| m.len())
            .unwrap_or(0);
        let wal = std::fs::metadata(self.inner.path.with_extension("db-wal"))
            .map(|m| m.len())
            .unwrap_or(0);
        main + wal
    }

    pub fn wal_size_bytes(&self) -> u64 {
        std::fs::metadata(self.inner.path.with_extension("db-wal"))
            .map(|m| m.len())
            .unwrap_or(0)
    }

    /// Replace the whole database with `src` (a standby copy), in place, so open
    /// connections stay valid. Brings the copy's schema up to date afterwards.
    pub fn replace_with(&self, src: &Path) -> AppResult<()> {
        let from = Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut to = self.inner.writer.lock();
        {
            let b = rusqlite::backup::Backup::new(&from, &mut to)?;
            b.run_to_completion(512, std::time::Duration::ZERO, None)?;
        }
        migrations()
            .to_latest(&mut to)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("database upgrade: {e}")))?;
        Ok(())
    }

    /// Consistent online backup via `VACUUM INTO`.
    pub async fn backup_to(&self, dest: PathBuf) -> AppResult<()> {
        self.write(move |c| {
            let dest_str = dest.to_string_lossy().to_string();
            c.execute("VACUUM INTO ?1", [dest_str])?;
            Ok(())
        })
        .await
    }
}

/// Small helpers for optional single-row queries.
pub trait OptionalExt<T> {
    fn optional(self) -> rusqlite::Result<Option<T>>;
}

impl<T> OptionalExt<T> for rusqlite::Result<T> {
    fn optional(self) -> rusqlite::Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

pub fn now() -> i64 {
    sl_proto::now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid() {
        assert!(migrations().validate().is_ok());
    }
}
