//! JMAP backend (architecture §4): JMAP Core and Mail (RFC 8620, RFC 8621) behind
//! [`MailBackend`], so the engine treats a JMAP account like any other.
//!
//! The HTTPS layer is a [`JmapHttp`] the caller supplies (it also adds the credentials); the
//! `http` feature has a blocking one for native builds.
//!
//! **Status:** tested against the in-process server in `tests.rs` and against a real one,
//! Stalwart (`tests/stalwart.rs`, `RAVENGLASS_STALWART_TEST=1`, see `tools/stalwart.sh`). Not
//! yet against Fastmail. Not here yet: push (EventSource), so `idle` returns at once and the
//! engine's five-minute poll finds new mail; attachment lists on envelopes.
//!
//! Folder, message and thread ids are `{account}/{JMAP id}`. A folder's sync token holds the
//! account's `Email` state and the folder query's state: `Email/queryChanges` says what entered
//! and left the folder, `Email/changes` what changed at all.

mod session;
#[cfg(test)]
mod tests;
#[cfg(feature = "http")]
mod transport;

pub use session::Session;
#[cfg(feature = "http")]
pub use transport::{Credential, HttpsTransport};

use mail_core::*;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::sync::mpsc::Sender;

/// The HTTPS requests JMAP needs. Implementations add `Authorization` and map HTTP 401/403 to
/// [`MailError::Auth`].
pub trait JmapHttp: Send + Sync {
    fn get<'a>(&'a self, url: &'a str) -> BoxFut<'a, Result<Vec<u8>>>;
    fn post<'a>(&'a self, url: &'a str, content_type: &'a str, body: Vec<u8>) -> BoxFut<'a, Result<Vec<u8>>>;
}

const USING: [&str; 3] = ["urn:ietf:params:jmap:core", "urn:ietf:params:jmap:mail", "urn:ietf:params:jmap:submission"];
/// Ids asked for per `Email/query` and `Email/get`.
const PAGE: usize = 256;
const ENVELOPE_PROPERTIES: [&str; 11] = ["id", "threadId", "mailboxIds", "keywords", "from", "to", "cc", "subject", "sentAt", "receivedAt", "preview"];
/// Characters of preview kept for the list row, as the IMAP backend keeps.
const PREVIEW_CHARS: usize = 200;

pub struct JmapBackend<H> {
    account: AccountId,
    session_url: String,
    http: H,
    session: Mutex<Option<Session>>,
}

impl<H: JmapHttp> JmapBackend<H> {
    pub fn new(account: AccountId, session_url: &str, http: H) -> Self {
        Self { account, session_url: session_url.to_owned(), http, session: Mutex::new(None) }
    }

    /// The session resource, fetched on first use.
    async fn session(&self) -> Result<Session> {
        if let Some(s) = self.session.lock().map_err(poisoned)?.clone() { return Ok(s); }
        let fetched = Session::parse(&self.http.get(&self.session_url).await?)?;
        *self.session.lock().map_err(poisoned)? = Some(fetched.clone());
        Ok(fetched)
    }

    /// One API request with these method calls; returns each call's result in order.
    async fn call(&self, session: &Session, calls: Vec<(&str, Value)>) -> Result<Vec<Value>> {
        let method_calls: Vec<Value> = calls.iter().enumerate().map(|(i, (name, args))| json!([name, args, i.to_string()])).collect();
        let body = serde_json::to_vec(&json!({ "using": USING, "methodCalls": method_calls })).map_err(unreadable)?;
        let answer: Value = serde_json::from_slice(&self.http.post(&session.api_url, "application/json", body).await?).map_err(unreadable)?;
        let responses = answer["methodResponses"].as_array().ok_or_else(|| MailError::Server("JMAP answer has no methodResponses".into()))?;
        let mut results = Vec::with_capacity(calls.len());
        for (i, (name, _)) in calls.iter().enumerate() {
            let found = responses.iter().find(|r| r[2].as_str() == Some(&i.to_string())).ok_or_else(|| MailError::Server(format!("no answer to {name}")))?;
            if found[0] == "error" { return Err(method_error(name, &found[1])); }
            results.push(found[1].clone());
        }
        Ok(results)
    }

