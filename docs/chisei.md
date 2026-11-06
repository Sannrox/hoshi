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
bridge pulses need an HMAC-SHA256 the host keys. A blitz shard
([ADR 0022](decisions/0022-blitz-shards-run-on-a-scaled-clock-and-end-with-a-winner.md))
is created with a fixed `epoch` and `factor`; game unix is
`epoch + factor x (wall - epoch)`, the tenant view carries both, and the
command rate limit stays on wall time. Unsigned or altered pulses
are rejected. A player raid is a mission with flight
([ADR 0019](decisions/0019-shard-raids-are-missions-with-flight.md)):
`ShardSession::launch_raid` flies the fleet at another tenant's home on the
current flight time, fuel, and speed rules, and the attacker can recall it
while outbound. A `you`, `empty`, `outpost`, or `site` dest or a tenant
colony is refused at launch, and so are a young defender (newbie
protection) and an attacker past its per-identity attack cap. Nothing
fights at launch. At arrival the host catches the defender up and resolves
the same fleet combat as local play against its parked home fleet; plunder
uses the defender's share then cap, no more than the survivors' holds, and
the survivors fly it home as cargo. A tenant read is `ShardSession::view`: own
planets, fleets, and stock, plus spy intel that has already arrived, plus
each raid on home still in its combat log (ships lost, stock taken, debris
left), never RNG, pirate timers, or another tenant's parked fleet and
stockpile. A raid writes that log on the defender's save, so the next read
shows it and later reads keep it. While another tenant's raid still flies
at home, the view lists it as an inbound: the arrival on the flight clock,
no attacker identity or origin, and a hull count only when the defender's
own phalanx covers the fleet (the pirate raid contact's noise; a probe reads
parked fleets, never one in flight). The view's expiring decision takes the
earliest inbound as one more candidate with the arm action, since the fight
reads the parked home fleet and ignores stance and defences; a sooner story,
fight, or pirate warning still wins. The attacker's view shows its own
mission instead. The attacker's save logs the fight too and
its mission carries the plunder until it lands. The
local TUI and CLI do not use this gate.

A host joins a tenant with `ShardSession::join`. A seated identity resumes
on its home and nothing is written. A new identity past
`ShardConfig::tenant_cap` (bundled default 16, the capped player list of
ADR 0005) is refused with `ShardFull` before anything is built or written.
Otherwise join settles, places, builds the empire for the placed coord, and
seats it. It places the home with `place_home`: given the host's `ShardMap` (seed and
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
`ShardSession::settle` (and `view`, `colonize`, `spy`, `recycle`,
`launch_raid`, which settle first) steps every empire to each colony,
recycler, and raid arrival in time order, and at one instant in identity
order; a raid's defender steps to the arrival before the fight. Two colony ships may launch at
one `empty` coord; the first arrival founds, and a ship that finds the coord
held by then turns back with the colony ship. Recyclers launched with
`ShardSession::recycle` harvest the shared pile in arrival order, each
taking at most its hold from what is left. `submit` catches up without the
other empires, so it refuses with `Unsettled` while a colony, recycler, or
raid arrival of that empire, another tenant's raid on its home, or a
market tick or expiry while it has an order on the regional book, is due;
the host settles and retries. The session keeps each target's next raid
arrival in its bookkeeping, so the refusal holds across a restart.

One regional market: `ShardCommons` holds one NPC book per region that
every tenant trades on. `ShardSession::place_order` takes the escrow from
the tenant's stock in that region (the planet there plus its own
warehouse) and matches price-time against the NPC and other tenants'
resting orders, with the same matching and pricing as local play. Order
ids are unique per shard and each order carries its tenant, so a fill pays
the seller's iron and the buyer's goods into each save in that region at
the host clock; goods still reach another region only by shipment. A
resting order expires back to its tenant. The NPC price walk and quotes
tick once per `[market].tick_secs` on the host clock, in the hub and each
tenant home, from an RNG the first join seeds with the `ShardMap` seed, so
`ShardSession::book` reads the same mids and history for every tenant.
That read lists the region's resting orders with the reader's own marked
and no other identity. `submit` refuses `Intent::PlaceOrder` with
`OrderNeedsBook`. Contracts, shipments, and over-the-counter trades stay
on each save's private board.

