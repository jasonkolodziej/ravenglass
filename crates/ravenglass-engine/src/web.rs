//! The web host for the engine (architecture §3, "Runtime and threading"): the same API as the
//! desktop runner (`runner.rs`), so the app's `Link` is the same code on both. Here the engine
//! runs as one `spawn_local` task on the thread that starts it (the page, today; a dedicated
//! Worker later), commands arrive over an async channel and timers are `setTimeout` promises.
//!
//! The engine never blocks: every backend call is a `fetch` or WebSocket future, so the page
//! keeps drawing while a sync is in flight. HTML layout and paint run inline on the same thread
//! (`mail_blitz::Renderer` on wasm) until the render Worker exists.

use crate::{Command, Engine, Event};
use mail_core::{AccountId, FolderId, MailBackend};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::task::{Context, Poll, Waker};
use wasm_bindgen::{JsCast, JsValue};

/// What the engine task receives. Push watchers hold a sender too, so the task is told to stop.
enum Msg { Run(Command), Stop }

pub struct Handle {
    commands: async_channel::Sender<Msg>,
    pub events: Receiver<Event>,
}

impl Handle {
    /// Queue a command. Returns false once the engine has stopped.
    pub fn send(&self, cmd: Command) -> bool { self.commands.try_send(Msg::Run(cmd)).is_ok() }

    /// Stop after the commands already queued. Nothing to join on the web: the task ends on its own.
    pub fn shutdown(self) { let _ = self.commands.try_send(Msg::Stop); }
}

impl Drop for Handle {
    fn drop(&mut self) { let _ = self.commands.try_send(Msg::Stop); }
}

/// Start the engine as a task on this thread. `wake` runs after each batch of events, to wake the
/// UI (Rune's waker, which asks the page for a frame).
pub fn spawn(mut engine: Engine, wake: impl Fn() + Send + 'static) -> std::io::Result<Handle> {
    let (commands, inbox) = async_channel::unbounded::<Msg>();
    let (outbox, events) = channel::<Event>();
    let (pushes, watching) = (commands.clone(), Rc::new(RefCell::new(BTreeSet::new())));
    wasm_bindgen_futures::spawn_local(async move {
        let mut timers: Vec<f64> = Vec::new();
        loop {
            let next = timers.iter().copied().fold(f64::INFINITY, f64::min);
            let msg = if next.is_finite() {
                match race(inbox.recv(), sleep((next - now()).max(0.0))).await {
                    Either::Left(msg) => msg.ok(),
                    Either::Right(()) => { timers.retain(|t| *t > now()); Some(Msg::Run(Command::Tick)) }
                }
            } else {
                inbox.recv().await.ok()
            };
            let cmd = match msg { Some(Msg::Run(cmd)) => cmd, Some(Msg::Stop) | None => break };
            let mut sent = false;
            for event in engine.handle(cmd, now()).await {
                match event {
                    Event::WakeAt(t) => timers.push(t),
                    Event::Watch { account, folder } => {
                        if let Some(backend) = engine.backend(&account) { watch(backend, account, folder, pushes.clone(), watching.clone()); }
                    }
                    event => { sent = true; if outbox.send(event).is_err() { return; } }
                }
            }
            if sent { wake(); }
        }
    });
    Ok(Handle { commands, events })
}

/// Hold a push connection (the Relay's WebSocket) as a task of its own and turn each push into a
/// [`Command::Refresh`]. One watcher per account; when the connection ends, the next
/// [`Event::Watch`] starts another.
fn watch(backend: Arc<dyn MailBackend>, account: AccountId, folder: FolderId, commands: async_channel::Sender<Msg>, watching: Rc<RefCell<BTreeSet<AccountId>>>) {
    if !watching.borrow_mut().insert(account.clone()) { return; }
    wasm_bindgen_futures::spawn_local(async move {
        let (pushed, pushes) = channel();
        let mut idle = pin!(backend.idle(&folder, pushed));
        // The idle future is polled exactly when its socket has news, so whatever it pushed is
        // forwarded right after each poll; nothing has to wait on the std channel.
        let _ = poll_fn(|cx| {
            let done = idle.as_mut().poll(cx);
            for _ in pushes.try_iter() {
                let _ = commands.try_send(Msg::Run(Command::Refresh(account.clone())));
            }
            done
        })
        .await;
        watching.borrow_mut().remove(&account);
    });
}

/// Seconds since the Unix epoch, from the browser's clock (`SystemTime` panics on wasm32).
pub fn now() -> f64 { js_sys::Date::now() / 1000.0 }

/// Run a future that never waits on anything outside itself (the in-memory store, the demo
/// backends) to completion on the calling thread. Headless runs use it; the browser never
/// blocks, so a future that is still pending after many polls is given up on as a bug.
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) { return out; }
    }
}

enum Either<A, B> { Left(A), Right(B) }

/// Whichever of two futures finishes first; the other is dropped.
async fn race<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    let (mut a, mut b) = (pin!(a), pin!(b));
    poll_fn(|cx| {
        if let Poll::Ready(v) = a.as_mut().poll(cx) { return Poll::Ready(Either::Left(v)); }
        if let Poll::Ready(v) = b.as_mut().poll(cx) { return Poll::Ready(Either::Right(v)); }
        Poll::Pending
    })
    .await
}

/// Resolve after `secs`, through the global `setTimeout` (a page or a Worker alike).
async fn sleep(secs: f64) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let set = js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("setTimeout")).ok().and_then(|f| f.dyn_into::<js_sys::Function>().ok());
        match set {
            Some(set) => { let _ = set.call2(&JsValue::NULL, &resolve, &JsValue::from_f64((secs * 1000.0).min(f64::from(i32::MAX)))); }
            None => { let _ = resolve.call0(&JsValue::NULL); }
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
