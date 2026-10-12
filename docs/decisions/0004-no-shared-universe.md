# ADR 0004: No shared universe

- Status: Partially superseded by [0005](0005-shared-shard-on-chisei.md)
- Date: 2026-10-12
- Follows: [0002](0002-terminal-idle-game-with-event-bridge.md)
- Source: [#18](https://github.com/Sannrox/hoshi/issues/18)

## Context

ADR 0002 ships a local idle game: injectable clock, deterministic catch-up,
SQLite save, optional event bridge. It left a shared universe as later
research. Issue #18 asks whether players should share a galaxy, who owns
timers and persistence, how cheating is stopped, and what hosting costs.

hoshi is played in a spare terminal pane. Progress happens on wall-clock
timers while the process is closed. The local save is the source of truth.
`tick --advance` exists only for deterministic harnesses. Core already
exposes the same actions to the TUI and the CLI.

A shared universe would invert that trust model. Comparable idle and
space-empire designs that share a world put the clock and the save on a
server the client cannot rewrite. Designs that keep the client as authority
fail as soon as two players interact. Live lockstep and high-rate dedicated
servers fit simultaneous action games; they do not fit catch-up idle play.
Asynchronous snapshot raids (resolve against a frozen view of the target)
fit idle play if, and only if, a server owns that snapshot.

Issue #68 (`hoshi serve`) is a local or self-hosted action API around one
save. It is not a shared universe.

## Decision

**Do not build a shared-universe server.** hoshi stays a local single-player
idle game. One player, one save, one clock. No account, no always-on host, no
player-vs-player raids.

Revisit only if all of these are true:

1. A maintainer wants a second product, not a mode of this one.
2. The server owns the wall clock, the RNG, and persistence. Clients send
   intents through the same core actions as the CLI and never write the live
   save.
3. Offline progression is `GameState::catch_up` on the server at action time
   and at interaction time (for example a raid), not a per-second loop of
   every empire.
4. Cross-player combat is asynchronous against a snapshot taken after
   catch-up, not lockstep live combat.
5. Galaxies are shards (one seed, a bounded player list), not one seamless
   universe.

Until then, #19 (player-vs-player raids) and #20 (authentication and
anti-cheat) have no shared-universe substrate. A local `hoshi serve` (#68)
may still wrap one save for remote clients of the same player.

## Threat model (why the local save cannot be shared)

| Threat | Local game | Shared universe |
| --- | --- | --- |
| Edit the SQLite save | Player's own run; no victim | Free resources, ships, research |
| Skew or freeze the clock | Catch-up uses the host clock; only the owner is fooled | Speed-hack production and flight |
| `tick --advance` | Refused on live saves; harness-only | Instant catch-up cheat if the client owns time |
| Hidden fields in `hoshi state` | Fair-play rule for bots | Intel leak if the API returns RNG, pirate timers, or unrevealed map |
| Duplicate a request after disconnect | Same save, same owner | Double-credit unless intents are idempotent on the server |
| Two sessions on one account | One lock per save | Last-write-wins or duplicated fleets |
| Raid a player who is offline | Not applicable | Griefing unless the target is a snapshot after honest catch-up |
| Forge bridge activity | Optional, capped, off by default | Uncapped payouts if bridge events are trusted in a shared score |
| Multi-accounting | Irrelevant | Farm alts, then raid the main |

Mitigations that work only if the server is authority: intent-only messages,
idempotent action ids, server clock, server RNG, snapshot raids, rate limits,
and never returning hidden state. They do not work if the client uploads a
save blob.

## Hosting and maintenance (if the decision is reversed)

Deferred cost today: **zero**. No process, no operator, no on-call.

A later shard of about 100 players, on-demand catch-up, one small always-on
host, and a server-owned store (SQLite until write contention, then a
single-node SQL database):

- Host: on the order of a few dollars per month for a toy shard; more if the
  operator wants backups, a second region, or TLS termination they do not
  already run.
- Work: catch-up on action and on raid, not a 1 Hz tick of every empire.
  Schema migrations ship with the binary, same as local saves, plus a
  backup-and-restore drill per bump.
- People: one maintainer for migrations and abuse reports. Continuous
  anti-cheat staffing is not justified at this scale.
- Failure mode: if catch-up of a late-game empire exceeds a second, raid
  resolution queues and the shard feels stalled. That is a capacity limit,
  not a reason to run a dedicated simulation tick.

A seamless always-on universe with live fleets for thousands of players is
out of scope even in a reversal: it needs sharding, a replication layer, and
ops this project does not have.

## Consequences

- Core, the TUI, and the CLI keep a local save and an injected clock.
- No new crate, protocol, or auth surface for a shared world.
- Issue #18 is resolved as "do not build it." Follow-up shared-universe
  Issues stay design-blocked until this ADR is superseded.
- `#68` remains the path for a remote view of **one** local save.

## Rejected alternatives

- **Ship a shared universe now.** Inverts ADR 0002, needs auth, hosting, and
  anti-cheat before the local game is finished.
- **Client-authoritative sync (upload the save, merge on the server).** The
  first edited save wins. Unusable for raids or markets.
- **Peer lockstep.** Every client must be online and simulate together.
  Idle catch-up does not fit.
- **Listen server (one player's machine is the authority).** The host still
  owns the clock and the save; other players trust a cheatable client.
- **Always-on per-empire tick loop.** Cost grows with player count even when
  nobody is looking. On-demand `catch_up` is the model hoshi already has.
