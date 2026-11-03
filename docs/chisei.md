# Optional Chisei backend

Off until you build with `--features chisei` **and** either pass a `*.chisei`
path or set `chisei = true` in `config.toml`. The local game stays SQLite.
hoshi-storage does not open a network connection; this is an in-process object
graph plus the same SQLite `GameState` codec as the default save. A live shared
shard is still a Chisei-hosted deployment ([ADR 0005](decisions/0005-shared-shard-on-chisei.md),
[ADR 0007](decisions/0007-optional-chisei-backend.md)). Join is a
title-screen client of that host
([ADR 0017](decisions/0017-local-tui-may-join-a-chisei-shard.md));
this backend is still in-process and is not that client.

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
are rejected. Player raids today catch up both empires, then resolve instantly against
that snapshot with the same fleet combat as local play.
[ADR 0019](decisions/0019-shard-raids-are-missions-with-flight.md) moves
raids to missions with flight; `ShardSession::raid` still resolves at
admit time until that follow-up. A young defender is
protected; an attacker has a per-identity attack cap; plunder uses the
defender's share then cap. A tenant read is `ShardSession::view`: own
planets, fleets, and stock, plus spy intel that has already arrived, plus
each raid on home still in its combat log (ships lost, stock taken, debris
left), never RNG, pirate timers, or another tenant's parked fleet and
stockpile. A raid writes that log on the defender's save, so the next read
shows it and later reads keep it. The attacker reads its plunder from the
raid result. The
local TUI and CLI do not use this gate.

Join places a home with `place_home`: given the host's `ShardMap` (seed and
bounds), the empires, and the pirate outposts and open sites, it returns the
first `Empty` coord on a walk that visits one position in every system before
the next position. `seat_home` writes the home only on a coord that is still
`Empty`, so a parallel place that lost the slot is refused and places again.
`targets` lists the coords a tenant's clearance can see tagged `you`,
`tenant`, `outpost`, `site`, or `empty`, with no other identity or stockpile;
spy and shard raids pick a `tenant` coord from it. The tenant view keeps the
coarser `you`, `occupied`, `empty` tags.

One shard map: the session holds a `ShardCommons` with the pirate outposts,
the open sites, and the battle debris per coord, so every tenant sees the
same `outpost` and the same pile. Tenant saves keep no shard debris; each
catch-up lends the pile to one tenant and takes it back with its harvests
and new wreckage. The host persists `ShardCommons::to_bytes` beside the
tenant saves and restores it through `ShardSession::commons_mut`.
`ShardSession::settle` (and `view`, `colonize`, `spy`, `recycle`, which
settle first) steps every empire to each colony and recycler arrival in time
order, and at one instant in identity order. Two colony ships may launch at
one `empty` coord; the first arrival founds, and a ship that finds the coord
held by then turns back with the colony ship. Recyclers launched with
`ShardSession::recycle` harvest the shared pile in arrival order, each
taking at most its hold from what is left. `submit` and `raid` catch up
without the other empires, so they refuse with `Unsettled` while a colony or
recycler arrival is due; the host settles and retries. The expedition dest
and the NPC regional book are still per save.

`ShardSession::spy` flies the probe hull with a hold of zero. At a `tenant`
coord the host stops catch-up at the probe's arrival, reads that planet's
stock and parked fleet, and writes a `SpyReport` with the espionage sensor
error for the range, the tier the probe score earned, and light-time from
the spy's home before it is delivered. The report is marked for the intel
compartment, so the player and intel-cleared staff read it and the tenant
view shows it as intel, never the target's save. When a lone catch-up (a
`submit` or a raid) ran the spy or the target past the arrival, the next
host catch-up reads the target as it is then and stamps the report with
that time. An
`outpost` coord keeps the local spy, `you` is refused, and `empty` and `site`
have nothing to spy. Local `launch_spy` still takes outposts only.

An idempotency id is remembered for the longer of the command and attack
windows (`ShardConfig::seen_ttl_secs`); an older id is forgotten and may
apply again. On restart the host restores the ids, command windows and
attack stamps from `ShardSession::export_bookkeeping`, opaque bytes it keeps
beside the tenant saves and never puts in a tenant view or bug report. It
loads tenants one at a time with `load_tenants` through the migrating save
loader; one tenant that fails to load does not stop or write the others.

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
