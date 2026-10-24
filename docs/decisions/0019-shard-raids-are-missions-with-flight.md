# ADR 0019: Shard raids are missions with flight

- Status: Accepted
- Date: 2026-10-24
- Amends: [0005](0005-shared-shard-on-chisei.md) raid snapshot timing
- Follows: [0005](0005-shared-shard-on-chisei.md)
- Source: [#388](https://github.com/Sannrox/hoshi/issues/388)

Issue #388 drafted this as 0018; [0018](0018-hoshi-storage-crate.md) was
already allocated to hoshi-storage, so this record is 0019.

## Context

Idle play already launches a fleet, pays fuel, waits on light-time and
flight, and can recall mid-outbound. An instant parked-fleet snapshot
makes inbound rows, pad cargo, and spy-then-raid meaningless.

## Decision

1. A shard raid is a mission: outbound, action, return. Flight time,
   fuel, and speed percent are the current fleet rules.
2. At action time the host catches both empires up and resolves combat
   against that snapshot. No lockstep live combat.
3. Recall is the attacker's. The defender's glance is arm, stance, or
   moving cargo off the pad.
4. `Invade` stays outpost troops. `Launch` stays a generic send.
   Player-versus-player raid is its own intent after this ADR.
5. Parked `ShardSession::raid` is replaced or wrapped so tests and the
   host call the mission path. A duplicate idempotency id still does
   not fight twice.

## Consequences

- [#386](https://github.com/Sannrox/hoshi/issues/386),
  [#372](https://github.com/Sannrox/hoshi/issues/372), and
  [#387](https://github.com/Sannrox/hoshi/issues/387) describe the same
  clock.
- Newbie protection and the attack cap still gate the launch, not a
  later parked snapshot.
- Changing `ShardSession::raid` is a follow-up. This ADR does not
  require that code change to land.

## Rejected alternatives

- Keep parked-fleet `raid()` as v1 and defer flight. Then inbound and
  pad cargo have nothing to attach to.
- Lockstep combat while both clients are connected. Rejected in 0005.
