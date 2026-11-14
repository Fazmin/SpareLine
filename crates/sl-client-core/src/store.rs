//! Local chat history (`chat.db`). Message bodies and chat titles are encrypted at rest
//! with XChaCha20-Poly1305 under a key kept in the OS keychain.

use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};
use sl_proto::client_ipc::{MessageKind, SendStatus};
use sl_proto::{
    ConversationId, ConversationInfo, ConversationKind, DeviceRef, DirectoryUser, GroupId, UserId,
};
use uuid::Uuid;

use crate::error::ClientResult;
use crate::payload::Payload;

const SCHEMA: &str = r#"
CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE conversations (
    id BLOB PRIMARY KEY,
    kind TEXT NOT NULL,
    scope_group BLOB,
    mls_group_id BLOB NOT NULL,
    title BLOB,
    members TEXT NOT NULL DEFAULT '[]',
    devices TEXT NOT NULL DEFAULT '[]',
    read_only INTEGER NOT NULL DEFAULT 0,
    removed INTEGER NOT NULL DEFAULT 0,
    wm INTEGER NOT NULL DEFAULT 0,
    read_seq INTEGER NOT NULL DEFAULT 0,
    last_seq INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX conversations_gid ON conversations(mls_group_id);
CREATE TABLE messages (
    id BLOB PRIMARY KEY,
    conversation BLOB NOT NULL,
    seq INTEGER,
    sender BLOB,
    kind TEXT NOT NULL,
    body BLOB NOT NULL,
    reply_to BLOB,
    ts INTEGER NOT NULL,
    edited INTEGER NOT NULL DEFAULT 0,
    deleted INTEGER NOT NULL DEFAULT 0,
    status TEXT NOT NULL,
    mine INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX messages_conv_ts ON messages(conversation, ts);
CREATE TABLE reactions (
    message BLOB NOT NULL,
    user_id BLOB NOT NULL,
    emoji TEXT NOT NULL,
    PRIMARY KEY (message, user_id, emoji)
);
CREATE TABLE outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation BLOB NOT NULL,
    payload BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE read_markers (
    conversation BLOB NOT NULL,
    user_id BLOB NOT NULL,
    seq INTEGER NOT NULL,
    PRIMARY KEY (conversation, user_id)
);
CREATE TABLE users (
    id BLOB PRIMARY KEY,
    username TEXT NOT NULL,
    display_name TEXT NOT NULL
);
"#;

#[derive(Debug, Clone)]
pub struct LocalConv {
    pub id: ConversationId,
    pub kind: ConversationKind,
    pub scope_group: Option<GroupId>,
    pub mls_group_id: Vec<u8>,
    pub title: Option<String>,
    pub members: Vec<UserId>,
    pub devices: Vec<DeviceRef>,
    pub read_only: bool,
    pub removed: bool,
    /// Highest contiguous envelope seq processed.
    pub wm: u64,
    pub read_seq: u64,
    pub last_seq: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct LocalMessage {
    pub id: Uuid,
    pub conversation: ConversationId,
    pub seq: Option<u64>,
    pub sender: Option<UserId>,
    pub kind: MessageKind,
    pub body: String,
    pub reply_to: Option<Uuid>,
    pub ts: i64,
    pub edited: bool,
    pub deleted: bool,
    pub status: SendStatus,
    pub mine: bool,
}

pub struct OutboxRow {
    pub id: i64,
    pub conversation: ConversationId,
    pub payload: Payload,
    pub attempts: u32,
}

pub struct Store {
    conn: Connection,
    cipher: XChaCha20Poly1305,
}

fn status_str(s: SendStatus) -> &'static str {
    match s {
        SendStatus::Pending => "pending",
        SendStatus::Sent => "sent",
        SendStatus::Read => "read",
        SendStatus::Failed => "failed",
    }
}

fn status(s: &str) -> SendStatus {
    match s {
        "pending" => SendStatus::Pending,
        "read" => SendStatus::Read,
        "failed" => SendStatus::Failed,
        _ => SendStatus::Sent,
    }
}

impl Store {
    pub fn open(dir: &Path, key: &[u8; 32]) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let mut conn = Connection::open(dir.join("chat.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Migrations::new(vec![M::up(SCHEMA)]).to_latest(&mut conn)?;
        let cipher = XChaCha20Poly1305::new(key.into());
        Ok(Self { conn, cipher })
    }

    pub fn wipe(dir: &Path) {
        for f in ["chat.db", "chat.db-wal", "chat.db-shm"] {
            let _ = std::fs::remove_file(dir.join(f));
        }
    }

    fn seal(&self, plain: &str) -> Vec<u8> {
        let nonce: [u8; 24] = sl_proto::crypto::random_bytes();
        let mut out = nonce.to_vec();
        match self
            .cipher
            .encrypt(XNonce::from_slice(&nonce), plain.as_bytes())
        {
            Ok(ct) => out.extend(ct),
            Err(_) => return Vec::new(),
        }
        out
    }

    fn open_str(&self, sealed: &[u8]) -> String {
        if sealed.len() < 24 {
            return String::new();
        }
        let (nonce, ct) = sealed.split_at(24);
        self.cipher
            .decrypt(XNonce::from_slice(nonce), ct)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
            .unwrap_or_default()
    }

    // ---- kv ----------------------------------------------------------------------------

    pub fn kv_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM kv WHERE key = ?1", [key], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
    }

    pub fn kv_set(&self, key: &str, value: &str) -> ClientResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO kv (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    // ---- conversations -----------------------------------------------------------------

    pub fn upsert_conversation(&self, info: &ConversationInfo) -> ClientResult<()> {
        let now = sl_proto::now_ms();
        let kind = match info.kind {
            ConversationKind::Direct => "direct",
            ConversationKind::Group => "group",
        };
        self.conn.execute(
            "INSERT INTO conversations (id, kind, scope_group, mls_group_id, members, devices, read_only, removed, last_seq, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET members = excluded.members, devices = excluded.devices,
                read_only = excluded.read_only, removed = 0, last_seq = MAX(last_seq, excluded.last_seq),
                scope_group = excluded.scope_group",
            params![
                info.id,
                kind,
                info.scope_group,
                info.mls_group_id.as_slice(),
                serde_json::to_string(&info.members)?,
                serde_json::to_string(&info.devices)?,
                info.read_only as i64,
                info.last_seq as i64,
                info.created_at,
                now,
            ],
        )?;
        Ok(())
    }

    fn conv_from_row(&self, r: &rusqlite::Row<'_>) -> rusqlite::Result<LocalConv> {
        let title: Option<Vec<u8>> = r.get(4)?;
        Ok(LocalConv {
            id: r.get(0)?,
            kind: if r.get::<_, String>(1)? == "group" {
                ConversationKind::Group
            } else {
                ConversationKind::Direct
            },
            scope_group: r.get(2)?,
            mls_group_id: r.get(3)?,
            title: title.map(|t| self.open_str(&t)).filter(|t| !t.is_empty()),
            members: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
            devices: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
            read_only: r.get::<_, i64>(7)? != 0,
            removed: r.get::<_, i64>(8)? != 0,
            wm: r.get::<_, i64>(9)? as u64,
            read_seq: r.get::<_, i64>(10)? as u64,
            last_seq: r.get::<_, i64>(11)? as u64,
            created_at: r.get(12)?,
            updated_at: r.get(13)?,
        })
    }

    const CONV_COLS: &'static str = "id, kind, scope_group, mls_group_id, title, members, devices, read_only, removed, wm, read_seq, last_seq, created_at, updated_at";

    pub fn conversation(&self, id: ConversationId) -> Option<LocalConv> {
        let sql = format!(
            "SELECT {} FROM conversations WHERE id = ?1",
            Self::CONV_COLS
        );
        self.conn
            .query_row(&sql, [id], |r| self.conv_from_row(r))
            .optional()
            .ok()
            .flatten()
    }

    pub fn conversations(&self) -> Vec<LocalConv> {
        let sql = format!(
            "SELECT {} FROM conversations ORDER BY updated_at DESC",
            Self::CONV_COLS
        );
        let Ok(mut st) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        st.query_map([], |r| self.conv_from_row(r))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn set_wm(&self, id: ConversationId, wm: u64) -> ClientResult<()> {
        self.conn.execute(
            "UPDATE conversations SET wm = ?1, last_seq = MAX(last_seq, ?1) WHERE id = ?2",
            params![wm as i64, id],
        )?;
        Ok(())
    }

    /// Move a conversation back to `seq` after the server was restored from a copy.
    pub fn rewind(&self, id: ConversationId, seq: u64) -> ClientResult<()> {
        self.conn.execute(
            "UPDATE conversations SET wm = MIN(wm, ?1), read_seq = MIN(read_seq, ?1), last_seq = ?1 WHERE id = ?2",
            params![seq as i64, id],
        )?;
        Ok(())
    }

    pub fn set_read_seq(&self, id: ConversationId, seq: u64) -> ClientResult<()> {
        self.conn.execute(
            "UPDATE conversations SET read_seq = MAX(read_seq, ?1) WHERE id = ?2",
            params![seq as i64, id],
        )?;
        Ok(())
    }

    pub fn set_title(&self, id: ConversationId, title: &str) -> ClientResult<()> {
        self.conn.execute(
            "UPDATE conversations SET title = ?1 WHERE id = ?2",
            params![self.seal(title), id],
        )?;
        Ok(())
    }

    pub fn set_removed(&self, id: ConversationId, removed: bool) -> ClientResult<()> {
        self.conn.execute(
            "UPDATE conversations SET removed = ?1 WHERE id = ?2",
            params![removed as i64, id],
        )?;
        Ok(())
    }

    pub fn touch(&self, id: ConversationId, ts: i64) -> ClientResult<()> {
        self.conn.execute(
            "UPDATE conversations SET updated_at = MAX(updated_at, ?1) WHERE id = ?2",
            params![ts, id],
        )?;
        Ok(())
    }

    // ---- messages ----------------------------------------------------------------------

    pub fn insert_message(&self, m: &LocalMessage) -> ClientResult<bool> {
        let kind = match m.kind {
            MessageKind::Text => "text",
            MessageKind::System => "system",
        };
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO messages (id, conversation, seq, sender, kind, body, reply_to, ts, edited, deleted, status, mine)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                m.id,
                m.conversation,
                m.seq.map(|s| s as i64),
                m.sender,
                kind,
                self.seal(&m.body),
                m.reply_to,
                m.ts,
                m.edited as i64,
                m.deleted as i64,
                status_str(m.status),
                m.mine as i64,
            ],
        )?;
        self.touch(m.conversation, m.ts)?;
        Ok(n > 0)
    }

    fn msg_from_row(&self, r: &rusqlite::Row<'_>) -> rusqlite::Result<LocalMessage> {
        let body: Vec<u8> = r.get(5)?;
        Ok(LocalMessage {
            id: r.get(0)?,
            conversation: r.get(1)?,
            seq: r.get::<_, Option<i64>>(2)?.map(|s| s as u64),
            sender: r.get(3)?,
            kind: if r.get::<_, String>(4)? == "system" {
                MessageKind::System
            } else {
                MessageKind::Text
            },
            body: self.open_str(&body),
            reply_to: r.get(6)?,
            ts: r.get(7)?,
            edited: r.get::<_, i64>(8)? != 0,
            deleted: r.get::<_, i64>(9)? != 0,
            status: status(&r.get::<_, String>(10)?),
            mine: r.get::<_, i64>(11)? != 0,
        })
    }

    const MSG_COLS: &'static str =
        "id, conversation, seq, sender, kind, body, reply_to, ts, edited, deleted, status, mine";

    pub fn message(&self, id: Uuid) -> Option<LocalMessage> {
        let sql = format!("SELECT {} FROM messages WHERE id = ?1", Self::MSG_COLS);
        self.conn
            .query_row(&sql, [id], |r| self.msg_from_row(r))
            .optional()
            .ok()
            .flatten()
    }

    /// Newest-last page of messages strictly older than `before` (or the latest page).
    pub fn messages(
        &self,
        conv: ConversationId,
        before: Option<i64>,
        limit: u32,
    ) -> Vec<LocalMessage> {
        let sql = format!(
            "SELECT {} FROM messages WHERE conversation = ?1 AND ts < ?2 ORDER BY ts DESC, seq DESC LIMIT ?3",
            Self::MSG_COLS
        );
        let Ok(mut st) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        let mut out: Vec<LocalMessage> = st
            .query_map(params![conv, before.unwrap_or(i64::MAX), limit], |r| {
                self.msg_from_row(r)
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();
        out.reverse();
        out
    }

    pub fn last_message(&self, conv: ConversationId) -> Option<LocalMessage> {
        self.messages(conv, None, 1).pop()
    }

    pub fn update_status(
        &self,
        id: Uuid,
        st: SendStatus,
        seq: Option<u64>,
        ts: Option<i64>,
    ) -> ClientResult<()> {
        self.conn.execute(
            "UPDATE messages SET status = ?1, seq = COALESCE(?2, seq), ts = COALESCE(?3, ts) WHERE id = ?4",
            params![status_str(st), seq.map(|s| s as i64), ts, id],
        )?;
        Ok(())
    }

    /// Apply an edit if `sender` wrote the message. Returns true if applied.
    pub fn apply_edit(&self, target: Uuid, sender: UserId, body: &str) -> ClientResult<bool> {
        let n = self.conn.execute(
            "UPDATE messages SET body = ?1, edited = 1 WHERE id = ?2 AND sender = ?3 AND deleted = 0 AND kind = 'text'",
            params![self.seal(body), target, sender],
        )?;
        Ok(n > 0)
    }

    pub fn apply_delete(&self, target: Uuid, sender: UserId) -> ClientResult<bool> {
        let n = self.conn.execute(
            "UPDATE messages SET body = ?1, deleted = 1 WHERE id = ?2 AND sender = ?3 AND kind = 'text'",
            params![self.seal(""), target, sender],
        )?;
        if n > 0 {
            self.conn
                .execute("DELETE FROM reactions WHERE message = ?1", [target])?;
        }
        Ok(n > 0)
    }

    pub fn apply_reaction(
        &self,
        target: Uuid,
        user: UserId,
        emoji: &str,
        add: bool,
    ) -> ClientResult<bool> {
        let exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM messages WHERE id = ?1 AND deleted = 0",
                [target],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if !exists {
            return Ok(false);
        }
        if add {
            self.conn.execute(
                "INSERT OR IGNORE INTO reactions (message, user_id, emoji) VALUES (?1, ?2, ?3)",
                params![target, user, emoji],
            )?;
        } else {
            self.conn.execute(
                "DELETE FROM reactions WHERE message = ?1 AND user_id = ?2 AND emoji = ?3",
                params![target, user, emoji],
            )?;
        }
        Ok(true)
    }

    pub fn reactions(&self, message: Uuid) -> Vec<(String, UserId)> {
        let Ok(mut st) = self
            .conn
            .prepare("SELECT emoji, user_id FROM reactions WHERE message = ?1 ORDER BY rowid")
        else {
            return Vec::new();
        };
        st.query_map([message], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    pub fn has_reaction(&self, message: Uuid, user: UserId, emoji: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM reactions WHERE message = ?1 AND user_id = ?2 AND emoji = ?3",
                params![message, user, emoji],
                |_| Ok(true),
            )
            .optional()
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    pub fn unread_count(&self, conv: ConversationId, read_seq: u64) -> u32 {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE conversation = ?1 AND mine = 0 AND kind = 'text' AND deleted = 0 AND seq > ?2",
                params![conv, read_seq as i64],
                |r| r.get(0),
            )
            .unwrap_or(0)
    }

    /// Case-insensitive search over recent messages (bodies are encrypted, so we scan).
    pub fn search(&self, query: &str, limit: usize) -> Vec<LocalMessage> {
        let q = query.to_lowercase();
        let sql = format!(
            "SELECT {} FROM messages WHERE kind = 'text' AND deleted = 0 ORDER BY ts DESC LIMIT 20000",
            Self::MSG_COLS
        );
        let Ok(mut st) = self.conn.prepare(&sql) else {
            return Vec::new();
        };
        let Ok(rows) = st.query_map([], |r| self.msg_from_row(r)) else {
            return Vec::new();
        };
        rows.flatten()
            .filter(|m| m.body.to_lowercase().contains(&q))
            .take(limit)
            .collect()
    }

    // ---- outbox ------------------------------------------------------------------------

    pub fn outbox_push(&self, conv: ConversationId, p: &Payload) -> ClientResult<()> {
        let text = serde_json::to_string(p)?;
        self.conn.execute(
            "INSERT INTO outbox (conversation, payload, created_at) VALUES (?1, ?2, ?3)",
            params![conv, self.seal(&text), sl_proto::now_ms()],
        )?;
        Ok(())
    }

    pub fn outbox(&self) -> Vec<OutboxRow> {
        let Ok(mut st) = self
            .conn
            .prepare("SELECT id, conversation, payload, attempts FROM outbox ORDER BY id")
        else {
            return Vec::new();
        };
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, ConversationId>(1)?,
                r.get::<_, Vec<u8>>(2)?,
                r.get::<_, u32>(3)?,
            ))
        });
        let Ok(rows) = rows else { return Vec::new() };
        rows.flatten()
            .filter_map(|(id, conversation, sealed, attempts)| {
                let payload = serde_json::from_str(&self.open_str(&sealed)).ok()?;
                Some(OutboxRow {
                    id,
                    conversation,
                    payload,
                    attempts,
                })
            })
            .collect()
    }

    pub fn outbox_done(&self, id: i64) -> ClientResult<()> {
        self.conn
            .execute("DELETE FROM outbox WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn outbox_attempt(&self, id: i64) -> ClientResult<()> {
        self.conn.execute(
            "UPDATE outbox SET attempts = attempts + 1 WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    pub fn outbox_drop_conversation(&self, conv: ConversationId) -> ClientResult<Vec<Uuid>> {
        let ids: Vec<Uuid> = self
            .outbox()
            .into_iter()
            .filter(|r| r.conversation == conv)
            .filter_map(|r| match r.payload {
                Payload::Text { id, .. } => Some(id),
                _ => None,
            })
            .collect();
        self.conn
            .execute("DELETE FROM outbox WHERE conversation = ?1", [conv])?;
        Ok(ids)
    }

    // ---- read markers -----------------------------------------------------------------

    pub fn set_marker(&self, conv: ConversationId, user: UserId, seq: u64) -> ClientResult<()> {
        self.conn.execute(
            "INSERT INTO read_markers (conversation, user_id, seq) VALUES (?1, ?2, ?3)
             ON CONFLICT(conversation, user_id) DO UPDATE SET seq = MAX(seq, excluded.seq)",
            params![conv, user, seq as i64],
        )?;
        Ok(())
    }

    /// Highest read marker of anyone other than `me`.
    pub fn others_read(&self, conv: ConversationId, me: UserId) -> u64 {
        self.conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM read_markers WHERE conversation = ?1 AND user_id != ?2",
                params![conv, me],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0) as u64
    }

    // ---- users cache -------------------------------------------------------------------

    pub fn upsert_users(&self, users: &[DirectoryUser]) -> ClientResult<()> {
        for u in users {
            self.conn.execute(
                "INSERT OR REPLACE INTO users (id, username, display_name) VALUES (?1, ?2, ?3)",
                params![u.id, u.username, u.display_name],
            )?;
        }
        Ok(())
    }

    pub fn upsert_user(&self, id: UserId, username: &str, display_name: &str) -> ClientResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO users (id, username, display_name) VALUES (?1, ?2, ?3)",
            params![id, username, display_name],
        )?;
        Ok(())
    }

    pub fn user_name(&self, id: UserId) -> Option<(String, String)> {
        self.conn
            .query_row(
                "SELECT username, display_name FROM users WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_are_encrypted_at_rest() {
        let dir = tempfile::tempdir().expect("tmp");
        let store = Store::open(dir.path(), &[7u8; 32]).expect("open");
        let conv = Uuid::now_v7();
        let m = LocalMessage {
            id: Uuid::now_v7(),
            conversation: conv,
            seq: Some(1),
            sender: Some(Uuid::now_v7()),
            kind: MessageKind::Text,
            body: "the quarterly numbers are on the shared drive".into(),
            reply_to: None,
            ts: 1,
            edited: false,
            deleted: false,
            status: SendStatus::Sent,
            mine: false,
        };
        assert!(store.insert_message(&m).expect("insert"));
        assert_eq!(store.messages(conv, None, 10)[0].body, m.body);
        assert_eq!(store.search("QUARTERLY", 10).len(), 1);
        drop(store);
        // The raw database file must not contain the plaintext.
        let mut raw = std::fs::read(dir.path().join("chat.db")).unwrap_or_default();
        raw.extend(std::fs::read(dir.path().join("chat.db-wal")).unwrap_or_default());
        assert!(!raw.windows(9).any(|w| w == b"quarterly"));
    }
}