    async fn mailboxes(&self, session: &Session) -> Result<Vec<Value>> {
        let mut got = self.call(session, vec![("Mailbox/get", json!({ "accountId": session.account_id, "ids": null }))]).await?;
        Ok(got.remove(0)["list"].as_array().cloned().unwrap_or_default())
    }

    async fn mailbox_with_role(&self, session: &Session, role: &str) -> Result<String> {
        let found = self.mailboxes(session).await?.into_iter().find(|m| m["role"] == role).and_then(|m| m["id"].as_str().map(str::to_owned));
        found.ok_or_else(|| MailError::NotFound(format!("no {role} mailbox")))
    }

    async fn envelopes(&self, session: &Session, ids: &[String]) -> Result<(Vec<Value>, String)> {
        let mut list = Vec::with_capacity(ids.len());
        let mut state = String::new();
        for page in ids.chunks(PAGE) {
            let mut got = self.call(session, vec![("Email/get", json!({ "accountId": session.account_id, "ids": page, "properties": ENVELOPE_PROPERTIES }))]).await?;
            let result = got.remove(0);
            state = result["state"].as_str().unwrap_or_default().to_owned();
            list.extend(result["list"].as_array().cloned().unwrap_or_default());
        }
        Ok((list, state))
    }

    /// The query behind a folder's list: its messages, newest first.
    fn folder_query(session: &Session, mailbox: &str) -> Value {
        json!({ "accountId": session.account_id, "filter": { "inMailbox": mailbox }, "sort": [{ "property": "receivedAt", "isAscending": false }] })
    }

    /// Every message in a mailbox, newest first, and the states they were read at.
    async fn full_sync(&self, session: &Session, folder: &FolderId) -> Result<FolderDelta> {
        let mailbox = unscope(&self.account, &folder.0)?;
        let mut ids: Vec<String> = Vec::new();
        let (mut state, mut query_state) = (String::new(), String::new());
        loop {
            let mut query = Self::folder_query(session, mailbox);
            query["position"] = json!(ids.len());
            query["limit"] = json!(PAGE);
            let mut got = self.call(session, vec![("Email/query", query), ("Email/get", json!({ "accountId": session.account_id, "ids": [], "properties": ["id"] }))]).await?;
            // An empty Email/get in the same request reads the state the query ran at.
            if state.is_empty() { state = got[1]["state"].as_str().unwrap_or_default().to_owned(); }
            let page_state = got[0]["queryState"].as_str().unwrap_or_default().to_owned();
            if ids.is_empty() {
                query_state = page_state;
            } else if page_state != query_state {
                // The mailbox changed between pages, so positions moved: read it again from the top.
                (ids, state, query_state) = (Vec::new(), String::new(), String::new());
                continue;
            }
            let page: Vec<String> = strings(&got.remove(0)["ids"]);
            let last = page.len() < PAGE;
            ids.extend(page);
            if last { break; }
        }
        let (list, _) = self.envelopes(session, &ids).await?;
        Ok(FolderDelta { added: list.iter().filter_map(|e| envelope(e, &self.account, folder)).collect(), token: Token { state, query_state }.encode(), ..Default::default() })
    }

