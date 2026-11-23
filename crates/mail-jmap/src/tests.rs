//! Tests against an in-process JMAP server. It implements the same reading of RFC 8620/8621 as
//! the backend, so these check the backend's logic, not interoperability with a real server.

use super::*;
use std::collections::BTreeMap;

const SESSION: &str = r#"{"apiUrl":"https://jmap.example/api","downloadUrl":"https://jmap.example/dl/{accountId}/{blobId}/{name}?type={type}","uploadUrl":"https://jmap.example/up/{accountId}/","primaryAccounts":{"urn:ietf:params:jmap:mail":"u1"},"state":"s"}"#;

#[derive(Default)]
struct Server {
    emails: BTreeMap<String, Value>,
    blobs: BTreeMap<String, Vec<u8>>,
    /// `(state after the change, kind, email id)`.
    log: Vec<(u64, &'static str, String)>,
    state: u64,
    submissions: Vec<Value>,
    requests: u32,
}

impl Server {
    fn touch(&mut self, kind: &'static str, id: &str) { self.state += 1; self.log.push((self.state, kind, id.to_owned())); }

    fn deliver(&mut self, id: &str, mailbox: &str, subject: &str, minute: u32, seen: bool) {
        let blob = format!("blob-{id}");
        self.blobs.insert(blob.clone(), format!("Subject: {subject}\r\n\r\nbody of {id}").into_bytes());
        let keywords = if seen { json!({ "$seen": true }) } else { json!({}) };
        self.emails.insert(id.to_owned(), json!({ "id": id, "blobId": blob, "threadId": format!("t-{id}"), "mailboxIds": { mailbox: true }, "keywords": keywords,
            "from": [{ "name": "Odin", "email": "odin@asgard.example" }], "to": [{ "name": null, "email": "me@example.com" }], "cc": [],
            "subject": subject, "receivedAt": format!("2026-11-13T16:{minute:02}:00Z"), "preview": format!("preview {id}") }));
        self.touch("created", id);
    }

    fn method(&mut self, name: &str, args: &Value) -> (String, Value) {
        let ok = |v: Value| (name.to_owned(), v);
        let strings = |v: &Value| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default();
        match name {
            "Mailbox/get" => {
                let count = |mb: &str, unread: bool| self.emails.values().filter(|e| e["mailboxIds"][mb] == true && (!unread || e["keywords"]["$seen"] != true)).count();
                let mb = |id: &str, name: &str, role: Value, parent: Value| json!({ "id": id, "name": name, "role": role, "parentId": parent, "totalEmails": count(id, false), "unreadEmails": count(id, true) });
                ok(json!({ "list": [mb("in", "Inbox", json!("inbox"), Value::Null), mb("dr", "Drafts", json!("drafts"), Value::Null), mb("se", "Sent", json!("sent"), Value::Null),
                    mb("ar", "Archive", json!("archive"), Value::Null), mb("r26", "2026", Value::Null, json!("ar"))] }))
            }
            "Email/query" => {
                let mailbox = args["filter"]["inMailbox"].as_str().unwrap_or_default();
                let mut found: Vec<&Value> = self.emails.values().filter(|e| e["mailboxIds"][mailbox] == true).collect();
                found.sort_by(|a, b| b["receivedAt"].as_str().cmp(&a["receivedAt"].as_str()));
                let (position, limit) = (args["position"].as_u64().unwrap_or(0) as usize, args["limit"].as_u64().unwrap_or(u64::MAX) as usize);
                ok(json!({ "ids": found.iter().skip(position).take(limit).map(|e| e["id"].clone()).collect::<Vec<_>>() }))
            }
            "Email/get" => {
                let properties = strings(&args["properties"]);
                let list: Vec<Value> = strings(&args["ids"]).iter().filter_map(|id| self.emails.get(id)).map(|e| {
                    Value::Object(e.as_object().unwrap().iter().filter(|(k, _)| *k == "id" || properties.contains(k)).map(|(k, v)| (k.clone(), v.clone())).collect())
                }).collect();
                ok(json!({ "list": list, "state": self.state.to_string() }))
            }
            "Email/changes" => {
                let Some(since) = args["sinceState"].as_str().and_then(|s| s.parse::<u64>().ok()) else { return ("error".into(), json!({ "type": "cannotCalculateChanges" })) };
                let max = args["maxChanges"].as_u64().unwrap_or(u64::MAX) as usize;
                let pending: Vec<_> = self.log.iter().filter(|(s, _, _)| *s > since).collect();
                let page = &pending[..pending.len().min(max.min(2))]; // tiny pages, to exercise hasMoreChanges
                let of = |kind: &str| page.iter().filter(|(_, k, _)| *k == kind).map(|(_, _, id)| id.clone()).collect::<Vec<_>>();
                ok(json!({ "created": of("created"), "updated": of("updated"), "destroyed": of("destroyed"), "newState": page.last().map_or(since, |(s, _, _)| *s).to_string(), "hasMoreChanges": page.len() < pending.len() }))
            }
            "Email/set" => {
                let mut not_updated = serde_json::Map::new();
                for (id, patch) in args["update"].as_object().cloned().unwrap_or_default() {
                    let Some(email) = self.emails.get_mut(&id) else { not_updated.insert(id, json!({ "type": "notFound" })); continue };
                    for (path, value) in patch.as_object().cloned().unwrap_or_default() {
                        match path.strip_prefix("keywords/") {
                            Some(keyword) if value.is_null() => { email["keywords"].as_object_mut().unwrap().remove(keyword); }
                            Some(keyword) => email["keywords"][keyword] = value,
                            None => email[path.as_str()] = value,
                        }
                    }
                    self.touch("updated", &id);
                }
                ok(json!({ "notUpdated": not_updated }))
            }
            "Email/import" => {
                let new = &args["emails"]["new"];
                let Some(raw) = new["blobId"].as_str().and_then(|b| self.blobs.get(b)).cloned() else { return ok(json!({ "notCreated": { "new": { "type": "blobNotFound" } } })) };
                let id = format!("imp{}", self.emails.len());
                let subject = String::from_utf8_lossy(&raw).lines().find_map(|l| l.strip_prefix("Subject: ").map(str::to_owned)).unwrap_or_default();
                self.emails.insert(id.clone(), json!({ "id": id, "blobId": new["blobId"], "threadId": "t", "mailboxIds": new["mailboxIds"], "keywords": new["keywords"], "from": [], "to": [], "cc": [], "subject": subject, "receivedAt": "2026-11-13T17:00:00Z", "preview": "" }));
                self.touch("created", &id);
                ok(json!({ "created": { "new": { "id": id } } }))
            }
            "Identity/get" => ok(json!({ "list": [{ "id": "id-other", "email": "other@example.com" }, { "id": "id-me", "email": "me@example.com" }] })),
            "EmailSubmission/set" => { self.submissions.push(args["create"]["send"].clone()); ok(json!({ "created": { "send": { "id": "sub1" } } })) }
            _ => ("error".into(), json!({ "type": "unknownMethod" })),
        }
    }
}

struct Fake { server: Mutex<Server>, session: &'static str }

impl Fake {
    fn new() -> Self {
        let mut s = Server::default();
        for i in 0..5 { s.deliver(&format!("m{i}"), "in", &format!("Subject {i}"), i, i >= 2); }
        s.deliver("a0", "ar", "Archived", 30, true);
        Self { server: Mutex::new(s), session: SESSION }
    }
}

impl JmapHttp for &Fake {
    fn get<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            if url == "https://jmap.example/session" { return Ok(self.session.as_bytes().to_vec()); }
            let blob = url.strip_prefix("https://jmap.example/dl/u1/").and_then(|r| r.split('/').next()).unwrap_or_default();
            self.server.lock().unwrap().blobs.get(blob).cloned().ok_or_else(|| MailError::NotFound(blob.to_owned()))
        })
    }

