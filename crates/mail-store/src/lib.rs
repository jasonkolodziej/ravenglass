//! SQLite cache (rusqlite, `bundled`, FTS5): accounts, folders, messages, bodies, search index.
//! Milestone M4 (plan §6). Keep the API free of `std::fs`/`PathBuf` (open via a storage trait) so a
//! wasm build can back it with browser storage later.
//!
//! TODO(M4): schema + migrations, FolderDelta apply, FTS5 search with operators
//! (`from:` `to:` `subject:` `has:attachment` `is:unread` `before:` `after:`), < 50 ms on 10k messages.