    /// What changed in a folder since `since`. `Email/queryChanges` says which messages entered
    /// and left it (new, moved, destroyed); `Email/changes` which messages changed at all (flags).
    async fn changes(&self, session: &Session, folder: &FolderId, since: &Token) -> Result<FolderDelta> {
        let mut query_changes = Self::folder_query(session, unscope(&self.account, &folder.0)?);
        query_changes["sinceQueryState"] = json!(since.query_state);
        let moved = self.call(session, vec![("Email/queryChanges", query_changes)]).await?.remove(0);
        let mut touched: Vec<String> = moved["added"].as_array().map(|a| a.iter().filter_map(|v| v["id"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
        let left = strings(&moved["removed"]);
        let query_state = moved["newQueryState"].as_str().unwrap_or(&since.query_state).to_owned();

        let mut state = since.state.clone();
        loop {
            let result = self.call(session, vec![("Email/changes", json!({ "accountId": session.account_id, "sinceState": state, "maxChanges": PAGE }))]).await?.remove(0);
            touched.extend(strings(&result["created"]));
            touched.extend(strings(&result["updated"]));
            state = result["newState"].as_str().unwrap_or(&state).to_owned();
            if result["hasMoreChanges"] != true { break; }
        }
        touched.extend(left.iter().cloned());
        touched.sort_unstable();
        touched.dedup();
        let (list, _) = self.envelopes(session, &touched).await?;
        // Created here, moved here or re-flagged: the store upserts all three the same way.
        // Changes to messages in other folders are theirs to report, not removals from this one.
        let added: Vec<Envelope> = list.iter().filter_map(|e| envelope(e, &self.account, folder)).collect();
        // Only a message that no longer exists is removed. One that moved keeps its id, and the
        // store holds one row per id: the destination's sync moves that row, whichever folder
        // syncs first, where a removal here could delete the row the destination already moved.
        let exists = |id: &String| list.iter().any(|e| e["id"].as_str() == Some(id.as_str()));
        let removed = left.into_iter().filter(|id| !exists(id)).map(|id| MessageId(scope(&self.account, &id))).collect();
        Ok(FolderDelta { added, removed, token: Token { state, query_state }.encode(), ..Default::default() })
    }

    /// The session resource's capabilities (fetching it if needed).
    pub async fn capabilities(&self) -> Result<Vec<String>> { Ok(self.session().await?.capabilities) }

    /// The newest `limit` messages of `folder`, newest first, and how many it holds. Changes nothing.
    pub async fn newest(&self, folder: &FolderId, limit: u32) -> Result<(Vec<Envelope>, u32)> {
        let session = self.session().await?;
        let mut query = Self::folder_query(&session, unscope(&self.account, &folder.0)?);
        query["limit"] = json!(limit);
        query["calculateTotal"] = json!(true);
        let found = self.call(&session, vec![("Email/query", query)]).await?.remove(0);
        let ids = strings(&found["ids"]);
        let (list, _) = self.envelopes(&session, &ids).await?;
        let total = found["total"].as_u64().unwrap_or(ids.len() as u64) as u32;
        // Email/get answers in any order; keep the query's.
        let mut newest: Vec<Envelope> = list.iter().filter_map(|e| envelope(e, &self.account, folder)).collect();
        newest.sort_by_key(|e| ids.iter().position(|id| scope(&self.account, id) == e.id.0));
        Ok((newest, total))
    }

    /// Create a top-level mailbox.
    pub async fn create_folder(&self, name: &str) -> Result<FolderId> {
        let session = self.session().await?;
        let args = json!({ "accountId": session.account_id, "create": { "new": { "name": name, "parentId": null } } });
        let result = self.call(&session, vec![("Mailbox/set", args)]).await?.remove(0);
        let id = result["created"]["new"]["id"].as_str().ok_or_else(|| set_error("create mailbox", &result["notCreated"]["new"]))?;
        Ok(FolderId(scope(&self.account, id)))
    }

    /// Delete a mailbox and the messages that are only in it.
    pub async fn delete_folder(&self, folder: &FolderId) -> Result<()> {
        let session = self.session().await?;
        let mailbox = unscope(&self.account, &folder.0)?;
        let args = json!({ "accountId": session.account_id, "destroy": [mailbox], "onDestroyRemoveEmails": true });
        let result = self.call(&session, vec![("Mailbox/set", args)]).await?.remove(0);
        if result["notDestroyed"][mailbox].is_object() { Err(set_error("delete mailbox", &result["notDestroyed"][mailbox])) } else { Ok(()) }
    }

    /// Store a message in a folder with these flags.
    pub async fn append_to(&self, folder: &FolderId, raw: &[u8], flags: Flags) -> Result<MessageId> {
        let session = self.session().await?;
        let mailbox = unscope(&self.account, &folder.0)?;
        let keywords: serde_json::Map<String, Value> = KEYWORDS.iter().filter(|(flag, _)| flags.contains(*flag)).map(|(_, k)| ((*k).to_owned(), json!(true))).collect();
        let blob = self.upload(&session, raw).await?;
        Ok(MessageId(scope(&self.account, &self.import(&session, &blob, mailbox, Value::Object(keywords)).await?)))
    }

    async fn upload(&self, session: &Session, raw: &[u8]) -> Result<String> {
        let answer: Value = serde_json::from_slice(&self.http.post(&session.upload_url(), "message/rfc822", raw.to_vec()).await?).map_err(unreadable)?;
        answer["blobId"].as_str().map(str::to_owned).ok_or_else(|| MailError::Server("upload returned no blobId".into()))
    }

    /// Store an uploaded message in a mailbox; returns its email id.
    async fn import(&self, session: &Session, blob: &str, mailbox: &str, keywords: Value) -> Result<String> {
        let args = json!({ "accountId": session.account_id, "emails": { "new": { "blobId": blob, "mailboxIds": { mailbox: true }, "keywords": keywords } } });
        let mut got = self.call(session, vec![("Email/import", args)]).await?;
        let result = got.remove(0);
        result["created"]["new"]["id"].as_str().map(str::to_owned).ok_or_else(|| set_error("import", &result["notCreated"]["new"]))
    }

    async fn update(&self, session: &Session, ids: &[MessageId], patch: Value) -> Result<()> {
        let update = ids.iter().map(|id| Ok((unscope(&self.account, &id.0)?.to_owned(), patch.clone()))).collect::<Result<serde_json::Map<String, Value>>>()?;
        let mut got = self.call(session, vec![("Email/set", json!({ "accountId": session.account_id, "update": update }))]).await?;
        match got.remove(0)["notUpdated"].as_object().and_then(|failed| failed.values().next()) {
            Some(first) => Err(set_error("update", first)),
            None => Ok(()),
        }
    }
}

impl<H: JmapHttp> MailBackend for JmapBackend<H> {
    fn account(&self) -> &AccountId { &self.account }

    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> {
        Box::pin(async move {
            let session = self.session().await?;
            let boxes = self.mailboxes(&session).await?;
            let mut folders: Vec<Folder> = boxes.iter().filter_map(|m| folder(m, &boxes, &self.account)).collect();
            roles_from_names(&mut folders);
            Ok(folders)
        })
    }

    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> {
        Box::pin(async move {
            let session = self.session().await?;
            // No token, or one from before folder queries were tracked: read the folder whole.
            let Some(since) = Token::decode(since) else { return self.full_sync(&session, folder).await };
            match self.changes(&session, folder, &since).await {
                // The server no longer remembers that state, or too much changed: start over.
                Err(MailError::Server(m)) if m.contains("cannotCalculateChanges") || m.contains("tooManyChanges") => self.full_sync(&session, folder).await,
                other => other,
            }
        })
    }

    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> {
        Box::pin(async move {
            let session = self.session().await?;
            let email = unscope(&self.account, &msg.0)?;
            let mut got = self.call(&session, vec![("Email/get", json!({ "accountId": session.account_id, "ids": [email], "properties": ["blobId"] }))]).await?;
            let blob = got.remove(0)["list"][0]["blobId"].as_str().map(str::to_owned).ok_or_else(|| MailError::NotFound(msg.to_string()))?;
            Ok(RawMessage { id: msg.clone(), bytes: self.http.get(&session.download_url(&blob)).await? })
        })
    }

    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            let mut patch = serde_json::Map::new();
            for (flag, keyword) in KEYWORDS {
                if add.contains(flag) { patch.insert(format!("keywords/{keyword}"), json!(true)); }
                if remove.contains(flag) { patch.insert(format!("keywords/{keyword}"), Value::Null); }
            }
            if patch.is_empty() || msgs.is_empty() { return Ok(()); }
            let session = self.session().await?;
            self.update(&session, msgs, Value::Object(patch)).await
        })
    }

    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            if msgs.is_empty() { return Ok(()); }
            let session = self.session().await?;
            let mailbox = unscope(&self.account, &to.0)?;
            self.update(&session, msgs, json!({ "mailboxIds": { mailbox: true } })).await
        })
    }

    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> {
        Box::pin(async move {
            let session = self.session().await?;
            let drafts = self.mailbox_with_role(&session, "drafts").await?;
            let blob = self.upload(&session, raw).await?;
            Ok(MessageId(scope(&self.account, &self.import(&session, &blob, &drafts, json!({ "$draft": true, "$seen": true })).await?)))
        })
    }

    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> {
        Box::pin(async move {
            let session = self.session().await?;
            let sent = self.mailbox_with_role(&session, "sent").await?;
            let blob = self.upload(&session, raw).await?;
            let email = self.import(&session, &blob, &sent, json!({ "$seen": true })).await?;
            let mut got = self.call(&session, vec![("Identity/get", json!({ "accountId": session.account_id, "ids": null }))]).await?;
            let identities = got.remove(0)["list"].as_array().cloned().unwrap_or_default();
            let identity = identities.iter().find(|i| i["email"].as_str().is_some_and(|e| e.eq_ignore_ascii_case(&from.email))).or(identities.first());
            let identity = identity.and_then(|i| i["id"].as_str()).ok_or_else(|| MailError::Server("the account has no sending identity".into()))?;
            let envelope = json!({ "mailFrom": { "email": from.email }, "rcptTo": rcpt.iter().map(|a| json!({ "email": a.email })).collect::<Vec<_>>() });
            let args = json!({ "accountId": session.account_id, "create": { "send": { "emailId": email, "identityId": identity, "envelope": envelope } } });
            let submitted = match self.call(&session, vec![("EmailSubmission/set", args)]).await {
                Ok(mut got) => { let result = got.remove(0); if result["created"]["send"].is_object() { Ok(()) } else { Err(set_error("submission", &result["notCreated"]["send"])) } }
                Err(error) => Err(error),
            };
            if submitted.is_err() {
                // Not sent, so not in Sent either: the outbox will try again and file a new copy.
                let _ = self.call(&session, vec![("Email/set", json!({ "accountId": session.account_id, "destroy": [email] }))]).await;
            }
            submitted
        })
    }

    fn idle<'a>(&'a self, _folder: &'a FolderId, _events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> { Box::pin(async { Ok(()) }) }
}

