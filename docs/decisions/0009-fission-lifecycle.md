# ADR 0009: Fuelled fission lifecycle in core

- Status: Accepted
- Source: [Issue #226](https://github.com/Sannrox/hoshi/issues/226)

## Context

Outer colonies need power independent of sunlight. Fuel, cooling and maintenance
must obey the same deterministic simulation and save ownership as other rules.

## Decision

Core owns reactor output, the uranium fuel chain, proportional burnup and on-site
spent-fuel capacity. Fabrication belongs to the reactor complex and uses the
local metal mine processing scale. Its electrical draw starts only when the
complex exists. Building specifications can require research levels.

The reactor caps output at available cooling. Catch-up integrates fabrication
and burnup together under fuel starvation, rather than scheduling each small
fuel delivery as a world event. Shutdowns from failure, servicing or exhausted
fuel/pool capacity retain a timestamp and rated thermal power per planet.
Schema 46 stores those values transactionally alongside the existing generic
building, goods and research rows. Decay heat survives restart and repair.

## Consequences

The TUI, production reports and simulation consume core's power calculation.
Existing saves migrate with no reactor or shutdown state. Older binaries reject
the new schema. Balance parameters and the pacing baseline are documented in
[balance](../balance.md); physical approximations are in [realism](../realism.md).
Spent fuel remains outside resource-only raid loot. Breeding and reprocessing
are separate work.

## Rejected alternatives

- A new top-level resource field would duplicate the intermediate-goods store.
- Enrichment draw on every mine would burden worlds before they use fission.
- Process-local shutdown timers would lose heat on restart.
- Per-second fuel events would make offline cost grow with elapsed seconds.
