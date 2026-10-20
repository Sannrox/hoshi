# Balance config and pacing

Design constraints for these numbers (energy, mass, travel, heat) live in
[docs/realism.md](realism.md). This page is the operator manual for the live file.

Production, construction, raids, bridge-effect sizes, expedition weights, flight,
starting stockpiles, and staged wonders live in `crates/hoshi-core/data/balance.toml`.
The file is embedded and validated on `BalanceConfig::default()`. Research stays in
`research.toml`; ships stay in `ships.toml`; story events stay in `story.toml`;
planet defences stay in `defences.toml`; laws stay in `laws.toml`; daily goals
stay in `missions.toml`; light tactical combat stays in `tactics.toml`.
Wonder ids must be unique; each wonder
needs at least one stage with `duration_secs > 0` and a positive resource cost.

Override the bundled file at runtime:

```sh
hoshi --balance path/to/balance.toml
```

`HOSHI_BALANCE` is used when `--balance` is omitted, including `hoshi simulate`. The override is a full file
(same keys as the bundled one), not a partial patch. Version must be `1`. Growth
factors must be `> 0`, bases `>= 0`, build and ship queues at least 1.

## Production chains

`[chains]` is the recipe graph. Empty recipe lists keep the legacy mine-to-stockpile path. With recipes, each mine extracts declared deposits and then processes goods in listed order; list every producer before the recipes that consume its outputs. Extractors need a deposit (no mass from nothing). Process recipes must not create mass. Every non-raw input has to be produced by some recipe. Cycles that feed an output back into its own inputs are rejected. Duplicate input or output names that alias the same construction stockpile (`frames`/`metal`, `electronics`/`crystal`, `deuterium`) are rejected.

Construction stockpiles are iron, silicon, rare earths, water ice, helium-3 and deuterium (also named frames, electronics and fuel). Three tanks hold them: metal storage for iron and rare earths, crystal storage for silicon, deuterium storage for water ice, helium-3 and fuel. Intermediate goods live in the save `goods` table. Building costs may use `frames`, `electronics` and `fuel`; ship costs may use `frames`, `electronics` and `propellant` (`fuel` on a ship is still flight consumption).

`[chains.deposits.<type>]` is richness by planet type. `deposit_jitter` (in `[0, 1)`) spreads that richness from the galaxy seed and coord. The planet screen lists the resulting deposits.

## Waste heat and pollution

`[waste]` is an instantaneous load versus sink. Mine energy times
`heat_per_energy` / `pollution_per_energy` plus each recipe's `heat` and
`pollution` (scaled by building level) is the load. Planet type sets the
base sink (ice worlds radiate heat well and hold dust; gas giants dump both
into a thick atmosphere). Each radiator and scrubber level adds a linear
sink and draws energy. Research `heat_sink` and `pollution_sink` scale the
total sink.

When load/sink reaches `warn_ratio` the TUI shows WARN. Above 1.0, work
speed is `1 / (1 + overflow * unrest_per_overflow)` and pirate fleets gain
`floor((heat_overflow + pollution_overflow) * raid_signature_per_overflow)`
ships. Starter mines stay under the rocky sinks; heavy industry needs
radiators and scrubbers. `hoshi simulate` prints that snapshot.

## Laws

`crates/hoshi-core/data/laws.toml` is the catalog. Each template has a parameter
range, a jurisdiction (`empire` or `planet`), optional `phase_in_secs`, mechanical
`effects` and declared `expected` metrics. An emission cap throttles industry
when pollution load would exceed `ratio * sink`. A trade tariff adds to the
market fee. Research priority multiplies research speed. Labour rules add to
mine output modifiers. A metal levy drains metal per hour. None of these
change physics constants; they change incentives and limits. `measure_secs`
is how often expected vs actual is sampled.

## Pacing targets

`[pacing]` is checked by `hoshi simulate`:

| Key | Meaning |
| --- | --- |
| `first_upgrade_secs` | First building upgrade must complete within this many seconds. |
| `min_metal_mine_per_day` | Metal mine levels gained per simulated day under the greedy strategy. |
| `max_bridge_metal_ratio` | `metal` after maxed PR-merge pulses divided by `metal` with activity off. |
| `keystone_secs` | Each playable branch keystone must complete within this many seconds under a research-greedy run. |