/// A folder's sync token: the account's `Email` state and the folder query's state, so that
/// changes elsewhere in the account are never mistaken for messages leaving this folder.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Token { state: String, query_state: String }

impl Token {
    const PREFIX: &'static str = "q1 ";

    fn encode(&self) -> SyncToken { SyncToken(format!("{}{} {}", Self::PREFIX, self.state, self.query_state)) }

    /// `None` for an empty token or one this version did not write: both mean a full sync.
    fn decode(token: &SyncToken) -> Option<Self> {
        let (state, query_state) = token.0.strip_prefix(Self::PREFIX)?.split_once(' ')?;
        (!state.is_empty() && !query_state.is_empty()).then(|| Self { state: state.to_owned(), query_state: query_state.to_owned() })
    }
}

/// Ids as the engine sees them: `{account}/{JMAP id}`, like the IMAP backend's. A JMAP id is
/// only unique within its account (Stalwart names every account's mailboxes from "a"), and the
/// store's ids are global, so two JMAP accounts would otherwise share folders and messages.
fn scope(account: &AccountId, id: &str) -> String { format!("{}/{id}", account.0) }

/// The JMAP id inside an id from [`scope`]; `NotFound` for another account's id.
fn unscope<'a>(account: &AccountId, id: &'a str) -> Result<&'a str> {
    id.strip_prefix(account.0.as_str()).and_then(|rest| rest.strip_prefix('/')).filter(|rest| !rest.is_empty()).ok_or_else(|| MailError::NotFound("message or folder".into()))
}

