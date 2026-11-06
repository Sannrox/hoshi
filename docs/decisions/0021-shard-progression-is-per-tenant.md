# ADR 0021: Shard progression is per tenant

- Status: Accepted; blitz finish line is
  [0022](0022-blitz-shards-run-on-a-scaled-clock-and-end-with-a-winner.md)
- Date: 2026-11-04
- Follows: [0005](0005-shared-shard-on-chisei.md), [0020](0020-shard-identity-is-a-host-tenant.md)
- Source: [#390](https://github.com/Sannrox/hoshi/issues/390)

## Context

Local play has a story arc with flags (#361), daily slots, a declared
victory focus (#363), and a prestige reset into a new galaxy. A shard is one
seed and a capped tenant list (ADR 0005), each tenant one `GameState`
(ADR 0020). Nothing recorded whether these four systems are per tenant,
shared on the seed, or local only.

## Decision

1. **Story is per tenant.** Each tenant walks the authored arcs with its own
   story flags. No galaxy-wide flags; one player cannot close another's arc.
2. **Daily slots are per tenant.** Picks come from the tenant's own state and
   in-game day, as in local play.
3. **Victory is recorded per tenant and the shard stays open.** A tenant that
   completes a victory path records it and keeps playing; no path closes the
   shard for the others.
4. **Prestige reset is local only.** On a shard, starting over means leaving
   and joining a new seed, not a second empire on the same one. The shard
   intent set has no prestige reset.

## Consequences

- All four already hold: the state lives in each tenant's `GameState`, and
  `Intent` has no prestige, daily, or victory variant, so `ShardSession::submit`
  cannot reach them. No host-owned story or victory object is needed.
- A shard-wide finish line is
  [0022](0022-blitz-shards-run-on-a-scaled-clock-and-end-with-a-winner.md)
  (opt-in at creation; ordinary shards still stay open). A future shared
  arc or shard prestige still needs a new ADR.
- #370 (whether the next local galaxy remembers the closed arc) stays a local
  question.

## Rejected alternatives

- Galaxy-wide story flags: one player could close everyone's arc, and the
  host would own a new object and schema.
- Host-day daily picks shared by every tenant: adds a host object for no
  gameplay gain in a bounded shard.
- Closing the shard on the first victory: ends play for everyone else.
- Prestige on the shard: a fresh empire would reset beside established ones
  and keep reopening its own newbie protection window.
