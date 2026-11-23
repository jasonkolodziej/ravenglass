//! The message boundary (architecture §3, §5): the UI sends [`Command`]s and renders [`Event`]s.
//! Both are plain serde data, so the same enums cross a thread channel on desktop and
//! `postMessage` on the web.

use mail_core::machine::{account_sync, image_load, outbox};
use mail_core::*;
use mail_render::{SanitizedHtml, TextBody};
use serde::{Deserialize, Serialize};

/// A journaled user action; the handle for undo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OpId(pub i64);

/// A queued outgoing message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OutboxId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReaderView { Html, Text }

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Connect an account and sync its folders, Inbox first.
    OpenAccount(AccountId),
    /// Check for changes now (the push path sends this when IDLE wakes).
    Refresh(AccountId),
    LoadRange { folder: FolderId, offset: u32, limit: u32 },
    OpenMessage { id: MessageId, view: ReaderView },
    SetFlags { ids: Vec<MessageId>, add: Flags, remove: Flags },
    Move { ids: Vec<MessageId>, to: FolderId },
    /// Move to the account's folder with this role (`Archive`, `Trash`, `Junk`).
    MoveToRole { ids: Vec<MessageId>, role: FolderRole },
    Undo(OpId),
    Search { query: String, account: Option<AccountId>, limit: u32 },
    Send { account: AccountId, raw: Vec<u8>, from: Address, rcpt: Vec<Address> },
    CancelSend(OutboxId),
    RetrySend(OutboxId),
    AllowImages { message: MessageId, scope: image_load::Scope },
    CompleteAddress { prefix: String, limit: u32 },
    /// Deliver due timers. The runner sends it when the earliest [`Event::WakeAt`] passes.
    Tick,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Folders with current local counts; sent after every change that moves a count.
    FolderList { account: AccountId, folders: Vec<Folder> },
    Envelopes { folder: FolderId, offset: u32, envelopes: Vec<Envelope> },
    SyncState { account: AccountId, state: account_sync::State },
    /// The action is applied locally and journaled. `sent` is false while it waits for the server.
    Applied { op: OpId, sent: bool },
    /// The server refused the action, so the local change was undone.
    Reverted { op: OpId, reason: String },
    Undone { op: OpId },
    SearchResults { query: String, envelopes: Vec<Envelope> },
    OutboxState { id: OutboxId, state: outbox::State },
    Body { id: MessageId, body: Box<BodyView> },
    ImagesAllowed { message: MessageId },
    Addresses { prefix: String, addresses: Vec<Address> },
    /// Call back with [`Command::Tick`] at this time (seconds, the caller's clock).
    WakeAt(f64),
    /// One line for the status bar. Never contains message content or addresses.
    Notice(String),
    Error { context: String, message: String },
}

/// What the reader needs for one message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BodyView {
    pub subject: String,
    pub from: Option<Address>,
    pub to: Vec<Address>,
    pub cc: Vec<Address>,
    pub date: Option<Timestamp>,
    pub attachments: Vec<Attachment>,
    /// Always present: the text view, and the fallback if HTML rendering fails.
    pub text: TextBody,
    /// Sanitized HTML for `mail-blitz`, when the message has HTML and the HTML view was asked for.
    pub html: Option<SanitizedHtml>,
    pub images: image_load::State,
}