The shard has one expedition coord. The host places it once with
`ShardCommons::place_expedition`: the last `Empty` slot on the `ShardMap` home
walk outside the frontier galaxy, so the seed and bounds alone fix it on a new
shard. It persists in the `ShardCommons` bytes (version 2 added it; version 1
bytes read with none, and the host places it after restoring them).
Version 3 added the market book; older bytes read with it closed, and the
next join opens it. `submit` flies an
`Intent::Expedition` without a site to that coord at the current flight rules,
so a farther tenant pays more fuel and time, and refuses with
`NoExpeditionCoord` until the host has placed it. Occupancy reads the coord as
`site` for every tenant, so join, colonize, and raids refuse it. Local
expeditions still fly to the home-system slot.

`ShardSession::spy` flies the probe hull with a hold of zero. At a `tenant`
coord the host stops catch-up at the probe's arrival, reads that planet's
stock and parked fleet, and writes a `SpyReport` with the espionage sensor
error for the range, the tier the probe score earned, and light-time from
the spy's home before it is delivered. The report is marked for the intel
compartment, so the player and intel-cleared staff read it and the tenant
view shows it as intel, never the target's save. When a lone catch-up (a
`submit`) ran the spy or the target past the arrival, the next
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

## Host contract

A shard host makes these calls, in this order. The executable reference is
[`crates/hoshi-storage/tests/shard_host_contract.rs`](../crates/hoshi-storage/tests/shard_host_contract.rs),
which runs under the default `cargo test`.

1. **Create.** `ShardSession::new` with the host clock, the bridge MAC key,
   and a `ShardConfig` whose `market` is the tenants' `[market]`. Pick a `ShardMap` once per shard; place outposts,
   sites, and the expedition coord through `commons_mut`.
2. **Join.** `ShardSession::join` per identity, up to `tenant_cap`. A
   reconnect returns the same home. The first join opens the market book.
3. **Act.** `submit` for intents, `launch_raid`, `spy`, `colonize`, and
   `recycle` for shard missions, `place_order` for the market book, each
   with an idempotency id. Client
   timestamps are ignored. When `submit` refuses with `Unsettled`, call
   `settle` and retry the same id.
4. **Settle.** `settle` steps every empire and the market book to the
   host clock. The mission calls, `place_order`, `view`, and `book` settle
   first.
5. **Read.** `view` per identity, and `book` for a region's market. A
   tenant never reads another tenant's `GameState`.
6. **Persist.** Each tenant's `GameState` through `hoshi-storage`
   (`save_state`; the save keeps the seated home), plus
   `export_bookkeeping` and `commons().to_bytes()` beside them.
7. **Restore.** A fresh `ShardSession::new`, then `restore_bookkeeping`,
   `*commons_mut() = ShardCommons::from_bytes(..)`, and `load_tenants` over
   the migrating loader. The next `settle` and `view` match a host that
   never stopped.

Story flags, daily slots, and victory are per tenant, and the shard stays
open after a victory; prestige reset is local only
([ADR 0021](decisions/0021-shard-progression-is-per-tenant.md)).
A shard created as a blitz shard may use a scaled clock and a finish line
([ADR 0022](decisions/0022-blitz-shards-run-on-a-scaled-clock-and-end-with-a-winner.md));
ordinary shards stay open after a per-tenant victory.

## Arena

```sh
hoshi --arena arena run --seed 7 --days 1 --backend chisei examples/bots/greedy.sh
```

Needs `--features chisei`. Without the feature the path or flag is refused.

## Throughput

The unit tests `apply_save_stays_within_three_times_sqlite_persist_work_for_fifty`
and `apply_save_persist_work_trips_when_chisei_work_doubles` run 50
propose-apply-save cycles (build queue + catch-up) and compare persist work
(SQLite saves plus Chisei graph writes), not CPU time. Chisei writes one sqlite
snapshot and one graph revision per cycle; the guard trips when that persist
work doubles. CLI and TUI still persist once per command, not per tick.
Catch-up stays in memory, then one save.

```sh
cargo test -p hoshi-storage --features chisei apply_save
```

## Tests

```sh
cargo test -p hoshi-storage --features chisei
```

Covers the shared storage contract from #25, object-graph rows, two-player fog
of war (hidden until alliance or spy), refused intents, and the persist-work
guard.
