//! How IMAP names map onto the engine's opaque ids, and what a sync token remembers.
//!
//! - `FolderId` is `{account}/{mailbox}`, the mailbox name exactly as the server spells it. The
//!   account prefix keeps two accounts' `INBOX`es apart in the store, where folder ids are global.
//! - `MessageId` is `{folder id};{UIDVALIDITY};{UID}`. A UID only means something inside one
//!   mailbox and one UIDVALIDITY, so a moved message gets a new id: the next sync of the two
//!   folders reports it removed from one and added to the other.
//! - `SyncToken` is `1;{UIDVALIDITY};{UIDNEXT};{HIGHESTMODSEQ or 0};{known UIDs}`. Carrying the
//!   UIDs lets removals be worked out exactly on any server, and lets a UIDVALIDITY change name
//!   every message that just disappeared.

use mail_core::{AccountId, FolderId, MailError, MessageId, Result, SyncToken};

const TOKEN_VERSION: &str = "1";
/// A token never legitimately names more messages than this; a larger one is treated as garbage.
const MAX_UIDS: usize = 20_000_000;

pub fn folder_id(account: &AccountId, mailbox: &str) -> FolderId { FolderId(format!("{}/{mailbox}", account.0)) }

/// The IMAP mailbox behind a folder id, or `NotFound` if the id belongs to another account.
pub fn mailbox_of<'a>(account: &AccountId, folder: &'a FolderId) -> Result<&'a str> {
    folder.0.strip_prefix(account.0.as_str()).and_then(|rest| rest.strip_prefix('/')).filter(|m| !m.is_empty())
        .ok_or_else(|| MailError::NotFound("folder".into()))
}

pub fn message_id(folder: &FolderId, validity: u32, uid: u32) -> MessageId { MessageId(format!("{};{validity};{uid}", folder.0)) }

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct MessageRef { pub folder: FolderId, pub validity: u32, pub uid: u32 }

/// Mailbox names may contain `;`, so the two numbers are split off the end.
pub fn parse_message_id(id: &MessageId) -> Result<MessageRef> {
    let mut parts = id.0.rsplitn(3, ';');
    let (uid, validity, folder) = (parts.next(), parts.next(), parts.next());
    match (folder, validity.and_then(|v| v.parse().ok()), uid.and_then(|u| u.parse().ok())) {
        (Some(folder), Some(validity), Some(uid)) if !folder.is_empty() && uid != 0 => Ok(MessageRef { folder: FolderId(folder.into()), validity, uid }),
        _ => Err(MailError::NotFound("message".into())),
    }
}

/// What the last sync of a folder saw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncState {
    pub validity: u32,
    pub uid_next: u32,
    /// HIGHESTMODSEQ, or 0 when the server has no CONDSTORE.
    pub modseq: u64,
    /// Every UID in the folder at that time, ascending.
    pub uids: Vec<u32>,
}

impl SyncState {
    pub fn encode(&self) -> SyncToken {
        SyncToken(format!("{TOKEN_VERSION};{};{};{};{}", self.validity, self.uid_next, self.modseq, compress(&self.uids)))
    }

    /// `None` for an empty or unreadable token: both mean a full sync.
    pub fn decode(token: &SyncToken) -> Option<Self> {
        let mut parts = token.0.splitn(5, ';');
        if parts.next()? != TOKEN_VERSION { return None; }
        let state = Self { validity: parts.next()?.parse().ok()?, uid_next: parts.next()?.parse().ok()?, modseq: parts.next()?.parse().ok()?, uids: expand(parts.next()?)? };
        // Every known UID is below UIDNEXT; a token that says otherwise was not written by `encode`.
        state.uids.last().is_none_or(|highest| *highest < state.uid_next).then_some(state)
    }
}

/// Ascending UIDs as an IMAP sequence set (`1:5,7,9:20`); the same text goes into commands.
pub fn compress(sorted: &[u32]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let start = sorted[i];
        let mut end = start;
        while i + 1 < sorted.len() && sorted[i + 1] <= end.saturating_add(1) { i += 1; end = sorted[i]; }
        if !out.is_empty() { out.push(','); }
        out.push_str(&start.to_string());
        if end != start { out.push(':'); out.push_str(&end.to_string()); }
        i += 1;
    }
    out
}

pub fn expand(set: &str) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    for part in set.split(',').filter(|p| !p.is_empty()) {
        let (start, end) = match part.split_once(':') { Some((a, b)) => (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?), None => { let n = part.parse().ok()?; (n, n) } };
        if start == 0 || end < start || out.len() + (end - start) as usize >= MAX_UIDS { return None; }
        out.extend(start..=end);
    }
    out.sort_unstable();
    out.dedup();
    Some(out)
}

/// Sequence sets for one command each, so a large selection never builds an over-long line.
pub fn chunked_sets(uids: &[u32]) -> Vec<String> {
    let mut sorted = uids.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.chunks(500).map(compress).collect()
}
