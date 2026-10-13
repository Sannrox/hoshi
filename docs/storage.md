# Save backends

hoshi-core talks to a [`Storage`](../crates/hoshi-core/src/storage.rs) trait.
Rules never open a database. The TUI and CLI call `open_storage` and then
`load` / `save`.

## SQLite (default)

The existing `save.sqlite` file is unchanged: WAL, schema migrations, and
`PRAGMA user_version`. Paths that are not `*.mikura` or `*.chisei` use this backend.

## Mikura (optional)

Build with `--features mikura` and pass a `*.mikura` path (`--save empire.mikura`
or `HOSHI_SAVE`). The crate pin is git tag `v0.2.0` (not crates.io). Without
the feature the game still builds and plays; a `.mikura` path is refused.

Each save writes one atomic `append_batch`:

| Kind | Key | Links / props |
| --- | --- | --- |
| Snapshot | `state` | Versioned sidecar `{log}.state.<n>.sqlite` (Mikura strings cap at 64 KiB) plus an identity index |
| Player | `player` | `last_tick`, `rng_seed` |
| Planet | `g:s:p` | `owner=player`, `kind` |
| Fleet | `parked:g:s:p` or `mission:id` | `stationed-at` or `en-route-to`, `ships` |
| Outpost | `g:s:p` | `garrison`, `kind` |

Load follows the sidecar id on the committed Snapshot record, so a crash after
writing a newer file and before `append_batch` cannot mix versions. The sidecar
is the codec: load reconstructs the same `GameState` as the default backend. Player / Planet / Fleet / Outpost rows are the object
graph for later shard work. Identities that leave the empire are hidden in the
same batch.

## Write rates

| Event | Writes |
| --- | --- |
| CLI command (`hoshi build`, `state`, …) | 1 save at the end |
| New game / `hoshi new --seed` | 1 save |
| TUI while playing | 1 save every 5 seconds |
| TUI quit | 1 save |
| Offline catch-up | in memory, then 1 save (not per tick) |
| Title prestige buy | 1 save |

SQLite uses one transaction per save. Mikura uses one `append_batch` per save.
A crash mid-write does not keep a partial empire: SQLite keeps the last
checkpointed snapshot (WAL); Mikura drops the whole batch.

## Chisei (optional)

Build with `--features chisei` and pass a `*.chisei` directory (`--save
empire.chisei`). Off in `config.toml` until `chisei = true`. Without the
feature a `.chisei` path is refused. The directory holds `state.sqlite` (the
codec), `graph.toml` (Player, Planet, Fleet, Faction, Outpost, links, markings),
and a copy of `hoshi-ontology/v1`. Moves go through propose then apply.
See [docs/chisei.md](chisei.md) and [ADR 0007](decisions/0007-optional-chisei-backend.md).
The API pin is `1.3.0`.

## Tests

Shared contract (round-trip, atomic colony/fleet/outpost, catch-up, colonize):

```sh
cargo test -p hoshi-core sqlite_backend_meets_storage_contract
cargo test -p hoshi-core --features mikura
cargo test -p hoshi-core --features chisei
```
