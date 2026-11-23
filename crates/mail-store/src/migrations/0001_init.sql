-- Schema v1 (architecture §6). Forward-only; never edit a shipped migration, add a new file.
CREATE TABLE accounts (
    id       TEXT PRIMARY KEY,
    settings TEXT NOT NULL DEFAULT '{}'        -- JSON; never secrets
) STRICT;

CREATE TABLE folders (
    id         TEXT PRIMARY KEY,
    account    TEXT NOT NULL,
    name       TEXT NOT NULL,
    path       TEXT NOT NULL,                  -- JSON array of segments
    role       TEXT NOT NULL,
    sync_token TEXT NOT NULL DEFAULT ''
) STRICT;
CREATE INDEX folders_account ON folders (account);

CREATE TABLE messages (
    rowid       INTEGER PRIMARY KEY,
    id          TEXT NOT NULL UNIQUE,
    account     TEXT NOT NULL,
    folder      TEXT NOT NULL,
    thread      TEXT NOT NULL,
    from_name   TEXT,
    from_email  TEXT NOT NULL,
    recipients  TEXT NOT NULL,                 -- JSON {to, cc}
    subject     TEXT NOT NULL,
    preview     TEXT NOT NULL,
    date        INTEGER NOT NULL,
    flags       INTEGER NOT NULL,
    attachments TEXT NOT NULL                  -- JSON array
) STRICT;
CREATE INDEX messages_folder_date ON messages (folder, date DESC);
CREATE INDEX messages_thread ON messages (thread);

CREATE TABLE bodies (
    message_id  TEXT PRIMARY KEY,
    raw         BLOB NOT NULL,
    size        INTEGER NOT NULL,
    last_opened INTEGER NOT NULL               -- monotonic counter, not a clock
) STRICT;
CREATE INDEX bodies_lru ON bodies (last_opened);

CREATE VIRTUAL TABLE search USING fts5 (
    sender, recipients, subject, preview, body,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE ops_journal (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    op  TEXT NOT NULL                          -- JSON, owned by the engine
) STRICT;

CREATE TABLE outbox (
    id    TEXT PRIMARY KEY,
    raw   BLOB NOT NULL,
    state TEXT NOT NULL                        -- JSON
) STRICT;

CREATE TABLE contacts (
    email TEXT PRIMARY KEY,
    name  TEXT,
    uses  INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE image_policy (
    sender TEXT PRIMARY KEY
) STRICT;
