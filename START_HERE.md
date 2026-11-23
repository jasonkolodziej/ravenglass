# Start here

Read this, then `docs/PLAN.md` end to end, then `CLAUDE.md`. Work milestone by milestone (plan §6).

## 0. What already works (verified)
Checked with Rust 1.99.1 on Linux: `cargo test --workspace` passes (60 tests) and `cargo clippy --workspace --all-targets -- -D warnings` is clean.
- **M0 is done.** `app/` compiles against Rune at the pinned rev with no changes, and `--snap` renders the wallpaper in Night and Frost.
- `mail-core`: domain types (serde), the `MailBackend` trait, and five state machines: `account_sync`, `outbox`, `compose`, `image_load`, `html_view`.
- `mail-mock`: a deterministic two-account, 200-message backend for scenes and tests.
- `mail-store`: SQLite schema v1 with migrations, sync apply, body cache with eviction, the offline journal, address completion and FTS5 search with operators (about 1 ms per query over 10k messages in release).
- `mail-render`: MIME parsing, the HTML sanitizer (CSS filter, `cid:` rewrite, tracking-pixel removal), the text view and HTML to text.
- `ravenglass-engine`: the Command/Event boundary over all of the above, with optimistic actions, undo, offline replay, the outbox and a desktop runner thread.
- `relay-proto`, `relay-client`, `ravenglass-relay`: the Relay API and both ends of it, tested against each other in-process.
- `ravenglass-theme`, `ravenglass-brand`: themes and the raven dot field.

**Blitz spike:** done, verdict go; read `spikes/blitz/REPORT.md` (one build fix to Rune's `rune-taffy` is needed before Blitz links next to the app).

**Not started:** the grid UI (M2), `mail-blitz`, `mail-imap`, `mail-jmap`, `mail-auth`, the Relay server, and the wasm build. Nothing has been built on macOS or Windows yet; the `Rust` workflow will be the first to try.

## 1. Set up
```sh
rustup toolchain install 1.99.1          # Rune's pin; rust-toolchain.toml selects it
# Linux only:
sudo apt-get install -y libxkbcommon-dev libwayland-dev libxkbcommon-x11-0 mesa-vulkan-drivers
cargo test -p mail-core -p mail-mock -p ravenglass-theme -p ravenglass-brand
cargo run                                # opens the window (M0)
cargo run -- --snap out/wallpaper.png --size 1400x880 --scale 2   # headless check
cargo run -- --snap out/frost.png --scene frost
```
Compare the snapshots with `brand/preview/ui-wallpaper-night.png` and `ui-wallpaper-frost.png`.

## 2. First tasks, in order
1. ~~**M0:** get `app/` compiling against Rune and the wallpaper snapshot matching the preview.~~ Done. Keep the `[patch.crates-io]` table in the root `Cargo.toml` in sync with Rune's.
2. **M1:** run the **Blitz spike** (`spikes/blitz/README.md`) and report on Parley/Taffy version conflicts, CSS gaps and raster times **before** any UI work. Extend `mail-core`/`mail-mock` as the UI needs.
3. **M2:** the grid UI (plan §4): cell metrics, panes, dot tabs, status line, icon rail, workspace bar, list + reader on mock data, themes through roles, the wallpaper drawn as one shape plus the intro.
4. Then M3–M9 as the plan lists them.

## 3. Visual references
- `brand/boards/board-{1-icon,2-ui,3-palette}.png`: the design boards as flat PNGs (icon dark + light, the grid UI in both themes, the palette).
- `brand/preview/ui-*.{png,svg}`: the target look for each screen (Night and Frost). The wallpaper is drawn as dots, as the app should draw it.
- The reference IDE: see `docs/reference/LINKS.md`. Use it for look and feel only (it's GPL-3 Go; copy no code).
- The design canvas with the icon, UI and palette: <https://claude.ai/artifact/CN6owMxyqWSZMnppPBQTX8> (Jason's; ask for access if you need it).

## 4. Decisions (plan §8)
Any modern provider including custom hosts · HTML is the default reader view · both dark-mode HTML treatments ship as a setting · biometric lock is in v1.
