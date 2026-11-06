# ADR 0022: Blitz shards run on a scaled clock and end with a winner

- Status: Accepted
- Date: 2026-11-06
- Follows: [0005](0005-shared-shard-on-chisei.md),
  [0017](0017-local-tui-may-join-a-chisei-shard.md),
  [0021](0021-shard-progression-is-per-tenant.md)
- Source: [#464](https://github.com/Sannrox/hoshi/issues/464)

Issue #464 drafted this as 0020; [0020](0020-shard-identity-is-a-host-tenant.md)
and [0021](0021-shard-progression-is-per-tenant.md) were already allocated,
so this record is 0022.

## Context

Shard pacing is the idle game's wall-clock pacing. An evening session
among a handful of players on one host never reaches the good part and
never concludes. Per-empire victory exists (conquest, wonder,
diplomacy) but races NPCs; no rule ends a shard or names a winner.
[0021](0021-shard-progression-is-per-tenant.md) records that on an
ordinary shard, victory is per tenant and the shard stays open. A
future shard-wide victory needed a new ADR.

## Decision

1. A shard may be created as a **blitz shard**. Its host clock is
   `game_now = epoch + factor x (real_now - epoch)`. `epoch` and
   `factor` are fixed at creation and are part of the shard's
   identity. A live shard's factor never changes; catch-up determinism
   and idempotent replay depend on it.
2. Game rules are unchanged and read the scaled clock. Gameplay
   windows (newbie protection, the attack cap window) follow the
   scaled clock. The command rate limit is host protection and stays
   on real time.
3. A blitz shard has a **finish line**. The first tenant whose empire
   records a victory (any existing path) wins the shard immediately.
   Otherwise, at the deadline fixed at creation, the highest empire
   score wins (arena scoring, version pinned at creation). An ended
   shard refuses gameplay intents; reads still work and show the
   final standing.
4. **Scoreboard visibility** is a creation option: `visible` puts a
   ranked scoreboard in every tenant view; `hidden` keeps scores on
   the host until the shard ends. Hidden is read policy, not a client
   filter. A race victory announces immediately in both modes.
5. Prestige is off on a blitz shard (season overlay machinery).
6. Capturing another player's planets stays out of scope. Raids
   plunder ([0019](0019-shard-raids-are-missions-with-flight.md)); the
   deadline guarantees a winner without conquest of players.
7. Ordinary shards (no finish line, factor 1) still follow 0021:
   victory is recorded per tenant and the shard stays open.

## Consequences

- The scaled clock and the finish line are independent features; a
  blitz shard is their configuration. Either lands alone: a scaled
  shard with no winner is a fast casual shard, and a finish line at
  factor 1 is a season shard that ends on a date. Follow-ups are
  #465 (scaled clock) and #466 (finish line and scoreboard).
- The tenant view carries `(epoch, factor)` so clients render
  countdowns in wall time.
- Local single-save play is unchanged. 0005 points 2-4 and the 0004
  threat table stay in force; a blitz shard is still Chisei-hosted.

## Rejected alternatives

- Re-tune every balance number for speed. Large surface, drifts from
  the live game; the clock scales all seconds-based rules at once.
- A separate fast game server. Rejected in 0005.
- A mutable speed factor on a live shard. Breaks idempotent replay
  and makes two hosts disagree about elapsed time.
- Ending every shard on first victory. Rejected in 0021 for ordinary
  shards; this ADR is opt-in at creation.
