# Checking real accounts with `ravenglass-check`

`ravenglass-check` runs Ravenglass's own protocol code (the `mail-imap` and `mail-jmap`
backends, autoconfig, the MIME parser and HTML sanitizer) against one of your real accounts and
prints a short report. It exists so that the IMAP/SMTP/JMAP code can be checked against real
providers without anyone handing their credentials to anyone: you run it, you read the report,
you paste the report.

## What it does and does not do

- **Read-only by default.** It lists folders, opens the inbox with `EXAMINE` (the read-only
  `SELECT`), reads the newest envelopes and one body with `BODY.PEEK` (nothing gets marked as
  read), checks the SMTP login and quits **without sending**, and watches the inbox with IDLE for
  30 seconds. It never sets a flag, moves, appends, deletes or sends.
- **`--write` is opt-in** and confined: it creates a folder named `Ravenglass-Check`, appends two
  made-up messages to it, syncs, flags one, syncs again, reads one back, then deletes the folder.
  If a folder of that name already exists it stops without touching it. Nothing else in the
  account is changed, and nothing is sent.
- **Every line is safe to paste.** The report holds server capabilities, folder roles and counts,
  timings, sizes, MIME types and counts. It never prints your address, your login, a password or
  token, a subject, a preview, a sender, an attachment's file name, or the name of a folder you
  made (those are counted, not named). Host names that contain your own domain are shown as
  `imap.<your domain>`; a known provider's hosts (`imap.gmail.com`) are shown as they are. The
  automated tests check this against Dovecot and Stalwart. Still, read the report before pasting.
- **Secrets never come from the command line.** The password, app password or token is read
  from a prompt that does not echo, or from `RAVENGLASS_CHECK_PASSWORD`. It is held in memory for
  the run and not written anywhere.

## Running it

```sh
cargo run --release -p ravenglass-check -- you@example.com
```

It looks the settings up (a built-in table for big providers, then the provider's or Mozilla's
autoconfig document over HTTPS, then conventional names), shows them, and asks
`Use these settings? [Y]es / [e]dit / [q]uit`. `e` lets you change the IMAP, SMTP or JMAP
server and the login name. Then it asks for the password.

Useful options (`--help` lists them all):

| Option | What for |
| --- | --- |
| `--imap host:port[:tls\|starttls]`, `--smtp …` | Give the servers yourself; `--smtp none` skips the SMTP check |
| `--login name` | Log in with something other than the address |
| `--jmap <session-url>` | Check over JMAP instead of IMAP |
| `--token` | The secret is an API token, sent as `Authorization: Bearer` (Fastmail JMAP) |
| `--oauth` | Sign in through the browser (Gmail, Microsoft); see below |
| `--ca file.pem` | A self-hosted server with a private CA: trust that certificate |
| `--idle 0` / `--idle 120` | Skip or lengthen the push check |
| `--limit 500` | Read more of the newest inbox messages (default 200) |
| `--yes` | Don't ask about the settings (needed when stdin is not a terminal) |
| `--write` | Also check changes, inside `Ravenglass-Check` only |

Exit status: 0 when every check passed, 1 when one failed, 2 for a usage or setup problem.

What a report looks like (Stalwart, over IMAP):

```text
ravenglass   check 0.0.0 — read-only
settings     from command line
             IMAP localhost:3995 TLS
             SMTP localhost:3587 STARTTLS
             login the address, sign-in password, trusting the --ca certificate
login        password (or app password) from RAVENGLASS_CHECK_PASSWORD
connect      logged in, 39 ms (TLS handshake, login and capabilities)
capabilities IMAP4REV2 ENABLE SASL-IR LITERAL+ ID UTF8=ACCEPT JMAPACCESS IDLE NAMESPACE …
             every extension Ravenglass uses is there
smtp         logged in without sending, 39 ms; offers SMTPUTF8 SIZE 104857600 … AUTH PLAIN LOGIN XOAUTH2 OAUTHBEARER 8BITMIME
folders      6 folders in 2 ms
             Inbox: "INBOX" (2 messages, 2 unread)
             Sent: "Sent Items" (0 messages, 0 unread)
             …
             other folders: 1 (0 messages, nested up to 1 deep, 0 with non-ASCII names)
inbox        read the newest 2 of 2 envelopes in 6 ms
             unread 2, flagged 0, with attachments 1, empty preview 0, empty subject 0, no sender 0, no date 0, dated in the future 0, threads 2
body         newest message: 612 bytes fetched in 3 ms, parsed in 0 ms
             parts: multipart/mixed, text/html, application/pdf
             plain text: no, HTML: yes, remote images: 1 (blocked until allowed), trackers removed: 1, attachments: 1, inline parts: 0
push         watch held for 30 s; events: 0 new mail, 1 flag changes, 0 removals
write        skipped (read-only; --write checks changes inside a folder of its own)
done         all checks passed
```

