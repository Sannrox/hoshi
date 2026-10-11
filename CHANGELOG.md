# Changelog

## Unreleased

### Added

- Overview and planet screens in the TUI: resources with rates and caps, energy balance, build and research countdowns, a building list with queue management, and a research list. Tab switches screens; enter starts the selected item; unaffordable rows are dimmed with the missing amount. Quit still writes the save.
- SQLite save in the user data directory (`hoshi/save.sqlite`). Schema v1 stores resources, buildings, the build queue, research, timestamps and the RNG seed. Override the path with `--save` or `HOSHI_SAVE`. Writes run in one transaction and use WAL, so a crash mid-write keeps the previous save.
- Research tree loaded from `crates/hoshi-core/data/research.toml` (validated: unique ids, existing prereqs, no cycles). One research at a time; it completes during `catch_up` like buildings. Combat and fleet modifiers are applied through a single `GameState::modifier` lookup.
- Planet buildings and a 5-slot build queue in `hoshi-core`. Costs and durations come from `BalanceConfig`; robotics factory level shortens build time. Completions that fall in an offline gap apply in timestamp order and production is recomputed from each completion instant. Cancel refunds the paid cost in full.
- Resource model in `hoshi-core`: metal, crystal, deuterium and energy. Production, energy factor and storage caps come from `BalanceConfig`. `GameState::catch_up` resolves any offline gap in one call and is a no-op if the clock moves backwards.
- Workspace skeleton: `hoshi-core` (state, injectable clock, offline catch-up, SQLite save), `hoshi-tui` (status screen), `hoshi-bridge` (event type placeholder).
- ADR 0001 and ADR 0002.
