//! Unit tests for the pure pieces (no server), plus scripted-server tests in [`scripted`] for the
//! paths GreenMail cannot exercise.

mod scripted;

use crate::config::{Credential, ImapConfig};
use crate::envelope::{self, Fetched};
use crate::folders::{self, Listed};
use crate::ids::*;
use crate::idle::{self, Effect, Event, State};
use crate::net::{self, Failure};
use crate::smtp;
use async_imap::imap_proto::{AttributeValue, Response};
use async_imap::types::NameAttribute;
use mail_core::*;

// ── ids and tokens ───────────────────────────────────────────────────────────────────────────

#[test]
fn folder_ids_carry_the_account_and_the_exact_mailbox_name() {
    let account = AccountId::from("Work");
    let id = folder_id(&account, "INBOX.Sent Items");
    assert_eq!(id, FolderId("Work/INBOX.Sent Items".into()));
    assert_eq!(mailbox_of(&account, &id), Ok("INBOX.Sent Items"));
    // Another account's folder, a prefix that only looks like the account, and an empty name.
    for foreign in ["Home/INBOX", "Workshop/INBOX", "Work/", "Work"] {
        assert!(matches!(mailbox_of(&account, &FolderId(foreign.into())), Err(MailError::NotFound(_))), "{foreign}");
    }
}

#[test]
fn message_ids_round_trip_even_with_semicolons_in_the_mailbox() {
    let folder = FolderId("Work/Odd;Name;7".into());
    let id = message_id(&folder, 1_795_407_300, 42);
    assert_eq!(id.0, "Work/Odd;Name;7;1795407300;42");
    assert_eq!(parse_message_id(&id), Ok(MessageRef { folder, validity: 1_795_407_300, uid: 42 }));
}

#[test]
fn ids_from_elsewhere_are_not_found() {
    for bad in ["", "iCloud/00012", "Work/INBOX;1", "Work/INBOX;x;1", "Work/INBOX;1;0", ";1;1", "Work/INBOX;1;-3", "Work/INBOX;1;99999999999"] {
        assert!(matches!(parse_message_id(&MessageId(bad.into())), Err(MailError::NotFound(_))), "{bad}");
    }
}

#[test]
fn tokens_round_trip_and_garbage_means_a_full_sync() {
    let state = SyncState { validity: 7, uid_next: 120, modseq: 9_000_000_000, uids: vec![1, 2, 3, 5, 9, 10, 119] };
    let token = state.encode();
    assert_eq!(token.0, "1;7;120;9000000000;1:3,5,9:10,119");
    assert_eq!(SyncState::decode(&token), Some(state));
    let empty = SyncState { validity: 7, uid_next: 1, modseq: 0, uids: vec![] };
    assert_eq!(SyncState::decode(&empty.encode()), Some(empty));
    for bad in ["", "1", "2;7;120;0;1:3", "1;7;120", "1;x;120;0;", "1;7;120;0;3:1", "1;7;120;0;0", "1;7;120;0;1:4000000000", "1;7;120;0;1:120", "1;7;1;0;5", "the mock's token"] {
        assert_eq!(SyncState::decode(&SyncToken(bad.into())), None, "{bad}");
    }
}

#[test]
fn uid_sets_compress_expand_and_chunk() {
    assert_eq!(compress(&[]), "");
    assert_eq!(compress(&[4]), "4");
    assert_eq!(compress(&[1, 2, 3, 7, 9, 10]), "1:3,7,9:10");
    assert_eq!(compress(&[u32::MAX - 1, u32::MAX]), format!("{}:{}", u32::MAX - 1, u32::MAX));
    assert_eq!(expand("1:3,7,9:10"), Some(vec![1, 2, 3, 7, 9, 10]));
    assert_eq!(expand("5,1:2,2"), Some(vec![1, 2, 5]), "sorted, no duplicates");
    assert_eq!(expand(""), Some(vec![]));
    assert_eq!(expand("1:*"), None);
    let many: Vec<u32> = (1..=1200).rev().map(|n| n * 2).collect();
    let sets = chunked_sets(&many);
    assert_eq!(sets.len(), 3);
    assert!(sets[0].starts_with("2,4,6,"));
    assert_eq!(sets.iter().map(|s| expand(s).unwrap().len()).sum::<usize>(), 1200);
    assert_eq!(chunked_sets(&[3, 1, 2, 3]), ["1:3"]);
}

