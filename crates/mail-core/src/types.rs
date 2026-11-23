//! Plain data. Ids are opaque strings so IMAP UIDs, JMAP ids and mock ids all fit.

use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.0) }
        }
        impl From<&str> for $name {
            fn from(s: &str) -> Self { Self(s.to_owned()) }
        }
    };
}
id!(AccountId);
id!(FolderId);
id!(MessageId);
id!(ThreadId);

/// Seconds since the Unix epoch, UTC. Kept as an integer so mock data is deterministic.
pub type Timestamp = i64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FolderRole { Inbox, Drafts, Sent, Archive, Trash, Junk, Flagged, Other }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: FolderId,
    pub account: AccountId,
    pub name: String,
    /// Path segments for nested folders, e.g. `["Receipts", "2026"]`.
    pub path: Vec<String>,
    pub role: FolderRole,
    pub unread: u32,
    pub total: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address { pub name: Option<String>, pub email: String }

impl Address {
    pub fn new(name: &str, email: &str) -> Self { Self { name: Some(name.into()), email: email.into() } }
    /// What a list row shows: the name, else the address.
    pub fn display(&self) -> &str { self.name.as_deref().unwrap_or(&self.email) }
}

bitflags_lite! {
    pub struct Flags: u8 { SEEN = 1, FLAGGED = 2, ANSWERED = 4, DRAFT = 8, DELETED = 16 }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment { pub name: String, pub mime: String, pub size: u64, pub content_id: Option<String> }

/// What the list needs; bodies are fetched lazily.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub id: MessageId,
    pub folder: FolderId,
    pub thread: ThreadId,
    pub from: Address,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub subject: String,
    pub preview: String,
    pub date: Timestamp,
    pub flags: Flags,
    pub attachments: Vec<Attachment>,
}

impl Envelope {
    pub fn is_unread(&self) -> bool { !self.flags.contains(Flags::SEEN) }
}

/// A fetched message: raw RFC 5322 bytes; `mail-render` parses them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawMessage { pub id: MessageId, pub bytes: Vec<u8> }

/// Opaque resume point for incremental sync (IMAP: UIDVALIDITY + HIGHESTMODSEQ; mock: a counter).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncToken(pub String);

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FolderDelta {
    pub added: Vec<Envelope>,
    pub changed_flags: Vec<(MessageId, Flags)>,
    pub removed: Vec<MessageId>,
    pub token: SyncToken,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdleEvent { Exists(FolderId), FlagsChanged(FolderId), Expunged(FolderId) }

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MailError {
    #[error("authentication failed")]
    Auth,
    #[error("network: {0}")]
    Network(String),
    #[error("server: {0}")]
    Server(String),
    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, MailError>;

/// Tiny flags type without the `bitflags` dependency.
#[macro_export]
macro_rules! bitflags_lite {
    (pub struct $name:ident: $t:ty { $($flag:ident = $v:expr),* $(,)? }) => {
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub $t);
        impl $name {
            $(pub const $flag: $name = $name($v);)*
            pub const fn empty() -> Self { Self(0) }
            pub const fn contains(self, o: Self) -> bool { self.0 & o.0 == o.0 }
            pub fn insert(&mut self, o: Self) { self.0 |= o.0 }
            pub fn remove(&mut self, o: Self) { self.0 &= !o.0 }
        }
        impl std::ops::BitOr for $name { type Output = Self; fn bitor(self, o: Self) -> Self { Self(self.0 | o.0) } }
    };
}
use bitflags_lite;
