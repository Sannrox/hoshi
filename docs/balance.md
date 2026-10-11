# Balance config and pacing

Production, construction, raids, bridge-effect sizes, expedition weights, flight,
starting stockpiles, and staged wonders live in `crates/hoshi-core/data/balance.toml`.
The file is embedded and validated on `BalanceConfig::default()`. Research stays in
`research.toml`; ships stay in `ships.toml`. Wonder ids must be unique; each wonder
needs at least one stage with `duration_secs > 0` and a positive resource cost.

Override the bundled file at runtime:

```sh
hoshi --balance path/to/balance.toml
```

`HOSHI_BALANCE` is used when `--balance` is omitted, including `hoshi simulate`. The override is a full file
(same keys as the bundled one), not a partial patch. Version must be `1`. Growth
factors must be `> 0`, bases `>= 0`, build and ship queues at least 1.

## Pacing targets

`[pacing]` is checked by `hoshi simulate`:

| Key | Meaning |
| --- | --- |
| `first_upgrade_secs` | First building upgrade must complete within this many seconds. |
| `min_metal_mine_per_day` | Metal mine levels gained per simulated day under the greedy strategy. |
| `max_bridge_metal_ratio` | `metal` after maxed PR-merge pulses divided by `metal` with activity off. |

Defaults: first upgrade in 5 minutes, at least one metal mine level per day, and
bridge pulses must not multiply stored metal by more than 8.

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

## Simulator

```sh
hoshi simulate --days 3
hoshi simulate --days 3 --balance path/to/balance.toml --report report.md
```

The run uses `GameState::catch_up` with a greedy miner (solar when energy would
go short). Raids are off so the report isolates the economy. A second pass fires
merged-PR pulses at the channel cooldown up to the daily cap. `--days` is 1–30,
default 3. Failed targets exit 2.