fn strings(list: &Value) -> Vec<String> { list.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default() }

const KEYWORDS: [(Flags, &str); 4] = [(Flags::SEEN, "$seen"), (Flags::FLAGGED, "$flagged"), (Flags::ANSWERED, "$answered"), (Flags::DRAFT, "$draft")];

fn poisoned<T>(_: T) -> MailError { MailError::Server("JMAP session lock poisoned".into()) }

fn unreadable(e: serde_json::Error) -> MailError { MailError::Server(format!("unreadable JMAP data: {e}")) }

fn method_error(method: &str, error: &Value) -> MailError {
    let kind = error["type"].as_str().unwrap_or("unknown");
    match kind { "accountNotFound" | "notFound" => MailError::NotFound(method.to_owned()), _ => MailError::Server(format!("{method}: {kind}")) }
}

fn set_error(what: &str, error: &Value) -> MailError {
    match error["type"].as_str() { Some("notFound") => MailError::NotFound(what.to_owned()), kind => MailError::Server(format!("{what}: {}", kind.unwrap_or("refused"))) }
}

fn role(name: Option<&str>) -> FolderRole {
    match name {
        Some("inbox") => FolderRole::Inbox, Some("drafts") => FolderRole::Drafts, Some("sent") => FolderRole::Sent, Some("archive") => FolderRole::Archive,
        Some("trash") => FolderRole::Trash, Some("junk") => FolderRole::Junk, Some("flagged") => FolderRole::Flagged, _ => FolderRole::Other,
    }
}

