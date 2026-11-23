//! One authenticated IMAP connection, and the shared state every operation works from.
//!
//! async-imap does the framing, response parsing, login and IDLE. Commands go through
//! [`Conn::run`] instead of its per-command helpers: those treat a connection that closes
//! mid-response as a complete (shorter) answer and ignore a tagged NO, either of which would make
//! a sync report mail as deleted.

use crate::config::{Credential, ImapConfig, Security};
use crate::folders::Listed;
use crate::net::{self, BoxIo, Failure, server_text, timed};
use async_imap::imap_proto::{Capability, MailboxDatum, Response, ResponseCode, Status};
use async_imap::types::UnsolicitedResponse;
use async_imap::{Authenticator, Client, Session};
use base64::Engine;
use mail_core::{Flags, FolderId, FolderRole, MailError, SyncToken};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls::ClientConfig;

/// Flags as of one issued token, so a server without CONDSTORE still reports only what changed.
pub struct FlagCache { pub token: SyncToken, pub flags: HashMap<u32, Flags> }

/// Everything the backend's tasks share. Lives in an `Arc`; tasks never borrow the backend.
pub struct Inner {
    pub config: ImapConfig,
    /// Replaceable, so a refreshed OAuth token takes effect on the next connect.
    pub credential: std::sync::Mutex<Credential>,
    pub tls: Arc<ClientConfig>,
    /// The command connection. `None` until first use and after a failure that left it out of step.
    pub conn: tokio::sync::Mutex<Option<Conn>>,
    /// The last LIST with roles, for finding Drafts and Sent without asking again.
    pub folders: std::sync::Mutex<Option<Vec<(Listed, FolderRole)>>>,
    pub flag_cache: std::sync::Mutex<HashMap<FolderId, FlagCache>>,
    /// How much one envelope FETCH asks for; raised when the server's answer cannot be parsed.
    pub fetch_level: std::sync::atomic::AtomicU8,
}

/// Everything in one command: headers, a partial body for the preview, BODYSTRUCTURE.
pub const FETCH_FULL: u8 = 0;
/// No partial fetch (some servers answer it in a form the parser rejects): previews come from a
/// second command that fetches the whole text of small messages only.
pub const FETCH_WHOLE_TEXT: u8 = 1;
/// Also no BODYSTRUCTURE, the most intricate response there is: attachments are left empty.
pub const FETCH_NO_STRUCTURE: u8 = 2;

impl Inner {
    pub fn new(config: ImapConfig) -> mail_core::Result<Self> {
        Ok(Self {
            tls: net::tls_config(&config.extra_roots)?,
            credential: std::sync::Mutex::new(config.credential.clone()),
            config,
            conn: tokio::sync::Mutex::new(None),
            folders: std::sync::Mutex::new(None),
            flag_cache: std::sync::Mutex::new(HashMap::new()),
            fetch_level: std::sync::atomic::AtomicU8::new(FETCH_FULL),
        })
    }
}

/// The extensions this crate has a use for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps { pub move_: bool, pub uidplus: bool, pub condstore: bool, pub idle: bool }

pub struct Conn {
    pub session: Session<BoxIo>,
    pub caps: Caps,
    pub limit: Duration,
    /// The selected mailbox and its UIDVALIDITY.
    selected: Option<(String, u32)>,
}

struct XOAuth2 { payload: Option<String> }

impl Authenticator for XOAuth2 {
    type Response = String;
    /// The token goes out once. A second challenge is the server's error report, answered empty.
    fn process(&mut self, _challenge: &[u8]) -> String { self.payload.take().unwrap_or_default() }
}

/// The SASL XOAUTH2 initial response, before base64.
pub fn xoauth2_payload(username: &str, access_token: &str) -> String { format!("user={username}\x01auth=Bearer {access_token}\x01\x01") }

/// Whether the server takes a SASL initial response, asked before login.
async fn sasl_ir(client: &mut Client<BoxIo>) -> Result<bool, async_imap::error::Error> {
    // The untagged CAPABILITY data comes back through the channel async-imap uses for
    // responses it has no other place for.
    let (aside, replies) = async_channel::bounded(16);
    client.run_command_and_check_ok("CAPABILITY", Some(aside)).await?;
    let mut found = false;
    while let Ok(reply) = replies.try_recv() {
        if let UnsolicitedResponse::Other(data) = reply && let Response::Capabilities(list) = data.parsed() {
            found |= list.iter().any(|c| matches!(c, Capability::Atom(name) if name.eq_ignore_ascii_case("SASL-IR")));
        }
    }
    Ok(found)
}

