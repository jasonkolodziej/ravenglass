# ADR 0001: Decisions taken while building the first pass

Status: accepted for now; each item names what would reopen it. Date: 2026-11-23.

The owner asked for the whole project to be built without stopping for questions. These are the choices that were made
in place of asking, with the reason and the cost of changing course. Product decisions the owner did make are in
`docs/PLAN.md` §8.

| # | Decision | Why | Reopen when |
|---|---|---|---|
| 1 | Vendor Rune's `rune-taffy` with one extra match arm (`vendor/rune-taffy`) | Blitz enables a Taffy feature that breaks Rune's exhaustive match; without the arm they cannot link into one binary | Rune handles `FlexWrap::Balance` itself; then delete the patch |
| 2 | Blitz documents live on one render thread behind a `Renderer` handle | They are not `Send`, and a panic must cost only one view | Blitz documents become `Send`, or a worker pool is needed for speed |
| 3 | Refuse HTML taller than 60,000 px or with a non-finite size | Painting one panics inside the rasteriser | The rasteriser stops panicking; raise or drop the cap |
| 4 | "Darken" inverts lightness after painting, photos included | It needs nothing from Blitz and cannot leak | Blitz supports CSS filters well enough to exempt images |
| 5 | Allowed remote images are fetched on the engine thread, one at a time | Simplest correct thing; the UI never blocks | A slow image host is seen to delay other commands |
| 6 | IMAP folder ids are `<account>/<mailbox>`; message ids carry folder, UIDVALIDITY and UID | The store keys folders globally, and ids must survive a UIDVALIDITY change | Never, without a store migration |
| 7 | SMTP is hand-written (about 190 lines), not `lettre` | It shares the TLS setup and error mapping with IMAP | SMTP features beyond AUTH PLAIN/LOGIN/XOAUTH2, 8BITMIME and STARTTLS are needed |
| 8 | The Relay has two modes: one shared bearer token for a self-hoster, and `users` mode (password or OIDC sign-in, server-side sessions, CSRF, per-user AES-GCM secrets) for an official instance | The owner wants both (2026-11-23); see `docs/RELAY.md` | Passkeys are wanted, or more than one Relay instance must share sessions (they are in memory) |
| 9 | Relay message routes sit under `/v1/accounts/{a}/` | The Relay should never guess an id's account | Never, without a protocol version bump |
| 10 | JMAP has no push; the engine polls every five minutes | EventSource needs a streaming HTTP client | A JMAP account is in daily use |
| 11 | Drafts are plain text | Rich text needs an editor model first | The compose pane grows formatting |
| 12 | OAuth ships with no client id; ids come from the environment | Registering an app with Google or Microsoft is the owner's to do | The owner registers clients |
| 13 | The lock's portable default is a local passcode (Argon2id hash in the OS keyring), else an account password; with neither, the lock does not close | Touch ID and Windows Hello need platform code that could not be tested here; a curtain anyone can lift is not a lock (owner, 2026-11-23) | Someone can build and test on macOS and Windows |
| 14 | Bodies are cached uncompressed | zstd is a C dependency or a slower pure-Rust one; size was not a problem yet | The body cache is seen to be large |
| 15 | Code is not `rustfmt`-formatted | The handed-over code was not, and reformatting it would bury every diff | The owner picks a format and it is applied in one commit |
| 16 | OIDC ID tokens are trusted because they come straight from the token endpoint over TLS, not by checking their signature | OIDC Core 3.1.3.7 allows it, and it avoids RSA/ECDSA crates and fetching the provider's keys; iss, aud, azp, exp and nonce are checked and the userinfo `sub` must match | An identity provider is used whose token endpoint is not trusted directly |
| 17 | `mailto:` links on Windows and Linux restart the app once with the link wrapped as `ravenglass://mailto?u=…` | Rune forwards only `scheme://` links to the running instance, and `mailto:` has none | Rune forwards plain `scheme:` links |
| 18 | Packaging placeholders: bundle id `io.github.jasonkolodziej.Ravenglass`, publisher "Ravenglass contributors", MSI (WiX) on Windows | Hard to change once shipped, so chosen to be easy to keep | The owner names a domain, a publisher or a store |