    fn post<'a>(&'a self, url: &'a str, _content_type: &'a str, body: Vec<u8>) -> BoxFut<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let mut server = self.server.lock().unwrap();
            server.requests += 1;
            if url == "https://jmap.example/up/u1/" {
                let id = format!("up{}", server.blobs.len());
                server.blobs.insert(id.clone(), body);
                return Ok(serde_json::to_vec(&json!({ "blobId": id })).unwrap());
            }
            assert_eq!(url, "https://jmap.example/api");
            let request: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["using"].as_array().unwrap().len(), 3);
            let responses: Vec<Value> = request["methodCalls"].as_array().unwrap().iter().map(|call| {
                let (name, result) = server.method(call[0].as_str().unwrap(), &call[1]);
                json!([name, result, call[2]])
            }).collect();
            Ok(serde_json::to_vec(&json!({ "methodResponses": responses })).unwrap())
        })
    }
}

fn backend(fake: &Fake) -> JmapBackend<&Fake> { JmapBackend::new(AccountId::from("fastmail"), "https://jmap.example/session", fake) }

fn ids(list: &[Envelope]) -> Vec<&str> { list.iter().map(|e| e.id.0.as_str()).collect() }

#[test]
fn folders_come_with_roles_paths_and_counts() {
    let fake = Fake::new();
    let folders = block_on(backend(&fake).list_folders()).unwrap();
    assert_eq!(folders.iter().map(|f| f.role).collect::<Vec<_>>(), [FolderRole::Inbox, FolderRole::Drafts, FolderRole::Sent, FolderRole::Archive, FolderRole::Other]);
    assert_eq!((folders[0].total, folders[0].unread), (5, 2));
    assert_eq!(folders[4].path, ["Archive", "2026"]);
}

