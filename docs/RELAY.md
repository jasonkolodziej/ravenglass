# Ravenglass Relay

The Relay is the server the browser build of Ravenglass talks to. Browsers cannot open IMAP or SMTP
connections, so the Relay holds them and speaks one versioned HTTP + WebSocket API (`relay-proto`) to
the web app. It stores no mail. It is one Rust binary, `ravenglass-relay`, with no OpenSSL and no
system dependencies, configured from the environment alone (a TOML file is optional).

It runs in one of two modes (`docs/PLAN.md` §8, decision 8):

| | **token** (default) | **users** |
| --- | --- | --- |
| For | One person, self-hosted | Many people: a household, an organisation, an official instance |
| Sign-in | A shared bearer token | Username and password (Argon2id), and/or OIDC against your identity provider |
| Browser credential | `Authorization: Bearer` | `HttpOnly; Secure; SameSite=Strict` session cookie plus a CSRF token |
| Mail accounts | Fixed at start (today: the demo accounts) | Each user adds their own through the API; settings and secrets encrypted per user |
| Storage | None | One SQLite file and one master key file |
| Isolation | One owner | Every route, every push and every secret is scoped to the signed-in user |

Both modes serve the image proxy (`/v1/img`) and the push socket (`/v1/events`), and send the same
security headers.

## Running it

Put the Relay behind a TLS-terminating reverse proxy (Caddy, nginx, a load balancer). It listens on
plain HTTP, on `127.0.0.1:8743` unless told otherwise. Have the proxy add HSTS.

The Relay and the web app must be on the **same site**: for example `relay.example.org` and
`mail.example.org`, or one origin for both. A `SameSite=Strict` cookie is not sent across sites, so a
Relay on another registrable domain cannot be signed in to from the app.

### Token mode

```sh
export RAVENGLASS_RELAY_TOKEN="$(head -c 32 /dev/urandom | base64)"   # at least 16 characters
export RAVENGLASS_RELAY_ORIGIN=https://mail.example.org
ravenglass-relay
```

Every request carries `Authorization: Bearer <token>`; the WebSocket may pass it as
`?access_token=`. Anyone with the token has every account the Relay serves, so this mode is for one
person. `RAVENGLASS_RELAY_DEMO=1` serves the built-in demo accounts (real accounts in token mode are
not wired in yet).

### Users mode

```sh
export RAVENGLASS_RELAY_AUTH=users
export RAVENGLASS_RELAY_DATA_DIR=/var/lib/ravenglass-relay
export RAVENGLASS_RELAY_ORIGIN=https://mail.example.org
read -rs PW && printf '%s\n' "$PW" | ravenglass-relay user add jason   # password from stdin, never an argument
ravenglass-relay
```

On first start the Relay creates `master.key` (32 random bytes, mode 0600) and `relay.sqlite3` (mode
0600) in the data directory and says so on standard error. **Back up the master key separately from
the database** (see "Key management").

User administration, always against the configured database:

| Command | Does |
| --- | --- |
| `ravenglass-relay user add <name>` | Adds a user. The password (at least 10 characters) is the first line of standard input. |
| `ravenglass-relay user passwd <name>` | Sets a new password, read the same way. |
| `ravenglass-relay user remove <name>` | Removes the user with every account and secret they had. A running Relay refuses their session on its next request. |
| `ravenglass-relay user list` | Prints `id<TAB>name` per user. |

OIDC users are created on their first successful sign-in and can be removed the same way.

## Settings

Every setting is an environment variable. If `RAVENGLASS_RELAY_CONFIG` names a TOML file, it is read
first and the environment overrides it. TOML keys are the variable names without the
`RAVENGLASS_RELAY_` prefix, in lowercase; OIDC settings go in an `[oidc]` table without the `OIDC_`
prefix (`[oidc] issuer = "…"`), and the two allowlists are TOML arrays. Unknown keys are an error.

