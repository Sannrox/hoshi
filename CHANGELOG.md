# Changelog

## Unreleased

### Added

- Resource model in `hoshi-core`: metal, crystal, deuterium and energy. Production, energy factor and storage caps come from `BalanceConfig`. `GameState::catch_up` resolves any offline gap in one call and is a no-op if the clock moves backwards.
- Workspace skeleton: `hoshi-core` (state, injectable clock, offline catch-up, SQLite save stub), `hoshi-tui` (placeholder screen), `hoshi-bridge` (event type placeholder).
- ADR 0001 and ADR 0002.
