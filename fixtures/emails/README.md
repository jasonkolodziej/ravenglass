# Email fixtures

Synthetic samples to start the M1 spike and M6 pipeline. Grow this to the **30-email corpus** in plan §3.4: newsletters with heavy table layouts, receipts, GitHub notifications, marketing mail with media queries, plain replies, RTL, CJK and malformed HTML. Strip personal data from any real mail before committing it.

| File | Exercises |
|---|---|
| `01-plain-reply.eml` | text/plain, quoted reply folding, URL linkify |
| `02-newsletter-tables.eml` | HTML-only, nested layout tables, `bgcolor`/`align`, a remote image and a 1×1 tracking pixel (both must be blocked) |
| `03-multipart-cid.eml` | multipart/alternative + related, inline `cid:` image, a PDF attachment |
| `04-receipt.eml` | A receipt table: `colspan`, header cells, `cellpadding`, borders |
| `05-github-notification.eml` | `pre`/`code`, nested blockquotes, a beacon pixel |
| `06-marketing-media-queries.eml` | Dark page, media queries, `@import`, `@font-face`, background image, a form, a script, `position: fixed`, a tracker that is not 1×1 |
| `07-rtl-arabic.eml` | Right-to-left text with embedded Latin words and digits |
| `08-cjk.eml` | Japanese, Chinese and Korean text with no spaces to wrap at |
| `09-malformed.eml` | Unclosed tags, a table without rows, broken CSS, bad entities, an unterminated comment |
| `10-encodings.eml` | Encoded-word subject, quoted-printable ISO-8859-1 text, base64 HTML |
| `11-tall-statement.eml` | A 400-row table, tall enough to need several tiles |