#[test]
fn full_sync_then_changes_report_new_changed_moved_and_destroyed_mail() {
    let fake = Fake::new();
    let b = backend(&fake);
    let inbox = FolderId::from("in");
    let first = block_on(b.sync_folder(&inbox, &SyncToken::default())).unwrap();
    assert_eq!(ids(&first.added), ["m4", "m3", "m2", "m1", "m0"], "newest first");
    let e = &first.added[4];
    assert_eq!((e.subject.as_str(), e.from.display(), e.is_unread(), e.date), ("Subject 0", "Odin", true, 1_794_588_240 - 44 * 60));
    assert!(block_on(b.sync_folder(&inbox, &first.token)).unwrap().added.is_empty(), "nothing changed");

    {
        let mut s = fake.server.lock().unwrap();
        s.deliver("m9", "in", "Fresh", 50, false);
        s.emails.get_mut("m0").unwrap()["keywords"]["$flagged"] = json!(true);
        s.touch("updated", "m0");
        s.emails.get_mut("m1").unwrap()["mailboxIds"] = json!({ "ar": true });
        s.touch("updated", "m1");
        s.emails.remove("m2");
        s.touch("destroyed", "m2");
    }
    let delta = block_on(b.sync_folder(&inbox, &first.token)).unwrap();
    assert_eq!(ids(&delta.added), ["m9", "m0"]);
    assert!(delta.added[1].flags.contains(Flags::FLAGGED));
    assert_eq!(delta.removed, [MessageId::from("m1"), MessageId::from("m2")]);
    assert_ne!(delta.token, first.token);
    // The archive sees the same changes from its side.
    let archive = block_on(b.sync_folder(&FolderId::from("ar"), &first.token)).unwrap();
    assert_eq!(ids(&archive.added), ["m1"]);
    // A state the server cannot diff from falls back to a full sync.
    let stale = block_on(b.sync_folder(&inbox, &SyncToken("not-a-state".into()))).unwrap();
    assert_eq!(ids(&stale.added), ["m9", "m4", "m3", "m0"]);
}