// ── folders ──────────────────────────────────────────────────────────────────────────────────

fn plain(mailbox: &str, delimiter: &str) -> Listed { Listed { mailbox: mailbox.into(), delimiter: Some(delimiter.into()), special_use: None } }

#[test]
fn well_known_names_map_to_roles() {
    use FolderRole::*;
    for (name, role) in [("INBOX", Inbox), ("Drafts", Drafts), ("Sent Items", Sent), ("Sent Mail", Sent), ("Deleted Items", Trash), ("Bin", Trash),
        ("Spam", Junk), ("Junk E-mail", Junk), ("Archives", Archive), ("All Mail", Archive), ("Starred", Flagged), ("Receipts", Other), ("Sentinel", Other)] {
        assert_eq!(folders::role_from_name(name), role, "{name}");
    }
}

#[test]
fn special_use_attributes_beat_names() {
    let attrs = |a: NameAttribute<'static>| vec![NameAttribute::Extension("\\HasNoChildren".into()), a];
    let listed = vec![
        folders::listed("INBOX", Some("/"), &[]).unwrap(),
        // A user folder that happens to be called Sent, listed before the real one.
        folders::listed("Sent", Some("/"), &[]).unwrap(),
        folders::listed("[Gmail]/Sent Mail", Some("/"), &attrs(NameAttribute::Sent)).unwrap(),
        folders::listed("[Gmail]/Drafts", Some("/"), &attrs(NameAttribute::Drafts)).unwrap(),
        folders::listed("[Gmail]/All Mail", Some("/"), &attrs(NameAttribute::All)).unwrap(),
        folders::listed("[Gmail]/Bin", Some("/"), &attrs(NameAttribute::Trash)).unwrap(),
        folders::listed("[Gmail]/Spam", Some("/"), &attrs(NameAttribute::Junk)).unwrap(),
        folders::listed("[Gmail]/Starred", Some("/"), &attrs(NameAttribute::Flagged)).unwrap(),
        folders::listed("Archive", Some("/"), &[]).unwrap(),
    ];
    // The `[Gmail]` container cannot be selected, so it is not a folder at all.
    assert_eq!(folders::listed("[Gmail]", Some("/"), &[NameAttribute::NoSelect]), None);
    use FolderRole::*;
    assert_eq!(folders::assign_roles(&listed), [Inbox, Other, Sent, Drafts, Archive, Trash, Junk, Flagged, Other]);
}

#[test]
fn each_role_goes_to_one_folder_and_top_level_names_win() {
    use FolderRole::*;
    let listed = vec![plain("Projects.Archive", "."), plain("INBOX", "."), plain("INBOX.Trash", "."), plain("Archive", "."), plain("Trash", "."), plain("inbox.old", ".")];
    // `INBOX.Trash` counts as top level (the INBOX namespace) and comes first; the nested Archive loses to the top-level one.
    assert_eq!(folders::assign_roles(&listed), [Other, Inbox, Trash, Archive, Other, Other]);
    // With nothing better, a nested name is used.
    assert_eq!(folders::assign_roles(&[plain("INBOX", "."), plain("Projects.Drafts", ".")]), [Inbox, Drafts]);
}

#[test]
fn folders_get_decoded_names_and_paths() {
    let account = AccountId::from("Work");
    let folder = folders::to_folder(&account, &plain("Receipts/2026/Entw&APw-rfe", "/"), FolderRole::Other);
    assert_eq!(folder.id, FolderId("Work/Receipts/2026/Entw&APw-rfe".into()), "the id keeps the server's spelling");
    assert_eq!((folder.name.as_str(), folder.path), ("Entwürfe", vec!["Receipts".to_owned(), "2026".to_owned(), "Entwürfe".to_owned()]));
    assert_eq!(folders::to_folder(&account, &plain("INBOX", "/"), FolderRole::Inbox).name, "Inbox");
    let flat = Listed { mailbox: "a/b".into(), delimiter: None, special_use: None };
    assert_eq!(folders::to_folder(&account, &flat, FolderRole::Other).path, ["a/b"]);
}