Defaults: first upgrade in 5 minutes, at least one metal mine level per day,
bridge pulses must not multiply stored metal by more than 8, and each branch
keystone within 14 days.

## Tactics

`crates/hoshi-core/data/tactics.toml` is the light combat layer on the fleet resolver. Neutral formation, stance and open terrain are identity: the same seed matches the plain resolver. Wedge, wall and screen scale attack and shields; aggressive, defensive and hit-and-run do the same plus accuracy. Terrain can cut accuracy (nebula), reduce damage to fighters (asteroid) or make retreat fail (gravity well). Every ship and defence has a fighter/bomber/capital role for rock-paper-scissors, applied only when a side leaves Neutral. Auto-retreat and reserve commits apply while the player is away. Admiral traits (`none`, `flanker`, `stalwart`) spend XP up to `xp_trait_cap`. `hoshi simulate` fails if one formation and stance pair wins every sample.

## Leaders

`crates/hoshi-core/data/leaders.toml` is the staff catalog. Traits scale combat and colony scheduling inside the energy budget (`production` stays near 1.0). `empty_governor` is the output drop with no ready governor. `reassign_secs` is the wait before a moved leader can act. `lifespan_secs` is a flat career length, not a demographic model. Recruit costs are paid from the home stockpile. Admirals still use `tactics.toml` XP and trait caps for combat.

## Espionage

`[espionage]` scores a probe wave as `probes * probe_weight + (own_espionage - target_level) * level_weight`. Own espionage is the existing research/faction modifier. The report tier is the highest `[[espionage.tiers]]` whose `min_score` the score still meets (activity, resources, fleet, defences, garrison). Detection chance is `detect_base + probes * detect_per_probe - (own - target) * detect_per_level`, clamped to `[0, 1]`. A detection then rolls `destroy_chance` once per probe and applies `decoy_bias` to counts. Measurement error is `error_base + error_per_au * range_au / quality`; light delay is radial range over `c`. Confidence starts from that error and spends `age_penalty_per_hour`. The TUI Intel screen lists age and confidence; Enter opens every arrived report for that target; `s` sends one probe. `hoshi intel` / `hoshi intel provenance` print the same facts.

## Outposts

`[outposts]` places pirate worlds from the galaxy seed. `min_ships` and
`min_garrison` are the early-game floor; `strength_factor` and `garrison_factor`
scale later rebuilds with the player's fighter-equivalent fleet. `rebuild_secs` is the
catch-up interval. `variance` is the ground-combat spread (`0` is an exact
comparison). `troop_strength` is the landing power of one troop transport.
`[[outposts.rare]]` rows are the rare-planet bonuses shown on the planet screen.
`shipyard_speed` speeds the empire shipyard (ships queue at home).

## Orbits

`[orbits]` is PlanMission only. `mu_star` and `au_metres` are SI. Slot 1 of system 1 sits at `inner_au` (1 AU in the bundle); each later slot adds `au_per_slot` (slot 6 is 1.5 AU). Later systems add `system_au_base` then `system_au_step`; later galaxies add `galaxy_au`, so a cross-galaxy hop is larger than an in-system hop. `coast_compress` is game seconds per real coast second (synodic waits use the same factor). `kg_per_hull` / `kg_per_cargo` turn ship hull and cargo units into kilograms for the rocket equation; `m3_per_cargo` is payload volume. Exhaust velocity is per drive (`combustion_ve`, `impulse_ve`, `hyperspace_ve`). Gravity-well burns are per planet type. `high_energy_dv` / `high_energy_time` scale Hohmann; `assist_dv` discounts it when a body sits between the two radii. `window_half_secs` and `window_count` size the listed launch windows; `turnaround_secs` books the vehicle after arrival. Existing `fleet send` still uses `[flight]`.

## Daily goals

`[daily]` sets the in-game day length (`day_secs`), an optional reset offset, how many slots to pick (`slots`), the streak cap, and the per-streak resource bonus. The catalog lives in `crates/hoshi-core/data/missions.toml` (upgrade a mine, finish research, hold a raid, trade, launch an expedition). Picks are deterministic from the galaxy seed and the in-game day. Progress counts during offline catch-up. Completing the full set raises the streak; a missed day resets it. Rewards stop at storage caps. GitHub and agent activity never count.

## Moons

