//! The demo accounts the UI runs on until real backends land: `mail-mock`'s two seeded accounts,
//! with two things the mock leaves out so every part of the UI has something to show: a Trash
//! folder (for delete), and bodies long enough for the reader (a link, a signature, and on
//! replies a quoted block long enough to fold). Deterministic, like the mock itself.

use mail_core::*;
use mail_mock::MockBackend;
use std::sync::mpsc::Sender;

pub struct DemoBackend {
    inner: MockBackend,
    trash: Folder,
}

impl DemoBackend {
    pub fn new(inner: MockBackend) -> DemoBackend {
        let account = inner.account().clone();
        let trash = Folder {
            id: FolderId(format!("{account}/Trash")),
            account,
            name: "Trash".into(),
            path: vec!["Trash".into()],
            role: FolderRole::Trash,
            unread: 0,
            total: 0,
        };
        DemoBackend { inner, trash }
    }

    /// The two demo accounts (`iCloud`, `Gmail`).
    pub fn accounts() -> Vec<DemoBackend> { MockBackend::demo().into_iter().map(DemoBackend::new).collect() }
}

/// Senders who write like people (their messages get a quoted block); the rest are services.
const PEOPLE: [&str; 5] = ["odin@", "muninn@", "huginn@", "mom@", "lab@"];

pub fn is_person(email: &str) -> bool { PEOPLE.iter().any(|p| email.starts_with(p)) }

const LINK: &str = "https://ferries.example/norrland/winter";

/// The mock's message with a fuller plain-text body. Headers are kept as the mock wrote them.
fn enrich(raw: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(raw) else { return raw.to_vec() };
    let Some((head, body)) = text.split_once("\r\n\r\n") else { return raw.to_vec() };
    let mut paragraphs = body.split("\r\n\r\n");
    let preview = paragraphs.next().unwrap_or("").trim();
    let signature = paragraphs.next().unwrap_or("").trim();
    let reply = head.lines().any(|l| l.starts_with("Subject: Re:") || l.strip_prefix("From: ").is_some_and(|from| PEOPLE.iter().any(|p| from.contains(&format!("<{p}")))));
    let mut out = String::from(head);
    out.push_str("\r\n\r\n");
    out.push_str(if reply { "Morning,\r\n\r\n" } else { "Hello,\r\n\r\n" });
    out.push_str(preview);
    out.push_str("\r\n\r\n");
    if reply {
        out.push_str("Maps are attached. The ferry schedule is here:\r\n");
        out.push_str(LINK);
    } else {
        out.push_str("The details are on the page below, and nothing here needs an answer today.\r\n");
        out.push_str("https://status.example/notes");
    }
    out.push_str("\r\n\r\n");
    out.push_str(signature);
    out.push_str("\r\n");
    if reply {
        out.push_str("\r\n> On Thu, Nov 12, Jason wrote:\r\n");
        out.push_str("> Is the pass open yet? I want to send the\r\n");
        out.push_str("> Gnomon build north before the snow.\r\n");
        out.push_str("> If it is not, the coast road adds two days\r\n");
        out.push_str("> and the ravens will have to carry the rest.\r\n");
        out.push_str("> Tell me what Huginn saw.\r\n");
    }
    out.into_bytes()
}

impl MailBackend for DemoBackend {
    fn account(&self) -> &AccountId { self.inner.account() }

    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> {
        Box::pin(async move {
            let mut folders = self.inner.list_folders().await?;
            folders.push(self.trash.clone());
            Ok(folders)
        })
    }

    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> {
        self.inner.sync_folder(folder, since)
    }

    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> {
        Box::pin(async move {
            let raw = self.inner.fetch_body(msg).await?;
            Ok(RawMessage { id: raw.id, bytes: enrich(&raw.bytes) })
        })
    }

    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> {
        self.inner.set_flags(msgs, add, remove)
    }

    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> { self.inner.move_to(msgs, to) }

    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> { self.inner.append_draft(raw) }

    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> {
        self.inner.send(raw, from, rcpt)
    }

    fn idle<'a>(&'a self, folder: &'a FolderId, events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> { self.inner.idle(folder, events) }
}