#[test]
fn modified_utf7_decodes() {
    for (raw, text) in [("INBOX", "INBOX"), ("Entw&APw-rfe", "Entwürfe"), ("&ZeVnLIqe-", "日本語"), ("Tom &- Jerry", "Tom & Jerry"), ("&2D3eAA-", "😀"), ("~peter/mail/&U,BTFw-/&ZeVnLIqe-", "~peter/mail/台北/日本語")] {
        assert_eq!(folders::decode_utf7(raw), text, "{raw}");
    }
    // Malformed input comes back untouched rather than half-decoded.
    for raw in ["R&D", "&!!!-", "&AA-", "&2D0-"] { assert_eq!(folders::decode_utf7(raw), raw); }
}

#[test]
fn new_folders_go_beside_the_existing_ones() {
    assert_eq!(folders::name_for_new(&[plain("INBOX", "."), plain("INBOX.Sent", "."), plain("INBOX.Trash", ".")], "Drafts"), "INBOX.Drafts");
    assert_eq!(folders::name_for_new(&[plain("INBOX", "/"), plain("Sent", "/")], "Drafts"), "Drafts");
    assert_eq!(folders::name_for_new(&[plain("INBOX", ".")], "Drafts"), "Drafts", "an account with only an inbox says nothing about a namespace");
    assert_eq!(folders::name_for_new(&[], "Sent"), "Sent");
}

// ── envelopes ────────────────────────────────────────────────────────────────────────────────

fn build(header: &str, text: &[u8]) -> Envelope {
    let folder = FolderId("Work/INBOX".into());
    let fetched = Fetched { header: header.as_bytes(), text, flags: Flags::SEEN, received: Some(1_700_000_000), attachments: vec![] };
    envelope::build(message_id(&folder, 1, 5), folder, fetched)
}

#[test]
fn an_envelope_is_built_from_headers_and_a_body_snippet() {
    let header = "From: =?utf-8?q?J=C3=BCrgen?= <j@example.org>\r\nTo: Ann <ann@example.org>, bob@example.org\r\nCc: \"Carol, C.\" <carol@example.org>\r\nSubject: =?utf-8?B?R3LDvMOfZQ==?= aus Wien\r\nDate: Mon, 23 Nov 2026 12:30:00 +0100\r\nMessage-ID: <c@example.org>\r\nIn-Reply-To: <b@example.org>\r\nReferences: <a@example.org>\r\n <b@example.org>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\n";
    let envelope = build(header, b"Sch=C3=B6ne Gr=C3=BC=C3=9Fe,\r\n\r\n   und   bis\tbald.\r\n");
    assert_eq!(envelope.id, MessageId("Work/INBOX;1;5".into()));
    assert_eq!(envelope.from, Address::new("Jürgen", "j@example.org"));
    assert_eq!(envelope.to, vec![Address::new("Ann", "ann@example.org"), Address { name: None, email: "bob@example.org".into() }]);
    assert_eq!(envelope.cc, vec![Address::new("Carol, C.", "carol@example.org")]);
    assert_eq!(envelope.subject, "Grüße aus Wien");
    assert_eq!(envelope.date, 1_795_433_400);
    assert_eq!(envelope.thread, ThreadId("a@example.org".into()), "the first reference is the thread root");
    assert_eq!(envelope.preview, "Schöne Grüße, und bis bald.");
    assert_eq!(envelope.flags, Flags::SEEN);
}

#[test]
fn threads_fall_back_from_references_to_reply_to_to_the_message_itself() {
    let thread = |extra: &str| build(&format!("From: a@example.org\r\n{extra}\r\n"), b"x").thread.0;
    assert_eq!(thread("Message-ID: <c@x>\r\nIn-Reply-To: <b@x>\r\n"), "b@x");
    assert_eq!(thread("Message-ID: <c@x>\r\n"), "c@x");
    assert_eq!(thread(""), "Work/INBOX;1;5", "no Message-ID: a thread of its own");
}

#[test]
fn a_bare_message_still_makes_an_envelope() {
    let envelope = build("", b"");
    assert_eq!((envelope.from.email.as_str(), envelope.subject.as_str(), envelope.preview.as_str()), ("", "", ""));
    assert_eq!(envelope.date, 1_700_000_000, "INTERNALDATE stands in for a missing Date");
    assert!(envelope.to.is_empty());
}

