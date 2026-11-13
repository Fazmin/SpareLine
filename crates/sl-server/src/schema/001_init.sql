-- SpareLine server schema v1.
-- Privacy: no table stores message plaintext. `envelopes.data` and `welcomes.data` are
-- MLS ciphertext produced on client devices.

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Control-panel admin (not a chat user).
CREATE TABLE admin (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    password_hash TEXT NOT NULL,
    recovery_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE users (
    id            BLOB PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE COLLATE NOCASE,
    display_name  TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    disabled      INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    last_seen     INTEGER
);

CREATE TABLE devices (
    id             BLOB PRIMARY KEY,
    user_id        BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    platform       TEXT NOT NULL,
    client_version TEXT NOT NULL,
    signature_key  BLOB NOT NULL,
    created_at     INTEGER NOT NULL,
    last_seen      INTEGER,
    revoked_at     INTEGER
);
CREATE INDEX devices_user ON devices(user_id);

CREATE TABLE sessions (
    token_hash TEXT PRIMARY KEY,
    user_id    BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id  BLOB NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    last_used  INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    ip         TEXT NOT NULL DEFAULT ''
);
CREATE INDEX sessions_user ON sessions(user_id);
CREATE INDEX sessions_device ON sessions(device_id);

CREATE TABLE groups (
    id          BLOB PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    description TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL
);

CREATE TABLE group_members (
    group_id BLOB NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    user_id  BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    added_at INTEGER NOT NULL,
    PRIMARY KEY (group_id, user_id)
);
CREATE INDEX group_members_user ON group_members(user_id);

-- Invite and setup codes. Only derived values are stored, never the code itself.
CREATE TABLE invites (
    id          BLOB PRIMARY KEY,
    invite_id   TEXT NOT NULL UNIQUE,
    proof_key   BLOB NOT NULL,
    kind        TEXT NOT NULL CHECK (kind IN ('new_user', 'setup')),
    target_user BLOB REFERENCES users(id) ON DELETE CASCADE,
    note        TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL,
    used_at     INTEGER,
    used_by     BLOB,
    revoked_at  INTEGER
);

CREATE TABLE invite_groups (
    invite   BLOB NOT NULL REFERENCES invites(id) ON DELETE CASCADE,
    group_id BLOB NOT NULL REFERENCES groups(id) ON DELETE CASCADE,
    PRIMARY KEY (invite, group_id)
);

CREATE TABLE conversations (
    id           BLOB PRIMARY KEY,
    kind         TEXT NOT NULL CHECK (kind IN ('direct', 'group')),
    scope_group  BLOB REFERENCES groups(id) ON DELETE SET NULL,
    mls_group_id BLOB NOT NULL UNIQUE,
    epoch        INTEGER NOT NULL,
    last_seq     INTEGER NOT NULL DEFAULT 0,
    created_by   BLOB NOT NULL,
    created_at   INTEGER NOT NULL,
    group_info   BLOB,
    -- "<smaller uuid>:<larger uuid>" for direct chats, so each pair has one chat.
    dm_key       TEXT UNIQUE
);

CREATE TABLE conversation_members (
    conversation BLOB NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id      BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    joined_at    INTEGER NOT NULL,
    PRIMARY KEY (conversation, user_id)
);
CREATE INDEX conversation_members_user ON conversation_members(user_id);

-- Devices that are currently leaves of the conversation's MLS group (for routing).
CREATE TABLE conversation_devices (
    conversation BLOB NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    device_id    BLOB NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    joined_at    INTEGER NOT NULL,
    PRIMARY KEY (conversation, device_id)
);
CREATE INDEX conversation_devices_device ON conversation_devices(device_id);

CREATE TABLE key_packages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id   BLOB NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    data        BLOB NOT NULL,
    last_resort INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE INDEX key_packages_device ON key_packages(device_id, last_resort);

CREATE TABLE welcomes (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    device_id    BLOB NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    conversation BLOB NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    data         BLOB NOT NULL,
    created_at   INTEGER NOT NULL
);
CREATE INDEX welcomes_device ON welcomes(device_id);

CREATE TABLE envelopes (
    conversation  BLOB NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    seq           INTEGER NOT NULL,
    sender_user   BLOB,
    sender_device BLOB,
    kind          TEXT NOT NULL,
    epoch         INTEGER NOT NULL,
    data          BLOB NOT NULL,
    ts            INTEGER NOT NULL,
    PRIMARY KEY (conversation, seq)
) WITHOUT ROWID;
CREATE INDEX envelopes_ts ON envelopes(ts);

CREATE TABLE device_cursors (
    device_id    BLOB NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    conversation BLOB NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    seq          INTEGER NOT NULL,
    PRIMARY KEY (device_id, conversation)
);

CREATE TABLE read_markers (
    user_id      BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    conversation BLOB NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    seq          INTEGER NOT NULL,
    PRIMARY KEY (user_id, conversation)
);

CREATE TABLE broadcasts (
    id         BLOB PRIMARY KEY,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL,
    severity   TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER,
    ended_at   INTEGER
);

-- Admin actions only. Never content.
CREATE TABLE audit_log (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    ts     INTEGER NOT NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT ''
);

-- Connection diagnostics (short retention).
CREATE TABLE connection_events (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    ts        INTEGER NOT NULL,
    ip        TEXT NOT NULL,
    user_id   BLOB,
    device_id BLOB,
    kind      TEXT NOT NULL,
    detail    TEXT NOT NULL DEFAULT ''
);
CREATE INDEX connection_events_ts ON connection_events(ts);
CREATE INDEX connection_events_user ON connection_events(user_id, ts);

CREATE TABLE health_reports (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    received_at INTEGER NOT NULL,
    user_id     BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    device_id   BLOB NOT NULL,
    report      TEXT NOT NULL
);
CREATE INDEX health_reports_user ON health_reports(user_id, received_at);
