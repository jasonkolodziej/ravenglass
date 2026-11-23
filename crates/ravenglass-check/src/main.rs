//! `ravenglass-check`: try a real mail account with Ravenglass's own protocol code and print a
//! report that is safe to paste into a bug report. See `docs/CHECKING-ACCOUNTS.md`.
//!
//! Read-only unless `--write` is given: it lists, examines and peeks, and never sets a flag,
//! moves, appends, deletes or sends. With `--write` it creates a folder of its own
//! (`Ravenglass-Check`), works only inside it, and deletes it again.
//!
//! What it prints: settings with the user's own domain masked, capabilities, folder roles and
//! counts, timings, sizes, MIME types and counts. Never an address, a login, a password or token,
//! a subject, a preview, a sender, a file name or the name of a folder the user made.

mod args;
mod body;
mod oauth;
mod probe;
mod settings;
#[cfg(test)]
mod tests;

use std::process::ExitCode;

/// Prints one report line: a fixed-width label, then the text.
pub fn line(label: &str, text: impl std::fmt::Display) { println!("{label:<13}{text}"); }

fn main() -> ExitCode {
    let args = match args::parse(std::env::args().skip(1)) {
        Ok(args::Parsed::Run(args)) => args,
        Ok(args::Parsed::Help) => { print!("{}", args::USAGE); return ExitCode::SUCCESS; }
        Err(problem) => { eprintln!("ravenglass-check: {problem}\n\n{}", args::USAGE); return ExitCode::from(2); }
    };
    line("ravenglass", format!("check {} — {}", env!("CARGO_PKG_VERSION"), if args.write { "WRITE mode: will create and delete a folder named Ravenglass-Check" } else { "read-only" }));
    let plan = match settings::resolve(&args) {
        Ok(plan) => plan,
        Err(problem) => { line("settings", format!("FAIL {problem}")); return ExitCode::from(2); }
    };
    let secret = match settings::secret(&args, &plan) {
        Ok(secret) => secret,
        Err(problem) => { line("login", format!("FAIL {problem}")); return ExitCode::from(2); }
    };
    let failures = probe::run(&args, &plan, secret);
    if failures == 0 {
        line("done", "all checks passed");
        ExitCode::SUCCESS
    } else {
        line("done", format!("{failures} check(s) failed"));
        ExitCode::FAILURE
    }
}

/// Run a future to completion on this thread. The backends do their own I/O on their own
/// threads; this only waits for them.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) { self.0.unpark(); }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::park(),
        }
    }
}