`[moons]` is debris-to-moon formation around an owned planet. Hull lost in a fight adds `debris_per_hull` to that coord. Once debris passes `form_threshold`, each extra point rolls `form_chance_per_debris` (capped) against a seeded RNG; the same seed repeats the same moon. A moon is an orbital facility of its parent, not a new map slot. `phalanx_range_systems` is the sensor radius in systems; `jump_cooldown_secs` is the gate cooldown. Jump is instant between two moons that both have a jump gate, and is refused if the destination has none. Costs are `phalanx_*` and `gate_*`. Set `enabled = false` to skip formation.

## Frontier galaxy

`[frontier]` is a second galaxy with richer worlds. `galaxy` must be `>= 2`. Research (`drive_id` + `astro_id` at the listed levels) opens `research_sectors`; completing any wonder opens `wonder_sectors`. Those two must cover `sectors`. Flights into that galaxy need `flight_drive_id`. Worlds roll a higher `rare_chance`, extra `fields_bonus`, and `production_bonus` on mines. Bridge `release_published` / `epic_closed` events add `release_sectors` / `epic_sectors` early, capped by `bridge_sector_cap`, and never exceed the catalog. `hoshi simulate` (when days cover the casual floor) reports the research unlock against `casual_max_secs`.

## Goal ladder

Tiers live in `crates/hoshi-core/data/goals.toml`. Each `[[tier]]` row names goods per hour (and optional energy supply or repeating routes) that must be running at once. Completing a tier grants a small stockpile reward and unlocks the next; `hold_secs` delays completion while the rates are held. Wonder stages may set `require_tier` so a megaproject cannot be paid from a lump sum. `hoshi simulate` (when days cover the casual floor) reports each tier against `casual_max_secs`.

## Victory

`[victory]` is the galaxy end. Conquest needs the pirate homeworld colonised and its outpost gone. Wonder needs `wonder_id` in `completed_wonders` (default `dyson_ring`). Diplomacy needs every faction at `allied_min` standing, then `hold_secs` without dropping below. The first matching path wins once and awards `base_prestige` plus a path bonus plus a speed bonus that falls to 0 at `speed_target_secs`. The farthest outpost at or beyond `homeworld_min_system` is marked the pirate homeworld; its defences and garrison scale by `homeworld_defence_mult` / `homeworld_garrison_mult`. Casual-play profiles should land in `casual_min_secs`–`casual_max_secs` (14–28 days in the bundle). Set `enabled = false` to skip checks. `hoshi simulate` (when days cover the casual floor) reports the three paths.

## Seasons

Rule sets live in `crates/hoshi-core/data/seasons.toml`, not in `balance.toml`. `epoch_unix` and `duration_secs` define UTC windows from the injected clock. Each `[[season]]` row may toggle known systems (`raid`, `market`, `activity`, `victory`, `moons`), scale expedition finds with `reward_mult`, and override raid timing. Unknown system keys fail load. Seasonal saves store the rule-set id and window so load re-applies the overlay. Prestige stays isolated unless `prestige = true`.

## Species

Playable empires live in `crates/hoshi-core/data/species.toml`. Each `[[species]]` row may scale construction, research, ship cost, expedition finds and espionage, add build-queue slots, and replace exclusive-group techs (`tree_remove` plus `[[species.tech]]`). `standing` offsets apply at new-game. Unique knobs: Aurelian `transmute_rate` and `brew_metal` / `brew_secs`; Void Wanderer `flagship`; Synth `energy_curve` / `energy_surplus`. Humans are `available`; other ids spend `unlock_cost` prestige and persist on `Prestige.unlocked_species`. Overlay validation rejects unknown factions, broken exclusive groups, and a catalog without five rows. `hoshi simulate` checks every species against every victory path inside the casual band, none more than 25% faster than the median.

## Rivals

NPC empires live in `crates/hoshi-core/data/rivals.toml`. They are off until a galaxy opts in (`rivals = true` in `config.toml`, the new-game toggle, or `hoshi new --rivals`). Each `[[personality]]` row is a utility mix (`build`, `research`, `trade`, `raid`, `ship`); the AI picks the highest-scoring action that `propose` accepts, mixing the galaxy seed so the same seed repeats. Difficulty knobs `aggression`, `efficiency` and `starting_bonus` are per galaxy (`--rival-aggression`, `--rival-efficiency`, `--rival-bonus`). Rivals use the same Intent layer as the player (plus the shipyard queue the CLI already has); they have no extra powers. Catch-up ticks them on `tick_secs` (shortened by efficiency). Intel on the diplomacy screen and `hoshi rivals` is gated by espionage research. `hoshi simulate` checks that each victory path still lands in the casual band with rivals on.

