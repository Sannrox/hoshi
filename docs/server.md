# hoshi serve

`hoshi serve` is a remote view of **one** local save. It is not a shared
universe. The TUI and CLI keep the same rules; this process holds the save
lock and answers health checks so a later HTTP contract can talk to that save
([ADR 0004](decisions/0004-no-shared-universe.md),
[ADR 0005](decisions/0005-shared-shard-on-chisei.md)).

Off unless you start it.

```sh
hoshi serve
hoshi serve --save PATH --bind 127.0.0.1:8787
hoshi --save PATH serve --bind 127.0.0.1:0
```

`--bind 127.0.0.1:0` asks the OS for an ephemeral port. The process prints one
JSON line with the bound address, then serves until it exits.

## Config

`config.toml` next to the save (or `--config DIR`):

```toml
serve_bind = "127.0.0.1:8787"
serve_save = ""
serve_token = ""
serve_token_file = ""
serve_rate_limit = 60
serve_rate_window_secs = 60
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
| `POST` | `/v1/research` | `hoshi research <tech>` |
| `POST` | `/v1/cancel` | `hoshi cancel <queue-item>` |
| `POST` | `/v1/fleet/send` | `hoshi fleet send <from> <to> <mission> <ships...>` |

POST bodies are JSON:

```json
{"planet":"home","building":"metal_mine"}
{"tech":"energy"}
{"item":"0"}
{"from":"home","to":"1:1:8","mission":"attack","ships":["light_fighter:1"]}
```

There is no `tick --advance` route. Clients see only what a player sees
through the CLI.

## Events

`GET /v1/events` is a WebSocket (RFC 6455) on the same bind. It uses the same
bearer token as `/v1`. Unauthenticated sockets receive `401` and are not
upgraded. `/healthz` stays open while a socket is held.

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
