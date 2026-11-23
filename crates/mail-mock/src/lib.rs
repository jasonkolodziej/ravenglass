//! A deterministic in-memory [`MailBackend`]. Same seed → same mailboxes, ids, dates and flags,
//! so headless snapshots are reproducible (plan §5). No randomness from the OS, no clock reads.

use mail_core::*;
use std::sync::Mutex;
use std::sync::mpsc::Sender;

/// Fixed "now" for all seed data: Fri 2026-11-13 16:44 UTC.
pub const NOW: Timestamp = 1_794_588_240;

/// Small, fast, deterministic PRNG (SplitMix64).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 { self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15); let mut z = self.0; z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9); z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB); z ^ (z >> 31) }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T { &xs[(self.next() % xs.len() as u64) as usize] }
    fn chance(&mut self, pct: u64) -> bool { self.next() % 100 < pct }
}

const SENDERS: &[(&str, &str)] = &[
    ("Odin Allfather", "odin@asgard.example"), ("Muninn", "muninn@ravens.example"), ("Huginn", "huginn@ravens.example"),
    ("GitHub", "notifications@github.example"), ("Fastmail", "billing@fastmail.example"), ("Kolodziej Lab", "lab@utsa.example"),
    ("Arc HPC", "slurm@arc.utsa.example"), ("Dioxus", "news@dioxus.example"), ("Servo", "news@servo.example"),
    ("Linear", "notify@linear.example"), ("Cloudflare", "noreply@cloudflare.example"), ("Mom", "mom@family.example"),
];
const SUBJECTS: &[(&str, &str)] = &[
    ("Re: Morning dispatch", "Huginn reports the northern pass is clear and the road to Uppsala is open again."),
    ("Archive digest — week 46", "Everything you asked me to remember, in one place."),
    ("[anomalyco/rune] dev: new rev", "rune-kit Combobox gains animated filtering."),
    ("Your invoice for November", "Thanks for using Fastmail. Your receipt is attached."),
    ("Gnomon scheduler sync", "Agenda: backfill policy, preemption, fair-share decay."),
    ("Job 48211 finished (COMPLETED)", "Elapsed 03:12:44 on gpu-a100-07."),
    ("Blitz 0.3 release notes", "Incremental layout, custom elements and more."),
    ("This month in Servo", "Monthly releases and embedding API changes."),
    ("3 issues assigned to you", "RG-14 Blitz spike: Parley version alignment."),
    ("Tunnel healthy", "prefect.example.dev is serving traffic."),
    ("Thanksgiving plans", "Are you bringing the green chile this year?"),
];

pub struct MockBackend {
    account: AccountId,
    folders: Vec<Folder>,
    messages: Mutex<Vec<Envelope>>,
    pub sent: Mutex<Vec<Vec<u8>>>,
}

impl MockBackend {
    /// `seed` picks the data; `count` messages spread over the account's folders.
    pub fn new(account: &str, seed: u64, count: usize) -> Self {
        let acc = AccountId::from(account);
        let mk = |name: &str, role| Folder { id: FolderId(format!("{account}/{name}")), account: acc.clone(), name: name.into(), path: vec![name.into()], role, unread: 0, total: 0 };
        let mut folders = vec![mk("Inbox", FolderRole::Inbox), mk("Drafts", FolderRole::Drafts), mk("Sent", FolderRole::Sent), mk("Archive", FolderRole::Archive), mk("Receipts", FolderRole::Other)];
        let mut rng = Rng(seed);
        let mut msgs = Vec::with_capacity(count);
        for i in 0..count {
            let (name, email) = *rng.pick(SENDERS);
            let (subject, preview) = *rng.pick(SUBJECTS);
            let folder = if i % 7 == 6 { &folders[3] } else if i % 11 == 10 { &folders[4] } else { &folders[0] };
            let mut flags = Flags::empty();
            if i >= 12 || !rng.chance(70) { flags.insert(Flags::SEEN); }
            if rng.chance(8) { flags.insert(Flags::FLAGGED); }
            let attachments = if rng.chance(15) { vec![Attachment { name: "route-north.pdf".into(), mime: "application/pdf".into(), size: 2_100_000, content_id: None }] } else { vec![] };
            msgs.push(Envelope {
                id: MessageId(format!("{account}/{i:05}")), folder: folder.id.clone(), thread: ThreadId(format!("{account}/t{}", i / 3)),
                from: Address::new(name, email), to: vec![Address::new("Jason", "jason@example.com")], cc: vec![],
                subject: subject.into(), preview: preview.into(), date: NOW - (i as i64) * 2_700 - (rng.next() % 600) as i64,
                flags, attachments,
            });
        }
        for f in &mut folders {
            f.total = msgs.iter().filter(|m| m.folder == f.id).count() as u32;
            f.unread = msgs.iter().filter(|m| m.folder == f.id && m.is_unread()).count() as u32;
        }
        Self { account: acc, folders, messages: Mutex::new(msgs), sent: Mutex::new(vec![]) }
    }

