//! The checks themselves, through the same backends the app uses. Each step prints one or more
//! report lines and counts as a failure when it does not work.

use crate::args::Args;
use crate::settings::{Plan, Secret, Servers};
use crate::{block_on, body, line};
use mail_core::*;
use mail_imap::{Credential, ImapBackend, ImapConfig};
use mail_jmap::{HttpsTransport, JmapBackend};
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

const ACCOUNT: &str = "check";
const CHECK_FOLDER: &str = "Ravenglass-Check";

enum Account { Imap(Arc<ImapBackend>), Jmap(Arc<JmapBackend<HttpsTransport>>) }

impl Account {
    fn backend(&self) -> Arc<dyn MailBackend> {
        match self { Account::Imap(b) => b.clone(), Account::Jmap(b) => b.clone() }
    }
}

fn ms(elapsed: Duration) -> String { format!("{} ms", elapsed.as_millis()) }

fn timed<T>(work: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let out = work();
    (out, started.elapsed())
}

/// Runs every check; returns how many failed.
pub fn run(args: &Args, plan: &Plan, secret: Secret) -> u32 {
    let account = match connect(plan, secret) {
        Ok(account) => account,
        Err(problem) => { line("connect", format!("FAIL {problem}")); return 1; }
    };
    let mut failures = 0;
    let mut fail = |label: &str, error: &dyn std::fmt::Display| { line(label, format!("FAIL {error}")); failures += 1; };

    // Connection, TLS, login and what the server can do.
    let (capabilities, took) = timed(|| match &account {
        Account::Imap(b) => block_on(b.capabilities()),
        Account::Jmap(b) => block_on(b.capabilities()),
    });
    match capabilities {
        Ok(list) => {
            line("connect", format!("logged in, {} (TLS handshake, login and capabilities)", ms(took)));
            line("capabilities", list.join(" "));
            if let Account::Imap(_) = account { line("", imap_summary(&list)); }
        }
        Err(error) => { fail("connect", &error); return failures; }
    }
    if let Account::Imap(b) = &account {
        if args.no_smtp { line("smtp", "skipped (--smtp none)"); } else { match timed(|| block_on(b.check_submission())) {
            (Ok(extensions), took) => line("smtp", format!("logged in without sending, {}; offers {}", ms(took), extensions.join(" "))),
            (Err(error), _) => fail("smtp", &error),
        } }
    }

    // Folders, with roles and counts. Only folders with a role are named: the others are the user's.
    let backend = account.backend();
    let folders = match timed(|| block_on(backend.list_folders())) {
        (Ok(folders), took) => { line("folders", format!("{} folders in {}", folders.len(), ms(took))); folders }
        (Err(error), _) => { fail("folders", &error); return failures; }
    };
    for role in [FolderRole::Inbox, FolderRole::Drafts, FolderRole::Sent, FolderRole::Archive, FolderRole::Trash, FolderRole::Junk, FolderRole::Flagged] {
        let with: Vec<&Folder> = folders.iter().filter(|f| f.role == role).collect();
        match with.as_slice() {
            [] => line("", format!("{role:?}: none")),
            [f] => line("", format!("{role:?}: \"{}\" ({} messages, {} unread)", f.path.join("/"), f.total, f.unread)),
            many => line("", format!("{role:?}: {} folders (unexpected)", many.len())),
        }
    }
    let others: Vec<&Folder> = folders.iter().filter(|f| f.role == FolderRole::Other).collect();
    let deepest = others.iter().map(|f| f.path.len()).max().unwrap_or(0);
    line("", format!("other folders: {} ({} messages, nested up to {deepest} deep, {} with non-ASCII names)", others.len(), others.iter().map(|f| u64::from(f.total)).sum::<u64>(), others.iter().filter(|f| !f.name.is_ascii()).count()));
    if !folders.iter().any(|f| f.role == FolderRole::Inbox) { fail("folders", &"no inbox"); return failures; }
    let inbox = folders.iter().find(|f| f.role == FolderRole::Inbox).unwrap().id.clone();

    // The newest inbox messages, as the list would show them. Changes nothing on the server.
    let newest = match timed(|| match &account {
        Account::Imap(b) => block_on(b.newest(&inbox, args.limit)),
        Account::Jmap(b) => block_on(b.newest(&inbox, args.limit)),
    }) {
        (Ok((envelopes, total)), took) => {
            line("inbox", format!("read the newest {} of {total} envelopes in {}", envelopes.len(), ms(took)));
            line("", envelope_summary(&envelopes));
            envelopes
        }
        (Err(error), _) => { fail("inbox", &error); Vec::new() }
    };

    // The newest message's body, parsed as the reader would. Only its shape is reported.
    match newest.first() {
        None => line("body", "skipped: the inbox is empty"),
        Some(envelope) => match timed(|| block_on(backend.fetch_body(&envelope.id))) {
            (Ok(raw), took) => match timed(|| body::shape(&raw.bytes)) {
                (Some(shape), parse) => {
                    line("body", format!("newest message: {} bytes fetched in {}, parsed in {}", shape.bytes, ms(took), ms(parse)));
                    line("", format!("parts: {}", shape.parts.iter().map(|p| p.trim()).collect::<Vec<_>>().join(", ")));
                    line("", format!("plain text: {}, HTML: {}, remote images: {} (blocked until allowed), trackers removed: {}, attachments: {}, inline parts: {}",
                        yes(shape.has_text), yes(shape.has_html), shape.remote_images, shape.trackers_removed, shape.attachments, shape.inline_parts));
                    line("", format!("reader text view: {} lines, {} links", shape.text_lines, shape.links));
                }
                (None, _) => fail("body", &format!("{} bytes that do not parse as a message", raw.bytes.len())),
            },
            (Err(error), _) => fail("body", &error),
        },
    }

    // Push.
    match &account {
        Account::Jmap(_) => line("push", "not checked: Ravenglass has no JMAP push yet, the app polls every 5 minutes"),
        Account::Imap(b) if args.idle_secs > 0 => failures += idle(b.clone(), &inbox, args.idle_secs),
        Account::Imap(_) => line("push", "skipped (--idle 0)"),
    }

    if args.write { failures += write(&account, &folders); } else { line("write", "skipped (read-only; --write checks changes inside a folder of its own)"); }
    failures
}

