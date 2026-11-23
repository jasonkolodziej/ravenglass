# Reference links

## Framework: Rune (Rust, MIT): what we build on
- Repo: https://github.com/anomalyco/rune (pinned rev `0ca075619d116f8f6e0e3bc8ff815c72c636cac8`)
- Copied here at that rev: `rune-core-README.md`, `rune-kit-README.md`, `rune-AGENTS.md`, `rune-hello-main.rs`
- Worth reading in the repo: `examples/hello`, `examples/settings`, `examples/todo`, `crates/rune-kit/examples/shell.rs`, `examples/frosted-bar`, `examples/video-tiles` (the Pixels/NativeSurface boundary)

## Visual reference: unstablebuild/rune (Go IDE, GPL-3): look and feel only
- Repo: https://github.com/unstablebuild/rune
- Demo video: https://github.com/user-attachments/assets/4ab84f7f-47c8-47af-9d32-7c69afd02669
- Screenshots in that repo: `.github/assets/rune-screenshot.webp` and `docs/static/img/intro/screenshot_1_*.png` (one per built-in theme)
- Docs worth reading for UX ideas: `docs/docs/learn/wallpaper.md` (empty-pane wallpaper + intro), `docs/docs/learn/themes.md` (16-name colour model), `docs/docs/config.md`

## HTML engine: Blitz (Rust, MIT/Apache)
- https://github.com/DioxusLabs/blitz (crates: `blitz-dom`, `blitz-html`, `blitz-paint`, `blitz-traits`; renderer abstraction `anyrender`)
- Pre-1.0: pin an exact version or git rev.

## Fallback engine considered: Servo (MPL-2.0)
- https://servo.org/blog/2026/04/13/servo-0.1.0-release/ (embeddable, with JS: heavier than we need)