| Variable | Default | Meaning |
| --- | --- | --- |
| `RAVENGLASS_RELAY_CONFIG` | none | Path of an optional TOML settings file. |
| `RAVENGLASS_RELAY_AUTH` | `token` | `token` or `users`. |
| `RAVENGLASS_RELAY_LISTEN` | `127.0.0.1:8743` | Address and port to listen on (plain HTTP; put TLS in front). |
| `RAVENGLASS_RELAY_ORIGIN` | none | The web app's origin, e.g. `https://mail.example.org`. The only origin given CORS access, allowed to send `Origin` on authenticated requests, and where OIDC sign-in returns to. Without it no browser can use the Relay. |
| `RAVENGLASS_RELAY_TOKEN` | none | Token mode: the shared secret, at least 16 characters. |
| `RAVENGLASS_RELAY_TOKEN_FILE` | none | Token mode: read the token from this file instead (trailing newline ignored). |
| `RAVENGLASS_RELAY_DEMO` | `false` | Token mode: serve the demo accounts. |
| `RAVENGLASS_RELAY_DATA_DIR` | `./ravenglass-relay-data` | Users mode: where the database and master key go unless named below. |
| `RAVENGLASS_RELAY_DATABASE` | `<data dir>/relay.sqlite3` | Users mode: the SQLite file. |
| `RAVENGLASS_RELAY_MASTER_KEY_FILE` | `<data dir>/master.key` | Users mode: the 32-byte master key; created (0600) if absent. |
| `RAVENGLASS_RELAY_PASSWORD_LOGIN` | `true` | Users mode: offer username and password sign-in. Turn off to allow OIDC only. |
| `RAVENGLASS_RELAY_INSECURE_DEV_COOKIES` | `false` | Users mode: drop `Secure` from cookies for local development over plain HTTP. Refused unless the listener is on a loopback address. Never in production. |
| `RAVENGLASS_RELAY_TRUST_FORWARDED_FOR` | `false` | Users mode: take the client address for sign-in rate limiting from the last `X-Forwarded-For` entry. Turn on only behind a proxy that sets it; otherwise clients can choose their own address. |
| `RAVENGLASS_RELAY_PUBLIC_URL` | none | The Relay's own public `https://` URL. Required with OIDC: the redirect URI is `<public URL>/v1/auth/oidc/callback`. |
| `RAVENGLASS_RELAY_OIDC_ISSUER` | none | Turns OIDC on. The issuer URL exactly as the provider's discovery document states it. Must be `https://` (plain `http://` only to localhost, for testing). |
| `RAVENGLASS_RELAY_OIDC_CLIENT_ID` | none | The client id registered at the provider. |
| `RAVENGLASS_RELAY_OIDC_CLIENT_SECRET` | none | The client secret, for a confidential client (sent with HTTP Basic). Omit for a public client. |
| `RAVENGLASS_RELAY_OIDC_CLIENT_SECRET_FILE` | none | Read the client secret from this file instead. |
| `RAVENGLASS_RELAY_OIDC_SCOPES` | `openid email profile` | Scopes requested. |
| `RAVENGLASS_RELAY_OIDC_ALLOWED_EMAILS` | none | Comma-separated email addresses allowed to sign in (case-insensitive). |
| `RAVENGLASS_RELAY_OIDC_ALLOWED_DOMAINS` | none | Comma-separated email domains allowed to sign in. With both lists empty, anyone the provider authenticates may sign in. |
| `RAVENGLASS_RELAY_OIDC_VERIFY_SIGNATURES` | `true` | Check each ID token's signature against the provider's published keys (`jwks_uri`). Turn off only for a provider that publishes no keys: the Relay then trusts the token because it came straight from the token endpoint over TLS, and warns at start-up. TOML: `[oidc] verify_signatures`. |
| `RAVENGLASS_GOOGLE_CLIENT_ID` | none | Users mode: OAuth client id for Gmail accounts (same variable as the desktop app). |
| `RAVENGLASS_MICROSOFT_CLIENT_ID` | none | Users mode: OAuth client id for Microsoft 365 accounts. |

Fixed limits, not configurable: sessions end after 30 minutes idle or 12 hours in total; a user's
mail connections close 30 minutes after their last request when no push socket is open; request
bodies are capped at 96 MB (16 KB for sign-in, 64 KB for adding an account).

## The HTTP contract

The authoritative description, with the JSON types, is the `relay_proto::auth` module
(`crates/relay-proto/src/auth.rs`). In short:

