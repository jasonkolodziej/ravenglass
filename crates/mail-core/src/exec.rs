//! The smallest executor that works for every backend: run one future on the calling thread,
//! parking while it waits. Backends that need a reactor bring their own and only hand results
//! back through a waker, so this is enough for the engine thread, push watchers and tests.

#![cfg(not(target_arch = "wasm32"))]

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) { self.0.unpark(); }
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) { Poll::Ready(value) => return value, Poll::Pending => thread::park() }
    }
}
