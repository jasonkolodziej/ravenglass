//! [`MailBackend`] over the Relay API (architecture §8), so the engine in the browser Worker is
//! the same engine as on desktop. The HTTP layer is a [`Transport`]: `fetch` in the browser, and
//! in tests a direct call into the Relay's request handler.
//!
//! Not here yet (milestone W2): the `fetch` transport, the session cookie and CSRF token, and
//! the WebSocket behind [`MailBackend::idle`], which for now returns at once without events.

use mail_core::*;
use relay_proto::*;
use std::sync::mpsc::Sender;

pub struct HttpResponse { pub status: u16, pub body: Vec<u8> }

/// One request to the Relay. Implementations add [`VERSION_HEADER`] with `version`.
pub trait Transport: Send + Sync {
    fn request<'a>(&'a self, version: u32, method: Method, path: &'a str, body: Vec<u8>) -> BoxFut<'a, Result<HttpResponse>>;
}

pub struct RelayBackend<T> { account: AccountId, transport: T }

impl<T: Transport> RelayBackend<T> {
    pub fn new(account: AccountId, transport: T) -> Self { Self { account, transport } }

    /// Check the Relay speaks our protocol and list the accounts it serves.
    pub async fn open_session(&self) -> Result<SessionOpened> {
        let opened: SessionOpened = decode(&self.call(Method::Post, paths::SESSION, Vec::new()).await?)?;
        if negotiate(&[opened.protocol]).is_none() { return Err(MailError::Server(format!("relay protocol v{} is not supported", opened.protocol))); }
        Ok(opened)
    }

    async fn call(&self, method: Method, path: &str, body: Vec<u8>) -> Result<Vec<u8>> {
        let response = self.transport.request(PROTOCOL_VERSION, method, path, body).await?;
        if (200..300).contains(&response.status) { return Ok(response.body); }
        Err(match serde_json::from_slice::<ApiError>(&response.body) {
            Ok(error) => error.into(),
            Err(_) => MailError::Server(format!("relay answered HTTP {}", response.status)),
        })
    }

    async fn post<B: serde::Serialize, R: serde::de::DeserializeOwned>(&self, path: &str, body: &B) -> Result<R> {
        let body = serde_json::to_vec(body).map_err(|e| MailError::Server(e.to_string()))?;
        decode(&self.call(Method::Post, path, body).await?)
    }

    async fn ops(&self, op: MessageOp) -> Result<()> {
        let response: OpsResponse = self.post(&paths::ops(&self.account), &OpsRequest { ops: vec![op] }).await?;
        response.results.into_iter().next().unwrap_or_else(|| Err(ApiError::new(ErrorCode::Server, "relay returned no result"))).map_err(Into::into)
    }
}

fn decode<R: serde::de::DeserializeOwned>(body: &[u8]) -> Result<R> {
    serde_json::from_slice(body).map_err(|e| MailError::Server(format!("unreadable relay response: {e}")))
}

impl<T: Transport> MailBackend for RelayBackend<T> {
    fn account(&self) -> &AccountId { &self.account }
    fn list_folders(&self) -> BoxFut<'_, Result<Vec<Folder>>> {
        Box::pin(async move { decode(&self.call(Method::Get, &paths::folders(&self.account), Vec::new()).await?) })
    }
    fn sync_folder<'a>(&'a self, folder: &'a FolderId, since: &'a SyncToken) -> BoxFut<'a, Result<FolderDelta>> {
        Box::pin(async move { self.post(&paths::sync(&self.account, folder), &SyncRequest { since: since.clone() }).await })
    }
    fn fetch_body<'a>(&'a self, msg: &'a MessageId) -> BoxFut<'a, Result<RawMessage>> {
        Box::pin(async move { Ok(RawMessage { id: msg.clone(), bytes: self.call(Method::Get, &paths::raw(&self.account, msg), Vec::new()).await? }) })
    }
    fn set_flags<'a>(&'a self, msgs: &'a [MessageId], add: Flags, remove: Flags) -> BoxFut<'a, Result<()>> {
        Box::pin(self.ops(MessageOp::SetFlags { ids: msgs.to_vec(), add, remove }))
    }
    fn move_to<'a>(&'a self, msgs: &'a [MessageId], to: &'a FolderId) -> BoxFut<'a, Result<()>> {
        Box::pin(self.ops(MessageOp::Move { ids: msgs.to_vec(), to: to.clone() }))
    }
    fn append_draft<'a>(&'a self, raw: &'a [u8]) -> BoxFut<'a, Result<MessageId>> {
        Box::pin(async move { Ok(decode::<DraftSaved>(&self.call(Method::Post, &paths::drafts(&self.account), raw.to_vec()).await?)?.id) })
    }
    fn send<'a>(&'a self, raw: &'a [u8], from: &'a Address, rcpt: &'a [Address]) -> BoxFut<'a, Result<()>> {
        Box::pin(async move { self.post(&paths::send(&self.account), &SendRequest { raw: raw.to_vec(), from: from.clone(), rcpt: rcpt.to_vec() }).await })
    }
    fn idle<'a>(&'a self, _folder: &'a FolderId, _events: Sender<IdleEvent>) -> BoxFut<'a, Result<()>> { Box::pin(async { Ok(()) }) }
}

#[cfg(test)]
mod tests;