#[test]
fn previews_survive_a_cut_multipart_html_or_base64_body() {
    let multipart = "From: a@example.org\r\nMIME-Version: 1.0\r\nContent-Type: multipart/alternative; boundary=\"xyz\"\r\n\r\n";
    // Cut in the middle of the second part: no closing boundary.
    let cut = b"This is a multi-part message in MIME format.\r\n--xyz\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThe plain part.\r\n--xyz\r\nContent-Type: text/html\r\n\r\n<html><body><p>The <b>HTML";
    assert_eq!(build(multipart, cut).preview, "The plain part.");

    let html = "From: a@example.org\r\nMIME-Version: 1.0\r\nContent-Type: text/html; charset=utf-8\r\n\r\n";
    assert_eq!(build(html, b"<html><head><style>p { color: red }</style></head><body><p>Hello&nbsp;<b>world</b></p><p>Second").preview, "Hello world Second");

    // Base64 cut inside a multi-byte character: the broken tail is dropped, not shown as U+FFFD.
    let base64 = "From: a@example.org\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n";
    let preview = build(base64, b"R3LDvMOfZSBhdXMgV2llbiDDvA").preview;
    assert!(preview.starts_with("Grüße aus Wien") && !preview.contains('\u{FFFD}'), "{preview:?}");
}

#[test]
fn previews_are_one_line_of_at_most_200_characters() {
    assert_eq!(envelope::preview("  a\r\n\r\n\tb   c\u{0}d  "), "a b c d");
    let long = "größer ".repeat(100);
    let preview = envelope::preview(&long);
    assert_eq!(preview.chars().count(), 200);
    assert!(long.starts_with(&preview));
    assert_eq!(envelope::preview(""), "");
}

#[test]
fn flags_map_both_ways() {
    assert_eq!(envelope::flag_list(Flags::SEEN | Flags::DELETED), Some("(\\Seen \\Deleted)".into()));
    assert_eq!(envelope::flag_list(Flags(0xFF)), Some("(\\Seen \\Flagged \\Answered \\Draft \\Deleted)".into()));
    assert_eq!(envelope::flag_list(Flags::empty()), None);
    assert_eq!(envelope::flag_list(Flags(0xE0)), None, "bits IMAP has no flag for");
    assert_eq!(envelope::flags_from(["\\Seen", "\\Recent", "$Forwarded", "\\DRAFT"]), Flags::SEEN | Flags::DRAFT);
    assert_eq!(envelope::flags_from([]), Flags::empty());
}

#[test]
fn internal_dates_become_timestamps() {
    assert_eq!(envelope::internal_date("23-Nov-2026 04:15:00 +0000"), Some(1_795_407_300));
    assert_eq!(envelope::internal_date("23-Nov-2026 05:15:00 +0100"), Some(1_795_407_300));
    assert_eq!(envelope::internal_date("22-nov-2026 19:45:00 -0830"), Some(1_795_407_300));
    assert_eq!(envelope::internal_date(" 1-Jan-1970 00:00:00 +0000"), Some(0));
    assert_eq!(envelope::internal_date("29-Feb-2024 12:00:00 +0000"), Some(1_709_208_000));
    for bad in ["", "23-Nov-2026", "23-Foo-2026 04:15:00 +0000", "23-Nov-2026 04:15 +0000", "23-Nov-2026 25:15:00 +0000", "23-Nov-2026 04:15:00 UTC"] {
        assert_eq!(envelope::internal_date(bad), None, "{bad}");
    }
}

fn structure_attachments(bodystructure: &str) -> Vec<Attachment> {
    let line = format!("* 1 FETCH (BODYSTRUCTURE {bodystructure})\r\n");
    let (_, response) = Response::parse(line.as_bytes()).expect("a valid FETCH response");
    let Response::Fetch(_, attributes) = response else { panic!("not a FETCH") };
    attributes.iter().find_map(|a| match a { AttributeValue::BodyStructure(b) => Some(envelope::attachments(b)), _ => None }).unwrap()
}