fn yes(flag: bool) -> &'static str { if flag { "yes" } else { "no" } }

fn connect(plan: &Plan, secret: Secret) -> std::result::Result<Account, String> {
    match &plan.servers {
        Servers::Imap { imap, smtp } => {
            let credential = match secret { Secret::Password(p) => Credential::Password(p), Secret::Token(t) => Credential::OAuth2 { access_token: t } };
            let mut config = ImapConfig::new(ACCOUNT, &imap.host, &smtp.host, &plan.login, credential);
            config.imap = imap.clone();
            config.smtp = smtp.clone();
            config.timeout = Duration::from_secs(30);
            // `--ca`: that certificate is added to the trust store; the public roots stay.
            config.extra_roots = plan.roots.clone();
            Ok(Account::Imap(Arc::new(ImapBackend::new(config).map_err(|e| e.to_string())?)))
        }
        Servers::Jmap { session_url } => {
            let credential = match secret {
                Secret::Password(password) => mail_jmap::Credential::Basic { username: plan.login.clone(), password },
                Secret::Token(token) => mail_jmap::Credential::Bearer(token),
            };
            let http = if plan.roots.is_empty() { HttpsTransport::new(&credential) } else { HttpsTransport::with_roots(&credential, &plan.roots) };
            Ok(Account::Jmap(Arc::new(JmapBackend::new(AccountId::from(ACCOUNT), session_url, http))))
        }
    }
}

/// The IMAP extensions Ravenglass uses, and which of them this server lacks.
fn imap_summary(capabilities: &[String]) -> String {
    let has = |name: &str| capabilities.iter().any(|c| c.eq_ignore_ascii_case(name));
    let used = ["IDLE", "CONDSTORE", "QRESYNC", "MOVE", "UIDPLUS", "SPECIAL-USE", "SASL-IR"];
    let missing: Vec<&str> = used.iter().copied().filter(|c| !has(c)).collect();
    if missing.is_empty() { "every extension Ravenglass uses is there".to_owned() } else { format!("missing (a fallback is used): {}", missing.join(" ")) }
}

/// The envelopes as counts: what parsed, what is empty, what looks off.
fn envelope_summary(envelopes: &[Envelope]) -> String {
    let count = |f: &dyn Fn(&Envelope) -> bool| envelopes.iter().filter(|e| f(e)).count();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let mut threads: Vec<&str> = envelopes.iter().map(|e| e.thread.0.as_str()).collect();
    threads.sort_unstable();
    threads.dedup();
    format!("unread {}, flagged {}, with attachments {}, empty preview {}, empty subject {}, no sender {}, no date {}, dated in the future {}, threads {}",
        count(&|e| e.is_unread()), count(&|e| e.flags.contains(Flags::FLAGGED)), count(&|e| !e.attachments.is_empty()), count(&|e| e.preview.is_empty()),
        count(&|e| e.subject.is_empty()), count(&|e| e.from.email.is_empty()), count(&|e| e.date == 0), count(&|e| e.date > now + 86_400), threads.len())
}

