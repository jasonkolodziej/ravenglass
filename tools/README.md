# tools

Local mail servers for the integration tests, without Docker. Each script keeps what it
downloads or generates under `tools/.cache/` (gitignored) or a temp dir, uses non-default ports
on 127.0.0.1, and has `start` and `stop`. The tests start a server themselves when its variable
is set and nothing answers yet.

| Script | Server | Needs | Ports | Tests |
| --- | --- | --- | --- | --- |
| `greenmail.sh` | GreenMail 2.1.15 (Java), pinned jar + sha256 | Java 11+ | IMAP 3143/3993, SMTP 3025/3465 | `RAVENGLASS_IMAP_TEST=1 cargo test -p mail-imap` |
| `dovecot.sh` | the system's Dovecot 2.3, run unprivileged from a generated config | `apt-get install dovecot-imapd` | IMAP 3144 (STARTTLS), 3994 (TLS) | `RAVENGLASS_DOVECOT_TEST=1 cargo test -p mail-imap --test dovecot` |
| `stalwart.sh` | Stalwart 1.0.3 (Rust), pinned binary + sha256 | `zstd`, `openssl`, `curl` | IMAP 3145/3995, submission 3587 (STARTTLS)/3466 (TLS), JMAP https://localhost:3443 | `RAVENGLASS_STALWART_TEST=1 cargo test -p mail-imap --test stalwart`, `… -p mail-jmap --features http` |

`ravenglass-check`'s own tests (`cargo test -p ravenglass-check`) run the binary against Dovecot
and Stalwart under the same two variables. CI runs all of it in `.github/workflows/servers.yml`.

## Dovecot

`tools/dovecot.sh start` writes a config, a self-signed certificate for `localhost`
(`CA:FALSE`, which rustls requires) and an empty user directory into a fresh temp dir, records
the dir in `tools/.cache/dovecot.dir`, and starts Dovecot as the current user (as `nobody` when
run by root, since Dovecot will not store mail as root). Configured: passwd-file users (one file
per user in `<dir>/users/`, password `dovecot-test-password`), maildir, STARTTLS and implicit
TLS, IDLE, CONDSTORE/QRESYNC, MOVE, SPECIAL-USE on Drafts, Sent, Trash, Junk and Archive. A user
file that ends with `userdb_namespace/inbox/prefix=INBOX. userdb_namespace/inbox/separator=.`
gets the Courier-style `INBOX.` layout. The tests put mail in with `doveadm save` and
`doveadm import`; `tools/dovecot.sh doveadm …` runs doveadm against the instance.

## Stalwart

`tools/stalwart.sh fetch` downloads the layer of the official image
`ghcr.io/stalwartlabs/stalwart:v1.0.3` (linux/amd64) that holds the binary, by its sha256
digest, and checks the extracted binary against a second pinned sha256. `start` writes
`config.json` (RocksDB in a temp dir), starts Stalwart once in recovery mode (a plain HTTP admin
port, 18080, on all interfaces, for a few seconds), creates the domain `ravenglass.test`, the
certificate and the listeners through Stalwart's JMAP management API, and restarts it normally.
`tools/stalwart.sh user NAME` creates `NAME@ravenglass.test` with the password
`correct-horse-battery-staple-42` (Stalwart refuses weak passwords). The tests put mail in with
IMAP APPEND.

`stop` on either script stops the server and deletes its temp dir.