| Route | Mode | Notes |
| --- | --- | --- |
| `GET /v1/auth/config` | both | No credentials. `{"mode":"token"\|"users","password":bool,"oidc":bool}` |
| `POST /v1/auth/login` | users | `Content-Type: application/json`, `{"username","password"}` → `200 {"csrf","user":{"id","name"}}` + session cookie; `401` (always the same text), `429` + `Retry-After` |
| `GET /v1/auth/oidc/start` | users | `303` to the provider, plus a `SameSite=Lax` flow cookie limited to `/v1/auth/oidc` |
| `GET /v1/auth/oidc/callback` | users | `303` to `RAVENGLASS_RELAY_ORIGIN` + session cookie; `400` / `403` text page on failure |
| `GET /v1/auth/session` | users | `200 {"csrf","user"}` or `401` |
| `POST /v1/auth/logout` | users | Needs the CSRF header. `204`, cookie cleared, session ended |
| `GET /v1/me/accounts` | users | The user's `AccountConfig`s, never secrets |
| `POST /v1/me/accounts` | users | `{"config": AccountConfig, "secret": {"kind":"Password"\|"RefreshToken","value"}}` → `200` config, `400`, `409` |
| `DELETE /v1/me/accounts/{id}` | users | `204` or `404` |
| `POST /v1/session`, `/v1/accounts/{a}/…` | both | The mail API, unchanged; needs `ravenglass-relay-version: 1` |
| `GET /v1/events` (WebSocket) | both | Pushes for the caller's own accounts only |
| `GET /v1/img?u=<url>` | both | The image proxy |

- **Cookie:** `__Host-ravenglass-session=<43 chars>; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=43200`
  (`ravenglass-session` without `Secure` in insecure dev mode). The browser sends it with
  `fetch(…, { credentials: "include" })` and never reads it.
- **CSRF:** every request whose method is not `GET`, `HEAD` or `OPTIONS`, except the login itself,
  must carry `x-ravenglass-csrf: <csrf>` from the login or session answer, or gets `403`.
- **Origin:** a browser `Origin` header on an authenticated users-mode request (and on login and the
  WebSocket) must equal `RAVENGLASS_RELAY_ORIGIN`, or the request gets `403`.
- **CORS:** only `RAVENGLASS_RELAY_ORIGIN` gets `Access-Control-Allow-Origin: <origin>`,
  `Access-Control-Allow-Credentials: true`, `Access-Control-Allow-Headers: authorization,
  content-type, ravenglass-relay-version, x-ravenglass-csrf` and `Access-Control-Allow-Methods: GET,
  POST, DELETE`. Preflights get `204`.
- **Security headers on every response:** `Content-Security-Policy: default-src 'none';
  frame-ancestors 'none'; sandbox`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
  `X-Frame-Options: DENY`, and `Cache-Control: no-store` unless the route sets its own.
- **Errors** are JSON `{"code","message"}` (`relay_proto::ApiError`), except the OIDC callback's
  plain-text page.

## OIDC

Register the Relay at your identity provider (Keycloak, Authentik, Google, Entra ID, …) as a web
application with the authorization-code flow and the redirect URI
`<RAVENGLASS_RELAY_PUBLIC_URL>/v1/auth/oidc/callback`. Then set `RAVENGLASS_RELAY_OIDC_ISSUER`,
`…_CLIENT_ID`, `…_CLIENT_SECRET` (or `…_FILE`) and `RAVENGLASS_RELAY_PUBLIC_URL`. The web app sends
the browser to `/v1/auth/oidc/start`.

How the Relay uses it:

- Discovery from `<issuer>/.well-known/openid-configuration` on first use (then cached until restart).
  The document's `issuer` must equal the configured one, and every endpoint must be `https`.
- Authorization code with PKCE (S256), a random `state` and `nonce`, and a flow cookie that binds the
  callback to the browser that started it (no login CSRF). A flow lasts at most 10 minutes and its
  callback works once.
