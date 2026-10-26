# hoshi serve

`hoshi serve` is a remote view of **one** local save. It is not a shared
universe. The TUI and CLI keep the same rules; this process holds the save
lock and answers health checks so a later HTTP contract can talk to that save
([ADR 0004](decisions/0004-no-shared-universe.md),
[ADR 0005](decisions/0005-shared-shard-on-chisei.md)). Join is a
title-screen client of the Chisei shard
([ADR 0017](decisions/0017-local-tui-may-join-a-chisei-shard.md)); serve is
still one save.

Off unless you start it.

```sh
hoshi serve
hoshi serve --save PATH --bind 127.0.0.1:8787
hoshi --save PATH serve --bind 127.0.0.1:0
```

`--bind 127.0.0.1:0` asks the OS for an ephemeral port. The process prints one
JSON line with the bound address, then serves until it exits.

## Web client

Release binaries are built with `--features ui` and serve the hoshi-web client
at `/ui/` on the same origin as `/v1`, with no files on disk. `GET /` redirects
to `ui/` (a relative `Location`, so a proxy that mounts hoshi under a sub-path
keeps working). Unknown paths under `/ui/` without a file extension return
`index.html` for client routing; deeper paths redirect (relative) back up to
a route directly under `ui/`, because the page links `./assets/...` and its
service worker caches one page for every route. A missing file with an
extension is `404`.
Every client response carries `Content-Security-Policy: default-src 'self';
style-src 'self' 'unsafe-inline'; base-uri 'self'; frame-ancestors 'none'`:
no inline or remote script, while the terminal view may generate its styles.
Files under `assets/` are cached as immutable; everything else is revalidated.

The bundle is one hoshi-web release, pinned in `crates/hoshi-tui/hoshi-web.pin`
as `<tag> <sha256-of-zip>`. A `--features ui` build downloads it once and
refuses a checksum mismatch; bumping the client is a one-line pin change.
Builds without `ui` download nothing, and `/` and `/ui/` return `404`.

`--ui-dir PATH` serves a client build from a directory instead, for working on
the client. It is never a default.

## Config

`config.toml` next to the save (or `--config DIR`):

```toml
serve_bind = "127.0.0.1:8787"
serve_save = ""
serve_token = ""
serve_token_file = ""
serve_rate_limit = 60
serve_rate_window_secs = 60
serve_terminal_idle_secs = 1800
serve_push = false
serve_vapid_private = ""
serve_vapid_subject = ""
serve_oidc = false
serve_oidc_issuer = ""
serve_oidc_client_id = ""
serve_oidc_authorization = ""
serve_oidc_token = ""
```

Empty `serve_bind` uses loopback port 8787. Empty `serve_save` uses the default
save slot (`--save` / `HOSHI_SAVE` still win).

## Tokens and limits

`/v1` requires `Authorization: Bearer <token>`. `GET /healthz` stays open.

Set the secret at runtime, never in the save:

- `HOSHI_SERVE_TOKEN` or `serve_token` in `config.toml`: one secret scoped to the
  process save.
- `HOSHI_SERVE_TOKEN_FILE` or `serve_token_file`: extra lines
  `secret<whitespace>save-path`. A secret bound to another save is refused.

An empty token list rejects every `/v1` call. Secrets are compared in constant
time. They are not written to SQLite.

Rate limits are per token and per action path (`/v1/state`, `/v1/build`, …).
The sliding window uses `serve_rate_limit` (default 60) and
`serve_rate_window_secs` (default 60). A 429 response may include `Retry-After`.

## Device sessions

A paired browser holds its own device session ([ADR 0016](decisions/0016-device-pairing-and-sessions.md)).
Its secret is accepted as `Authorization: Bearer <secret>` wherever a scoped
token is: same save scope, same rate limits per device, never admin scope.
That pairing never authenticates to a shard; Join credentials are a
separate file ([ADR 0020](decisions/0020-shard-identity-is-a-host-tenant.md)).

