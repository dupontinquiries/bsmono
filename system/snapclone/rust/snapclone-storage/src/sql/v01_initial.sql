CREATE TABLE IF NOT EXISTS snaps (
    id           TEXT PRIMARY KEY,
    sender_id    TEXT NOT NULL,
    recipient_id TEXT NOT NULL,
    content_hash TEXT NOT NULL DEFAULT '',
    local_path   TEXT NOT NULL DEFAULT '',
    duration_secs INTEGER NOT NULL DEFAULT 5,
    sent_at      TEXT NOT NULL,
    delivered_at TEXT NOT NULL DEFAULT '',
    viewed       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS users (
    id           TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    avatar_path  TEXT NOT NULL DEFAULT '',
    online       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS peer_addresses (
    user_id TEXT PRIMARY KEY,
    host    TEXT NOT NULL,
    port    INTEGER NOT NULL
);