/// Watch the inbox for `seconds` and count what the server reports.
fn idle(backend: Arc<ImapBackend>, inbox: &FolderId, seconds: u64) -> u32 {
    let (events, received) = channel();
    let folder = inbox.clone();
    let watcher = std::thread::spawn(move || block_on(backend.idle(&folder, events)));
    line("push", format!("watching the inbox for {seconds} s (send yourself a message, or flag one elsewhere, to see events)"));
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let (mut new, mut flags, mut gone) = (0, 0, 0);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match received.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(IdleEvent::Exists(_)) => new += 1,
            Ok(IdleEvent::FlagsChanged(_)) => flags += 1,
            Ok(IdleEvent::Expunged(_)) => gone += 1,
            Err(_) if watcher.is_finished() => break,
            Err(_) => {}
        }
    }
    if watcher.is_finished() {
        return match watcher.join() {
            Ok(Err(error)) => { line("push", format!("FAIL the watch ended: {error}")); 1 }
            _ => { line("push", "FAIL the watch ended early"); 1 }
        };
    }
    // The watcher thread notices the dropped receiver at its next event; the process ends first.
    line("push", format!("watch held for {seconds} s; events: {new} new mail, {flags} flag changes, {gone} removals"));
    0
}

/// `--write`: create a folder of its own, work only inside it, delete it.
fn write(account: &Account, folders: &[Folder]) -> u32 {
    if folders.iter().any(|f| f.path.len() == 1 && f.name == CHECK_FOLDER) {
        line("write", format!("FAIL a folder named {CHECK_FOLDER} already exists; delete it (or rename it) and run again"));
        return 1;
    }
    let backend = account.backend();
    let started = Instant::now();
    let created = match account {
        Account::Imap(b) => block_on(b.create_folder(CHECK_FOLDER)),
        Account::Jmap(b) => block_on(b.create_folder(CHECK_FOLDER)),
    };
    let folder = match created {
        Ok(folder) => folder,
        Err(error) => { line("write", format!("FAIL could not create {CHECK_FOLDER}: {error}")); return 1; }
    };
    let outcome = write_inside(account, backend.as_ref(), &folder);
    let deleted = match account {
        Account::Imap(b) => block_on(b.delete_folder(&folder)),
        Account::Jmap(b) => block_on(b.delete_folder(&folder)),
    };
    match (outcome, deleted) {
        (Ok(report), Ok(())) => { line("write", format!("{report}; folder deleted, {} in all", ms(started.elapsed()))); 0 }
        (Err(problem), Ok(())) => { line("write", format!("FAIL {problem}; folder deleted")); 1 }
        (outcome, Err(error)) => {
            line("write", format!("FAIL could not delete {CHECK_FOLDER} ({error}); delete it by hand{}", outcome.err().map(|p| format!("; also: {p}")).unwrap_or_default()));
            1
        }
    }
}

fn write_inside(account: &Account, backend: &dyn MailBackend, folder: &FolderId) -> std::result::Result<String, String> {
    let message = |n: u32| format!("From: Ravenglass Check <check@example.invalid>\r\nTo: check@example.invalid\r\nSubject: Ravenglass check {n}\r\nDate: Mon, 23 Nov 2026 10:00:00 +0000\r\nMessage-ID: <ravenglass-check-{n}@example.invalid>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nA test message from ravenglass-check. It is deleted with its folder.\r\n");
    let append = |n: u32| match account {
        Account::Imap(b) => block_on(b.append_to(folder, message(n).as_bytes(), Flags::empty())),
        Account::Jmap(b) => block_on(b.append_to(folder, message(n).as_bytes(), Flags::empty())),
    };
    let (first, second) = (append(1).map_err(|e| format!("append: {e}"))?, append(2).map_err(|e| format!("append: {e}"))?);
    let (full, took) = timed(|| block_on(backend.sync_folder(folder, &SyncToken::default())));
    let full = full.map_err(|e| format!("sync: {e}"))?;
    if full.added.len() != 2 { return Err(format!("sync found {} of the 2 appended messages", full.added.len())); }
    let sync_time = took;
    block_on(backend.set_flags(std::slice::from_ref(&first), Flags::SEEN | Flags::FLAGGED, Flags::empty())).map_err(|e| format!("set flags: {e}"))?;
    let (changed, took) = timed(|| block_on(backend.sync_folder(folder, &full.token)));
    let changed = changed.map_err(|e| format!("incremental sync: {e}"))?;
    // IMAP reports a flag change in `changed_flags`, JMAP sends the changed message again.
    let seen_change = changed.changed_flags.iter().any(|(id, f)| *id == first && f.contains(Flags::FLAGGED))
        || changed.added.iter().any(|e| e.id == first && e.flags.contains(Flags::FLAGGED));
    if !seen_change { return Err("the incremental sync did not report the flag change".into()); }
    let raw = block_on(backend.fetch_body(&second)).map_err(|e| format!("fetch: {e}"))?;
    if raw.bytes != message(2).as_bytes() { return Err("the stored message came back different".into()); }
    Ok(format!("created {CHECK_FOLDER}, appended 2 messages, full sync {}, flagged one, incremental sync {} saw it, body round-trip ok", ms(sync_time), ms(took)))
}