async fn greeting<T>(client: &mut Client<T>, limit: Duration) -> mail_core::Result<()>
where T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send {
    match timed(limit, async { client.read_response().await.map_err(async_imap::error::Error::Io) }).await.map_err(|f| f.error)? {
        None => Err(MailError::Network("connection lost".into())),
        Some(response) => match response.parsed() {
            Response::Data { status: Status::Bye, .. } => Err(MailError::Server("the server refused the connection".into())),
            _ => Ok(()),
        },
    }
}

impl Conn {
    /// Connect, secure, authenticate and read the capabilities.
    pub async fn open(inner: &Inner) -> mail_core::Result<Conn> {
        let config = &inner.config;
        let limit = config.timeout;
        let tcp = net::connect(&config.imap, limit).await?;
        let mut client: Client<BoxIo> = match config.imap.security {
            Security::Tls => Client::new(net::secure(&inner.tls, &config.imap.host, tcp, limit).await?),
            Security::Plain => Client::new(Box::new(tcp) as BoxIo),
            Security::StartTls => {
                let mut plain = Client::new(tcp);
                greeting(&mut plain, limit).await?;
                // A refusal ends the attempt: nothing else is ever sent in the clear.
                timed(limit, plain.run_command_and_check_ok("STARTTLS", None)).await.map_err(|f| match f.error {
                    MailError::Server(_) => MailError::Server("the server does not offer STARTTLS".into()),
                    other => other,
                })?;
                // `into_inner` drops anything buffered before the handshake, as RFC 3501 requires.
                Client::new(net::secure(&inner.tls, &config.imap.host, plain.into_inner(), limit).await?)
            }
        };
        if config.imap.security != Security::StartTls { greeting(&mut client, limit).await?; }

        let credential = inner.credential.lock().unwrap().clone();
        let login = async {
            match credential {
                Credential::Password(password) => client.login(&config.username, &password).await.map_err(|(error, _client)| error),
                Credential::OAuth2 { access_token } => {
                    let payload = xoauth2_payload(&config.username, &access_token);
                    // With SASL-IR (RFC 4959) the response rides on the command itself; some
                    // servers accept nothing else. Without it, wait to be asked.
                    let (mechanism, payload) = if sasl_ir(&mut client).await? {
                        (format!("XOAUTH2 {}", base64::engine::general_purpose::STANDARD.encode(&payload)), None)
                    } else {
                        ("XOAUTH2".to_owned(), Some(payload))
                    };
                    client.authenticate(mechanism, XOAuth2 { payload }).await.map_err(|(error, _client)| error)
                }
            }
        };
        let session = match tokio::time::timeout(limit, login).await {
            Err(_) => return Err(MailError::Network("timed out".into())),
            Ok(Err(async_imap::error::Error::No(_))) => return Err(MailError::Auth),
            Ok(Err(other)) => return Err(Failure::from(other).error),
            Ok(Ok(session)) => session,
        };

        let mut conn = Conn { session, caps: Caps::default(), limit, selected: None };
        let mut advertised: Vec<String> = Vec::new();
        conn.run("CAPABILITY", |response| {
            if let Response::Capabilities(list) = response {
                advertised.extend(list.iter().filter_map(|c| match c { Capability::Atom(name) => Some(name.to_ascii_uppercase()), _ => None }));
            }
        }).await.map_err(|f| f.error)?;
        let has = |name: &str| advertised.iter().any(|a| a == name) && !config.disabled_capabilities.iter().any(|off| off.eq_ignore_ascii_case(name));
        conn.caps = Caps { move_: has("MOVE"), uidplus: has("UIDPLUS"), condstore: has("CONDSTORE"), idle: has("IDLE") };
        Ok(conn)
    }

