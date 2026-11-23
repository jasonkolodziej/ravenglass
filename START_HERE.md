# Start here

Read this, then `docs/PLAN.md` end to end, then `CLAUDE.md`. Work milestone by milestone (plan §6).

## 0. What already works (verified)
The pure-Rust crates compile and their tests pass (`cargo test -p mail-core -p mail-mock -p ravenglass-theme -p ravenglass-brand`, 18 tests; clippy clean). Checked with Rust 1.97 in the planning sandbox:
- `mail-core`: domain types, the `MailBackend` trait, and the `account_sync` and `outbox` state machines with tests.
- `mail-mock`: a deterministic two-account, 200-message backend for scenes and tests.
- `ravenglass-theme`: loads the Night and Frost TOML themes, layers user themes on them, resolves roles, reports contrast, fixes contrast, and imports Ghostty schemes.
- `ravenglass-brand`: the raven geometry and the braille dot field for the wallpaper and intro.

**Not yet compiled:** `app/` (needs Rust 1.99.1 + Rune). It was written against Rune's API at the pinned rev; expect small fixes.

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
1. **M0:** get `app/` compiling against Rune and the wallpaper snapshot matching the preview. Keep the `[patch.crates-io]` table in the root `Cargo.toml` in sync with Rune's.
2. **M1:** run the **Blitz spike** (`spikes/blitz/README.md`) and report on Parley/Taffy version conflicts, CSS gaps and raster times **before** any UI work. Extend `mail-core`/`mail-mock` as the UI needs.
3. **M2:** the grid UI (plan §4): cell metrics, panes, dot tabs, status line, icon rail, workspace bar, list + reader on mock data, themes through roles, the wallpaper drawn as one shape plus the intro.
4. Then M3–M9 as the plan lists them.

## 3. Visual references
- `brand/boards/board-{1-icon,2-ui,3-palette}.png`: the design boards as flat PNGs (icon dark + light, the grid UI in both themes, the palette).
- `brand/preview/ui-*.{png,svg}`: the target look for each screen (Night and Frost). The wallpaper is drawn as dots, as the app should draw it.
- The reference IDE: see `docs/reference/LINKS.md`. Use it for look and feel only (it's GPL-3 Go; copy no code).
- The design canvas with the icon, UI and palette: <https://claude.ai/artifact/CN6owMxyqWSZMnppPBQTX8> (Jason's; ask for access if you need it).

## 4. Open questions for Jason (plan §8)
Accounts to support first · HTML in dark mode · HTML vs text as the default reader view · biometric lock in v1 or later.
