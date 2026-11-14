-- Standby copies: remember a restore so apps can resynchronise.
CREATE TABLE server_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Where each conversation stood in the copy this server was restored from.
ALTER TABLE conversations ADD COLUMN restore_seq INTEGER;
ALTER TABLE conversations ADD COLUMN restore_epoch INTEGER;
