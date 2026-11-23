use super::*;
use mail_mock::{MockBackend, NOW, block_on};

/// A store filled from one mock account's folders.
fn seeded(account: &str, count: usize) -> (Store, AccountId) {
    let backend = MockBackend::new(account, 46, count);
    let mut store = Store::open_in_memory().unwrap();
    let id = backend.account().clone();
    let folders = block_on(backend.list_folders()).unwrap();
    store.upsert_folders(&folders).unwrap();
    for f in &folders {
        let delta = block_on(backend.sync_folder(&f.id, &SyncToken::default())).unwrap();
        store.apply_delta(&id, &f.id, &delta).unwrap();
    }
    (store, id)
}

fn inbox(account: &str) -> FolderId { FolderId(format!("{account}/Inbox")) }

#[test]
fn migrations_run_once_and_set_the_version() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), MIGRATIONS.len() as u32);
}

#[test]
fn delta_round_trips_envelopes_newest_first_with_local_counts() {
    let (store, acc) = seeded("iCloud", 140);
    let page = store.envelopes(&inbox("iCloud"), 0, 50).unwrap();
    assert_eq!(page.len(), 50);
    assert!(page.windows(2).all(|w| w[0].date >= w[1].date));
    let backend = MockBackend::new("iCloud", 46, 140);
    let original = block_on(backend.sync_folder(&inbox("iCloud"), &SyncToken::default())).unwrap().added;
    assert_eq!(store.envelope(&page[0].id).unwrap().as_ref(), original.iter().find(|e| e.id == page[0].id));
    let folders = store.folders(&acc).unwrap();
    assert_eq!(folders.iter().map(|f| f.total).sum::<u32>(), 140);
    assert_eq!(folders[0].unread, original.iter().filter(|e| e.is_unread()).count() as u32);
    assert_eq!(store.sync_token(&inbox("iCloud")).unwrap(), SyncToken("1".into()));
}

#[test]
fn flags_moves_and_removals_update_counts_and_search() {
    let (mut store, acc) = seeded("x", 30);
    let first = store.envelopes(&inbox("x"), 0, 1).unwrap().remove(0);
    let before = store.folders(&acc).unwrap()[0].clone();
    store.set_flags(std::slice::from_ref(&first.id), Flags::FLAGGED, Flags::SEEN).unwrap();
    let e = store.envelope(&first.id).unwrap().unwrap();
    assert!(e.flags.contains(Flags::FLAGGED) && e.is_unread());
    store.move_messages(std::slice::from_ref(&first.id), &FolderId("x/Archive".into())).unwrap();
    assert_eq!(store.folders(&acc).unwrap()[0].total, before.total - 1);
    let gone = FolderDelta { removed: vec![first.id.clone()], token: SyncToken("2".into()), ..Default::default() };
    store.apply_delta(&acc, &inbox("x"), &gone).unwrap();
    assert!(store.envelope(&first.id).unwrap().is_none());
    assert!(store.search(&Query::parse(&first.subject), None, 500).unwrap().iter().all(|e| e.id != first.id));
}

#[test]
fn search_operators_combine() {
    let (store, _) = seeded("iCloud", 140);
    let all = store.envelopes(&inbox("iCloud"), 0, 500).unwrap();
    let everything = store.search(&Query::default(), None, 500).unwrap();
    let expect = |f: &dyn Fn(&Envelope) -> bool| everything.iter().filter(|e| f(e)).count();
    let github = store.search(&Query::parse("from:github is:read"), None, 500).unwrap();
    assert!(!github.is_empty());
    assert_eq!(github.len(), expect(&|e| e.from.email.contains("github") && !e.is_unread()));
    let unread = store.search(&Query::parse("is:unread"), None, 500).unwrap();
    assert_eq!(unread.len(), expect(&|e| e.is_unread()));
    assert!(!unread.is_empty() && unread.iter().all(Envelope::is_unread));
    let with_files = store.search(&Query::parse("has:attachment in:inbox"), None, 500).unwrap();
    assert_eq!(with_files.len(), all.iter().filter(|e| !e.attachments.is_empty()).count());
    let prefix = store.search(&Query::parse("subject:invoi"), None, 500).unwrap();
    assert!(!prefix.is_empty() && prefix.iter().all(|e| e.subject.contains("invoice")));
    let phrase = store.search(&Query::parse("\"green chile\""), None, 500).unwrap();
    assert!(!phrase.is_empty() && phrase.iter().all(|e| e.preview.contains("green chile")));
    assert!(store.search(&Query::parse("\"chile green\""), None, 500).unwrap().is_empty(), "a phrase keeps its word order");
    assert!(store.search(&Query::parse("from:nobody-here"), None, 10).unwrap().is_empty());
}

#[test]
fn search_dates_and_hostile_input() {
    let (store, _) = seeded("iCloud", 140);
    assert_eq!(parse_date("1970-01-02"), Some(86_400));
    assert_eq!(parse_date("2026-11-13"), Some(NOW - NOW % 86_400));
    let today = store.search(&Query::parse("after:2026-11-13"), None, 500).unwrap();
    assert!(!today.is_empty() && today.iter().all(|e| e.date >= NOW - NOW % 86_400));
    let older = store.search(&Query::parse("before:2026-11-13"), None, 500).unwrap();
    assert_eq!(today.len() + older.len(), 140);
    // FTS5 syntax in user input is searched as text, never run as a query.
    for q in ["\" OR 1", "NEAR(a b)", "subject:* AND", "a:b:c", "before:tomorrow", "(", "*"] {
        store.search(&Query::parse(q), None, 10).unwrap();
    }
}

