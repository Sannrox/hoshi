# Balance config and pacing

Design constraints for these numbers (energy, mass, travel, heat) live in
[docs/realism.md](realism.md). This page is the operator manual for the live file.

Production, construction, raids, bridge-effect sizes, expedition weights, flight,
starting stockpiles, and staged wonders live in `crates/hoshi-core/data/balance.toml`.
The file is embedded and validated on `BalanceConfig::default()`. Research stays in
`research.toml`; ships stay in `ships.toml`; story events stay in `story.toml`;
planet defences stay in `defences.toml`; laws stay in `laws.toml`.
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

Construction stockpiles stay the three resource columns (`metal` / `crystal` / `deuterium`), also named frames, electronics and fuel. Intermediate goods live in the save `goods` table. Building costs may use `frames`, `electronics` and `fuel`; ship costs may use `frames`, `electronics` and `propellant` (`fuel` on a ship is still flight consumption).

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

## Espionage

`[espionage]` scores a probe wave as `probes * probe_weight + (own_espionage - target_level) * level_weight`. Own espionage is the existing research/faction modifier. The report tier is the highest `[[espionage.tiers]]` whose `min_score` the score still meets (activity, resources, fleet, defences, garrison). Detection chance is `detect_base + probes * detect_per_probe - (own - target) * detect_per_level`, clamped to `[0, 1]`. A detection then rolls `destroy_chance` once per probe. The TUI reports screen lists outposts with report age; Enter opens the latest report; `s` sends one probe.

## Outposts

`[outposts]` places pirate worlds from the galaxy seed. `min_ships` and
`min_garrison` are the early-game floor; `strength_factor` and `garrison_factor`
scale later rebuilds with the player's fighter-equivalent fleet. `rebuild_secs` is the
catch-up interval. `variance` is the ground-combat spread (`0` is an exact
comparison). `troop_strength` is the landing power of one troop transport.
`[[outposts.rare]]` rows are the rare-planet bonuses shown on the planet screen.
`shipyard_speed` speeds the empire shipyard (ships queue at home).

## Prestige

`[prestige]` is the late-game reset. Score is building levels plus research levels plus `colony_weight` per colony plus `wonder_weight` per completed wonder. Reset is allowed with a completed wonder or when score reaches `min_score`. Currency earned is `score / points_per_currency`, at least 1. `[[prestige.bonuses]]` rows are permanent purchases: extra starting resources, production and research speed, extra build-queue slots. They live in the save apart from the run. `hoshi simulate` reports three runs with 0, 1 and 2 mine-focus bonuses so later runs produce more without skipping construction time.

## Market

`[market]` is the NPC resource exchange. Each of metal, crystal and deuterium has a mid price in abstract units. A trade of A for B converts at `price_A / price_B`, then a fee and volume slippage. Prices walk toward their means with noise, occasional shocks, and clamps at `min_mult`/`max_mult` of the mean. Story choice fields `metal_price`, `crystal_price` and `deuterium_price` apply a relative shock when the market is on. Trades larger than `cargo_free` need home cargo capacity at least the pay amount. `tick_secs` is the catch-up interval. Set `enabled = false` to freeze the walk. The traders' current diplomacy tier adds to `fee` (clamped to `[0, 0.95]`).

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
