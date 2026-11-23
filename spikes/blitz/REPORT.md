# Blitz spike report (M1)

2026-11-23 · Linux x86-64, Rust 1.99.1, release build · Blitz `0.3.0-beta.4`, Rune `dev` @ `0ca07561`

**Verdict: go, with one build fix needed.** Blitz lays out and rasters the fixture emails correctly and far inside the
time budget, and the bitmap shows in a Rune window through `rune::Pixels`. Blitz and Rune do not compile into one binary
as they stand; a one-line change to Rune's `rune-taffy` crate fixes it.

**Update (2026-11-23):** that fix is upstream. From Rune `dev` @ `5e527e71`, `encode_wrap` spills unknown `FlexWrap`
variants to `extra` itself, so the spike and the app both build against Rune unmodified and `vendor/rune-taffy` is gone.
The findings below are as they were at `0ca07561`.

Run it: `cd spikes/blitz/blitz-spike && cargo run --release -- ../../../fixtures/emails out`, or
`SPIKE_EML=../../../fixtures/emails/02-newsletter-tables.eml cargo run --release -- --snap out/window.png`.

## Dependencies

| Question | Answer |
|---|---|
| Duplicate Parley or Taffy? | No. One `parley` 0.12 (with Rune's patched `parley_engine`) and one `taffy` 0.15.1 are shared by both |
| Does it build with Rune's `[patch]` table? | Not unmodified. `blitz-dom` enables Taffy's `flexbox_balance` feature, which adds `FlexWrap::Balance` and `BalanceReverse`. Cargo unifies features, so `rune-taffy`'s exhaustive `match` in `encode_wrap` stops compiling. Rune's current `dev` head (`b7a08d6a`) has the same match |
| Fix used here | `vendor/rune-taffy`: Rune's crate at the pinned rev plus a catch-all arm, applied with `[patch."https://github.com/anomalyco/rune"]`. It was the only conflict |
| Paint backend | `blitz-paint` is on `anyrender` 0.14, so the CPU backend must be `anyrender_vello_cpu` 0.18 (not the newest 0.19) |
| macOS and Windows | Not built. Only Linux was available |

The main workspace needs the same fix before `mail-blitz` can link next to the app. The clean route is a change in Rune
itself (a catch-all arm, or enabling the feature and encoding the two variants); the vendored patch works until then.

## Rendering

| Fixture | Size at 2× | Parse + style + layout | Raster | RGBA |
|---|---|---|---|---|
| `02-newsletter-tables` | 1360 × 780 | 8.5 ms | 3.6 ms | 4 MB |
| `03-multipart-cid` | 1360 × 452 | 8.3 ms | 1.2 ms | 2 MB |

Medians of 5 runs; at 1× raster takes 1.7 ms and 0.7 ms. The budget is 50 ms for a typical email at 2×. About 8 ms of
every run is fixed set-up cost (a new document and font context each time), which a reused font context should cut.
`01-plain-reply` has no HTML part and never reaches Blitz.

What rendered correctly: nested layout tables, `width` and `bgcolor` attributes, `align="center"`, `<style>` blocks with
class selectors, `border-radius`, inline styles, lists, bold, links. The remote hero image stayed blocked (its box is
reserved and empty) and the tracking pixel was already gone after sanitizing.

## Gaps that matter for email

| Gap | Seen in | Remedy |
|---|---|---|
| `cellpadding` is ignored | Newsletter cells sit flush against the card edge | Pre-pass in `mail-render`: turn `cellpadding="n"` into `padding` on the table's cells |
| Padding on an inline `<a>` button is not applied | The "Read the notes" button is a tight box | Check `display: inline-block` in Blitz; if that works, the pre-pass can set it on padded links |
| Blocked images show nothing, not their `alt` text | Hero image area is blank | Paint a placeholder with the alt text from the net provider, or in the pre-pass |
| `cid:` images | Not drawn: the spike's net provider refuses everything | `mail-blitz` needs the real provider that serves `cid://` parts |

Not tested: media queries, `background-image`, web fonts, RTL, CJK, a tall newsletter's memory, and real-world mail (the
three fixtures are synthetic). The 30-email corpus in M6 covers those.

## Integration

- **Pixels bridge:** works. One `Pixels::set(w, h, rgba)` and `el().pixels(&pixels)`; the snapshot shows the email in the
  window. Rune caps a `Pixels` image at 8192 px per axis and the CPU renderer at 65,535, so tall mail must be tiled, as
  planned (1024-px tiles).
- **Link hit-testing:** works. `BaseDocument::hit(x, y)` returns the element under a point; walking up to the nearest
  `<a>` gives the `href`. Probing the newsletter found both links. Forwarding real pointer events from Rune
  (`on_pointer_move_local`) was not wired in the spike.
- **Text extraction:** not needed from Blitz; `mail_render::html_to_text` already produces the text view and the search
  text from the sanitized HTML.
- **Binary size:** the spike is 27 MB in release with Rune and Blitz together, unstripped.

## Later findings (after the spike)

- **`cellpadding`** is now converted to cell padding by `mail-render`, and blocked remote images show a sized box with
  their alt text.
- **Padded link buttons** render correctly when the email sets `display: inline-block` on them (fixture 06), which is
  what most marketing mail does. A padded plain inline link is still tight.
- **Absurd sizes crash the painter.** `height: 1e99px` (fixture 09) lays out but panics inside `vello_common` when
  painted. `mail-blitz` refuses any document taller than 60,000 px or with a non-finite height and the reader shows the
  text view; the panic guard is the second line of defence.
- **Right-to-left text** shapes and aligns correctly (fixture 07), but list bullets are drawn on the left.
- **CJK** wraps correctly without spaces (fixture 08).
- **A 400-row table** (fixture 11) is 26,974 px tall at 2× and paints as 27 tiles.
