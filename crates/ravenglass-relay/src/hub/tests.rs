use super::*;
use crate::crypto::MasterKey;
use mail_auth::SecretKind;
use mail_mock::MockBackend;
use std::sync::atomic::AtomicU64;

fn config(id: &str) -> AccountConfig {
    let mut c = crate::store::tests::account(id, "x@example.net");
    c.id = AccountId::from(id);
    c
}

type Opened = Arc<Mutex<Vec<(String, Option<String>)>>>;

struct Fixture { hub: Hub, store: Arc<Store>, opened: Opened, now: Arc<AtomicU64>, pushes: broadcast::Sender<Pushed> }

fn fixture() -> Fixture {
    let store = Arc::new(Store::open_in_memory(MasterKey::generate()).unwrap());
    let opened = Arc::new(Mutex::new(Vec::new()));
    let opener: Opener = Arc::new({
        let opened = opened.clone();
        move |config: &AccountConfig, secrets: Arc<dyn SecretStore>| {
            opened.lock().unwrap().push((config.id.0.clone(), secrets.get(&config.id, SecretKind::Password).unwrap()));
            Ok(Arc::new(MockBackend::new(&config.id.0, 1, 3)) as Arc<dyn MailBackend>)
        }
    });
    let now = Arc::new(AtomicU64::new(1000));
    let clock: Clock = Arc::new({ let now = now.clone(); move || now.load(Ordering::Relaxed) });
    let (pushes, _) = broadcast::channel(16);
    Fixture { hub: Hub::new(store.clone(), opener, pushes.clone(), clock), store, opened, now, pushes }
}

fn run<F: std::future::Future>(f: F) -> F::Output { tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f) }

#[test]
fn accounts_open_on_first_use_with_their_owners_secret_and_only_for_their_owner() {
    let f = fixture();
    let (alice, bob) = (f.store.add_user("alice", None).unwrap().id, f.store.add_user("bob", None).unwrap().id);
    f.store.add_account(alice, &config("work")).unwrap();
    f.store.set_secret(alice, &AccountId::from("work"), SecretKind::Password, "alice-pw").unwrap();
    f.store.add_account(bob, &config("home")).unwrap();
    let work = AccountId::from("work");

    let view = run(f.hub.view(alice, None)).unwrap();
    assert_eq!(view.ids(), std::slice::from_ref(&work));
    assert!(f.opened.lock().unwrap().is_empty(), "listing opens nothing");
    let view = run(f.hub.view(alice, Some(&work))).unwrap();
    assert!(view.backend(&work).is_some());
    run(f.hub.view(alice, Some(&work))).unwrap();
    assert_eq!(*f.opened.lock().unwrap(), [("work".to_owned(), Some("alice-pw".to_owned()))], "opened once");

    let bobs_view = run(f.hub.view(bob, Some(&work))).unwrap();
    assert!(bobs_view.backend(&work).is_none(), "bob cannot reach alice's account");
    assert_eq!(bobs_view.ids(), [AccountId::from("home")]);
    assert_eq!(f.opened.lock().unwrap().len(), 1);
}

#[test]
fn idle_users_are_closed_after_thirty_minutes_unless_a_socket_is_open() {
    let f = fixture();
    let (alice, bob) = (f.store.add_user("alice", None).unwrap().id, f.store.add_user("bob", None).unwrap().id);
    for user in [alice, bob] {
        f.store.add_account(user, &config("a")).unwrap();
        run(f.hub.view(user, Some(&AccountId::from("a")))).unwrap();
    }
    f.hub.socket_opened(bob);
    f.now.fetch_add(CLOSE_AFTER - 1, Ordering::Relaxed);
    f.hub.reap();
    assert_eq!((f.hub.open_count(alice), f.hub.open_count(bob)), (1, 1));
    f.now.fetch_add(1, Ordering::Relaxed);
    f.hub.reap();
    assert_eq!((f.hub.open_count(alice), f.hub.open_count(bob)), (0, 1), "bob still has a socket");
    f.hub.socket_closed(bob);
    f.now.fetch_add(CLOSE_AFTER, Ordering::Relaxed);
    f.hub.reap();
    assert_eq!(f.hub.open_count(bob), 0);
    run(f.hub.view(alice, Some(&AccountId::from("a")))).unwrap();
    assert_eq!(f.hub.open_count(alice), 1, "reopened on demand");
    assert_eq!(f.opened.lock().unwrap().len(), 3);
}

/// A mock account whose IDLE reports one new message, then ends.
struct Pinging(MockBackend);

impl MailBackend for Pinging {
    fn account(&self) -> &AccountId { self.0.account() }
    fn list_folders(&self) -> mail_core::BoxFut<'_, mail_core::Result<Vec<mail_core::Folder>>> { self.0.list_folders() }
    fn sync_folder<'a>(&'a self, f: &'a mail_core::FolderId, s: &'a mail_core::SyncToken) -> mail_core::BoxFut<'a, mail_core::Result<mail_core::FolderDelta>> { self.0.sync_folder(f, s) }
    fn fetch_body<'a>(&'a self, m: &'a mail_core::MessageId) -> mail_core::BoxFut<'a, mail_core::Result<mail_core::RawMessage>> { self.0.fetch_body(m) }
    fn set_flags<'a>(&'a self, m: &'a [mail_core::MessageId], a: mail_core::Flags, r: mail_core::Flags) -> mail_core::BoxFut<'a, mail_core::Result<()>> { self.0.set_flags(m, a, r) }
    fn move_to<'a>(&'a self, m: &'a [mail_core::MessageId], t: &'a mail_core::FolderId) -> mail_core::BoxFut<'a, mail_core::Result<()>> { self.0.move_to(m, t) }
    fn append_draft<'a>(&'a self, r: &'a [u8]) -> mail_core::BoxFut<'a, mail_core::Result<mail_core::MessageId>> { self.0.append_draft(r) }
    fn send<'a>(&'a self, r: &'a [u8], f: &'a mail_core::Address, t: &'a [mail_core::Address]) -> mail_core::BoxFut<'a, mail_core::Result<()>> { self.0.send(r, f, t) }
    fn idle<'a>(&'a self, folder: &'a mail_core::FolderId, events: std::sync::mpsc::Sender<IdleEvent>) -> mail_core::BoxFut<'a, mail_core::Result<()>> {
        Box::pin(async move { let _ = events.send(IdleEvent::Exists(folder.clone())); Ok(()) })
    }
}

#[test]
fn pushes_carry_their_owner_and_stop_when_closed() {
    let f = fixture();
    let mut received = f.pushes.subscribe();
    let stop = Arc::new(AtomicBool::new(false));
    watch(Arc::new(Pinging(MockBackend::new("acct", 1, 3))), 42, f.pushes.clone(), stop.clone());
    let pushed = run(async { tokio::time::timeout(Duration::from_secs(5), received.recv()).await }).unwrap().unwrap();
    assert_eq!(pushed.owner, 42);
    assert!(pushed.json.contains("Exists") && pushed.json.contains("acct/Inbox"), "{}", pushed.json);
    stop.store(true, Ordering::Relaxed);
}
