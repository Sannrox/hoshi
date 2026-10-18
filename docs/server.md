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
```

Empty `serve_bind` uses loopback port 8787. Empty `serve_save` uses the default
save slot (`--save` / `HOSHI_SAVE` still win).

## Lock

The process takes the same exclusive save lock as the TUI and CLI. A second
writer on that file is refused (CLI exit 4). Closing the process drops the lock.

## Health

`GET /healthz` returns JSON:

```json
{"ok":true,"contract_versions":["v1"]}
```

`contract_versions` lists the HTTP contract this binary will speak. Game
routes are not served yet.

## Not a shard

A live shared shard is a Chisei-hosted deployment. `hoshi serve` wraps one
local save for remote clients of the same player. It does not own a second
clock or a shared galaxy.
