use super::*;
use mail_mock::{MockBackend, block_on};
use ravenglass_relay::Relay;
use std::sync::Arc;

/// The Relay's request handler called in-process: the whole protocol, no sockets.
struct Direct { relay: Relay, version: Option<u32> }

impl Transport for Direct {
    fn request<'a>(&'a self, version: u32, method: Method, path: &'a str, body: Vec<u8>) -> BoxFut<'a, Result<HttpResponse>> {
        Box::pin(async move {
            let response = self.relay.handle(self.version.unwrap_or(version), method, path, &body).await;
            Ok(HttpResponse { status: response.status, body: response.body })
        })
    }
}

fn pair(version: Option<u32>) -> (RelayBackend<Direct>, Arc<MockBackend>) {
    let mock = Arc::new(MockBackend::new("iCloud", 46, 40));
    let mut relay = Relay::new();
    relay.add_backend(mock.clone());
    (RelayBackend::new(AccountId::from("iCloud"), Direct { relay, version }), mock)
}

#[test]
fn every_backend_call_matches_the_backend_behind_the_relay() {
    let (client, mock) = pair(None);
    assert_eq!(block_on(client.open_session()).unwrap().accounts, [AccountId::from("iCloud")]);
    let folders = block_on(client.list_folders()).unwrap();
    assert_eq!(folders, block_on(mock.list_folders()).unwrap());
    let inbox = &folders[0].id;
    let delta = block_on(client.sync_folder(inbox, &SyncToken::default())).unwrap();
    assert_eq!(delta, block_on(mock.sync_folder(inbox, &SyncToken::default())).unwrap());
    let first = &delta.added[0];
    assert_eq!(block_on(client.fetch_body(&first.id)).unwrap(), block_on(mock.fetch_body(&first.id)).unwrap());

    let archive = FolderId::from("iCloud/Archive");
    block_on(client.set_flags(std::slice::from_ref(&first.id), Flags::FLAGGED, Flags::empty())).unwrap();
    block_on(client.move_to(std::slice::from_ref(&first.id), &archive)).unwrap();
    let moved = block_on(mock.sync_folder(&archive, &SyncToken::default())).unwrap();
    assert!(moved.added.iter().any(|e| e.id == first.id && e.flags.contains(Flags::FLAGGED)));

    block_on(client.send(b"Subject: hi\r\n\r\n", &Address::new("J", "j@example.com"), &[])).unwrap();
    assert_eq!(*mock.sent.lock().unwrap(), [b"Subject: hi\r\n\r\n".to_vec()]);
    assert_eq!(block_on(client.append_draft(b"draft")).unwrap(), MessageId::from("draft/0"));
}

#[test]
fn relay_errors_arrive_as_mail_errors() {
    let (client, _) = pair(None);
    assert!(matches!(block_on(client.fetch_body(&MessageId::from("nope"))), Err(MailError::NotFound(_))));
    let stranger = RelayBackend::new(AccountId::from("someone-else"), pair(None).0.transport);
    assert!(matches!(block_on(stranger.list_folders()), Err(MailError::NotFound(_))));
}

#[test]
fn a_relay_on_another_protocol_version_is_refused() {
    let (client, _) = pair(Some(99));
    assert!(matches!(block_on(client.open_session()), Err(MailError::Server(_))));
}
