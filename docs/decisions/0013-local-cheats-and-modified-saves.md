# Local cheats and modified saves

Status: Accepted

Source: [Issue #103](https://github.com/Sannrox/hoshi/issues/103)

## Context

Local experimentation needs reproducible state changes without allowing those
changes into competitive or shared play. Deterministic simulation is also used
for legitimate arena runs, so it cannot represent cheated provenance.

## Decision

Core owns a governed cheat intent shared by CLI and TUI. Authorization precedes
parsing. Successful use marks the save modified before simulation runs. A separate
persistent audit records enabling and commands. SQLite saves reject stale writes
that would clear the marker or replace an existing audit prefix. Prestige retains
both. Schema 48 stores this state in the same transaction as gameplay.

Time skips use ordinary catch-up and persist a logical-time offset. Application
boundaries apply that offset once to injected wall time. Server and shard entry
points reject cheat actions and modified states. Achievement awards,
shared career aggregation and arena scoring exclude modified provenance.

## Consequences

The console remains one prompt and the adapters contain no cheat rules. Time
continues after restart without waiting for wall time to catch up. Local auditing
survives bounded gameplay-history retention. Save exports retain provenance.
These rules do not authenticate manually edited databases.

## Rejected alternatives

Reusing the deterministic flag would reject legitimate simulations. Adapter-only
cheats would duplicate rules. Advancing the save timestamp without an offset would
freeze subsequent live progression. Clearing provenance on prestige would allow
cheated progression into clean careers.
