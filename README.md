# Ravenglass

An all-Rust, cross-platform (macOS · Windows · Linux) mail client with a character-grid UI, built on
the [Rune](https://github.com/anomalyco/rune) GPU UI framework. HTML email is rendered with
[Blitz](https://github.com/DioxusLabs/blitz).

> **Status:** pre-M0. Planning and brand are done; the code is a scaffold. Start with **[START_HERE.md](START_HERE.md)**.

| Where | What |
|---|---|
| `START_HERE.md` | Setup and the first tasks, in order |
| `CLAUDE.md` | Rules for coding agents working in this repo |
| `docs/ARCHITECTURE.md` | End-to-end architecture: targets, crates, sync, storage, HTML rendering, web build and Relay, security, testing, roadmap |
| `docs/PLAN.md` | The full plan: decisions, architecture, HTML pipeline, UI spec, brand, themes, milestones |
| `docs/reference/` | Rune's own docs (core, rune-kit, AGENTS.md, hello example) at the pinned rev, plus links to the visual reference |
| `app/` | The Rune app (`ravenglass` binary) |
| `crates/` | `mail-core` (types, backend trait, state machines), `mail-mock` (seed data), `ravenglass-theme`, `ravenglass-brand`, and stubs for `mail-store`, `mail-imap`, `mail-auth`, `mail-render`, `mail-blitz` |
| `assets/` | App icon (D · Braille, dark + light), wallpaper art, Night/Frost themes, IBM Plex Mono |
| `brand/` | Generator scripts (Python) and preview PNGs of the icon and UI |
| `spikes/blitz/` | The M1 de-risking spike brief |
| `fixtures/emails/` | Sample `.eml` files; grow to the 30-email corpus in M6 |
| `tools/` | Local Greenmail IMAP/SMTP server for integration tests |

Licence: MIT OR Apache-2.0 (proposed). Bundled third-party: IBM Plex Mono (OFL, `assets/fonts/`), Rune docs (MIT, `docs/reference/rune-LICENSE`).

## Repository tooling

This repository was created from `jasonkolodziej/git-repo-template`, which contributes the agent setup around the code:

- `.github/copilot-instructions.md` and `CLAUDE.md`: shared rules for coding agents.
- `skills/`, `.agents/`: reusable agent skills.
- `graphify-out/`: a [graphify](https://github.com/Graphify-Labs/graphify) knowledge graph of the directories listed in
  `.github/graphify/targets.txt`; see [GRAPHIFY.md](GRAPHIFY.md).
- `.github/workflows/`: `ci.yml` (YAML and secret-pattern checks), `rust.yml` (build, test, lint, headless snapshots),
  `graphify.yml` (graph rebuild).