Sessions are runtime data in `<save>.devices.json` beside the save (mode
`0600`, replaced atomically). The file stores each device's label, type, OS,
browser, address and timestamps, and only the SHA-256 of its secret. A session
expires 30 days after its last use. If the file cannot be read, every device
session is refused and the file is left untouched.

### Session cookie

A browser on the same origin sends its device session in an `HttpOnly`
cookie, `hoshi_session`, that page scripts cannot read. Pairing sets it with
`SameSite=Lax`, `Path` set to the install's base path (so two installs behind
one proxy origin do not collide) and `Secure` over HTTPS. `/v1` routes,
`/v1/events` and `/v1/terminal` accept the cookie as well as a bearer token:

- The cookie only ever carries a device session, never an operator token.
  Same save scope and rate limits as a bearer token.
- Cookie and bearer are alternatives; a request with both is `400`.
- Writes (`POST`, `DELETE`) with the cookie must send
  `Content-Type: application/json`, which a cross-site form cannot; otherwise
  `403`.
- A WebSocket upgrade with the cookie must send an `Origin` whose host is the
  request's `Host`; otherwise `403`.
- An expired or unknown session is `401`.

### Pairing a phone

`hoshi serve` started on a LAN or tailnet address prints a pairing link and QR
code to stderr. Scan it with the phone, or open the link: the phone is signed
in to the client at `/ui/` with its own device session, and no secret is
typed. On a loopback bind it prints a hint instead, since a phone cannot reach
loopback. `hoshi pair [--label TEXT]` mints a fresh code on the running server
any time.

- Codes are 8 characters from an alphabet without look-alikes (shown as
  `ABCD-2345`, typeable when a camera fails), work once, expire after 10
  minutes and live in memory only: a restart drops unused codes, never paired
  sessions.
- `POST /v1/admin/pair {"label"?}` (admin token) mints one:
  `{"ok":true,"code","expires_at","url"}`. The link is
  `<base>/ui/#pair=<code>`; the code travels in the fragment, which browsers
  never send in requests.
- `POST /v1/pair {"code"}` (no token, `Content-Type: application/json`, else
  `403` before the attempt counts) spends the code, creates a device session
  from the request's User-Agent and peer address, and sets the session cookie.
  The reply is `{"ok":true,"device":{"id","label","device_type"}}` with
  `Cache-Control: no-store`; the secret is only in the cookie. A wrong code is
  `401`; five wrong codes from one address within 10 minutes make further tries
  `429` for the rest of that window. Behind a reverse proxy on the same machine
  (with `--url` set) the address is the last `X-Forwarded-For` entry, which
  that proxy appends; otherwise it is the TCP peer. A proxy on another machine
  shares one limit for all its clients.
- `--url http[s]://HOST[:PORT][/prefix]` on `hoshi serve` sets the base players
  use, for a reverse proxy or HTTPS front end. The cookie `Path` is that
  prefix (else `/`), and the cookie is `Secure` when the base is HTTPS or the
  proxy sends `X-Forwarded-Proto: https`. Without `--url`, links use the
  request's host; a `0.0.0.0` bind (or `hoshi pair` run on the server's own
  machine) advertises this machine's outbound address.

### Managing devices

- `GET /v1/admin/devices` (admin token) lists live sessions: `id`, `label`,
  `device_type`, `os`, `browser`, `ip`, `created_at`, `last_used_at`,
  `expires_at`. Never the secret or its hash.