    /// The default scene data: two accounts, ~200 messages total.
    pub fn demo() -> Vec<MockBackend> { vec![Self::new("iCloud", 46, 140), Self::new("Gmail", 7, 60)] }

    fn raw(env: &Envelope) -> Vec<u8> {
        format!("From: {} <{}>\r\nTo: Jason <jason@example.com>\r\nSubject: {}\r\nMessage-ID: <{}@mock>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{}\r\n\r\n— {}\r\n",
            env.from.display(), env.from.email, env.subject, env.id, env.preview, env.from.display()).into_bytes()
    }
}

impl MailBackend for MockBackend {
    fn account(&self) -> &AccountId { &self.account }
    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> { let f = self.folders.clone(); Box::pin(async move { Ok(f) }) }
    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> {
        Box::pin(async move {
            let msgs = self.messages.lock().unwrap();
            let added = if since.0.is_empty() { msgs.iter().filter(|m| &m.folder == folder).cloned().collect() } else { vec![] };
            Ok(FolderDelta { added, changed_flags: vec![], removed: vec![], token: SyncToken("1".into()) })
        })
    }
    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> {
        Box::pin(async move {
            let msgs = self.messages.lock().unwrap();
            let env = msgs.iter().find(|m| &m.id == msg).ok_or_else(|| MailError::NotFound(msg.to_string()))?;
            Ok(RawMessage { id: msg.clone(), bytes: Self::raw(env) })
        })
    }
    fn set_flags<'a>(&'a self, ids: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            for m in self.messages.lock().unwrap().iter_mut().filter(|m| ids.contains(&m.id)) { m.flags.insert(add); m.flags.remove(remove); }
            Ok(())
        })
    }
    fn move_to<'a>(&'a self, ids: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            for m in self.messages.lock().unwrap().iter_mut().filter(|m| ids.contains(&m.id)) { m.folder = to.clone(); }
            Ok(())
        })
    }
    fn append_draft<'a>(&'a self, _raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> { Box::pin(async move { Ok(MessageId("draft/0".into())) }) }
    fn send<'a>(&'a self, raw: &'a [u8], _from: &'a Address, _rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.sent.lock().unwrap().push(raw.to_vec()); Ok(()) })
    }
    fn idle<'a>(&'a self, _folder: &'a FolderId, _events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> { Box::pin(async move { Ok(()) }) }
}

/// Drive a ready-immediately future (all mock futures are). Tests and headless scenes only.
pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut f = std::pin::pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    match f.as_mut().poll(&mut cx) { Poll::Ready(v) => v, Poll::Pending => panic!("mock futures never pend") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_data() {
        let a = MockBackend::new("x", 1, 50);
        let b = MockBackend::new("x", 1, 50);
        assert_eq!(*a.messages.lock().unwrap(), *b.messages.lock().unwrap());
    }

    #[test]
    fn demo_has_about_200_messages_and_unread_counts() {
        let demo = MockBackend::demo();
        let total: u32 = demo.iter().flat_map(|b| b.folders.iter()).map(|f| f.total).sum();
        assert_eq!(total, 200);
        let inbox = &demo[0].folders[0];
        assert!(inbox.unread > 0 && inbox.unread <= 12, "unread {}", inbox.unread);
    }

    #[test]
    fn archive_moves_a_message() {
        let b = MockBackend::new("x", 3, 20);
        let id = MessageId("x/00000".into());
        let archive = FolderId("x/Archive".into());
        block_on(b.move_to(std::slice::from_ref(&id), &archive)).unwrap();
        let d = block_on(b.sync_folder(&archive, &SyncToken::default())).unwrap();
        assert!(d.added.iter().any(|m| m.id == id));
    }

    #[test]
    fn body_is_rfc5322ish() {
        let b = MockBackend::new("x", 3, 5);
        let raw = block_on(b.fetch_body(&MessageId("x/00001".into()))).unwrap();
        assert!(String::from_utf8(raw.bytes).unwrap().contains("Subject: "));
    }
}
