# ADR 0017: Local TUI may join a Chisei shard

- Status: Accepted
- Date: 2026-10-24
- Partially supersedes: [0005](0005-shared-shard-on-chisei.md) point 1
  (the local binary never a client) and 0005's "not a mode of the local
  binary" sentence; and the rejected alternative “Network client in the
  TUI” in [0007](0007-optional-chisei-backend.md)
- Follows: [0005](0005-shared-shard-on-chisei.md),
  [0007](0007-optional-chisei-backend.md)
- Source: [#382](https://github.com/Sannrox/hoshi/issues/382)

## Context

Shared play is a Chisei-hosted shard. The daily player already uses the
local TUI. Uploading a SQLite save would fail the ADR 0004 threat table.
A second client would split the vision: local pane versus shard pane.

## Decision

1. **Local SQLite play is unchanged.** Continue / New still uses one
   save, one clock, no account, no network. Cheats stay local.
2. **The TUI may join a shard.** Title-screen “Join shard” authenticates
   to the Chisei host and renders the per-identity view. Keystrokes send
   intents with idempotency ids. The host owns clock, RNG, and
   persistence. Client timestamps are ignored. `tick --advance` is not
   offered.
3. **The local save never goes to the shard.** Join does not read, copy,
   or upload `save.sqlite`. A shard session that loses the host does not
   fall back to the last local save. It shows a disconnect.
4. **Network lives outside hoshi-core.** Sockets stay in hoshi-tui or a
   thin client crate. `ShardSession` and the tenant view stay in-process
   library code. `hoshi serve` remains a remote view of one local save.
   `--shard` on `hoshi serve` and a dedicated hoshi game server stay
   rejected (0005 point 2).
5. **Hidden fields never reach the TUI** on a shard session: RNG, pirate
   timers, unrevealed map, another tenant's full fleet and stock. Debug
   dumps of a shard session use the same filter.

## Consequences

- 0005 points 2–4 remain the host rules.
- 0004 condition 1 stays: the shard is a second product (Chisei-hosted).
  The TUI is only a client front end of that product. Join is a
  title-screen client, not a multiplayer toggle on the local save, and
  Continue / New still need no network.
- Implementation of Join is a TUI issue blocked on the tenant view
  ([#379](https://github.com/Sannrox/hoshi/issues/379)) and the Chisei host
  ([#380](https://github.com/Sannrox/hoshi/issues/380)).
- Local play and a shard session must not share a live `GameState`.

## Rejected alternatives

- **Upload the local save.** First edited blob wins; rejected in 0004.
- **A second shard-only binary.** Splits the pane the player already
  uses.
- **`--shard` on `hoshi serve`.** Dedicated hoshi game server; rejected
  in 0005.
- **Sockets in hoshi-core.** Rejected in 0005 and 0007.