During the 30-second push check, send yourself a message or flag one in another client: the
events line should count it.

## Provider notes

Checked against the providers' own documentation in November 2026. Providers change these
pages; when the tool and a page disagree, the page wins, and the report is worth filing.

### Gmail

- Servers: `imap.gmail.com:993` (TLS), `smtp.gmail.com:465` (TLS) or `:587` (STARTTLS).
  ([Google: IMAP, POP and SMTP](https://developers.google.com/workspace/gmail/imap/imap-smtp))
- **Password sign-in needs an app password**, and app passwords exist only for accounts with
  2-Step Verification on. They may be unavailable for work or school accounts, accounts with
  Advanced Protection, or 2-Step Verification that uses security keys only.
  ([Google: Sign in with app passwords](https://support.google.com/mail/answer/185833)) Create
  one at <https://myaccount.google.com/apppasswords> and give it to the prompt.
- **Labels appear as folders.** A message with three labels is in three folders; `[Gmail]/All Mail`
  holds every message and is what Ravenglass uses as **Archive** (its `\All` attribute maps to the
  Archive role). Gmail files sent mail itself when you send over SMTP, so Ravenglass does not
  append a copy to Sent for Gmail; deleting from a label only removes the label, the message stays
  in All Mail. ([Google: IMAP client settings and folders](https://support.google.com/mail/answer/78892))
- `--oauth` (XOAUTH2): Gmail's servers take OAuth 2.0 through SASL XOAUTH2
  ([Google](https://developers.google.com/workspace/gmail/imap/imap-smtp)). You need your own
  Google Cloud OAuth client of type *Desktop app*, with the `https://mail.google.com/` scope and,
  while the consent screen is in testing, your account added as a test user. Set
  `RAVENGLASS_GOOGLE_CLIENT_ID`; `RAVENGLASS_GOOGLE_CLIENT_SECRET` is optional for desktop clients
  (Google treats them as public clients using PKCE) and is sent only if set. The redirect is a
  loopback address on a random port, which Google allows for desktop clients.
  ([Google: OAuth 2.0 for iOS & Desktop Apps](https://developers.google.com/identity/protocols/oauth2/native-app))

### iCloud Mail

- Servers: `imap.mail.me.com:993` (SSL), `smtp.mail.me.com:587` (STARTTLS). No POP.
  ([Apple: iCloud Mail server settings](https://support.apple.com/en-us/102525))
- **Needs an app-specific password**, which needs two-factor authentication on the Apple Account.
  ([Apple: app-specific passwords](https://support.apple.com/en-us/102654))
- The login: Apple says the IMAP user name is the part before `@icloud.com` ("johnappleseed"),
  and to try the full address if that fails; SMTP takes the full address. Ravenglass logs in with
  the full address on both. If IMAP refuses it, run again with `--login johnappleseed` and
  please report it, because the app would need a separate IMAP login.

### Outlook.com and Microsoft 365

- Servers: `outlook.office365.com:993` (TLS), `smtp-mail.outlook.com:587` (STARTTLS) for
  Outlook.com; Microsoft 365 uses `smtp.office365.com:587`. IMAP has to be switched on in
  Outlook.com's settings (Mail → Forwarding and IMAP).
  ([Microsoft: POP, IMAP and SMTP settings for Outlook.com](https://support.microsoft.com/en-us/office/pop-imap-and-smtp-settings-for-outlook-com-d088b986-291d-42b8-9564-9c414e2aa040))
- **OAuth only**: "Outlook.com requires the use of Modern Auth / OAuth2" (same page). Passwords
  will not work, so use `--oauth`, which needs a client id: register an app in Microsoft Entra
  (Azure portal → App registrations), add a *Mobile and desktop applications* platform with the
  redirect URI `http://127.0.0.1`, add the delegated permissions
  `https://outlook.office.com/IMAP.AccessAsUser.All` and `https://outlook.office.com/SMTP.Send`
  (and `offline_access`), allow personal Microsoft accounts if the account is an Outlook.com one,
  and set `RAVENGLASS_MICROSOFT_CLIENT_ID` to its application (client) id. Ravenglass then uses
  SASL XOAUTH2 on IMAP and SMTP as Microsoft describes.
  ([Microsoft: Authenticate an IMAP, POP or SMTP connection using OAuth](https://learn.microsoft.com/en-us/exchange/client-developer/legacy-protocols/how-to-authenticate-an-imap-pop-smtp-application-by-using-oauth))
  In Microsoft 365 the tenant admin may have to consent to the app, and SMTP AUTH can be disabled
  per mailbox or tenant; the `smtp` line then fails while IMAP works.

### Fastmail

- IMAP and SMTP: `imap.fastmail.com:993` (TLS) and `smtp.fastmail.com:465` (TLS, or 587 with
  STARTTLS), full address as the login, and **an app password** — "You cannot use your regular
  Fastmail password."
  ([Fastmail: Server names and ports](https://www.fastmail.help/hc/en-us/articles/1500000278342-Server-names-and-ports))
- JMAP: the session URL is `https://api.fastmail.com/jmap/session`, and JMAP takes an **API token**
  (Settings → Privacy & Security → Manage API tokens) sent as a Bearer token, not an app password;
  apps for other people use OAuth. ([Fastmail: developer documentation](https://www.fastmail.com/dev/))
  So check JMAP with `ravenglass-check --token you@fastmail.com` (the built-in settings already
  point at the JMAP session URL), and IMAP with
  `ravenglass-check --imap imap.fastmail.com:993 --smtp smtp.fastmail.com:465 you@fastmail.com`
  and an app password.

### Yahoo Mail

- Servers: `imap.mail.yahoo.com:993` (SSL), `smtp.mail.yahoo.com:465` (SSL) or `:587`, full
  address as the login. ([Yahoo: IMAP server settings](https://help.yahoo.com/kb/SLN4075.html))
- **Use an app password**: Account Security → External connections → Create app password.
  ([Yahoo: Generate and manage app passwords](https://help.yahoo.com/kb/SLN15241.html))

### Your own server

- Autoconfig is tried at `autoconfig.<domain>`, `<domain>/.well-known/autoconfig/…` and
  Mozilla's ISPDB; otherwise `imap.<domain>` / `smtp.<domain>` are guessed and marked as a guess.
  Use `e` at the prompt or `--imap`/`--smtp` when they are wrong.
- A certificate from a private CA: pass the CA's certificate with `--ca ca.pem`. A self-signed
  server certificate works too, but only if it says `CA:FALSE` (rustls refuses a self-signed
  certificate that also claims to be a CA, which is what `openssl req -x509` makes by default).
- Dovecot, Courier and other servers whose folders all live under `INBOX.` work, including
  creating Drafts and Sent there.

## What has been verified where

- **Dovecot 2.3** (`tools/dovecot.sh`, `RAVENGLASS_DOVECOT_TEST=1`): STARTTLS and implicit TLS,
  CONDSTORE incremental sync, SPECIAL-USE, an `INBOX.` namespace, modified UTF-7 names, partial
  fetches for previews, IDLE, a 5,000-message mailbox, the engine end to end, and this tool.
- **Stalwart 1.0** (`tools/stalwart.sh`, `RAVENGLASS_STALWART_TEST=1`): IMAP4rev2 sync, flags,
  moves, drafts, IDLE, SMTP submission with STARTTLS and implicit TLS, the whole JMAP backend
  (session discovery, sync, `Email/changes` and `Email/queryChanges`, flags, moves, drafts,
  `EmailSubmission`, the engine end to end), and this tool over IMAP and JMAP.
- **Not verified yet**: XOAUTH2 and OAuth sign-in against Google and Microsoft, Gmail's labels,
  iCloud's login name, Microsoft 365, Yahoo, Fastmail's JMAP. That is what this tool is for:
  reports from real accounts close these gaps.
