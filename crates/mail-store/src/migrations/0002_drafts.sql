-- Local drafts, so an unsent message survives a restart (architecture §5, ComposeSession).
CREATE TABLE drafts (
    id    TEXT PRIMARY KEY,
    draft TEXT NOT NULL                        -- JSON, owned by the engine
) STRICT;