#[test]
fn query_tokens_mark_operators_for_highlighting() {
    let (q, tokens) = Query::parse_tokens("from:odin  dispatch is:flagged");
    assert_eq!(q, Query { from: vec!["odin".into()], text: vec!["dispatch".into()], flagged: Some(true), ..Default::default() });
    assert_eq!(tokens.iter().map(|t| t.operator).collect::<Vec<_>>(), [true, false, true]);
    assert_eq!(tokens[1], Token { start: 11, end: 19, operator: false });
}

#[test]
fn bodies_are_cached_indexed_and_evicted_least_recently_opened_first() {
    let (mut store, _) = seeded("x", 10);
    store.set_body_cache_bytes(250);
    let ids: Vec<MessageId> = store.envelopes(&inbox("x"), 0, 3).unwrap().into_iter().map(|e| e.id).collect();
    let raw = |i: usize| RawMessage { id: ids[i].clone(), bytes: vec![b'a'; 100] };
    store.put_body(&raw(0), "the northern pass zzyzx").unwrap();
    store.put_body(&raw(1), "").unwrap();
    assert!(store.body(&ids[0]).unwrap().is_some(), "opening 0 makes 1 the oldest");
    store.put_body(&raw(2), "").unwrap();
    assert!(store.body(&ids[1]).unwrap().is_none());
    assert!(store.body(&ids[0]).unwrap().is_some() && store.body(&ids[2]).unwrap().is_some());
    let found = store.search(&Query::parse("zzyzx"), None, 10).unwrap();
    assert_eq!(found.iter().map(|e| &e.id).collect::<Vec<_>>(), [&ids[0]]);
}

#[test]
fn journal_contacts_and_image_policy() {
    let (mut store, _) = seeded("iCloud", 140);
    let a = store.journal_push("{\"op\":1}").unwrap();
    let b = store.journal_push("{\"op\":2}").unwrap();
    store.journal_remove(a).unwrap();
    assert_eq!(store.journal_pending().unwrap(), vec![(b, "{\"op\":2}".to_owned())]);
    let d = DraftId::from("d1");
    store.put_draft(&d, "{\"v\":1}").unwrap();
    store.put_draft(&d, "{\"v\":2}").unwrap();
    assert_eq!(store.drafts().unwrap(), ["{\"v\":2}"]);
    store.delete_draft(&d).unwrap();
    assert!(store.drafts().unwrap().is_empty());
    let odin = store.complete_address("odi", 5).unwrap();
    assert_eq!(odin.first().map(|a| a.email.as_str()), Some("odin@asgard.example"));
    assert!(!store.images_allowed_from("News@Dioxus.example").unwrap());
    store.allow_images_from("News@Dioxus.example").unwrap();
    assert!(store.images_allowed_from("news@dioxus.example").unwrap());
}

/// Architecture §13 budget: 10k cached messages search in < 50 ms (release). Debug builds and
/// shared CI runners are slower, so the assertion allows 5× and prints the measurement.
#[test]
fn search_10k_messages_within_budget() {
    let (store, _) = seeded("big", 10_000);
    let queries = ["from:github is:read", "dispatch", "subject:invoice has:attachment", "odin northern after:2026-01-01"];
    let start = std::time::Instant::now();
    for q in queries { assert!(!store.search(&Query::parse(q), None, 200).unwrap().is_empty(), "{q}"); }
    let per_query = start.elapsed() / queries.len() as u32;
    eprintln!("search over 10k messages: {per_query:?} per query");
    assert!(per_query.as_millis() < 250, "{per_query:?}");
}

#[test]
fn removing_an_account_leaves_no_trace_of_its_mail_and_keeps_the_others() {
    let (mut store, keep) = seeded("iCloud", 30);
    let gone = AccountId::from("Gmail");
    let backend = MockBackend::new("Gmail", 7, 20);
    let folders = block_on(backend.list_folders()).unwrap();
    store.upsert_folders(&folders).unwrap();
    for f in &folders {
        let delta = block_on(backend.sync_folder(&f.id, &SyncToken::default())).unwrap();
        store.apply_delta(&gone, &f.id, &delta).unwrap();
    }
    let theirs = store.envelopes(&inbox("Gmail"), 0, 1).unwrap().remove(0);
    store.put_body(&RawMessage { id: theirs.id.clone(), bytes: b"raw".to_vec() }, "xyzzy body text").unwrap();
    store.put_draft(&DraftId::from("d1"), r#"{"id":"d1","account":"Gmail","subject":"s"}"#).unwrap();
    store.put_draft(&DraftId::from("d2"), r#"{"id":"d2","account":"iCloud","subject":"mentions Gmail"}"#).unwrap();
    store.journal_push(r#"{"Move":{"account":"Gmail","from":[],"to":"x"}}"#).unwrap();

    store.remove_account(&gone).unwrap();
    assert!(store.folders(&gone).unwrap().is_empty() && store.envelope(&theirs.id).unwrap().is_none());
    assert!(store.body(&theirs.id).unwrap().is_none() && store.search(&Query::parse("xyzzy"), None, 10).unwrap().is_empty());
    assert_eq!(store.search(&Query::default(), None, 500).unwrap().len(), 30, "the other account is untouched");
    assert_eq!(store.folders(&keep).unwrap().iter().map(|f| f.total).sum::<u32>(), 30);
    assert_eq!(store.drafts().unwrap().len(), 1, "only the removed account's draft goes");
    assert!(store.journal_pending().unwrap().is_empty());
}