#[test]
fn attachments_come_from_the_body_structure() {
    // text + html alternative, an inline image the HTML shows, a PDF, and an unnamed binary part.
    let found = structure_attachments(concat!(
        "(((\"text\" \"plain\" (\"charset\" \"utf-8\") NIL NIL \"7bit\" 12 1 NIL NIL NIL)",
        "(\"text\" \"html\" (\"charset\" \"utf-8\") NIL NIL \"7bit\" 40 1 NIL NIL NIL) \"alternative\" (\"boundary\" \"b2\") NIL NIL)",
        "(\"image\" \"png\" (\"name\" \"logo.png\") \"<logo@x>\" NIL \"base64\" 400 NIL (\"inline\" (\"filename\" \"logo.png\")) NIL)",
        "(\"application\" \"pdf\" (\"name\" \"=?utf-8?B?R3LDvMOfZS5wZGY=?=\") NIL NIL \"base64\" 2800000 NIL (\"attachment\" (\"filename\" \"=?utf-8?B?R3LDvMOfZS5wZGY=?=\")) NIL)",
        "(\"application\" \"octet-stream\" NIL NIL NIL \"8bit\" 99 NIL NIL NIL)",
        " \"mixed\" (\"boundary\" \"b1\") NIL NIL)"));
    assert_eq!(found, vec![
        Attachment { name: "Grüße.pdf".into(), mime: "application/pdf".into(), size: 2_100_000, content_id: None },
        Attachment { name: "attachment".into(), mime: "application/octet-stream".into(), size: 99, content_id: None },
    ]);
    // A plain text message has none; a text part sent as a named file is one.
    assert!(structure_attachments("(\"text\" \"plain\" (\"charset\" \"utf-8\") NIL NIL \"7bit\" 12 1 NIL NIL NIL)").is_empty());
    let named = structure_attachments("(\"text\" \"csv\" (\"name\" \"data.csv\") NIL NIL \"7bit\" 12 1 NIL NIL NIL)");
    assert_eq!(named[0].name, "data.csv");
}

// ── errors, TLS setup, configuration ─────────────────────────────────────────────────────────

#[test]
fn server_text_is_shortened_and_loses_addresses() {
    assert_eq!(net::server_text("code: None, info: Some(\"LOGIN failed. Invalid login/password for user id odin@asgard.example\")"), "LOGIN failed. Invalid login/password for user id <address>");
    assert_eq!(net::server_text("550 5.1.1 <muninn@ravens.example>: Recipient address rejected"), "550 5.1.1 <address> Recipient address rejected");
    assert_eq!(net::server_text(""), "request refused");
    assert_eq!(net::server_text(&"x".repeat(1000)).len(), 160);
}

#[test]
fn an_unparseable_response_never_reaches_the_error_text() {
    // async-imap puts the offending bytes (possibly mail) into an `Other` I/O error.
    let leaky = std::io::Error::other("Error(...) during parsing of \"* 1 FETCH (BODY[] {5}\\r\\nDear Odin, the secret is\"");
    let failure = Failure::from(async_imap::error::Error::Io(leaky));
    assert_eq!(failure.error, MailError::Server("unreadable server response".into()));
    assert!(!failure.reusable);
    let reset = Failure::from(async_imap::error::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionReset)));
    assert_eq!((reset.error, reset.reusable), (MailError::Network("connection reset".into()), false));
    let refused = Failure::from(async_imap::error::Error::No("code: None, info: Some(\"[CANNOT] no\")".into()));
    assert_eq!((refused.error, refused.reusable), (MailError::Server("[CANNOT] no".into()), true));
}

#[test]
fn pem_certificates_are_read_and_bad_roots_are_refused() {
    let pem = "junk\n-----BEGIN CERTIFICATE-----\nAQID\nBAU=\n-----END CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n-----BEGIN CERTIFICATE-----\n!!\n-----END CERTIFICATE-----\n";
    assert_eq!(net::certificates_from_pem(pem), vec![vec![1, 2, 3, 4, 5]]);
    assert!(net::tls_config(&[]).is_ok());
    assert!(matches!(net::tls_config(&[vec![1, 2, 3]]), Err(MailError::Server(_))));
}

#[test]
fn debug_output_hides_the_login() {
    let config = ImapConfig::new("Work", "imap.example.org", "smtp.example.org", "odin@asgard.example", Credential::Password("hunter2".into()));
    let printed = format!("{config:?} {:?}", Credential::OAuth2 { access_token: "ya29.secret".into() });
    assert!(printed.contains("imap.example.org") && printed.contains("993"));
    for secret in ["odin", "hunter2", "ya29"] { assert!(!printed.contains(secret), "{printed}"); }
}

