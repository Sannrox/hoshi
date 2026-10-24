# ADR 0005: Shared shard hosted on Chisei

- Status: Partially superseded by [0017](0017-local-tui-may-join-a-chisei-shard.md)
- Date: 2026-10-12
- Partially supersedes: [0004](0004-no-shared-universe.md)
- Follows: [0002](0002-terminal-idle-game-with-event-bridge.md)
- Source: [#18](https://github.com/Sannrox/hoshi/issues/18)

Point 1's "The local game never becomes a client of a shared world" and
the later "not a mode of the local binary" sentence are superseded by
[0017](0017-local-tui-may-join-a-chisei-shard.md). Points 2–4 stay: Chisei
host, intents, snapshot raids, markings, bounded shard. The shard remains
a Chisei-hosted workload; the local TUI may join it as a client.

## Context

ADR 0004 keeps hoshi a local single-player idle game and lists the
conditions under which a shared universe may be revisited. Those conditions
describe a server that owns time, randomness and persistence, accepts only
intents, catches up on demand, resolves combat against snapshots, and runs
bounded galaxies.

hoshi has a second purpose besides the terminal game. It is a showcase for
ontology use in business and government: markets (#105), classification
and markings (#114), and governed versus ungoverned AI (#69). It is also the
reference workload for Chisei (#65, Sannrox/chisei#1426). Those scenarios
need several actors acting on shared objects under policy. A single save
with a single owner cannot demonstrate them. Multi-actor play is the point
of the showcase, not an extra mode.

Chisei already provides the pieces 0004 asks for: an authoritative store, a
clock and scheduler it owns, governed Actions with audit, tenants and spaces,
and markings on objects.

## Decision

1. **The local game is unchanged.** Everything in ADR 0004 about local play
   stays in force: one player, one save, one clock, no account, no network
   requirement. The local game never becomes a client of a shared world.
2. **A shared universe is allowed only as a server-authoritative shard
   hosted on Chisei.** No other substrate (client sync, peer lockstep, a
   player-hosted server, a separate hoshi game server) is permitted.
3. A shard must meet every revisit condition of ADR 0004:
   - **Chisei owns the wall clock, the RNG and persistence.** Clients never
     hold or write the live shard state.
   - **Every player intent is a governed Chisei Action** that calls the same
     hoshi-core actions as the CLI. Actions carry idempotency ids, are rate
     limited and are audited.
   - **Catch-up is on demand.** `GameState::catch_up` runs inside the Action
     at action time and at interaction time (for example a raid). There is
     no per-second loop over every empire.
   - **Raids are asynchronous against a snapshot** of the target taken after
     honest catch-up. No lockstep live combat.
   - **Shards are bounded:** one seed and a capped player list per shard.
     There is no seamless universe.
4. **Each player is a tenant or space in Chisei**, and their objects carry
   markings. Reads go through Chisei's policy, so hidden state (RNG,
   pirate timers, unrevealed map, other players' intel) is never returned
   to a player who is not cleared for it.

This satisfies the "second product" condition of 0004: the shard is a
Chisei workload and showcase, not a mode of the local binary.

## Threat model as requirements

The threat table in ADR 0004 is kept and becomes the acceptance list for the
shard. Each row must have a test or a documented control before a shard is
opened to more than one player:

| Threat (from 0004) | Required control on the shard |
| --- | --- |
| Edit the save | No client-side state; persistence is Chisei-owned |
| Skew or freeze the clock | Chisei clock only; client timestamps ignored |
| `tick --advance` | Not exposed as an Action on a shard |
| Hidden fields in state | Markings and policy filter every read |
| Duplicate request after disconnect | Idempotent Action ids |
| Two sessions on one account | Per-player serialisation of Actions |
| Raid an offline player | Snapshot after server catch-up |
| Forge bridge activity | Bridge events not trusted for shard score, or capped and governed |
| Multi-accounting | Tenant identity from the platform issuer; rate limits per identity |

## Consequences

- hoshi-core stays free of network and server code. The shard reuses core
  actions behind Chisei Actions.
- #19 (raids) and #20 (authentication and anti-cheat) are unblocked and
  must target this design. #18 is answered by 0004 for local play and by
  this ADR for shared play.
- #68 (`hoshi serve`) is still a remote view of one local save, not a shard.
- 0004's hosting estimate still applies, but the clock, store and audit come
  from Chisei instead of a new host.

## Rejected alternatives

- **Keep 0004 unchanged.** Blocks the multi-actor showcase scenarios.
- **A dedicated hoshi game server.** Duplicates clock, store, identity and
  audit that Chisei already owns, and would not exercise Chisei.
- **All of 0004's other rejected alternatives** remain rejected.
