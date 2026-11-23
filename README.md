# Ravenglass

An all-Rust, cross-platform (macOS · Windows · Linux) mail client with a character-grid UI, built on
the [Rune](https://github.com/anomalyco/rune) GPU UI framework. HTML email is rendered with
[Blitz](https://github.com/DioxusLabs/blitz).

> **Status:** M0 done; the engine side of the architecture is under way. The app opens and draws the wallpaper; the store,
> search, MIME parsing, HTML sanitizing, the engine and the Relay protocol have tests; the grid UI, IMAP/JMAP, Blitz
> rendering and the web build are not started. Start with **[START_HERE.md](START_HERE.md)**.

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
| `mail-core` | Core | Types, `MailBackend`, and the `AccountSync`, `Outbox`, `ComposeSession`, `ImageLoad` and `HtmlView` state machines |
| `ravenglass-theme`, `ravenglass-brand` | Core | Themes, roles, contrast fixer, Ghostty import; raven geometry and dot field |
| `relay-proto` | Core | Relay API v1 types, routes and version negotiation |
| `mail-store` | Adapters | SQLite schema v1, migrations, sync apply, body cache, journal, FTS5 search with operators (native only) |
| `mail-mock` | Adapters | Deterministic backend for tests and scenes |
| `relay-client` | Adapters | `MailBackend` over the Relay API; no HTTP transport or push yet |
| `mail-imap`, `mail-jmap`, `mail-auth` | Adapters | Not started |
| `mail-render` | Engine | MIME parse, part choice, HTML sanitizer, text view, HTML to text |
| `mail-blitz` | Engine | Not started (spike first: `spikes/blitz/`) |
| `ravenglass-engine` | Engine | Command/Event loop: sync, optimistic actions with journal and undo, reader, outbox, search; desktop runner thread |
| `ravenglass-relay` | Relay | Request handling over a `MailBackend`; no server, sign-in or push yet |
| `ravenglass` (`app/`) | App | M0: window, themes, braille wallpaper. Not yet connected to the engine |

```sh
cargo test --workspace                         # all crates
cargo clippy --workspace --all-targets -- -D warnings
cargo run -- --snap out/night.png --size 1400x880 --scale 2
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
