# ADR 0006: Persistence through a Storage trait

- Status: Accepted
- Date: 2026-10-13
- Follows: [0002](0002-terminal-idle-game-with-event-bridge.md)
- Source: [#25](https://github.com/Sannrox/hoshi/issues/25)

## Context

hoshi-core owned a SQLite file directly. The TUI held a `Connection` and called
`save_state`. That couples rules-adjacent code to one engine and blocks a later
Chisei shard, which needs object identity (Player, Planet, Fleet, Outpost) and
atomic multi-object writes.

An in-process object log (Mikura v0.2.0) can store those identities. It is
pre-1.0, not on crates.io, and must not become a default dependency.

## Decision

1. **hoshi-core talks to a `Storage` trait** (`load`, `save` / `apply_batch`).
   Game rules never open a database.
2. **SQLite stays the default backend** and the on-disk format for existing
   saves. Schema migrations stay in the SQLite implementation. Default builds
   do not depend on Mikura.
3. **Mikura is an optional cargo feature**, pinned to git tag `v0.2.0`. A
   `*.mikura` path selects it. Each save is one `append_batch` of Snapshot,
   Player, Planet, Fleet and Outpost records.    The SQLite-format blob lives in a versioned sidecar named from the committed
   Snapshot (Mikura property strings are u16-capped); the named objects carry
   `owner`, `stationed-at`, and `en-route-to`.
4. **Write rates stay low:** one persist per CLI command, one every five
   seconds in the TUI, catch-up in memory then one persist. See
   [docs/storage.md](../storage.md).

## Consequences

- Existing `save.sqlite` files keep loading. Schema version does not bump for
  this change.
- The TUI and CLI open storage through `open_storage`, not `rusqlite`.
- A Mikura save is a different file, not a migration of SQLite.
- Shared tests run the same round-trip, atomic, catch-up, and colonize checks
  on both backends.

## Rejected alternatives

- **Replace SQLite now.** The local game already has migrations, WAL, and
  inspect-without-lock. Switching the default would break every save.
- **Always depend on Mikura.** The pin is pre-1.0 and git-only; default `cargo
  build` must stay offline-friendly aside from the first fetch of optional
  deps.
- **Serde-json snapshot only.** That would duplicate the schema and drift from
  SQLite. The SQLite blob is the codec; Mikura objects are the graph.
