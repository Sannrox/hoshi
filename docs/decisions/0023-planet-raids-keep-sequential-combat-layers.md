# ADR 0023: Planet raids keep sequential combat layers

- Status: Accepted
- Date: 2026-11-06
- Source: [#470](https://github.com/Sannrox/hoshi/issues/470)

## Context

`resolve_planet_battle` fights a home raid in layers. Space combat always
includes the parked fleet plus orbital stations and hangar fighters.
Batteries fire only after orbit is lost and the shield dome is punched. A
battery-heavy planet can lose every stationed ship in the orbital round,
then win with batteries. That matches
[docs/realism.md](../realism.md) (Combat maths). Player-facing raid copy
listed ship losses next to returned defences, so a win still looked like
the fleet and batteries mixed.

Object types in this sequence: parked fleet, orbital stations, station
hangar fighters, shield dome, batteries, garrison.

## Decision

Keep the current layers (option C).

1. The orbital round is parked fleet, orbital stations, and hangar
   fighters together.
2. The shield dome absorbs a round of fire after orbit is lost.
3. Batteries fire after the dome is punched.
4. Garrison is ground combat after batteries are gone.
5. Player-facing raid copy and the combat report name that parked-fleet
   losses happen in orbit, before batteries fire.

No follow-up implementation Issue. Copy lands with this record. Maths and
the save schema stay as they are.

## Consequences

- Existing saves keep the same raid outcomes. Ships lost in orbit stay
  gone for good; repair crews still return defences.
- A later change that mixes batteries into the orbital round, or that
  withholds the parked fleet from auto-join, needs a new ADR.

## Rejected alternatives

- **A.** Batteries join the orbital round with the parked fleet. Collapses
  the documented sequence and changes who dies on existing saves.
- **B.** Withhold the parked fleet from auto-join so stations, hangar
  fighters, dome, and batteries fight first. Leaves the parked fleet idle
  on auto-raids until a scramble action exists.
