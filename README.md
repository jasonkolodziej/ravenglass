# Ravenglass

An all-Rust, cross-platform (macOS · Windows · Linux) mail client with a character-grid UI, built on
the [Rune](https://github.com/anomalyco/rune) GPU UI framework. HTML email is rendered with
[Blitz](https://github.com/DioxusLabs/blitz).

> **Status:** the desktop app runs end to end on Linux against a local test mail server. Nothing has
> been built on macOS or Windows yet, no real mail provider has been used (IMAP is tested against GreenMail, JMAP only
> against an in-process server), and the web build only type-checks. Start with **[START_HERE.md](START_HERE.md)**.

| Where | What |
|---|---|
| `START_HERE.md` | Setup and the first tasks, in order |
| `CLAUDE.md` | Rules for coding agents working in this repo |
| `docs/ARCHITECTURE.md` | End-to-end architecture: targets, crates, sync, storage, HTML rendering, web build and Relay, security, testing, roadmap |
| `docs/PLAN.md` | The full plan: decisions, architecture, HTML pipeline, UI spec, brand, themes, milestones |
| `docs/reference/` | Rune's own docs (core, rune-kit, AGENTS.md, hello example) at the pinned rev, plus links to the visual reference |
| `app/` | The Rune app (`ravenglass` binary) |
| `crates/` | See the crate table below |
| `assets/` | App icon (D · Braille, dark + light), wallpaper art, Night/Frost themes, IBM Plex Mono |
| `brand/` | Generator scripts (Python) and preview PNGs of the icon and UI |
| `spikes/blitz/` | The M1 de-risking spike brief |
| `fixtures/emails/` | Sample `.eml` files; grow to the 30-email corpus in M6 |
| `tools/` | Local Greenmail IMAP/SMTP server for integration tests |

## Crates

| Crate | Layer | State |
|---|---|---|
| `mail-core` | Core | Types, `MailBackend`, a small executor, and the `AccountSync`, `Outbox`, `ComposeSession`, `ImageLoad`, `HtmlView` and `AppLock` state machines |
| `ravenglass-theme`, `ravenglass-brand` | Core | Themes, roles, contrast fixer; importers for Ghostty, Kitty, Alacritty, Windows Terminal and iTerm2; raven geometry and dot field |
| `ravenglass-config` | Core | `config.toml`: strict parsing, defaults, validation |
| `relay-proto` | Core | Relay API v1 types, routes and version negotiation |
| `mail-store` | Adapters | SQLite schema v2, migrations, sync apply, body cache, journal, drafts, FTS5 search with operators |
| `mail-imap` | Adapters | IMAP + SMTP for any standard host: password and XOAUTH2, CONDSTORE where offered, MOVE with fallbacks, IDLE. Tested against GreenMail |
| `mail-jmap` | Adapters | JMAP Core + Mail. Tested against an in-process server only; no push |
| `mail-auth` | Adapters | Account settings, autoconfig (built-in providers, ISPDB, guesses), OAuth2 with PKCE, keyring secrets |
| `ravenglass-accounts` | Adapters | Settings + secrets to a connected backend; OAuth token refresh |
| `mail-mock` | Adapters | Deterministic backend for tests and scenes |
| `relay-client` | Adapters | `MailBackend` over the Relay API, with a native HTTP transport; no push socket yet |
| `mail-render` | Engine | MIME parse, part choice, HTML sanitizer, text view, HTML to text, message building, replies and forwards |
| `mail-blitz` | Engine | Blitz host: layout, RGBA tiles, deny-by-default images, link hit-testing, darken, render thread |
| `ravenglass-engine` | Engine | Command/Event loop: sync, push and polling, optimistic actions with journal and undo, reader, HTML views, drafts, outbox, search; desktop runner thread |
| `ravenglass-relay` | Relay | Request handling plus an HTTP/WebSocket server with a shared token and CORS; no per-user sign-in, image proxy or Web Push |
| `ravenglass` (`app/`) | App | Grid UI: list, reader (text and HTML), compose with undo send, modes, command line, palette, panes, workspaces; real accounts from a profile on disk, add-account flow, settings, user themes, app lock. `--demo` runs on sample mail |

```sh
cargo test --workspace                         # all crates
RAVENGLASS_IMAP_TEST=1 cargo test -p mail-imap # IMAP/SMTP against GreenMail (needs Java; starts it itself)
cargo clippy --workspace --all-targets -- -D warnings
cargo run -- --demo                            # the app on sample mail
cargo run -- --profile /tmp/rg                 # a fresh profile: then `:account add you@example.com`
cargo run -- --snap out/night.png --scene default --size 1400x880 --scale 2   # headless; --list-scenes for all
```

Licence: MIT OR Apache-2.0 (proposed). Bundled third-party: IBM Plex Mono (OFL, `assets/fonts/`), Rune docs (MIT, `docs/reference/rune-LICENSE`).

## Repository tooling

This repository was created from `jasonkolodziej/git-repo-template`, which contributes the agent setup around the code:

- `.github/copilot-instructions.md` and `CLAUDE.md`: shared rules for coding agents.
- `skills/`, `.agents/`: reusable agent skills.
- `graphify-out/`: a [graphify](https://github.com/Graphify-Labs/graphify) knowledge graph of the directories listed in
  `.github/graphify/targets.txt`; see [GRAPHIFY.md](GRAPHIFY.md).
- `.github/workflows/`: `ci.yml` (YAML and secret-pattern checks), `rust.yml` (build, test, lint, headless snapshots),
  `graphify.yml` (graph rebuild).
