# ADR 0011: Powered dismantling in the build queue

- Status: Accepted
- Source: [Issue #231](https://github.com/Sannrox/hoshi/issues/231)

## Context

Finished building levels need a reversible investment path without making salvage
an instant source of resources or bypassing prerequisites and raids.

## Decision

Core owns one-level dismantling through the existing governed build queue.
Jobs carry an operation and remaining powered work. Physical levels remain until
completion; operational snapshots suppress the building during work. Catch-up
integrates available power and completion boundaries, including maintenance and
reactor changes. Cancellation preserves the level and produces no salvage.

Salvage uses the removed level's configured construction cost and current asset
condition. Deuterium and helium-3 are excluded. Normal stock caps apply; lost
overflow is recorded in history. Existing dependent buildings, queued work,
active research, maintenance and announced home raids block unsafe removal.
Research knowledge remains after removal.

The reactor model stores unused fuel in local stock, with no separate loaded
core. Starting dismantling shuts the reactor down atomically and leaves that fuel
in stock. Persistent decay heat remains. The remaining reactor levels must have
enough spent-fuel capacity, including an empty pool before final retirement.

Schema 47 adds `operation` and `work_left` to `build_queue`; the existing `kind`
column continues to identify the building. Old rows become ordinary builds.
All surfaces submit the same core intent; no interface writes queue rows directly.

## Consequences

Power starvation pauses work across restarts. A waiting head rechecks blockers
before starting. Maintenance cannot start on the same active dismantling asset.
Older binaries reject schema 47. Optional backends persist the same state model.
The current shard has snapshot raids; future announced attack models must extend
the central blocker before allowing dismantling during their warning window.

## Rejected alternatives

- A second queue would add independent capacity and duplicate cancellation rules.
- Wall-clock-only completion would allow dismantling without available power.
- Returning fuel as salvage would duplicate stock that has never left storage.
