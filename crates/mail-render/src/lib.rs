//! MIME → body model. Milestone M6 (plan §3).
//! `mail-parser` to parse; pick text/html vs text/plain; plain → grid text; HTML → `ammonia` sanitize
//! (strip scripts/iframes/forms/handlers/remote CSS, rewrite `cid:` to `cid://<msg>/<id>`, drop
//! tracking pixels) → hand to `mail-blitz`; fallback HTML → Markdown for the text view.
