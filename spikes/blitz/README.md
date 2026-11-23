# M1 spike: Blitz inside a Rune window

**Goal:** prove the HTML pipeline (plan §3) before building UI on it. This is a throwaway crate; keep it out of the workspace until it works.

## Do
1. `cargo new blitz-spike` here. Depend on the same Rune rev and `[patch.crates-io]` as the root `Cargo.toml`, plus `blitz-dom`, `blitz-html`, `blitz-paint` and `blitz-traits` at one pinned version or git rev, and an `anyrender` **CPU** backend.
2. Load the emails in `../../fixtures/emails/` (add 2 real newsletters of your own). Sanitize with `ammonia` (rules in plan §3.1 step 3).
3. Use a net provider that **denies all remote requests** and serves `cid:` parts from the message.
4. Lay out at 680 px, paint into the CPU backend, take the RGBA, and show it with `el().pixels(&pixels)` (`rune::Pixels::set`) inside a scroll view.
5. Forward clicks (element-local coordinates) to Blitz hit-testing; print the link href.

## Report back (to Jason, and in `spikes/blitz/REPORT.md`)
- Does it build on macOS, Windows and Linux with Rune's patches? Are there duplicate `parley`/`taffy` versions (`cargo tree -d`), and how did you resolve them?
- CSS gaps that matter for email: table layout, `bgcolor`/`align`/`width` attributes, media queries, `background-image`, fonts.
- Raster time per fixture at 1× and 2× (median of 5), and memory for a tall newsletter.
- Whether link hit-testing and text extraction (for find-in-message and the text view) are workable.

If the spike fails, the fallback is the text view everywhere for v1 (plan §3.2). Say so early.
