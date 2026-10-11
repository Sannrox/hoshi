# Changelog

## Unreleased

### Added

- Pirate raids: a countdown (default one hour) then a six-round fight against a pirate fleet scaled to the player. Shields refill each round; hull does not. A lost fight takes a capped share of stored resources. The combat report goes in the event log and the overview shows the countdown. Save schema v4 stores the next raid and the log.
- Fleets launch to `galaxy:system:position` with flight time from distance, slowest ship speed and a speed percent. Deuterium fuel is paid at launch. Missions run outbound, action and return phases during `catch_up`; recall mid-outbound returns in the elapsed time. Computer technology adds fleet slots. Save schema v3 stores missions.
- Nine ship types in `crates/hoshi-core/data/ships.toml` (fighters, cargos, cruiser, battleship, recycler, probe, explorer) with cost, time, combat stats, cargo, speed, fuel, shipyard level and research unlocks. `GameState::ship_stats` applies weapons, shields, armour and the ship's drive. Batches complete one ship at a time during `catch_up`. Counts persist in save schema v2. The TUI shipyard screen lists ships, requirements and the queue.
- Overview and planet screens in the TUI: resources with rates and caps, energy balance, build and research countdowns, a building list with queue management, and a research list. Tab switches screens; enter starts the selected item; unaffordable rows are dimmed with the missing amount. Quit still writes the save.
- SQLite save in the user data directory (`hoshi/save.sqlite`). Schema v1 stores resources, buildings, the build queue, research, timestamps and the RNG seed. Override the path with `--save` or `HOSHI_SAVE`. Writes run in one transaction and use WAL, so a crash mid-write keeps the previous save.
- Research tree loaded from `crates/hoshi-core/data/research.toml` (validated: unique ids, existing prereqs, no cycles). One research at a time; it completes during `catch_up` like buildings. Combat and fleet modifiers are applied through a single `GameState::modifier` lookup.
- Planet buildings and a 5-slot build queue in `hoshi-core`. Costs and durations come from `BalanceConfig`; robotics factory level shortens build time. Completions that fall in an offline gap apply in timestamp order and production is recomputed from each completion instant. Cancel refunds the paid cost in full.
- Resource model in `hoshi-core`: metal, crystal, deuterium and energy. Production, energy factor and storage caps come from `BalanceConfig`. `GameState::catch_up` resolves any offline gap in one call and is a no-op if the clock moves backwards.
- Workspace skeleton: `hoshi-core` (state, injectable clock, offline catch-up, SQLite save), `hoshi-tui` (status screen), `hoshi-bridge` (event type placeholder).
- ADR 0001 and ADR 0002.