/// A top-level mailbox named like a role gets it when no mailbox has that role from the server,
/// as on IMAP: a folder a user made called "Archive" is where archiving goes (Stalwart and
/// Cyrus give a user-made mailbox no role).
fn roles_from_names(folders: &mut [Folder]) {
    for i in 0..folders.len() {
        if folders[i].role != FolderRole::Other || folders[i].path.len() != 1 { continue; }
        let named = match folders[i].name.trim().to_lowercase().as_str() {
            "drafts" | "draft" => FolderRole::Drafts,
            "sent" | "sent items" | "sent mail" | "sent messages" => FolderRole::Sent,
            "trash" | "deleted items" | "deleted messages" | "bin" => FolderRole::Trash,
            "junk" | "spam" | "junk e-mail" | "junk email" | "junk mail" | "bulk mail" => FolderRole::Junk,
            "archive" | "archives" => FolderRole::Archive,
            _ => continue,
        };
        if !folders.iter().any(|f| f.role == named) { folders[i].role = named; }
    }
}

fn folder(mailbox: &Value, all: &[Value], account: &AccountId) -> Option<Folder> {
    let (id, name) = (mailbox["id"].as_str()?, mailbox["name"].as_str()?);
    // Walk up the parents for the path; the depth cap stops a malformed cycle.
    let mut path = vec![name.to_owned()];
    let mut parent = mailbox["parentId"].as_str();
    while let Some(p) = parent.filter(|_| path.len() < 32) {
        let Some(above) = all.iter().find(|m| m["id"] == p) else { break };
        path.insert(0, above["name"].as_str().unwrap_or_default().to_owned());
        parent = above["parentId"].as_str();
    }
    let count = |key: &str| mailbox[key].as_u64().unwrap_or(0) as u32;
    Some(Folder { id: FolderId(scope(account, id)), account: account.clone(), name: name.to_owned(), path, role: role(mailbox["role"].as_str()), unread: count("unreadEmails"), total: count("totalEmails") })
}

fn addresses(list: &Value) -> Vec<Address> {
    list.as_array().map(|a| a.iter().filter_map(|x| Some(Address { name: x["name"].as_str().filter(|n| !n.is_empty()).map(str::to_owned), email: x["email"].as_str()?.to_owned() })).collect()).unwrap_or_default()
}

/// An `Email` object as an envelope of `folder`, or `None` if it is not in that mailbox.
fn envelope(email: &Value, account: &AccountId, folder: &FolderId) -> Option<Envelope> {
    if email["mailboxIds"][unscope(account, &folder.0).ok()?] != true { return None; }
    let mut flags = Flags::empty();
    for (flag, keyword) in KEYWORDS { if email["keywords"][keyword] == true { flags.insert(flag); } }
    let from = addresses(&email["from"]).into_iter().next().unwrap_or(Address { name: None, email: String::new() });
    Some(Envelope {
        id: MessageId(scope(account, email["id"].as_str()?)),
        folder: folder.clone(),
        thread: ThreadId(scope(account, email["threadId"].as_str().unwrap_or_default())),
        from,
        to: addresses(&email["to"]),
        cc: addresses(&email["cc"]),
        subject: email["subject"].as_str().unwrap_or_default().to_owned(),
        preview: one_line(email["preview"].as_str().unwrap_or_default()),
        // The Date header, as the IMAP backend uses; the arrival time when there is none.
        date: email["sentAt"].as_str().or(email["receivedAt"].as_str()).and_then(session::parse_utc).unwrap_or(0),
        flags,
        attachments: Vec::new(),
    })
}

/// A server's preview as one line of at most [`PREVIEW_CHARS`] characters. Servers send up to 256
/// characters with the message's own line breaks (Stalwart ends it with one).
fn one_line(text: &str) -> String {
    let words: Vec<&str> = text.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect();
    words.join(" ").chars().take(PREVIEW_CHARS).collect()
}