## Prestige

`[prestige]` is the late-game reset. Score is building levels plus research levels plus `colony_weight` per colony plus `wonder_weight` per completed wonder. Reset is allowed after a galaxy victory, with a completed wonder, or when score reaches `min_score`. Currency earned is `score / points_per_currency`, at least 1. `[[prestige.bonuses]]` rows are permanent purchases: extra starting resources, production and research speed, extra build-queue slots. They live in the save apart from the run. `hoshi simulate` reports three runs with 0, 1 and 2 mine-focus bonuses so later runs produce more without skipping construction time.

## Market

`[market]` is the NPC resource exchange plus the regional book. Quoted mids (`metal_mean` iron, `crystal_mean` silicon, `deuterium_mean` fuel) are L1 extraction energy per unit relative to iron, not ordinal 1/2/3. Extra materials share those quotes. A trade of A for B converts at `price_A / price_B`, then a fee and volume slippage. Prices walk toward their means with noise, occasional shocks, and clamps at `min_mult`/`max_mult` of the mean. Story choice fields `metal_price`, `crystal_price` and `deuterium_price` apply a relative shock when the market is on. Trades larger than `cargo_free` need home cargo capacity at least the pay amount. `tick_secs` is the catch-up interval. Set `enabled = false` to freeze the walk and refuse PlaceOrder. The traders' current diplomacy tier adds to `fee` (clamped to `[0, 0.95]`). The hub coordinate (`hub_galaxy` / `hub_system` / `hub_position`) and `hub_*` stocks seed the guild warehouse. `npc_order_size` is the NPC PlaceOrder lot; `contract_penalty` is the missed-contract metal fraction; `order_ttl_secs` is the default limit-order life. Silicon and deuterium books are priced in iron.

## Diplomacy

Factions live in `crates/hoshi-core/data/factions.toml`, not in `balance.toml`. `tick_secs` is the catch-up interval. Each tick moves standing toward 0 by `decay` (`next = standing * (1 - decay)`), clamped to `min_standing`/`max_standing`. A faction's current tier is the highest `min` that standing still meets. Tier rows may add `market_fee`, `raid_delay_mult` (0 stops pirate raids), `schedule_raid`, and the same `modifiers` kinds as research. Trades apply `trade_delta` to traders; a raid win or loss applies `raid_win_delta` / `raid_loss_delta` to pirates; a conquered outpost applies `conquest_delta`. Story choices may set `standing = { traders = 6.0, pirates = -8.0 }`. `tribute_metal` / `tribute_delta` is the diplomacy-screen tribute. Set `enabled = false` to freeze standing.

## Simulator

```sh
hoshi simulate --days 3
hoshi simulate --days 3 --balance path/to/balance.toml --report report.md
```

The run uses `GameState::catch_up` with a greedy miner (solar when energy would
go short). Raids are off so the report isolates the economy. A second pass fires
merged-PR pulses at the channel cooldown up to the daily cap. `--days` is 1–30,
default 3. Failed targets exit 2.

## Self-play

`[self_play]` is the opt-in CI matrix. It never starts during normal play.

`hoshi --self-play self-play --matrix pr` runs seed 1 for every bundled species
through the wonder, diplomacy and conquest strategies. `--matrix nightly` uses
the longer seed list. `--seed N` replays one flagged seed. `--bots` also runs
the sample path bots in `examples/bots/` through the JSON CLI for one day.

The job fails when a finish lands outside `[victory].casual_min_secs`..`casual_max_secs`,
when a species or path is more than `dominance_max` faster than the median, or
(nightly, three or more seeds) when one species takes more than `win_share_max`
of the fastest finishes. Thresholds live in `balance.toml`.


Solar output uses the orbital radius from `[orbits]`: rated output times
`(1 AU / radius)^2`, followed by technology and planet modifiers. At 1 AU
output is unchanged; at 2 AU it is 25%, and at 2.5 AU it is 16%. Ice worlds
retain their additional 10% penalty. Existing colonies use this rule on load.
