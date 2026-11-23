# Email fixtures

Synthetic samples to start the M1 spike and M6 pipeline. Grow this to the **30-email corpus** in plan §3.4: newsletters with heavy table layouts, receipts, GitHub notifications, marketing mail with media queries, plain replies, RTL, CJK and malformed HTML. Strip personal data from any real mail before committing it.

| File | Exercises |
|---|---|
| `01-plain-reply.eml` | text/plain, quoted reply folding, URL linkify |
| `02-newsletter-tables.eml` | HTML-only, nested layout tables, `bgcolor`/`align`, a remote image and a 1×1 tracking pixel (both must be blocked) |
| `03-multipart-cid.eml` | multipart/alternative + related, inline `cid:` image, a PDF attachment |
