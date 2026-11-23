# Handoff plan: Ravenglass, a Glasshouse-style mail client on Rune

**Audience:** the coding agent that will build this. Read the whole file before writing code.
**Owner:** Jason. **Name:** **Ravenglass**. Crate, binary and repo: `ravenglass` (unclaimed on crates.io as of 2026-11-13, along with `raven-glass`). Bundle/display name: `Ravenglass`. The internal workspace crates keep their `mail-*` names with `publish = false`. Check the GitHub org and domain before publishing.

> *Name rationale:* ravens are the Norse messengers (Odin's Huginn and Muninn bring him the world's news): mail, and a nod to the **Rune** framework it's built on. *Glass* makes it a sibling of **Glasshouse** and names the design language. Don't shorten it to "Raven" publicly: an open-source team chat app already uses that name and the `raven` crate is taken. Icon, watermark and palette: see §4 "Brand" and the `ravenglass-brand/` bundle.

**Status:** starter repo delivered (`ravenglass/`). The pure crates (`mail-core`, `mail-mock`, `ravenglass-theme`, `ravenglass-brand`) compile with 18 passing tests. `app/` is an M0 scaffold, not yet compiled (needs Rust 1.99.1). Begin with `START_HERE.md`.

### Decisions locked by Jason (2026-11-13)
- **All Rust.** No C/C++/Objective-C engines, no system webviews.
- **OS-agnostic.** macOS, Windows and Linux are first-class from v1. Platform code is limited to optional enhancements behind `platform/`, each with a portable default.
- **HTML email is rendered with [Blitz](https://github.com/DioxusLabs/blitz)** (DioxusLabs; Stylo + Taffy + Parley, no JavaScript). See §3.

---

## 0. Context you need first

### What Rune is
[`anomalyco/rune`](https://github.com/anomalyco/rune) is a GPU-rendered Rust UI framework (wgpu + winit, Taffy layout, Parley text, its own springs). It targets macOS, Windows, Linux and the web. Views are plain Rust functions: `fn view(s: &State, cx: &mut Cx<State>) -> El`. Every app can run **headless**, so you can render PNGs, dump the element tree as JSON and drive the app with scripted input. Use that to check your own work.

Read these in the Rune repo before you start:

| File | Why |
|---|---|
| `README.md` | Overview and how to depend on Rune |
| `AGENTS.md` | Repo conventions. Follow them in this app too (file sizes, state machines, performance rules) |
| `crates/rune/README.md` | Core API. Read at least: "What exists", "Rich text, scrolling and app hooks", "Launching fast", "Requests off the main thread (`rune::Latest`)", "State machines", "Text fields", "Pop-up and context menus", "System materials", "Frosted panels", "Backdrop blur", "Agent tooling" |
| `crates/rune-kit/README.md` | Components (TextArea, TextInput, PopupList, Combobox, Toast, Markdown). The **App shell** (`SplitView`, `PaneState`) is useful for pane resizing logic, but the grid look is custom (§4) |
| `examples/hello` | Smallest app; start here |
| `rune-kit --example shell` | Closest existing thing to a three-pane mail layout |
| `examples/glass-panel`, `examples/frosted-bar`, `examples/settings`, `examples/todo` | Glass materials, backdrop blur, a full settings window, a list app |

**Rev to pin:** `dev` @ `0ca075619d116f8f6e0e3bc8ff815c72c636cac8` (2026-11-13). `dev` is never rewritten.

### Toolchain gotcha
Rune pins Rust **1.99.1** in `rust-toolchain.toml`. The sandbox this plan was written in had only 1.97.0 and couldn't download 1.99.1. On each dev machine and in CI, run `rustup toolchain install 1.99.1`. On Linux, also install `libxkbcommon-dev libwayland-dev` (build) and `libxkbcommon-x11-0 mesa-vulkan-drivers` (windowed runs without a GPU), per Rune's README. Give the app its own `rust-toolchain.toml` with the same channel.

### Visual reference (primary): the unstablebuild/rune IDE
Jason's reference video (`github.com/user-attachments/assets/4ab84f7f-…`) is the demo of **[unstablebuild/rune](https://github.com/unstablebuild/rune)**. That project is a GPU-rendered IDE and terminal multiplexer written in **Go, licensed GPL-3**, unrelated to the Rust `anomalyco/rune` framework despite the name. **Ravenglass should look and feel like it.** Use it as a visual and interaction reference only: don't copy code (licence and language). Its screenshots are in that repo at `.github/assets/rune-screenshot.webp` and `docs/static/img/intro/*.png`. What to match:

- **A character-grid interface.** Everything is laid out on a monospace cell grid, as if it were a TUI, but drawn natively on the GPU (not inside a terminal). Text is the primary visual element; chrome is minimal and flat.
- **Window chrome:** traffic lights, then **dot-prefixed tabs** in the title bar (`•  Inbox — iCloud   •  Re: Morning dispatch`). The active tab is bright and the rest are muted.
- **Split panes** separated by 1-cell gutters. The focused pane is marked by a thin **accent bar** on its edge. Each pane can have a **pane title bar** (muted fill, centred title, right-aligned meta).
- **A vim-style status line** per pane: a coloured **mode block** (`NORMAL`, `COMPOSE`, `SEARCH`, `VISUAL`), then context segments and a coloured filetype-style block on the right (`mail`).
- **Line-number gutters** with relative numbers, used here for message-list rows.
- **A right-hand icon rail** of thin stroke icons for global actions.
- **A bottom workspace bar** of numbered slots (`1 iCloud  2 Gmail  3 Work  5 receipts`), switched with `1`–`9`. In Ravenglass a slot is an account, a mailbox or a saved search.
- **A wallpaper in every empty pane:** the app's mark, optionally as braille/ASCII art, with an **intro** where braille dots fly in and assemble the logo at launch. Ravenglass does the same with the raven (§4 Brand).
- **Themes are remaps of 16 named colours** plus background/foreground/cursor, switched live with a command. Ravenglass adopts this model (§4 Default palette, §4 Theme customization).
- Prompt boxes are outlined rectangles; links are underlined in the accent colour; quoted and secondary text uses the muted colour.

### Visual reference (secondary): Glasshouse
Jason's macOS terminal app **Glasshouse** contributes the *glass*, but only as an option: a translucent, blurred **window background** (`window_opacity` < 1 with `window_blur`) behind the grid, as terminals do. The default is opaque, matching the IDE. Translucency switches off under Reduce Transparency / Increase Contrast. Text never sits directly on a busy backdrop: panes keep a minimum backing opacity.

Shortcuts are **modal first** (vim-style keys in NORMAL mode, like the reference), with the same actions also on Mail.app chords (`mod+…` through Rune's `Chord`, so Ctrl on Windows/Linux).

---

## 1. Scope

### v1 (this plan)
- **macOS, Windows and Linux (X11 and Wayland) in v1**, all built and tested in CI. Platform-specific code lives only in `platform/{macos,windows,linux}.rs`. Each feature there is an enhancement over a portable default (e.g. a native translucent window material over Rune's backdrop blur, Quick Look over the built-in preview).
- **Multiple accounts over IMAP + SMTP.** Generic accounts use a password or app password. Gmail and Outlook use OAuth2.
- **A character-grid, pane-based UI** like the reference IDE (§0, §4): message list, reader and compose as splittable panes, with workspaces for accounts, mailboxes and saved searches.
- **Messages:** read, reply, reply all, forward, compose, attachments in and out, drafts, archive, delete, move, flag, mark read/unread.
- Local search (SQLite FTS5) with operators: `from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `before:`/`after:`.
- A command palette for every action.
- **Push:** IMAP IDLE on INBOX, with polling for other folders.
- Undo send (a 10 s delay, configurable) and an undo toast for destructive actions.
- **Privacy:** remote images are blocked by default, with a per-sender allow list. Tracking pixels are stripped.

### Explicitly later
JMAP and the Gmail API, rules/filters, unified smart mailboxes beyond "All Inboxes", calendar invites (show `.ics` as an attachment only), S/MIME/PGP, a biometric app lock (Touch ID / Windows Hello; a nice tie-in to Glasshouse, but not v1), iOS and the web. The web (wasm32) is a real later target since Rune and Blitz both support it, so keep `mail-store` and persistence APIs free of `std::fs` per Rune's rules.

---

## 2. Architecture

### Workspace layout
```
ravenglass/
  rust-toolchain.toml            # channel = "1.99.1"
  Cargo.toml                     # workspace + [patch.crates-io] copied from Rune (see §2.4)
  crates/
    mail-core/                   # domain types, MailBackend trait, sync state machines. No UI, no I/O impls
    mail-store/                  # SQLite cache (rusqlite, bundled, FTS5), migrations, queries
    mail-imap/                   # IMAP + SMTP backend: async-imap + rustls, lettre for SMTP
    mail-auth/                   # OAuth2 (loopback redirect), credentials via `keyring` (Keychain / Credential Manager / Secret Service)
    mail-render/                 # MIME → body model: text/plain → Rune rich text; sanitized HTML → Blitz document
    mail-blitz/                  # Blitz host: document lifecycle, net provider (image policy), off-thread raster → rune::Pixels, hit-testing
    ravenglass-theme/            # themes: 16 names + roles, user overrides, contrast fixer, terminal-scheme import (done: tests pass)
    ravenglass-brand/            # raven geometry + braille dot field for wallpaper/intro (done: tests pass)
    mail-mock/                   # deterministic fake backend + seed data for headless scenes and tests
    app/                         # crate and binary `ravenglass`: the Rune app: views, theme, shortcuts, scenes, scripts
      src/
        main.rs                  # AppSpec, run()
        state.rs                 # AppState and selection model
        grid.rs                  # cell metrics + snapping helpers
        panes.rs                 # split tree, focus, resize, per-workspace layout
        modes.rs                 # NORMAL/VISUAL/SEARCH/COMPOSE state machine + command line
        view/{mod,tabs,rail,workspaces,statusline,list,reader,compose,palette,search,toast,wallpaper,settings}.rs
        theme/{mod,editor,contrast,store}.rs  # token Theme struct, Appearance editor, contrast fixer, user-theme storage
        keys.rs                  # Chord table (single source for shortcuts, menus, palette)
        actions.rs               # Action enum: what palette, menu and shortcuts all dispatch
        platform/{mod,macos,windows,linux}.rs  # optional enhancements; mod.rs holds the portable defaults
                                 # (transparency/contrast settings, menu bar, file preview, notifications)
      scenes/ scripts/           # headless scenes and input scripts (see §5)
  assets/                        # copied from the ravenglass-brand/ bundle (§4 Brand)
    icon/                        # ravenglass-icon-D_braille{,-light}.svg (chosen; A–C kept for reference)
    watermark/                   # ravenglass-wallpaper-braille{,-small}.txt + ravenglass-watermark-{etched,silhouette}.svg
    themes/                      # ravenglass-night.toml, ravenglass-frost.toml (built-in themes)
```

### Data flow
- **UI thread:** the view reads `AppState` only. Handlers mutate state and emit `Action`s. Never block or do I/O in the frame loop (`AGENTS.md`).
- **Sync engine** (`mail-core`): one actor per account on a Tokio runtime started from `on_ready`, after the first frame. It talks to `MailBackend` and writes to `mail-store`. It posts deltas back through `rune::Latest`/`Tx` wakes; use `tx.instant()` for state that lands just after launch.
- **Explicit state machines** (`rune::machine` style: one `enum State`, one `fn step(&mut State, Event) -> Effects<Effect>`). No boolean soup:
  - `AccountSync`: `Disconnected → Connecting → Authenticating → Syncing{folder, progress} → Idle(IDLE) → Backoff{attempt, due}`
  - `Outbox`: `Queued{undo_until} → Sending → Sent | Failed{error, attempt}`
  - `ComposeSession`: `Editing → SavingDraft → Editing | Sending → Closed`
  - `ImageLoad` per message: `Blocked → Allowed → Loading → Loaded | Failed`
- Timers are events (`Tx::wake_at`). Undo-send, backoff and draft autosave all go through `step`.

### `MailBackend` trait (`mail-core`)
```rust
#[async_trait]
pub trait MailBackend: Send + Sync {
    async fn list_folders(&self) -> Result<Vec<Folder>>;
    async fn sync_folder(&self, folder: &FolderId, since: SyncToken) -> Result<FolderDelta>; // CONDSTORE/QRESYNC when available
    async fn fetch_body(&self, msg: &MessageId) -> Result<RawMessage>;                       // lazy, on open
    async fn set_flags(&self, msgs: &[MessageId], add: Flags, remove: Flags) -> Result<()>;
    async fn move_to(&self, msgs: &[MessageId], to: &FolderId) -> Result<()>;               // MOVE, else COPY+EXPUNGE
    async fn append_draft(&self, raw: &[u8]) -> Result<MessageId>;
    async fn send(&self, raw: &[u8], envelope: Envelope) -> Result<()>;                      // SMTP, then APPEND to Sent unless the server does it (Gmail)
    fn idle(&self, folder: &FolderId) -> BoxStream<'static, IdleEvent>;
}
```
`mail-mock` implements it with seeded, deterministic data (fixed dates, stable ids), so headless snapshots are reproducible.

### Suggested crates (check current versions)
`async-imap` + `tokio-rustls` · `lettre` (SMTP, rustls) · `mail-parser` and `mail-builder` (Stalwart) · `rusqlite` (`bundled`, FTS5) · `oauth2` · `keyring` (cross-platform credential store) · `ammonia` (HTML sanitize) · **`blitz-dom`, `blitz-html`, `blitz-paint`, `blitz-traits`** + an `anyrender` CPU backend (Blitz HTML rendering, §3) · `html5ever` (fallback text extraction) · `open` (open files/URLs with the system handler) · `notify-rust` (desktop notifications) · `chrono` · `tokio` · `tracing`. Pin Blitz to an exact version or git rev; it is pre-1.0 (0.3.0 alphas as of mid-2026).

**Dependency hygiene:** Blitz and Rune both depend on Taffy and Parley. Rune patches `parley_engine`, so the patch applies to Blitz's copy too. Run `cargo tree -d` and make sure there is one Parley and as few Taffy versions as possible. If Blitz's pins conflict with Rune's, resolve it in the M1 spike (§3.5) before building on it.

### Rune `[patch.crates-io]` (required)
An app must patch the same crates Rune patches, **from the same rev**, or it builds against stock crates:
```toml
[dependencies]
rune     = { git = "https://github.com/anomalyco/rune", rev = "0ca0756…" }
rune-kit = { git = "https://github.com/anomalyco/rune", rev = "0ca0756…", features = ["markdown"] }

[patch.crates-io]
winit             = { git = "https://github.com/anomalyco/rune", rev = "0ca0756…" }
parley_engine     = { git = "https://github.com/anomalyco/rune", rev = "0ca0756…" }
wgpu-hal          = { git = "https://github.com/anomalyco/rune", rev = "0ca0756…" }
accesskit_windows = { git = "https://github.com/anomalyco/rune", rev = "0ca0756…" }
```
Re-check this list against Rune's root `Cargo.toml` whenever you bump the rev. Release builds also need `features = ["tooling"]` on `rune` for the headless flags; debug builds have them always.

---

## 3. The hard problem: rendering HTML email

Rune has no web engine. **Decision: render HTML email with Blitz**, a pure-Rust HTML/CSS engine with no JavaScript. Emails don't run JS in any client, so that's a feature here: a smaller attack surface and a smaller binary than Servo. It works on every OS Rune targets. It uses Taffy and Parley like Rune, so text layout behaves similarly.

### 3.1 Pipeline (`mail-render` → `mail-blitz` → reader)
1. **Pick the part.** Prefer `text/html` when present; otherwise use `text/plain`. Add a per-account setting "Prefer plain text".
2. **Plain text** never touches Blitz. Render it with Rune `rich()`, linkify URLs, and fold quoted `>` blocks into a "Show quoted text" `Collapsible`.
3. **Sanitize HTML** with `ammonia` *before* Blitz sees it:
   - Strip `script`, `iframe`, `object`, `embed`, `form`/inputs, `meta refresh`, `base`, event-handler attributes and `javascript:` URLs.
   - Keep inline `style` and `<style>` blocks (emails depend on them), but strip `@import`, `url()` to remote hosts, and `position: fixed`.
   - Rewrite `cid:` references to an internal `cid://<message>/<content-id>` scheme.
   - Detect tracking pixels (1×1 images, known tracker hosts) and drop them.
4. **Load into Blitz.** Create a `blitz-dom` document from the sanitized string with a **custom net provider** (`blitz-traits`) that:
   - serves `cid://` parts from the message (already in `mail-store`);
   - **blocks every remote request by default**. While `ImageLoad` is `Blocked`, it returns an empty or placeholder response. Once the user allows images, it fetches through the app's own HTTP client, with a byte cap per image and per message, a timeout, and no cookies or referrer;
   - never fetches remote CSS or fonts, so there is nothing to leak.
5. **Inject a user-agent stylesheet** first: the theme's text colour, background and link colour, `max-width: 100%` on `img`/`table`, `word-break: break-word` and a system font stack mapped to Inter. Dark mode: render emails in their own colours on a light "paper" card by default (most HTML mail assumes white). Offer an opt-in "Darken emails" that injects an inverting stylesheet, as Apple Mail and Outlook do.
6. **Lay out and raster off the UI thread.** Resolve style and layout at the reader's content width and device scale. Paint with `blitz-paint` into an **`anyrender` CPU backend**, producing an RGBA buffer, and hand it to **`rune::Pixels::set(w, h, rgba)`**. Rune's Pixels API is portable, thread-safe, wakes only the windows that show it, and does a row upload for same-size updates. That makes it the portable bridge. A GPU path (Blitz's wgpu-texture integration feeding the same element) is a later optimisation. Keep it behind a trait so the swap is local.
7. **Embed in the reader** as `el().pixels(&pixels)` sized to the document's laid-out height, inside the reader's scroll view. For very tall emails, raster in **tiles** (e.g. 1024 px high), render only the tiles near the viewport, and cap the total bytes (`AGENTS.md`: bound caches by decoded pixels).
8. **Interaction** (Rune → Blitz):
   - **Pointer:** use `on_press`/`on_drag`/hover with element-local coordinates (`on_secondary_press_local` for the context menu) and forward them to Blitz's hit-testing to resolve links. Hover shows the link URL in a status line. Click opens it with `open` after a confirm if the visible text and the href domain differ (a phishing guard). Set the cursor to `Pointer` over links.
   - **Text selection and copy:** v1 can offer "Copy text" (DOM text) and "Select all". Drag-selection inside the Blitz content is a stretch goal, depending on Blitz's selection support at the pinned version.
   - **Find in message (⌘F inside the reader):** search the DOM text and highlight matches by injecting `<mark>` spans, then re-raster.
9. **Re-raster triggers:** width change (debounced to the end of a pane resize; the stale bitmap scales during the drag), scale change, theme change, image policy change, and find highlights. Each is an event in an `HtmlView` state machine: `Parsing → LaidOut{w} → Rastering{tiles} → Ready | Failed(err)`.

### 3.2 Fallbacks
- **Blitz fails or panics:** catch it at the worker boundary (`catch_unwind`), log the message id only, and fall back to the HTML → Markdown → `rune-kit::Markdown` path. That is the unstablebuild approach: flatten layout tables, keep tables with header cells. Show a small "Simplified view" chip.
- **"Open in browser":** write the *sanitized* HTML to a temp file and open it with `open`. Always available from the ⋯ menu.

### 3.3 Security notes
Sanitization is the primary defence and Blitz is defence in depth. There is no JS engine, the net provider has a deny-by-default policy, and nothing loads without explicit consent. Fuzz `mail-render` (`cargo fuzz`) against the sanitizer + Blitz parse path.

### 3.4 Acceptance
A corpus of 30 real-world emails in `mail-mock/fixtures/` renders headlessly to PNG with no panics: newsletters (heavy table layouts), receipts, GitHub notifications, marketing mail with media queries, plain replies, RTL, CJK and a malformed-HTML set. Compare each against a reference screenshot from a browser and have Jason review 10. Rasterizing a typical email at 2× takes < 50 ms on a laptop; first paint of a 5 MB newsletter takes < 250 ms with tiling.

### 3.5 De-risking spike (do this in M1, before the UI work)
In a throwaway crate, load 5 fixture emails into Blitz at the pinned rev, raster them through the CPU backend, display them in a Rune window via `Pixels`, and forward clicks to resolve links. Confirm it builds on all three OSes in CI with Rune's `[patch]` table. Report back on any dependency conflicts, CSS gaps that matter for email (tables, `bgcolor`/`align` presentational attributes, media queries) and the raster times. If presentational HTML attributes are missing, add a pre-pass in `mail-render` that converts them to inline CSS.

---

## 4. UI specification

### The grid (how to build a TUI look with Rune)
Rune is a GPU widget framework, not a terminal, so the grid is a discipline the app enforces:
- **Cell metrics.** Load the UI font (default **IBM Plex Mono**, which already ships in `crates/rune/assets`) and measure one cell: `cell_w` is the advance of `0`, and `cell_h` is `font_size × line_height`, rounded to device pixels. Keep both in a `Grid` struct in state, recomputed on font/size/scale change.
- **Everything snaps to cells.** Pane widths and heights are whole cells; gutters, borders and the focus bar are one cell or a fraction drawn inside a cell. Pane resizing moves in whole cells. Use Rune `row()/col()` with sizes in `cell_w`/`cell_h` multiples, and fixed-width text runs (pad and truncate with `…` in code, not with flex).
- **Text-first components:** list rows, headers and status segments are `text`/`rich` runs with background spans (`Span` backgrounds) for blocks like the mode indicator. Real controls (buttons, inputs) are styled as bracketed or outlined cell blocks, but they're still Rune focusable elements with roles and labels, for accessibility.
- **Icons:** the right rail uses thin stroke SVG icons via Rune `icon()`, sized to 2×2 cells.
- **Motion:** keep it subtle and fast (snappy springs for selection and pane focus). The exception is the wallpaper intro (Brand). Reduce Motion skips the intro and snaps everything else.
- **Braille and box-drawing glyphs:** don't rely on the font. Draw the braille wallpaper as dot shapes (crisp at any size, and needed for the intro animation) and draw rules and borders as Rune rects.

### Layout
Previews: `ravenglass-brand/preview/ui-triage-night.png`, `ui-triage-frost.png`, `ui-wallpaper-night.png`, `ui-wallpaper-frost.png`.

```
 ● ● ●   •  Inbox — iCloud   •  Re: Morning dispatch   •  Draft: Gnomon sync          (title bar, dot tabs)
 INBOX iCloud · 248 · 12 unread · threaded        ┃ From:    Odin Allfather <odin@…>   view: html │ t text  ✎
  1 ● Odin Allfather   Re: Morning dispatch  10:40┃ To:      me                                          ⌕
      Huginn reports the northern pass is clear   ┃ Subject: Re: Morning dispatch                        ▭
  1 ● Muninn           Archive digest — week 46   ┃ ⎘ route-north.pdf · 2.1 MB                            ⚑
  2   GitHub           [anomalyco/rune] dev: …    ┃ ─────────────────────────────────────────            ◫
  …                                               ┃ body (text or Blitz HTML)                            ⚙
                                                  ┃ ⠿          reply ▸ Odin Allfather          draft ✓     ?
                                                  ┃ ┌──────────────────────────────────────────┐
                                                  ┃ │ compose box                               │
 NORMAL iCloud/Inbox ⎇ threaded ● 12 ⚑ 3  1/248 mail
 1 iCloud  2 Gmail  3 Work  5 receipts  6 from:github                         ⟳ synced 10:44 · IDLE
```

- **Panes are the layout primitive, not fixed columns.** The default workspace is a vertical split: the message list on the left, and on the right the reader with an optional compose pane below it. Users split, close and resize panes (`ctrl-w s/v/q`, `ctrl-w h/j/k/l`, drag the gutter) and the layout is saved per workspace. A narrow window collapses to one pane with `enter`/`esc` navigation.
- **Mailbox navigation** has no permanent sidebar. Use the workspace bar (`1`–`9`), `gm` (go to mailbox, a fuzzy picker), or `:mailbox name`. An optional mailbox tree pane can be split in (`ctrl-w t`) for those who want one.
- **Title-bar tabs** are open things: mailboxes, messages opened in their own tab, drafts. Use `gt`/`gT` or `ctrl-tab` to switch.
- **Message list rows:**
  - Two lines per row. Line 1: a relative-number gutter, the unread dot (`mail.unread`), the sender (bold if unread), the subject, flag/attachment glyphs and a right-aligned time. Line 2: the preview in muted text.
  - The selected row gets the `ui.selection` background and an accent bar.
  - Virtualized: use `virtual_rows` + `.virtualize(2*cell_h)` inside a scroll view, and remember the `.min_h(0.0)` gotcha.
  - Threads collapse to one row with a count; `za` toggles one open.
- **The reader:**
  - Headers are key/value cell columns (`mail.header.key` keys). Attachments are a line of `⎘ name · size` chips: `enter` previews, `o` opens with the system handler, `S` saves; drag out to the file manager.
  - The right of the header shows `view: html │ t text`. `t` toggles between the **Blitz HTML view** (§3, a pixel region inside the pane, on its paper card) and the **text view** (sanitized HTML → Markdown → grid text). The text view is the native look of this UI, so a per-user setting picks the default view; the text view is always used for plain-text mail. Quoted text is muted and folded past 3 lines (`zo` opens it).
  - The blocked-images notice is one status line in the reader: `remote images blocked · i load once · I always for sender`.
- **Compose** is a pane (below the reader for replies, or a full tab for new mail) with a pane title bar (`reply ▸ Odin Allfather … draft ✓`). Headers are editable grid fields with address completion in a popup list. The body uses `TextArea`/`TextEdit::rich()` in the outlined prompt-box style. Its mode is `COMPOSE`: `esc` returns to NORMAL, `⇧⌘D` or `:send` sends, and the undo-send countdown shows in the status line.
- **Command line and palette:**
  - `:` opens a vim-style command line in the status line (`:archive`, `:move Receipts`, `:theme frost`, `:search from:github is:unread`).
  - `⇧⌘P` opens the fuzzy palette. It's a centred outlined box listing every `Action` with its keys, and it's the same action table as the command line.
- **Search:** `/` puts the active list into `SEARCH` mode with the query in the status line. Results stream into the list, and operator tokens are highlighted in the query.
- **Notifications and undo** are one-line toasts above the workspace bar (`archived 3 · u undo`).

### Keys (`keys.rs`: one table; NORMAL mode first, chords second)
| Action | NORMAL key | Chord |
|---|---|---|
| Move / open / back | `j` `k` / `enter` / `esc` | ↓ ↑ / Return |
| Top / bottom | `gg` / `G` | `mod+↑` / `mod+↓` |
| Archive / delete | `e` / `#` | `ctrl+mod+a` / Backspace |
| Toggle read / flag | `U` / `s` | `mod+shift+u` / `mod+shift+l` |
| Reply / reply all / forward | `r` / `R` / `f` | `mod+r` / `mod+shift+r` / `mod+shift+f` |
| Compose / send | `c` / `:send` | `mod+n` / `mod+shift+d` |
| Search / command / palette | `/` / `:` / — | `mod+f` / — / `mod+shift+p` |
| Move to mailbox / go to mailbox | `m` / `gm` | `mod+ctrl+m` |
| Toggle HTML ↔ text | `t` | — |
| Workspaces | `1`–`9` | `mod+1`–`mod+9` |
| Panes | `ctrl-w` + `h j k l s v q t` | — |
| Visual select (multi) | `v`, then `j`/`k` | Shift/⌘-click |
| Undo | `u` | `mod+z` |

Users can rebind every key in the config (`[keys]` table), validated with `Chord::same_keys` for clashes. Menus, the palette, the command line and keys all dispatch the same `Action` enum.

### Brand

All brand assets are in the **`ravenglass-brand/`** bundle delivered with this plan. Copy them into `app/assets/`. The generator scripts are in `ravenglass-brand/src/` (Python: `raven.py` defines the geometry, and the others build the icons, wallpaper, palette checks and UI mocks).

**The mark.** A faceted (low-poly) raven bust in profile, facing right. It has a heavy beak, shaggy throat hackles and a small ice-blue eye. The facets read as cut glass (the Glasshouse tie), and they turn naturally into dots and lines for the character-grid UI. The geometry is defined once (outline polygon + interior points, Delaunay-triangulated), so every variant stays identical.

**App icon: D · Braille (decided 2026-11-13).** The raven is built from braille dots, so the icon, the empty-pane wallpaper and the launch intro are one mark.

| Variant | File | Look | Used when |
|---|---|---|---|
| **Dark (default)** | `icon/ravenglass-icon-D_braille.svg` | Ice-blue (`#A9DCFF`) glowing dots on near-black `#0E0C1A`, white eye | Default everywhere; dark OS appearance |
| **Light** | `icon/ravenglass-icon-D_braille-light.svg` | Deep-violet (`#2F2A80`) dots on a frost gradient (`#FFFFFF`→`#DCE0F4`, faint ice mist), ice-blue `#1F6FBF` eye, no glow | Light OS appearance where the platform supports it; Frost-theme About/onboarding screens |

Per platform:
- **macOS 26+:** build one Icon Composer `.icon` with **Default = dark and a light appearance from the light variant**, plus a tinted/mono appearance using the dot layer alone. Older macOS gets the dark `.icns`.
- **Linux:** ship the dark variant as the main `hicolor` icon and the light variant as `ravenglass-light`, for icon themes and light panels that use it.
- **Windows:** `.ico` from the dark variant (Windows has no per-appearance app icons). Tile and taskbar assets use the dark variant.
- **In-app** (About, onboarding, notifications): pick the variant matching the active theme's `appearance`.
- **Small sizes:** both variants hold at 64 and 32 px. At **≤ 24 px, regenerate with a coarser dot grid** (`src/icon_d.py`, lower `rows`, larger dots) instead of downscaling, or the beak fills in. Export 16, 24, 32, 48, 64, 128, 256, 512 and 1024 from the generator.

Rejected options A (Through the Pane), B (Etched) and C (Messenger) stay in `icon/` for reference only. B's etched-line art remains available as a wallpaper style.

**Wallpaper (the reference IDE's "wallpaper" pattern).** Every pane with nothing to show draws the raven: the empty reader, an empty mailbox, offline/first run and closed-tab panes.
- **Default `braille`:** the faceted raven as braille dots (`watermark/ravenglass-wallpaper-braille.txt` is the 50×26-cell reference rendering; `-small.txt` is for small panes). **Render the dots as Rune shapes** (circles on the 2×4-dots-per-cell grid), not font glyphs. That keeps them crisp at any size, avoids missing braille glyphs (IBM Plex Mono has none), and makes the intro possible. Generate the dot field at runtime from the polygon data for the pane's size, so the raven scales to the pane.
- **Other options:** `etched` (`watermark/ravenglass-watermark-etched.svg`, facet lines), `silhouette` (`…-silhouette.svg`), `none`, or a user-supplied PNG/ASCII file (like the reference IDE).
- **Colour:** the `wallpaper` role (default palette `blue`) at `wallpaper_opacity`. The wallpaper sits under content only and is decorative: no a11y label, no hit-testing (`.inert()`).
- **Under it, centred:** `R A V E N G L A S S` (letter-spaced), then a muted line (`No message open`) and a key-hint line (`/ search   ⇧⌘P commands   1–9 workspaces   c compose`).
- **Intro:** on first window open, the dots fly in from random positions and settle into the raven over ~1.2 s on springs (`rune-motion`), then the facet edges brighten once. Play it once per launch, skip it under Reduce Motion, and turn it off with `intro = false`. Script it as a headless scenario so `--video` can record it and `scripts/motion-audit` can check it.

### Default palette

Ravenglass adopts the reference IDE's colour model: **every colour the UI draws is one of 16 named palette colours plus `background`, `foreground` and `cursor`.** A `[roles]` table maps UI parts to palette names, so remapping the palette re-themes everything, and any terminal colour scheme can be imported. There are two built-in themes: **Night** (default) and **Frost** (light). "Follow system" switches between them. All foreground colours pass **≥ 4.5:1** against `background` in both themes. Foreground on the `navy` selection is ≥ 12:1; mode-block text on `aqua` is 13.2:1 (Night) and 4.7:1 (Frost).

| Name | Night (default) | Frost (light) | Used for |
|---|---|---|---|
| `background` | `#0E0C1A` | `#F4F5FB` | Default background |
| `foreground` | `#ECEAF6` | `#17142A` | Default text |
| `cursor` | `#A9DCFF` | `#1F6FBF` | Text cursor |
| `black` | `#0E0C1A` | `#F4F5FB` | Same as background (ANSI slot) |
| `gray` | `#1D1930` | `#E2E4F3` | Chrome: pane bars, status line, borders |
| `silver` | `#8A85AD` | `#6B6787` | Muted text: previews, timestamps, quotes |
| `white` | `#ECEAF6` | `#17142A` | Same as foreground (ANSI slot) |
| `maroon` | `#B04A5A` | `#A12A3E` | Dim red |
| `red` | `#FF7A8A` | `#C8344B` | Errors, delete |
| `green` | `#4FA37A` | `#1C6B42` | Dim green |
| `lime` | `#7FD6A6` | `#22804F` | OK/sent, COMPOSE mode |
| `olive` | `#B08A3E` | `#7A5005` | Dim yellow |
| `yellow` | `#F2C46D` | `#93600A` | Flags, warnings, SEARCH mode |
| `navy` | `#2A2350` | `#D9DBF2` | Selection background, glass tint |
| `blue` | `#9C99E6` | `#4E48A0` | Brand violet: header keys, attachments, wallpaper |
| `purple` | `#4E48A0` | `#3A3580` | Deep brand violet |
| `fuchsia` | `#D3A6FF` | `#8A3FBF` | VISUAL mode, highlights |
| `teal` | `#3E8FB8` | `#155A9E` | Secondary accent |
| `aqua` | `#A9DCFF` | `#1F6FBF` | Ice accent: unread, links, focus bar, NORMAL mode |

Default roles: `ui.bg`→`background`, `ui.text`→`foreground`, `ui.muted`→`silver`, `ui.chrome`→`gray`, `ui.border`→`gray`, `ui.selection`→`navy`, `ui.accent`→`aqua`, `ui.focusBar`→`aqua`, `ui.brand`→`blue`, `mode.normal`→`aqua`, `mode.compose`→`lime`, `mode.search`→`yellow`, `mode.visual`→`fuchsia`, `mail.unread`→`aqua`, `mail.flag`→`yellow`, `mail.attachment`→`blue`, `mail.quote`→`silver`, `mail.link`→`aqua`, `mail.header.key`→`blue`, `state.ok`→`lime`, `state.warn`→`yellow`, `state.error`→`red`, `wallpaper`→`blue`, `glass.tint`→`navy`.

Files: `theme/ravenglass-night.toml` and `theme/ravenglass-frost.toml` hold the palette, the roles and `[ui]` (font, size, line height, window opacity/blur, wallpaper, intro). Map them onto Rune at load time: build `Kit::from_palette(PaletteSeed { bg: background, fg: foreground, accent: aqua })` for rune-kit components, and resolve every app colour through `roles`.

### User theme customization (required, not optional)

Users must be able to make Ravenglass their own:

1. **Theme = data.** No hard-coded colours anywhere in views. Views ask for a *role* (`mail.unread`), the role resolves to a palette name, and the name resolves to a colour. Night and Frost ship read-only.
2. **Small themes.** A user theme is a TOML file that overrides any subset of `[palette]`, `[roles]` and `[ui]`, over a named base (`base = "ravenglass-night"`). Two lines can be a valid theme.
3. **Import terminal schemes:** Ghostty, Alacritty (TOML), iTerm2 (`.itermcolors`), Kitty, Windows Terminal JSON, and the reference IDE's YAML `gui.themes` entries all map onto the 16 names + bg/fg/cursor. Provide `:theme import <file>`.
4. **Switch and reload live:** `:theme <name>` and the palette's "Theme: …" entries switch instantly (colour transitions on `Tx::color_motion(calm)`). Theme files are watched and hot-reloaded (debounced, off the UI thread). `:colors` shows a live palette preview pane, like the reference's `colorPalette`.
5. **Settings → Appearance** (a grid-styled page) for people who don't want to edit files:
   - pick the theme, plus separate light/dark choices when following the system;
   - a swatch editor for each palette name (hex/OKLCH input);
   - a roles editor (which name each UI part uses);
   - font family and size, line height, window opacity and blur (Glasshouse glass), and wallpaper style/opacity/intro;
   - the HTML-email default view (HTML or text), and darken-HTML in dark themes.
   - "Save as…" writes a TOML file. Export and import share the same format.
6. **Guardrails:** show the contrast of each role's colour against its background. Warn below 4.5:1 (3:1 for large text and the wallpaper is exempt), and offer "Fix contrast", which nudges OKLCH lightness until it passes. Never block saving.
7. **Safety:** theme files contain only colours, names and numbers (plus a wallpaper file path, loaded with size and pixel caps). Reject anything else with a clear error and fall back to the base theme.
8. **Storage:** in the app config directory behind a storage trait (no `std::fs` in UI code, per Rune's rules; the wasm build will use browser storage).
9. **Tests:** a headless scene per built-in theme, a `custom-theme` scene from a fixture TOML, an importer test per terminal format, and a property test that "Fix contrast" always produces passing roles.

## 5. Verification (headless, every milestone)

Build `app` with `rune` feature `tooling`. Define scenes (`--list-scenes`), all driven by `mail-mock` seed data:

`default`, `night`, `frost`, `translucent`, `reduce-transparency`, `increase-contrast`, `wallpaper`, `intro` (video), `custom-theme`, `split-panes`, `command-line`, `empty-inbox`, `compose`, `compose-reply`, `palette`, `search-results`, `html-newsletter`, `html-receipt`, `html-dark-darkened`, `html-blocked-images`, `html-fallback-markdown`, `attachments`, `narrow` (800×700), `offline-error`, `chrome-windows`, `chrome-linux`.

Compare M2 snapshots against the reference IDE screenshots side by side (grid density, tab bar, status line, icon rail, workspace bar) and against `ravenglass-brand/preview/ui-*.png`.

**CI matrix:** build and run the headless suite on `macos-latest`, `windows-latest` and `ubuntu-latest` (Linux: software Vulkan via `mesa-vulkan-drivers` per Rune's README). Golden PNGs are compared per OS. Text rasterization differs by platform defaults, so keep separate goldens or set one `FontSmoothing` mode in tests.

```sh
A=target/debug/ravenglass
$A --snap out/default-dark.png --scene dark --size 1280x800 --scale 2
$A --tree out/default.json --scene default            # assert keys/roles/labels present
$A --script scripts/triage.json --frames 40 --out out/triage/   # ↓ ↓ Return, archive, undo
$A --script scripts/compose.json --video out/compose.mp4 --keys --pointer
```

**Acceptance per milestone:**
- Snapshots for each scene, reviewed by eye. Check glass only on controls, opaque content, nothing clipped, and that the traffic lights are clear (`--chrome`).
- `--tree` assertions in `cargo test`: every `IconButton` has a label, the list has role `list`, focus order is list → reader → compose, the mode block shows the current mode, and the selected row's key matches state.
- Scripts cover keyboard triage, the compose → send → undo flow and palette actions.
- Unit tests on every `step` function (pure, no I/O). Integration tests for `mail-imap` run against a local **Greenmail** or **Dovecot** container (`docker compose` in `tools/`).

---

## 6. Milestones

| # | Deliverable | Done when |
|---|---|---|
| **M0** | Workspace, toolchain, Rune pinned and patched, `hello` window boots from `app/` | `--snap` of an empty window renders |
| **M1** | `mail-core` types, `MailBackend`, `mail-mock` with ~200 seeded messages across 2 accounts. **Plus the Blitz spike (§3.5)** | `cargo test` green; deterministic seed; spike report: builds on all 3 OSes, no Parley conflict, raster times |
| **M2** | The character grid (`Grid` cell metrics, IBM Plex Mono), panes with focus bar and status lines, dot tabs, icon rail and workspace bar; the list + reader from mock data (text view); Night/Frost loaded from TOML through roles (no hard-coded colours); the braille wallpaper and intro | Scenes `default/dark/light/reduce-transparency/narrow` approved by Jason |
| **M3** | Interaction: modal keys (NORMAL/VISUAL/SEARCH/COMPOSE), the `:` command line, palette, pane splitting/resizing, workspaces 1–9, flags/archive/delete with undo, context menus, `keys.rs` with user rebinding | `triage.json` and `palette.json` scripts pass `--tree` assertions |
| **M4** | `mail-store` (SQLite + FTS5) and search UI with operators | Search scene; 10k-message mock searches in < 50 ms |
| **M5** | `mail-imap`: login (password), folder sync with CONDSTORE, lazy bodies, IDLE, `AccountSync` machine with backoff | Works against Greenmail and one real iCloud/Fastmail account |
| **M6** | `mail-render` + `mail-blitz`: sanitizer, Blitz document, net provider with image policy, tiled raster to `Pixels`, link hit-testing, find-in-message, Markdown fallback | §3.4 acceptance met on all 3 OSes; fuzz target runs 1 h clean |
| **M7** | Compose: windows, token fields, rich body, attachments, drafts autosave, SMTP send, outbox with undo | Send round-trip to Greenmail; `compose.json` passes |
| **M8** | `mail-auth`: `keyring` credentials, OAuth2 for Gmail and Outlook; account settings page (`shell::Page`). **Settings → Appearance, theme import (terminal schemes), hot reload and the contrast fixer (§4)** | Add/remove accounts and token refresh verified on all 3 OSes; theme editor, import/export and contrast fixer pass their headless scenes |
| **M9** | Polish: launch < 300 ms to first frame, idle draws nothing, a11y pass, app icon. **Packaging:** macOS `.app` (`scripts/mac-app`/`rune-mac`), Windows MSI or MSIX (`cargo-wix` or `cargo-packager`), Linux AppImage + `.deb` (`cargo-packager`) | Perf numbers from `--profile perf`; installable artifacts from CI for each OS |

---

## 7. Rules to follow (from Rune's `AGENTS.md`, adopted for this app)
- Keep files under 800 non-test lines; tests go in sibling `tests.rs`.
- Modes and phases are explicit state machines; timers are events.
- No file I/O on the UI thread. Start network and indexing from `on_ready`.
- `poll` returns true only when visible state changed, so an idle window draws nothing.
- Build only mounted rows (`virtual_rows`), and use `memo` for rows that rarely change.
- Bound every cache (bodies, decoded images) by entries **and** bytes.
- **Never log message bodies, addresses or tokens.** Redact in `tracing` fields.

---

## 8. Decisions from Jason
Resolved earlier: all Rust, OS-agnostic, Blitz for HTML, reference video identified (§0), name Ravenglass, app icon D · Braille (dark + light).

Resolved 2026-11-23:
1. **Accounts:** any modern provider, custom hosts included. Generic IMAP/SMTP with autoconfig first, then JMAP and OAuth for Gmail and Outlook.
2. **HTML in dark mode:** both modes ship (original colours on a light card, and darkened), as a setting. The light card stays the default.
3. **Default reader view:** HTML.
4. **Biometric app lock (Touch ID / Windows Hello):** v1. This overrides "Explicitly later" in §1.
