# Optional Chisei backend

Off until you build with `--features chisei` **and** either pass a `*.chisei`
path or set `chisei = true` in `config.toml`. The local game stays SQLite.
hoshi-storage does not open a network connection; this is an in-process object
graph plus the same SQLite `GameState` codec as the default save. A live shared
shard is still a Chisei-hosted deployment ([ADR 0005](decisions/0005-shared-shard-on-chisei.md),
[ADR 0007](decisions/0007-optional-chisei-backend.md)).

Pinned product/API version: **1.3.0** (`CHISEI_API_VERSION` in hoshi-core).
Ontology package: **hoshi-ontology/v1**, `schema_version: 1`, at
`crates/hoshi-core/data/ontology/hoshi-v1.json`.

```sh
cargo build -p hoshi --features chisei
hoshi --features chisei --save empire.chisei state
```

`config.toml`:

```toml
chisei = false
```

When `chisei = true` (and the feature is on), `hoshi --arena arena run` writes
bot saves as `*.chisei`. Override per run with `--backend chisei` or
`--backend sqlite`.

## Object model

| Kind | Key | Links / markings |
| --- | --- | --- |
| Player | tenant id | — |
| Planet | `{tenant}:g:s:p` | `owns` from Player; `tenant` marking |
| Fleet | `{tenant}:parked:g:s:p` or `{tenant}:mission:id` | `stationed-at` |
| Faction | `{tenant}:id` | — |
| Outpost | `{tenant}:g:s:p` | garrison |
| — | — | `allied-with` (shared visibility) |
| — | — | `cleared-for` spy marking |

Need-to-know stays in the tenant SQLite codec (`classified`, sharing agreements, incidents). The shared graph does not copy marked object ids, classes or compartments, so allies and other graph readers cannot inspect them. Ontology classes Marking, Compartment, Clearance and SharingAgreement remain in `hoshi-v1.json`.

Moves go through `propose` then `apply`. A refused proposal never writes the
live state or the save.

A live shard (not this in-process backend) wraps those moves in
`ShardSession`: the host clock is the only time source, a client timestamp is
ignored, each identity is rate limited, intents carry an idempotency id, and
bridge pulses need an HMAC-SHA256 the host keys. Unsigned or altered pulses
are rejected. Player raids catch up both empires, then resolve against that
snapshot with the same fleet combat as local play. A young defender is
protected; an attacker has a per-identity attack cap; plunder uses the
defender's share then cap. A tenant read is `ShardSession::view`: own
planets, fleets, and stock, plus spy intel that has already arrived, never
RNG, pirate timers, or another tenant's parked fleet and stockpile. The
local TUI and CLI do not use this gate.

The optional governance demo (`hoshi --demo demo governance --seed N`) puts an
ungoverned CLI player next to a Chisei-governed one. Policies (spend caps,
no attacks on allies, invasion approval, token budgets) wrap propose/apply.
See [docs/demo.md](demo.md).

## Arena

```sh
hoshi --arena arena run --seed 7 --days 1 --backend chisei examples/bots/greedy.sh
```

Needs `--features chisei`. Without the feature the path or flag is refused.

## Throughput

The unit test `apply_save_throughput_stays_under_twelve_seconds_for_fifty` runs
50 propose-apply-save cycles (build queue + catch-up) and requires the whole
loop to finish in under twelve seconds. CLI and TUI still persist once per
command, not per tick. Catch-up stays in memory, then one save.

```sh
cargo test -p hoshi-storage --features chisei apply_save_throughput
```

## Tests

```sh
cargo test -p hoshi-storage --features chisei
```

Covers the shared storage contract from #25, object-graph rows, two-player fog
of war (hidden until alliance or spy), refused intents, and the throughput
bound.
