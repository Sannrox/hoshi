# ADR 0018: Persistence lives in hoshi-storage

- Status: Accepted
- Date: 2026-10-24
- Amends: [0002](0002-terminal-idle-game-with-event-bridge.md) crate
  boundaries; [0006](0006-storage-trait.md) trait location
- Follows: [0002](0002-terminal-idle-game-with-event-bridge.md),
  [0006](0006-storage-trait.md)
- Source: [#384](https://github.com/Sannrox/hoshi/issues/384)

## Context

ADR 0006 put persistence behind a `Storage` trait so game rules never open a
database, but the trait, SQLite glue, and optional Mikura / Chisei backends
still compiled inside `hoshi-core`. A hot edit in `state.rs` type-checked
thousands of lines of rusqlite and rebuilt every workspace crate that depended
on core. Public `open_save` / `load_state` / `save_state` named
`rusqlite::Connection`. `hoshi-bridge` depended on core only for save-path
helpers.

## Decision

1. **`hoshi-storage` owns persistence.** SQLite, the `Storage` trait, optional
   Mikura and Chisei backends, save locks, inspect, and backup live in this
   crate. On-disk schema and file format stay the same; `SCHEMA_VERSION` does
   not bump.
2. **`hoshi-core` has no database edge.** Rules, `GameState`, catalogs, and the
   clock stay there. Core does not depend on `hoshi-storage` (that edge would
   cycle). `CheatState` is public so storage can round-trip cheat provenance.
3. **The TUI and CLI persist through `open_storage`.** They do not hold a
   rusqlite `Connection` as the primary path.
4. **`hoshi-bridge` inlines save-path helpers** and depends on neither
   `hoshi-core` nor `hoshi-storage`.

## Consequences

- `cargo tree -p hoshi-core -i rusqlite` does not resolve. Storage contract
  tests (`sqlite_backend_meets_storage_contract`, optional mikura/chisei) run
  on `hoshi-storage`.
- Callers import save APIs from `hoshi_storage`. Persist-specific unit tests
  that used to live in core moved with the glue.
- Optional `--features mikura` / `--features chisei` on the binary forward to
  `hoshi-storage`.

## Rejected alternatives

- **Keep rusqlite in core behind a feature.** Default core builds would still
  compile SQLite into the rules crate.
- **A tiny paths crate for the bridge.** Extra surface; two helpers inline.
- **Split `GameState` or extract `hoshi-serve`.** Out of scope for this cut.
