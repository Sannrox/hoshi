# ADR 0015: Fusion lifecycle in core

- Status: Accepted
- Source: [Issue #238](https://github.com/Sannrox/hoshi/issues/238)

## Context

Fission (ADR 0009) gives outer worlds power independent of sunlight, but
deuterium and helium-3 are mined and refined without turning into energy.
Fusion is the dense late-game source. It must follow the same deterministic
simulation and save ownership as fission while staying cheap to catch up.

## Decision

Core owns a `fusion_plant` building with three modes: `dd`, `dhe3` (after
`fusion_dhe3`) and `off`. Fuel burns from the planet's deuterium and helium-3
stock in proportion to delivered energy. Output is capped by the heat sink the
planet's other buildings leave; fusion claims cooling before fission, which
keeps load-following on top of a constant fusion heat load.

Switching on, after building, a mode change, a fuel outage or a maintenance
disruption, starts a plasma start-up: a grid load of a multiple of rated output
that must deliver a fixed energy, while the plant produces nothing. Like
dismantling work, it progresses with the planet's integrated energy factor, so
a short supply stretches it and no supply pauses it; it never runs on credit.

Fusion output and heat are constant between scheduled events: start-up done,
fuel exhausted, leftover heat over. Before a segment's production,
a plant burns only the fuel a full store would clip from it; the rest burns
afterwards. Production then refills the store and consumes its own inputs as
it does second by second. Fuel spent elsewhere before a segment is an outage
even if that segment's production refills the store. The fuel
deadline uses net production, including recipes that consume the same fuel.
Long gaps equal step-by-step ticking without sampling. Leftover heat after the
plasma stops is a flat step for a fixed time.

Schema 49 stores mode, remaining start-up energy, activation, last stop and
leftover heat per planet. A built plant without a row is a fresh D-D plant.
The mode switch goes through the intent layer like `build`.

## Consequences

Dismantling needs the plant off, and the last level of a plant that burned D-D
waits an activation cooldown after the last stop. Maintenance adds neutron
wear scaled by output. Deuterium and helium-3 stay lootable stock; the plant is
never a raid target. Existing saves migrate with no plant state; older
binaries reject the new schema. Numbers live in [balance](../balance.md) and
approximations in [realism](../realism.md).

## Rejected alternatives

- A decay curve for leftover heat would vary fusion heat inside a segment and
  break fission's exact cooling integration for little gameplay gain.
- Drawing only spare supply would vary within a segment while fission recovers
  from a shutdown, so offline and live start-ups would differ.
- Fission-first cooling would make fusion output vary with fission decay heat.
- Per-second fuel events would make offline cost grow with elapsed seconds.
