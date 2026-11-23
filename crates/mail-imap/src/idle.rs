//! Push: IMAP IDLE (RFC 2177) on a connection of its own, re-issued every 25 minutes, with a
//! NOOP poll (`ImapConfig::idle_poll`) for servers that have no IDLE. The policy is a small state machine ([`step`]); the
//! driver ([`run`]) only performs its effects.

use crate::conn::{Conn, Inner};
use crate::ids::mailbox_of;
use crate::net::{BoxIo, Failure, timed};
use async_imap::extensions::idle::{Handle, IdleResponse};
use async_imap::imap_proto::{MailboxDatum, Response};
use mail_core::{FolderId, IdleEvent, MailError, Result};
use std::sync::Arc;
use std::sync::mpsc::Sender;
use std::time::Duration;

/// Servers may drop an IDLE after 30 minutes (RFC 2177); start a new one before that.
pub const REISSUE: Duration = Duration::from_secs(25 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Opening the connection. `resumed`: a watch was lost and this is the one reconnect.
    Connecting { resumed: bool },
    Watching,
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Connected,
    /// The server said something and it was passed on (or was not worth passing on).
    Seen,
    /// [`REISSUE`] went by quietly.
    Timer,
    /// Nobody is listening for events any more.
    ReceiverGone,
    Failed(MailError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Connect,
    Wait,
    /// Tell the listener to look again (changes may have been missed while disconnected), then wait.
    CatchUp,
    /// End this IDLE and start a fresh one, then wait.
    Reissue,
    Finish(Result<()>),
}

pub fn initial() -> (State, Effect) { (State::Connecting { resumed: false }, Effect::Connect) }

/// A lost watch reconnects once; a connection that cannot be opened ends the watch with its
/// error, and the engine's own backoff decides when to try again.
pub fn step(state: &mut State, event: Event) -> Effect {
    let (next, effect) = match (*state, event) {
        (State::Done, _) | (_, Event::ReceiverGone) => (State::Done, Effect::Finish(Ok(()))),
        (State::Connecting { resumed }, Event::Connected) => (State::Watching, if resumed { Effect::CatchUp } else { Effect::Wait }),
        (State::Connecting { .. }, Event::Failed(error)) => (State::Done, Effect::Finish(Err(error))),
        (State::Watching, Event::Seen) => (State::Watching, Effect::Wait),
        (State::Watching, Event::Timer) => (State::Watching, Effect::Reissue),
        (State::Watching, Event::Failed(MailError::Network(_))) => (State::Connecting { resumed: true }, Effect::Connect),
        (State::Watching, Event::Failed(error)) => (State::Done, Effect::Finish(Err(error))),
        // Not reachable from the driver; ignore rather than guess.
        (current @ State::Connecting { .. }, Event::Seen | Event::Timer) => (current, Effect::Connect),
        (State::Watching, Event::Connected) => (State::Watching, Effect::Wait),
    };
    *state = next;
    effect
}

/// What an untagged response during IDLE means for the folder. `None` for chatter.
pub fn classify(response: &Response<'_>, folder: &FolderId) -> Option<IdleEvent> {
    match response {
        Response::MailboxData(MailboxDatum::Exists(_)) => Some(IdleEvent::Exists(folder.clone())),
        Response::Expunge(_) => Some(IdleEvent::Expunged(folder.clone())),
        Response::Fetch(..) => Some(IdleEvent::FlagsChanged(folder.clone())),
        _ => None,
    }
}

enum Watch { Idle(Handle<BoxIo>), Poll { conn: Box<Conn>, every: Duration } }

async fn open(inner: &Inner, mailbox: &str) -> std::result::Result<Watch, Failure> {
    let mut conn = Conn::open(inner).await?;
    conn.select(mailbox).await?;
    if !conn.caps.idle { return Ok(Watch::Poll { conn: Box::new(conn), every: inner.config.idle_poll }); }
    let mut handle = conn.session.idle();
    timed(inner.config.timeout, handle.init()).await?;
    Ok(Watch::Idle(handle))
}

/// Wait for the next thing worth an [`Event`], forwarding mailbox changes to `events`.
async fn wait(watch: &mut Watch, folder: &FolderId, events: &Sender<IdleEvent>) -> Event {
    let forward = |event: Option<IdleEvent>| match event.map(|event| events.send(event)) {
        Some(Err(_)) => Event::ReceiverGone,
        _ => Event::Seen,
    };
    match watch {
        Watch::Idle(handle) => {
            // Dropping the stop source would interrupt the wait, so it lives as long as the wait.
            let (waiting, _stop) = handle.wait_with_timeout(REISSUE);
            match waiting.await {
                Ok(IdleResponse::NewData(data)) => forward(classify(data.parsed(), folder)),
                Ok(IdleResponse::Timeout) => Event::Timer,
                // Nothing here interrupts by hand, so this is the stream ending.
                Ok(IdleResponse::ManualInterrupt) => Event::Failed(MailError::Network("connection lost".into())),
                Err(error) => Event::Failed(Failure::from(error).error),
            }
        }
        Watch::Poll { conn, every } => {
            tokio::time::sleep(*every).await;
            // A server reports what changed in the selected mailbox as untagged replies to NOOP.
            let mut outcome = Event::Seen;
            let polled = conn.run("NOOP", |response| if forward(classify(response, folder)) == Event::ReceiverGone { outcome = Event::ReceiverGone; }).await;
            match polled { Ok(()) => outcome, Err(failure) => Event::Failed(failure.error) }
        }
    }
}

async fn reissue(watch: Watch, limit: Duration) -> std::result::Result<Watch, Failure> {
    match watch {
        Watch::Idle(handle) => {
            let session = timed(limit, handle.done()).await?;
            let mut handle = session.idle();
            timed(limit, handle.init()).await?;
            Ok(Watch::Idle(handle))
        }
        poll => Ok(poll),
    }
}

/// Watch `folder` until the receiver of `events` is dropped (noticed at the next event), the
/// task is cancelled, or the connection cannot be kept.
pub async fn run(inner: Arc<Inner>, folder: FolderId, events: Sender<IdleEvent>) -> Result<()> {
    let mailbox = mailbox_of(&inner.config.account, &folder)?.to_owned();
    let (mut state, mut effect) = initial();
    let mut watch: Option<Watch> = None;
    loop {
        let event = match effect {
            Effect::Finish(result) => return result,
            Effect::Connect => {
                watch = None;
                match open(&inner, &mailbox).await { Ok(opened) => { watch = Some(opened); Event::Connected } Err(failure) => Event::Failed(failure.error) }
            }
            Effect::CatchUp if events.send(IdleEvent::Exists(folder.clone())).is_err() => Event::ReceiverGone,
            Effect::Reissue => match watch.take() {
                Some(current) => match reissue(current, inner.config.timeout).await { Ok(fresh) => { watch = Some(fresh); Event::Seen } Err(failure) => Event::Failed(failure.error) },
                None => Event::Failed(MailError::Network("connection lost".into())),
            },
            Effect::Wait | Effect::CatchUp => match watch.as_mut() {
                Some(current) => wait(current, &folder, &events).await,
                None => Event::Failed(MailError::Network("connection lost".into())),
            },
        };
        effect = step(&mut state, event);
    }
}