    /// Send one command and hand every untagged response to `each` until the command's own
    /// completion arrives. Only a tagged OK is success: NO and BAD are `Server` errors on a
    /// connection that is still good, and a connection that ends first is a `Network` error.
    pub async fn run(&mut self, command: &str, each: impl FnMut(&Response<'_>)) -> Result<(), Failure> { self.run_within(self.limit, command, each).await }

    /// [`Conn::run`] with its own deadline for each response, for commands that return large ones.
    pub async fn run_within(&mut self, limit: Duration, command: &str, mut each: impl FnMut(&Response<'_>)) -> Result<(), Failure> {
        let id = timed(self.limit, self.session.run_command(command)).await?;
        loop {
            let next = timed(limit, async { self.session.read_response().await.map_err(async_imap::error::Error::Io) }).await?;
            let Some(response) = next else { return Err(Failure::broken(MailError::Network("connection lost".into()))) };
            match response.parsed() {
                Response::Done { tag, status, outcome } if *tag == id => return match status {
                    Status::Ok => Ok(()),
                    Status::No | Status::Bad => Err(MailError::Server(server_text(outcome.information.as_deref().unwrap_or(""))).into()),
                    _ => Err(Failure::broken(MailError::Server("unexpected server response".into()))),
                },
                other => each(other),
            }
        }
    }

    /// SELECT, always, so the counters are fresh. A refusal means the folder is gone.
    pub async fn select(&mut self, mailbox: &str) -> Result<Selected, Failure> {
        self.selected = None;
        let command = format!("SELECT {}{}", quote(mailbox)?, if self.caps.condstore { " (CONDSTORE)" } else { "" });
        let (mut exists, mut validity, mut uid_next, mut highest_modseq) = (0, None, None, None);
        let outcome = self.run(&command, |response| match response {
            Response::MailboxData(MailboxDatum::Exists(count)) => exists = *count,
            Response::Data { status: Status::Ok, outcome } => match outcome.code {
                Some(ResponseCode::UidValidity(value)) => validity = Some(value),
                Some(ResponseCode::UidNext(value)) => uid_next = Some(value),
                Some(ResponseCode::HighestModSeq(value)) => highest_modseq = Some(value),
                _ => {}
            },
            _ => {}
        }).await;
        outcome.map_err(|f| match f.error {
            MailError::Server(_) if f.reusable => Failure::from(MailError::NotFound("folder".into())),
            _ => f,
        })?;
        // Without it no UID can be trusted, and a made-up value would look like a changed one.
        let Some(validity) = validity else { return Err(Failure::broken(MailError::Server("the server did not report UIDVALIDITY".into()))) };
        self.selected = Some((mailbox.to_owned(), validity));
        Ok(Selected { exists, validity, uid_next, highest_modseq })
    }

    /// The UIDVALIDITY of `mailbox`, selecting it only if it is not selected already.
    pub async fn ensure_selected(&mut self, mailbox: &str) -> Result<u32, Failure> {
        if let Some((current, validity)) = &self.selected && current == mailbox { return Ok(*validity); }
        Ok(self.select(mailbox).await?.validity)
    }
}

/// What SELECT said about a mailbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selected { pub exists: u32, pub validity: u32, pub uid_next: Option<u32>, pub highest_modseq: Option<u64> }

/// A mailbox name as a quoted string. Line breaks cannot be sent that way and no real name has them.
pub fn quote(mailbox: &str) -> Result<String, Failure> {
    if mailbox.contains(['\r', '\n', '\0']) { return Err(MailError::Server("a folder name contains a character IMAP cannot send".into()).into()); }
    Ok(format!("\"{}\"", mailbox.replace('\\', "\\\\").replace('"', "\\\"")))
}

/// Run `$body` (an expression using `$conn: &mut Conn`) on the command connection, connecting
/// first if there is none. If a connection that was already open turns out to be dead, connect
/// again and run the body once more. A connection left out of step is dropped, not reused.
///
/// A macro, not a function taking a closure, so the resulting future stays `Send` without
/// boxing every operation.
macro_rules! with_conn {
    ($inner:expr, |$conn:ident| $body:expr) => {{
        let mut slot = $inner.conn.lock().await;
        let mut retried = false;
        loop {
            // Taken out while in use: if this task is cancelled mid-command, the half-used
            // connection is dropped with it.
            let (mut owned, fresh) = match slot.take() {
                Some(open) => (open, false),
                None => ($crate::conn::Conn::open(&$inner).await?, true),
            };
            let outcome: ::std::result::Result<_, $crate::net::Failure> = { let $conn = &mut owned; $body.await };
            match outcome {
                Ok(value) => { *slot = Some(owned); break Ok(value); }
                Err(failure) if !fresh && !retried && matches!(failure.error, ::mail_core::MailError::Network(_)) => { retried = true; }
                Err(failure) => { if failure.reusable { *slot = Some(owned); } break Err(failure.error); }
            }
        }
    }};
}
pub(crate) use with_conn;
