//! JMAP backend (architecture §4): JMAP Core and Mail (RFC 8620, RFC 8621) behind
//! [`MailBackend`], so the engine treats a JMAP account like any other.
//!
//! The HTTPS layer is a [`JmapHttp`] the caller supplies (it also adds the credentials); the
//! `http` feature has a blocking one for native builds.
//!
//! **Status:** written against the RFCs and tested against the in-process server in
//! `tests.rs`, which implements the same reading of them. It has not yet talked to a real
//! JMAP server (Fastmail, Stalwart). Not here yet: push (EventSource), so `idle` returns at
//! once and the engine's five-minute poll finds new mail; attachment lists on envelopes.

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
const ENVELOPE_PROPERTIES: [&str; 10] = ["id", "threadId", "mailboxIds", "keywords", "from", "to", "cc", "subject", "receivedAt", "preview"];

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

    /// Every message in a mailbox, newest first, and the state they were read at.
    async fn full_sync(&self, session: &Session, folder: &FolderId) -> Result<FolderDelta> {
        let mut ids: Vec<String> = Vec::new();
        let mut state = String::new();
        loop {
            let query = json!({ "accountId": session.account_id, "filter": { "inMailbox": folder.0 }, "sort": [{ "property": "receivedAt", "isAscending": false }], "position": ids.len(), "limit": PAGE });
            let mut got = self.call(session, vec![("Email/query", query), ("Email/get", json!({ "accountId": session.account_id, "ids": [], "properties": ["id"] }))]).await?;
            // An empty Email/get in the same request reads the state the query ran at.
            if state.is_empty() { state = got[1]["state"].as_str().unwrap_or_default().to_owned(); }
            let page: Vec<String> = got.remove(0)["ids"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
            let last = page.len() < PAGE;
            ids.extend(page);
            if last { break; }
        }
        let (list, _) = self.envelopes(session, &ids).await?;
        Ok(FolderDelta { added: list.iter().filter_map(|e| envelope(e, folder)).collect(), token: SyncToken(state), ..Default::default() })
    }

    async fn changes(&self, session: &Session, folder: &FolderId, since: &str) -> Result<FolderDelta> {
        let (mut touched, mut destroyed, mut state) = (Vec::new(), Vec::new(), since.to_owned());
        loop {
            let mut got = self.call(session, vec![("Email/changes", json!({ "accountId": session.account_id, "sinceState": state, "maxChanges": PAGE }))]).await?;
            let result = got.remove(0);
            let strings = |key: &str| result[key].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect::<Vec<_>>()).unwrap_or_default();
            touched.extend(strings("created"));
            touched.extend(strings("updated"));
            destroyed.extend(strings("destroyed"));
            state = result["newState"].as_str().unwrap_or(&state).to_owned();
            if result["hasMoreChanges"] != true { break; }
        }
        let (list, _) = self.envelopes(session, &touched).await?;
        let mut delta = FolderDelta { token: SyncToken(state), ..Default::default() };
        for email in &list {
            match envelope(email, folder) {
                // Created here, moved here or re-flagged: the store upserts all three the same way.
                Some(e) => delta.added.push(e),
                // Changed and no longer in this mailbox: it left.
                None => delta.removed.extend(email["id"].as_str().map(MessageId::from)),
            }
        }
        delta.removed.extend(destroyed.iter().map(|id| MessageId::from(id.as_str())));
        Ok(delta)
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
        let update: serde_json::Map<String, Value> = ids.iter().map(|id| (id.0.clone(), patch.clone())).collect();
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
            Ok(boxes.iter().filter_map(|m| folder(m, &boxes, &self.account)).collect())
        })
    }

    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> {
        Box::pin(async move {
            let session = self.session().await?;
            if since.0.is_empty() { return self.full_sync(&session, folder).await; }
            match self.changes(&session, folder, &since.0).await {
                // The server no longer remembers that state: start over.
                Err(MailError::Server(m)) if m.contains("cannotCalculateChanges") => self.full_sync(&session, folder).await,
                other => other,
            }
        })
    }

    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> {
        Box::pin(async move {
            let session = self.session().await?;
            let mut got = self.call(&session, vec![("Email/get", json!({ "accountId": session.account_id, "ids": [msg.0], "properties": ["blobId"] }))]).await?;
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
            self.update(&session, msgs, json!({ "mailboxIds": { to.0.clone(): true } })).await
        })
    }

    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> {
        Box::pin(async move {
            let session = self.session().await?;
            let drafts = self.mailbox_with_role(&session, "drafts").await?;
            let blob = self.upload(&session, raw).await?;
            Ok(MessageId(self.import(&session, &blob, &drafts, json!({ "$draft": true, "$seen": true })).await?))
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
            let mut got = self.call(&session, vec![("EmailSubmission/set", args)]).await?;
            let result = got.remove(0);
            if result["created"]["send"].is_object() { Ok(()) } else { Err(set_error("submission", &result["notCreated"]["send"])) }
        })
    }

    fn idle<'a>(&'a self, _folder: &'a FolderId, _events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> { Box::pin(async { Ok(()) }) }
}

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
    Some(Folder { id: FolderId::from(id), account: account.clone(), name: name.to_owned(), path, role: role(mailbox["role"].as_str()), unread: count("unreadEmails"), total: count("totalEmails") })
}

fn addresses(list: &Value) -> Vec<Address> {
    list.as_array().map(|a| a.iter().filter_map(|x| Some(Address { name: x["name"].as_str().filter(|n| !n.is_empty()).map(str::to_owned), email: x["email"].as_str()?.to_owned() })).collect()).unwrap_or_default()
}

/// An `Email` object as an envelope of `folder`, or `None` if it is not in that mailbox.
fn envelope(email: &Value, folder: &FolderId) -> Option<Envelope> {
    if email["mailboxIds"][&folder.0] != true { return None; }
    let mut flags = Flags::empty();
    for (flag, keyword) in KEYWORDS { if email["keywords"][keyword] == true { flags.insert(flag); } }
    let from = addresses(&email["from"]).into_iter().next().unwrap_or(Address { name: None, email: String::new() });
    Some(Envelope {
        id: MessageId::from(email["id"].as_str()?),
        folder: folder.clone(),
        thread: ThreadId::from(email["threadId"].as_str().unwrap_or_default()),
        from,
        to: addresses(&email["to"]),
        cc: addresses(&email["cc"]),
        subject: email["subject"].as_str().unwrap_or_default().to_owned(),
        preview: email["preview"].as_str().unwrap_or_default().to_owned(),
        date: email["receivedAt"].as_str().and_then(session::parse_utc).unwrap_or(0),
        flags,
        attachments: Vec::new(),
    })
}