#[test]
fn bodies_flags_moves_drafts_and_sending() {
    let fake = Fake::new();
    let b = backend(&fake);
    let m0 = MessageId::from("m0");
    assert_eq!(block_on(b.fetch_body(&m0)).unwrap().bytes, b"Subject: Subject 0\r\n\r\nbody of m0");
    assert!(matches!(block_on(b.fetch_body(&MessageId::from("nope"))), Err(MailError::NotFound(_))));

    block_on(b.set_flags(std::slice::from_ref(&m0), Flags::SEEN | Flags::FLAGGED, Flags::empty())).unwrap();
    block_on(b.set_flags(std::slice::from_ref(&m0), Flags::empty(), Flags::FLAGGED)).unwrap();
    block_on(b.move_to(std::slice::from_ref(&m0), &FolderId::from("ar"))).unwrap();
    {
        let s = fake.server.lock().unwrap();
        assert_eq!((&s.emails["m0"]["keywords"], &s.emails["m0"]["mailboxIds"]), (&json!({ "$seen": true }), &json!({ "ar": true })));
    }
    assert!(matches!(block_on(b.move_to(&[MessageId::from("gone")], &FolderId::from("ar"))), Err(MailError::NotFound(_))));

    let draft = block_on(b.append_draft(b"Subject: Draft one\r\n\r\nhi")).unwrap();
    let me = Address::new("Me", "ME@example.com");
    block_on(b.send(b"Subject: Sent one\r\n\r\nhi", &me, &[Address::new("O", "odin@asgard.example")])).unwrap();
    let s = fake.server.lock().unwrap();
    assert_eq!((&s.emails[&draft.0]["mailboxIds"], &s.emails[&draft.0]["keywords"]["$draft"]), (&json!({ "dr": true }), &json!(true)));
    let submission = &s.submissions[0];
    assert_eq!(submission["identityId"], "id-me", "the identity matching the sender, whatever its case");
    assert_eq!(submission["envelope"], json!({ "mailFrom": { "email": "ME@example.com" }, "rcptTo": [{ "email": "odin@asgard.example" }] }));
    assert_eq!(s.emails[submission["emailId"].as_str().unwrap()]["mailboxIds"], json!({ "se": true }));
}

#[test]
fn sessions_must_be_https_and_are_fetched_once() {
    let fake = Fake::new();
    let b = backend(&fake);
    block_on(b.list_folders()).unwrap();
    block_on(b.list_folders()).unwrap();
    assert_eq!(fake.server.lock().unwrap().requests, 2, "two API calls, one session fetch");
    let plain = Fake { session: r#"{"apiUrl":"http://jmap.example/api","downloadUrl":"https://x/","uploadUrl":"https://x/","primaryAccounts":{"urn:ietf:params:jmap:mail":"u1"}}"#, ..Fake::new() };
    assert!(matches!(block_on(backend(&plain).list_folders()), Err(MailError::Server(m)) if m.contains("https")));
    let no_mail = Fake { session: r#"{"apiUrl":"https://x/","downloadUrl":"https://x/","uploadUrl":"https://x/","primaryAccounts":{}}"#, ..Fake::new() };
    assert!(block_on(backend(&no_mail).list_folders()).is_err());
}

#[test]
fn dates_with_offsets_and_fractions_parse() {
    use session::parse_utc;
    assert_eq!(parse_utc("1970-01-02T00:00:00Z"), Some(86_400));
    assert_eq!(parse_utc("2026-11-13T16:44:00Z"), Some(1_794_588_240));
    assert_eq!(parse_utc("2026-11-13T17:44:00.250+01:00"), Some(1_794_588_240));
    assert_eq!(parse_utc("2026-11-13T10:14:00-06:30"), Some(1_794_588_240));
    assert_eq!((parse_utc("yesterday"), parse_utc("2026-13-01T00:00:00Z"), parse_utc("")), (None, None, None));
}
