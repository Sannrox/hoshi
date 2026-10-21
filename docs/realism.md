# Realism guidelines

These rules constrain **physics and maths**: production, energy, heat, travel,
orbits, combat numbers and the economy. They do not constrain flavour.
Species, factions, story, characters and technology names stay fictional.

A species with unusual traits is fine. Its mines, drives and energy budget
still obey the sections below.

New design work and balance changes follow this file. Shape Issues against it.
Record a departure in the [simplifications](#simplifications) list, with the
gameplay reason.

## Domain model

hoshi is a fictional game. It is also a place to show a clear domain model:
named object types, explicit links, moves that go through one governed path,
and outcomes you can audit.

When adding a feature, name:

| Piece | Meaning |
| --- | --- |
| Object types | The nouns (planet, fleet, faction, outpost, route, facility, good, law, proposal, approver, jurisdiction, measurement, observation, source, fact, confidence, leader, role, assignment, permission, delegation, marking, compartment, clearance, sharing agreement, region, warehouse, order, contract, trader, shipment, goal, tier, frontier world, treasury, budget, cost center, expense, forecast, approval, kpi, target, report, incident, risk, resolution, review, follow-up, asset, condition, maintenance plan, work order, telemetry reading, failure prediction). |
| Links | How those nouns point at each other (fleet at coord, route between owned planets, standing on a faction id, a proposal that amends a law in a jurisdiction, a leader holding a role through an assignment, a role granting permissions, a delegation with limits, owned by a leader, approved by approvers, a marking on a fact/plan/report/law, a clearance that includes compartments, a sharing agreement with an allied faction, an order placed in a region, a contract that binds a trader, a shipment that fulfils a contract, a tier that requires sustained good rates, a wonder stage that requires a tier, an expense charged to a cost center under a budget and approved by a role). |
| Actions | Propose, check, apply. The TUI and CLI call the same `hoshi-core` function. They do not write the save. `AssignLeader`, `Delegate` and `RevokeDelegation` are the staff path. `ProposeLaw`, `ApproveLaw`, `EnactLaw` and `RepealLaw` are the law path: owner, approvals, effective date, audit trail. `PlaceOrder`, `AcceptContract`, `FulfilContract` and `ShipGoods` are the regional market path. `SetBudget`, `RequestSpend` and `ApproveSpend` are the finance path: a spend over a budget waits, then applies through the same build/research/ship actions. `SetTarget` and `GenerateReport` are the KPI path: a colony or role holds a goal, catch-up archives measurements with source events. `OpenIncident`, `AssignIncident`, `ResolveIncident` and `ReviewIncident` are the incident path: an event opens a row, a leader owns it, a review archives lineage. `ScheduleMaintenance` and `CompleteWorkOrder` are the maintenance path: condition wears with use, age and heat; work orders cost parts and downtime; telemetry is noisy; predicted failure uses a bathtub hourly chance. Every action a leader takes is checked against the role and recorded. |
| Outcomes | A typed result plus a log line (`GameEvent`) with a timestamp. Catch-up and live play share that path. Measured effects attach to the law as evidence. |

State changes go through core actions. Offline catch-up replays the same
actions on an injected clock. The save stores the resulting state, not a
second copy of the rules. Lineage for a change is the event log plus the
save schema version.

## Materials

Use matter that can exist. Current chain goods already lean this way: iron
ore, silicon, rare earths, water ice, helium-3, then steel, purified silicon,
magnets, propellant, fuel, frames and electronics.

Stockpiles that buildings and ships spend are iron, silicon, rare earths,
water ice, helium-3, and deuterium fuel. Chain goods (ore, steel, purified
silicon) still sit in the per-planet map; the last construction step credits
the matching stockpile. Do not add a resource whose origin is unexplained.

Deuterium stays as fuel: it is already-enriched tankage. Natural D/H in water
ice is about 156 ppm; the synthesizer recipes pay energy and heat for that
enrichment. Migrated saves keep old deuterium as deuterium, not as ice.

Plausible sources:

- metals and silicon from rocky crusts
- ices and volatiles from cold worlds and comets
- helium-3 from giant-planet atmospheres or solar wind on airless regolith
- deuterium from water ice (low natural fraction; enrichment costs energy)

Mass is conserved in process recipes. Extractors need a deposit. See
[docs/balance.md](balance.md) production chains.

## Energy

Every process has an energy cost. Output is limited by the energy that is
actually on the planet at that instant (the existing energy factor already
does this).

Sources, with tradeoffs:

| Source | Sketch | Tradeoff |
| --- | --- | --- |
| Solar | Flux falls as `1/r²`. At 1 AU, ~1.36 kW/m² before atmosphere and angle. | Cheap at inner orbits; poor on ice worlds and gas giants. |
| Fission | Dense, independent of sunlight. | Fuel and waste; finite stock unless bred. |
| Fusion | High output; deuterium is common in ice, helium-3 is not. | Hard to light; helium-3 wants giant-planet or regolith infrastructure. |

Solar follows the inverse-square orbital flux. Fission rejects twice its
electrical output as heat and throttles to available cooling. Uranium enrichment
loses most ore mass from the usable fuel stream; burned fuel becomes equal mass
of spent fuel in an on-site pool. Protective shutdowns leave declining decay
heat, initially 7% of rated thermal power and about 1% after a day. These are
compressed engineering approximations, with no explosions or area damage.

## Propulsion and travel time

Transit time comes from distance and the ship's drive, not from a narrative
timer. Drives in increasing exhaust velocity, decreasing thrust:

1. chemical
2. nuclear thermal
3. electric / ion
4. fusion

High thrust shortens burn time and fights gravity wells. High exhaust
velocity cuts propellant mass (`delta-v = ve * ln(m0/mf)`). Ships pick a
point on that tradeoff. Fuel paid at launch should scale with delta-v and
wet mass, not only with a flat distance band.

## Orbital mechanics

Keep orbits simple enough to tick:

- planets sit on fixed circular (or Keplerian) slots in a system
- a transfer uses a Hohmann-like estimate: two burns, coast, arrive
- no n-body integration
- climbing out of a gravity well costs extra delta-v and fuel

Reference: a Hohmann transfer between circular orbits at 1 AU and 1.5 AU is
on the order of 8–9 months of coast. Game times may compress (see
simplifications) but the **ordering** stays: inner hops are shorter than
cross-galaxy hops, and a heavier ship burns more fuel for the same delta-v.

Current `flight` units (`galaxy_unit`, `system_unit`, `position_unit`) are
abstract lengths used by existing fleet send. PlanMission treats map slots as
scaled astronomical units in `[orbits]` (`inner_au`, `au_per_slot`,
`system_au_*`, `galaxy_au` times `au_metres`). Bundled slot 1 of system 1 is
1 AU; slot 6 is 1.5 AU. Fuel for those missions is
`delta-v = ve * ln(m0/mf)` with payload mass in the wet stack.

## Heat, waste and pollution

Industry produces waste heat and byproducts. Heat leaves only through
radiators (or an atmosphere that can dump it). Radiated power is `σ A T⁴`
(`σ ≈ 5.67e-8 W/m²/K⁴`). Hotter radiators are smaller and more visible.
Byproducts have mass: slag, effluent and emissions go into storage, recycling
or the environment. They are not deleted.

Unmanaged heat and waste hurt habitability and make a world easier to
spot. Mitigation (radiators, scrubbers, thermal and ecology research)
costs energy and materials. Load versus sink is instantaneous: nothing is
stockpiled, and overflow drops as soon as capacity catches up.

## Intel

A probe is a ship. It flies on the same distance and drive rules as any other
mission. What it learns is a score from how many probes arrived and the
espionage research gap, not a narrative reveal. Detection and destruction are
seeded chances from config. The snapshot is taken on site; it becomes visible
at home after light-time `distance / c` along the difference of heliocentric
radii. Measurement error is `error_base + error_per_au * range_au / quality`.
A detected wave can be fed a biased decoy. Stored reports age; later attacks
read those snapshots, not live omniscience. Two reports of the same field both
stay visible.

## Combat maths

Weapon names may be fictional. The numbers should still be physical:

- kinetic: energy scales with `½ m v²`; more mass or more closing speed hits
  harder
- directed energy: limited by source power, aperture and waste heat in the
  firing ship
- armour is mass that must be accelerated; shields are energy that must be
  replenished
- fire rate is limited by heat and reload mass, not by an arbitrary round cap
  alone

The seeded round resolver can stay. New stats should map onto those
quantities so a heavier hull is slower and a hotter gun needs more radiator
or a longer wait. Planet layers (orbital stations, a dome that absorbs a
round of fire, then batteries, then garrison) are still the same hull and
shield points in sequence, not a separate physics model.

## Economics maths

Prices live in abstract units today. Formulas should still be explainable:

- a mid price is a scarcity signal, not a random score
- a trade of A for B converts at `price_A / price_B`
- fee is a two-sided spread
- slippage grows with volume relative to a market depth (`impact_unit`)
- transport adds time and fuel; it does not teleport mass

Mean reversion and shocks are allowed if they have a stated cause (stockpile
news, story events). They should not print mass from nothing.

## Simplifications

Each row is an allowed departure from physics. New features do not add a
silent extra departure; they extend this list.

| Simplification | Gameplay reason |
| --- | --- |
| Explicit local cheats in permanently modified saves | Testing and experimentation can grant material or suppress damage; these saves are excluded from fair play. |
| Compressed clocks (upgrades in minutes, not months) | The game is meant to run in a spare terminal while other work happens. |
| Player commands apply as soon as they are legal | The injected clock is the authority for own actions; remote facts still wait for light-time. |
| Light delay uses the absolute difference of heliocentric radii | Same radius table as PlanMission; no 3D chord or occlusion. |
| Passive rival sensors stamp light delay on a noisy present-time glance | Reconstructing the past light cone would store a second empire history. |
| No n-body gravity, no eclipses | Catch-up must stay deterministic and fast. |
| Abstract flight units and ship `speed` points | Existing fleets already fly; a documented scale can replace the points later. |
| Circular coplanar orbits, mean anomaly 0 at unix epoch | Catch-up stays a closed-form Hohmann estimate, not an integrator. |
| Synodic windows and coast times share `coast_compress` | Real Hohmann months become minutes; inner hops stay shorter than outer hops. |
| High-energy is a Hohmann scale factor, not a Lambert solver | One extra burn multiplier; still two burns and a coast. Windows use the shortened coast with the same 180° lead. |
| Gravity assist is a delta-v discount when a body sits between the two radii | No patched-conic flyby; geometry is radius-only. |
| Combined vehicle uses the lowest exhaust velocity among its ships | One stack, one rocket equation. |
| Gravity-well burns are scaled below LEO so chemical ships can leave | Full well escape would demand unplayable mass ratios on the idle clock. |
| Hull points map to kilograms through `kg_per_hull` | Keeps a combustion cargo inside a mid-game deuterium stockpile for a 1–1.5 AU hop. |
| Propellant mass is paid from deuterium; it does not fill cargo volume | Tanks are part of the vehicle; cargo is payload. |
| Inter-system and galaxy hops sit on summed AU radii around the same star | Keeps one μ and one Hohmann formula; ordering of hop size stays. |
| Frontier galaxy uses the same `[flight].galaxy_unit` distance as a one-galaxy hop | Inter-galaxy flights stay on the existing clock; richer worlds are a generation overlay, not a second physics table. |
| Automation control energy is a flat draw per unlocked route or enabled rule | Models computers and radios without a second power grid; routes still pay transit time. |
| NPC hub and home books start with a finite guild warehouse | Depth for PlaceOrder without minting player stockpiles; the warehouse does not restock from nothing. |
| Regional shipments bill fuel and transit from a cargo-capacity flight but leave the parked fleet in place | They model contracted haulers rather than a second fleet object. Catch-up stays event-driven. |
| Construction stockpiles are iron / silicon / rare earths / water ice / helium-3 / deuterium | Names match deposits and chain goods; deuterium is enriched fuel, not a third mystery column. |
| Credits value a spend at the live market mid plus a flat energy_credit; interest is simple per period on negative cash | Budgeting needs one unit of account. Compound curves and inflation wait for reports (#116). |
| Round-based combat with rapid-fire tables | Seeded, short, offline-safe fights. Map new stats onto energy and mass rather than replacing the resolver. |
| Formation, stance and role modifiers | Light tactics scale the existing resolver; Neutral is identity. Battles never wait for input. |
| Catch-up splits at storage-full and input-empty instants, ceiled to whole seconds | Offline gaps stay event-driven and sub-second; `add_capped` still stops exactly at the cap. |
| History time-travel is a recorded glance (stockpile, energy, heat, mass), not a full replay | The log stays bounded (256 events); reconstructing the whole save would duplicate catch-up. |
| Instantaneous heat and pollution (no stored slag or thermal inertia) | Catch-up stays a single pass; overflow is a work-speed and raid-signature modifier. |
| FTL, if any | Only through late-game gates or a listed wonder. Default travel is sublight on the flight clock. |
| Moons from battle debris | A moon is an orbital facility of its parent planet, not a new galaxy slot. Formation is a seeded roll on hull lost so fights stay fast and deterministic. |
| Story, species and faction flavour | Out of scope for realism; they must not mint mass or energy. |

## Known gaps in current config

These live values disagree with the rules above. They are follow-up work, not
silent exceptions. Do not "fix" them in an unrelated PR.

- `[flight]` lengths and `ships.toml` `speed` values are unitless points
  (`speed = 5000`–`12500`), not metres per second or a documented scale.
- Fuel is a flat `fuel_distance` band plus a per-ship constant, not
  `delta-v * mass`.
- Combat `attack` / `shield` / `hull` are points. They do not yet cite
  joules, kilograms or radiator load.
- Empty `[chains]` recipes still allow the legacy mine-to-stockpile path
  (mass from a building level with no deposit). Bundled config has recipes;
  overrides that clear them regress.

When a follow-up Issue closes one of these, remove the bullet here.

## Checking a design Issue

1. Physics and maths: does every new rate, cost, or timer have a quantity
   (mass, energy, time, distance) and a config key?
2. Flavour: are species, factions and story still free to be fictional?
3. Domain: are object types, links and the core action named? Does catch-up
   use that action?
4. Simplification: if the numbers skip physics, is the skip in the table
   above?
5. Gaps: if the work touches a known gap, it either closes it or leaves it
   listed.

`docs/balance.md` stays the operator manual for the live file. This page is
the design constraint on what that file may contain.