#[test]
fn dropping_a_bridged_future_cancels_its_task() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Flag(std::sync::Arc<AtomicBool>);
    impl Drop for Flag {
        fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); }
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    let dropped = std::sync::Arc::new(AtomicBool::new(false));
    let held = Flag(dropped.clone());
    let task = runtime.spawn(async move { let _held = held; std::future::pending::<Result<()>>().await });
    let bridged = crate::backend::Bridged(task);
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!dropped.load(Ordering::SeqCst), "still running while someone waits");
    drop(bridged);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !dropped.load(Ordering::SeqCst) {
        assert!(std::time::Instant::now() < deadline, "the task outlived its future");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

// ── SMTP wire format ─────────────────────────────────────────────────────────────────────────

#[test]
fn bcc_is_removed_from_the_header_only() {
    let raw = b"From: a@x\r\nBcc: hidden@x,\r\n\tother@x\r\nTo: b@x\r\nbcc: third@x\r\nSubject: s\r\n continued\r\n\r\nBcc: this line is body text\r\n";
    assert_eq!(String::from_utf8(smtp::without_bcc(raw)).unwrap(), "From: a@x\r\nTo: b@x\r\nSubject: s\r\n continued\r\n\r\nBcc: this line is body text\r\n");
    assert_eq!(smtp::without_bcc(b"Bcc: only@x\r\n\r\nbody"), b"\r\nbody");
}

#[test]
fn data_is_dot_stuffed_with_crlf_line_ends() {
    assert_eq!(String::from_utf8(smtp::wire_format(b"Subject: s\r\n\r\n.hidden\r\n..\nbare\nlast")).unwrap(), "Subject: s\r\n\r\n..hidden\r\n...\r\nbare\r\nlast\r\n.\r\n");
    assert_eq!(smtp::wire_format(b""), b".\r\n");
}

// ── the IDLE state machine ───────────────────────────────────────────────────────────────────

#[test]
fn a_watch_connects_waits_and_reissues() {
    let (mut state, effect) = idle::initial();
    assert_eq!((state, effect), (State::Connecting { resumed: false }, Effect::Connect));
    assert_eq!(idle::step(&mut state, Event::Connected), Effect::Wait);
    assert_eq!(idle::step(&mut state, Event::Seen), Effect::Wait);
    assert_eq!(idle::step(&mut state, Event::Timer), Effect::Reissue);
    assert_eq!(state, State::Watching);
    assert_eq!(idle::step(&mut state, Event::ReceiverGone), Effect::Finish(Ok(())));
    assert_eq!(state, State::Done);
    assert_eq!(idle::step(&mut state, Event::Timer), Effect::Finish(Ok(())));
}

#[test]
fn a_lost_watch_reconnects_once_and_catches_up() {
    let lost = || Event::Failed(MailError::Network("connection lost".into()));
    let mut state = State::Watching;
    assert_eq!(idle::step(&mut state, lost()), Effect::Connect);
    assert_eq!(state, State::Connecting { resumed: true });
    assert_eq!(idle::step(&mut state, Event::Connected), Effect::CatchUp);
    // Lost again and this time the server cannot be reached: the watch ends with the error.
    assert_eq!(idle::step(&mut state, lost()), Effect::Connect);
    assert_eq!(idle::step(&mut state, lost()), Effect::Finish(Err(MailError::Network("connection lost".into()))));
    assert_eq!(state, State::Done);
}

#[test]
fn a_watch_that_cannot_start_or_is_refused_ends_with_the_error() {
    let (mut state, _) = idle::initial();
    assert_eq!(idle::step(&mut state, Event::Failed(MailError::Auth)), Effect::Finish(Err(MailError::Auth)));
    let mut state = State::Watching;
    assert_eq!(idle::step(&mut state, Event::Failed(MailError::Server("BAD".into()))), Effect::Finish(Err(MailError::Server("BAD".into()))));
}

#[test]
fn untagged_responses_become_idle_events() {
    let folder = FolderId("Work/INBOX".into());
    let classify = |line: &str| idle::classify(&Response::parse(line.as_bytes()).unwrap().1, &folder);
    assert_eq!(classify("* 23 EXISTS\r\n"), Some(IdleEvent::Exists(folder.clone())));
    assert_eq!(classify("* 3 EXPUNGE\r\n"), Some(IdleEvent::Expunged(folder.clone())));
    assert_eq!(classify("* 4 FETCH (FLAGS (\\Seen))\r\n"), Some(IdleEvent::FlagsChanged(folder.clone())));
    assert_eq!(classify("* 1 RECENT\r\n"), None);
    assert_eq!(classify("* OK Still here\r\n"), None);
}
