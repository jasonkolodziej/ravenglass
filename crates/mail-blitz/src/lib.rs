//! Blitz host. Milestone M6; spike in M1 (plan §3.5, `spikes/blitz/README.md`).
//! Sanitized HTML → blitz-dom document with a deny-by-default net provider → layout at the pane's
//! width → `blitz-paint` into an anyrender CPU backend → RGBA → `rune::Pixels::set` (tiled for tall
//! mail). Pointer events forwarded for link hit-testing. Runs off the UI thread; `catch_unwind` at
//! the worker boundary falls back to the text view.