- The ID token's signature is checked before any of its claims are read, then its `iss`, `aud`,
  `azp`, `exp` and `nonce` are checked:
  - The keys come only from the discovery document's `jwks_uri` (which must be `https`, like the
    other endpoints). Header fields that name or carry a key (`jku`, `x5u`, `jwk`, `x5c`) are ignored,
    and a token with a `crit` header is refused.
  - Algorithms: `RS256`, `RS384`, `RS512`, `PS256`, `ES256`, `ES384` and `EdDSA` (Ed25519). Never
    `none`, never an HMAC algorithm, and the `alg` must fit the key's type and curve (and the key's own
    `alg`, if it states one). RSA keys must be 2048 to 8192 bits.
  - The key is picked by `kid`; a token without a `kid` needs exactly one key that fits its `alg`.
  - The key set is kept in memory for its `Cache-Control: max-age`, held between 5 minutes and
    24 hours (1 hour without one). A token naming a `kid` the set lacks makes the Relay fetch the set
    once more (providers rotate keys), but never more than once a minute, so a stream of bad tokens
    cannot make it hammer the provider. If a refresh fails, the old set is used until the next try.
  - Verification is pure Rust (RustCrypto `rsa`, `p256`, `p384`, `ed25519-dalek`) and only ever
    verifies: there is no RSA decryption or signing in the Relay.
  - Any signature failure gives the same error page ("its signature does not verify").

  With `RAVENGLASS_RELAY_OIDC_VERIFY_SIGNATURES=false`, no keys are fetched and, as OIDC Core
  3.1.3.7 allows, the TLS connection the Relay made to the token endpoint stands in for the
  signature.
- The user is then read from the userinfo endpoint; its `sub` must equal the ID token's.
- With an allowlist set, the email must be present **and `email_verified` must be true**. Some
  providers (notably Entra ID) do not send `email_verified`; with them, use no allowlist and restrict
  access at the provider instead.
- A user is identified by `issuer|sub`, stored hashed. The display name is the email, else
  `preferred_username`, else the subject. A name already taken by a local user gets a `#n` suffix.

## Key management

- **Master key:** 32 random bytes in `RAVENGLASS_RELAY_MASTER_KEY_FILE`, created with mode 0600 if
  absent. The Relay warns at start if others can read it. It is never logged and never in an error.
  Mount it from your secret store if you have one.
- **Per-user keys:** derived with HKDF-SHA256 from the master key and the user id; one for sealing,
  one for blinding account ids. They exist only in memory, per request.
- **Sealing:** AES-256-GCM, a fresh random 96-bit nonce per value, associated data naming the user,
  the account and the field, so a ciphertext moved to another row does not decrypt.
- **What is sealed:** every account's settings (address, server names, login name) and every
  password or refresh token. Account ids are stored only as keyed hashes.
- **What is not:** user ids, usernames (for OIDC users, their email) and Argon2id password hashes.
- **Losing the master key** makes every stored account and secret unreadable; users must add their
  accounts again. **Leaking it together with the database** exposes every stored secret. Keep backups
  of the two apart.
- **Rotation** is not implemented yet: changing the key today means users re-adding their accounts.

## What is protected, and what is not

Protected:

- Another user, signed in or not, cannot list, open, use, delete or receive pushes for your
  accounts: every route resolves accounts within the signed-in user only, and an account that is not
  yours answers exactly like one that does not exist.
- A stolen database without the master key reveals no mail credentials, addresses or server names.
- A memory dump does not contain usable session cookies (only their SHA-256).
- Another website cannot act as you: `SameSite=Strict`, the CSRF token, the `Origin` check, and CORS
  limited to the app origin.
- Password guessing is slowed per username (5 free failures, then exponential backoff up to 15
  minutes) and per client address (20 free failures). Unknown usernames take as long as wrong
  passwords and get the same answer.
- The image proxy cannot reach loopback, private, link-local (cloud metadata), CGNAT, multicast or
  reserved addresses, including through redirects or DNS rebinding, and returns only images.

Not protected (by design or not yet):

- **A compromised Relay host** sees credentials when accounts are opened and mail as it passes. The
  Relay must be trusted with the mail it relays.
- **Usernames and OIDC emails** are in the database in clear.
- **Sessions and rate-limit counters are in memory.** Restarting signs everyone out and resets the
  counters, and the Relay cannot run as several instances behind a load balancer yet.
- **Token mode** has one owner: whoever holds the token has every account.
- **The image proxy** is open to any signed-in user for any public URL; it is not limited to images
  in the user's own messages.
- **Web Push and passkeys** are not implemented.

## Image proxy

`GET /v1/img?u=<percent-encoded URL>`, signed in. Only `http` and `https` URLs without credentials;
the host is resolved by the Relay and refused if **any** address it resolves to is not public; the
connection goes to the checked address (TLS is still verified against the host name); at most three
redirects, each checked again; no cookies, no `Referer`, no `Authorization` sent; 10 MB and 10
seconds at most; only `image/*` answers. Images come back with `Cache-Control: private,
max-age=86400` and the security headers above (`sandbox` stops SVG scripts). The Relay connects
directly, not through `HTTPS_PROXY`.