- `DELETE /v1/admin/devices/{id}` (admin token) revokes one session.
- `DELETE /v1/devices/self` (the session's own credential) revokes it and
  clears the cookie. With the cookie it needs `Content-Type: application/json`
  like every cookie write.

A revoked session is refused at once: its next request is `401` and its open
`/v1/events` and `/v1/terminal` sockets close. Revocation is written to the
sessions file before it answers, so it survives a restart. Other devices keep
working.

From a shell, `hoshi devices` lists and `hoshi devices revoke ID` revokes
through these routes. The server is `--url http://HOST:PORT[/prefix]`, else
`serve_bind`; the admin token comes from `HOSHI_SERVE_ADMIN_TOKEN` or
`serve_admin_token`.

## Web push

Raid warnings and finished builds reach browsers with the tab closed. Off until
the player opts in and the operator sets VAPID keys. Keys and subscriptions
never enter the save.

- `HOSHI_SERVE_PUSH` or `serve_push`: player opt-in (default off).
- `HOSHI_SERVE_VAPID_PRIVATE` / `serve_vapid_private` and
  `HOSHI_SERVE_VAPID_SUBJECT` / `serve_vapid_subject`: operator VAPID private
  key (the 32-byte P-256 scalar, base64url) and contact URI (`mailto:` or
  `https:`). A missing subject or a key that does not parse keeps push off even
  when opt-in is on.

A browser opts in through three routes. They use the same bearer token and
rate limits as the rest of `/v1`, and answer `404` while push is off.

| Route | Body | Result |
| --- | --- | --- |
| `GET /v1/push/key` | | `{"ok":true,"key":"<base64url>"}`, the `applicationServerKey` |
| `POST /v1/push/subscription` | `PushSubscription.toJSON()` | stores it for this save; `400` for an endpoint outside the browser push services or keys that do not parse, `409` past 16 subscriptions |
| `DELETE /v1/push/subscription` | `{"endpoint":"…"}` | `{"ok":true,"removed":true\|false}` |

Endpoints must be HTTPS on port 443 at a browser push service:
`fcm.googleapis.com`, `*.push.services.mozilla.com`, `*.notify.windows.com`,
`web.push.apple.com` or `*.push.apple.com`. The server never posts anywhere
else, so a subscription cannot aim it at an internal address.

Subscriptions are runtime data in `<save>.push.json` beside the save (mode
`0600`), like token files. Posting the same endpoint again replaces its keys.

Each notice is encrypted for each subscription (RFC 8291, `aes128gcm`), signed
with VAPID (RFC 8292), and posted by `curl` over HTTPS with a one-day `TTL`. A
push service answering `404` or `410` removes that subscription; other failures
are logged and the notice is dropped. Sends run on their own thread behind a
short queue, so a slow push service never delays the next raid warning; when
the queue is full the notice is dropped and logged. Payloads are JSON:

```json
{"kind":"raid","title":"hoshi","body":"raid incoming in 50s","url":"#/bridge"}
```

`url` is relative to the web client (`#/bridge` for raids, `#/colony` for
builds). Payloads use the same public text as the TUI and omit hidden
simulation (`next_raid_at`, RNG, unrevealed map). Fleet returns stay off this
channel. While push is on, the process catches up the save about once a second
so notices fire without a connected client.

This is not the in-session WebSocket and not a shared-universe notifier.

## Identity login

Optional Authorization Code with PKCE (S256). Off until the operator opts in.
Bearer tokens stay the default. Identity-provider settings are runtime config,
never the save.

- `HOSHI_SERVE_OIDC` or `serve_oidc`: opt-in (default off).
- `HOSHI_SERVE_OIDC_ISSUER` / `serve_oidc_issuer`: issuer identifier.
- `HOSHI_SERVE_OIDC_CLIENT_ID` / `serve_oidc_client_id`: relying-party id.
- `HOSHI_SERVE_OIDC_AUTHORIZATION` / `serve_oidc_authorization`: authorization
  endpoint. Incomplete settings leave the flow off even when opt-in is on.
- `HOSHI_SERVE_OIDC_TOKEN` / `serve_oidc_token`: token endpoint. The process
  does not add a TLS HTTP client; a token exchange is injected for tests.

`GET /v1/oidc/start` redirects to the authorization endpoint with `state`,
`nonce`, and a PKCE `code_challenge`. The callback URI is `http://<listen-address>/v1/oidc/callback` from the
accepted socket (loopback when the process bound an unspecified address),
not the request Host header. `GET /v1/oidc/callback` exchanges the code and
mints a bearer secret scoped to this save. That secret is the same kind of
token `/v1` already accepts. Anyone the configured issuer authenticates can
mint one; the issuer's user set is the access list. Unauthenticated `/v1`
game routes stay rejected. `/healthz` stays open. Callback JSON sets
`Cache-Control: no-store`.

This is not a shared-universe account system and does not replace bearer tokens.

## Lock

The process takes the same exclusive save lock as the TUI and CLI. A second
writer on that file is refused (CLI exit 4). Closing the process drops the lock.

## Health

`GET /healthz` returns JSON:

```json
{"ok":true,"contract_versions":["v1"]}
```

`contract_versions` lists the HTTP contract this binary speaks.

## `/v1` actions

Game routes call the same hoshi-core actions as the CLI. Responses carry
`schema_version` 2 and match `docs/schema/cli-v1.json` plus
`docs/schema/serve-v1.json`. A breaking shape bumps that version.

| Method | Path | CLI |
| --- | --- | --- |
| `GET` | `/v1/state` | `hoshi state` |
| `POST` | `/v1/build` | `hoshi build <planet> <building>` |
| `POST` | `/v1/dismantle` | `hoshi dismantle <planet> <building>` |
| `POST` | `/v1/fusion/mode` | `hoshi fusion mode <planet> dd\|dhe3\|off` |
| `POST` | `/v1/research` | `hoshi research <tech>` |
| `POST` | `/v1/cancel` | `hoshi cancel <queue-item>` |
| `POST` | `/v1/fleet/send` | `hoshi fleet send <from> <to> <mission> <ships...>` |
| `POST` | `/v1/ship` | `hoshi ship <planet> <ship-or-defence> [count]` |
| `POST` | `/v1/trade` | `hoshi trade <pay> <receive> <amount>` |

POST bodies are JSON:

```json
{"planet":"home","building":"metal_mine"}
{"planet":"home","mode":"off"}
{"tech":"energy"}
{"item":"0"}
{"from":"home","to":"1:1:8","mission":"attack","ships":["light_fighter:1"]}
{"planet":"home","unit":"light_cannon","count":5}
{"pay":"iron","recv":"deuterium","amount":1000}
```

There is no `tick --advance` route. Clients see only what a player sees
through the CLI.

## Events

`GET /v1/events` is a WebSocket (RFC 6455) on the same bind. It uses the same
bearer token as `/v1`. Unauthenticated sockets receive `401` and are not
upgraded. `/healthz` stays open while a socket is held.

Browsers cannot set `Authorization` on a WebSocket, so they send the token as
a subprotocol next to `hoshi.v1`, base64url-encoded without padding:

```js
new WebSocket(url, ["hoshi.v1", "hoshi.bearer." + base64url(token)])
```

The server checks it like the header (same save scope and rate limit) and
answers `Sec-WebSocket-Protocol: hoshi.v1`; the token is never echoed. Keep it
out of the URL. A request with both the header and a `hoshi.bearer.` entry,
more than one `hoshi.bearer.` entry, or one without `hoshi.v1` gets `400`.

Reconnect with `?cursor=<id>`, where `<id>` is the last event `id` the client
stored. The server replays every retained public event with `id` greater than
that cursor, then sends new ones as they land. The log keeps the last 256
events; a cursor older than that window has already dropped.

Each text frame is JSON (`schema_version` 2):

```json
{"schema_version":2,"ok":true,"id":12,"cursor":12,"at":1710000000,"type":"build","action":"complete","objects":["home","metal_mine"],"outcome":"lv 1"}
```

| `type` | History kind | When it fires |
| --- | --- | --- |
| `build` | build | queued or finished construction |
| `research` | research | started or finished research |
| `fleet` | fleet | launch, arrival, or other fleet motion |
| `raid` | combat | pirate raid and other fight results |

Frames omit hidden simulation fields (RNG, unrevealed map, pending intel,
`next_raid_at`). A breaking event shape bumps `schema_version`.

## Terminal

`GET /v1/terminal` is a WebSocket that runs a TUI session against the served
save inside the `hoshi serve` process, for a browser terminal. It
authenticates like `/v1/events`: the `Authorization` header, or the
`hoshi.v1` and `hoshi.bearer.` subprotocols from a browser. It is not a shell
or a PTY; it only draws the game.

- **Binary frames** carry terminal bytes both ways. The client sends key
  input as an xterm would: printable characters, Enter, Esc, Tab, Backspace,
  arrows, Home, End, Page Up/Down. The server sends rendered output as ANSI
  escape sequences.
- **Text frames** carry JSON control messages. The only one is
  `{"type":"resize","cols":120,"rows":40}`; sizes are clamped to 20x8 at
  least and 500x500 at most. A session starts at 80x24 and repaints in full
  after a resize.

One writer per save. While a terminal session holds the save, `/v1` game
actions (every `POST` route) and a second `/v1/terminal` get `409 refused`
with "a terminal session holds the save", like the TUI's save lock.
`GET /v1/state`, `/healthz` and `/v1/events` keep working.

The server pings every 5 seconds; a client that sends no frame at all (pongs
count) for 15 seconds is treated as dropped. The session ends when the player
quits, the client sends a close frame, or no
input arrives for `serve_terminal_idle_secs` (default 1800, 0 disables); the save is free
at once and every change is already saved. If the socket drops instead, the
save stays held for a 10-second grace period, so an old socket that is still
closing and a new one never both write. After that a new socket takes over
and draws the whole screen.

### Errors

Every error is JSON with HTTP status:

```json
{"schema_version":2,"ok":false,"error":{"code":"invalid","message":"not found"}}
```

| HTTP | `error.code` | Meaning |
| --- | --- | --- |
| 400 | `invalid` | Bad path, method, JSON, or arguments |
| 401 | `unauthorized` | Missing, unknown, or wrong-save bearer token |
| 404 | `invalid` | Unknown route |
| 405 | `invalid` | Known route, wrong method |
| 409 | `refused` | Rule refusal (same as CLI exit 3) |
| 429 | `rate_limited` | Token exceeded the per-action window |
| 500 | `internal` | Save or IO failure |

## Not a shard

A live shared shard is a Chisei-hosted deployment. `hoshi serve` wraps one
local save for remote clients of the same player. It does not own a second
clock or a shared galaxy.

## Build versions and optional update checks

`GET /healthz` includes running `version`, `channel`, `save_schema`,
`cli_contract`, supported `contract_versions`, and `installed_version` when the
managed installation has changed beneath the process. A differing installed
version means the operator should restart the server.

Authenticated `GET /v1/version` exposes the same version information with the
CLI response schema version. Clients should compare contracts first: different
contracts require a warning naming the client and server versions. A differing
build with the same contract is informational.

The server makes no automatic release request by default. Opt in with
`hoshi serve --update-check`. The check uses the same signature verification,
ten-second timeout, daily cache and endpoint settings as interactive play.
`--offline`, `HOSHI_OFFLINE=1`, `HOSHI_SKIP_VERSION_CHECK=1` and
`update_check = false` suppress it. A verified newer release appears under
`latest`; otherwise that field is null. Each WebSocket connection receives one
`server_update_available` notification per known newer version, outside the game
history cursor. See [installation](install.md#update-checks-and-release-log) for
the request contents and opt-outs.
## Support reports

The admin-only `/v1/admin/support-report` endpoint returns redacted local diagnostics
and server counters. See [Support reports](support.md) for credentials and opt-out.
