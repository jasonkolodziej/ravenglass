# Start here

Read this, then `docs/PLAN.md` end to end, then `CLAUDE.md`. Work milestone by milestone (plan §6).

## 0. What already works (verified)
Checked with Rust 1.99.1: by hand on Linux, and by CI (tests, lint, headless snapshots) on Linux, macOS and Windows. The crate table in `README.md` lists every crate and its state.
- **Engine side:** core types and state machines, the SQLite store with search, MIME parsing and the HTML sanitizer, the Blitz host, the engine (sync, push, actions with undo, reader, HTML tiles, drafts, outbox), IMAP + SMTP, JMAP, accounts and OAuth, the Relay over HTTP, themes and config. All have tests.
- **App:** the grid UI on the engine: list, reader in text and HTML, compose (reply, reply all, forward, address completion, undo send), modes, `:` command line, palette, panes, workspaces, link hover and open with a phishing check. Real accounts from a profile on disk (`--profile`), `:account add`, settings from `config.toml`, user themes and import, `:colors`, the app lock. 129 app tests; one drives a real IMAP/SMTP account on GreenMail end to end.
- **Blitz spike:** done, verdict go; read `spikes/blitz/REPORT.md`. Blitz and Rune link together only with `vendor/rune-taffy` (Rune's crate plus one match arm).
- **Web:** the app runs in headless Chromium against the Relay in both sign-in modes (`web/tests/e2e.mjs`, `.github/workflows/web.yml`). The engine runs on the page thread and the mail cache is in memory; moving them into Workers (and the cache onto OPFS) is next.
- **Real servers:** IMAP, SMTP and JMAP pass against Dovecot and Stalwart (`.github/workflows/servers.yml`); eight bugs were found and fixed that way. `ravenglass-check` checks a real account read-only.
- **Decisions taken without asking** are listed in `.github/plans/0001-ravenglass-build-decisions.md`.

**Not verified anywhere:** the app in a real window on macOS or Windows; any real mail provider (run `ravenglass-check` on yours); the web app in Firefox or Safari; the OS keyring; a real OIDC identity provider; OAuth against Google or Microsoft (no client id exists); autoconfig lookups over the network; the app in a real window with a keyboard (it was run under a virtual display and driven headlessly).

**Packaging:** Linux `.deb` and AppImage built and run here; macOS, Windows, the Relay image and signing run only in `.github/workflows/release.yml` (see `packaging/README.md`).

**Not started:** Touch ID and Windows Hello (the lock asks for the app passcode, `:lock passcode`, or an account password instead, and will not close if neither exists), the wallpaper intro animation, threads, context menus, a settings page with forms (settings are `config.toml` and `:` commands), Web Push.

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
