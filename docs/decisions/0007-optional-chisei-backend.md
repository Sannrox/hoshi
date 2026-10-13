# ADR 0007: Optional in-process Chisei backend

- Status: Accepted
- Date: 2026-10-13
- Follows: [0005](0005-shared-shard-on-chisei.md), [0006](0006-storage-trait.md)
- Source: [#65](https://github.com/Sannrox/hoshi/issues/65)

## Context

ADR 0005 allows a shared universe only as a shard hosted on Chisei. ADR 0006
put persistence behind a `Storage` trait so hoshi-core never opens a database.
The local game must stay a single-player SQLite save. A reference workload
still needs the Chisei object model (Player, Planet, Fleet, Faction, Outpost),
governed propose-then-apply moves, tenant markings for fog of war, and a
pinned API version — without pulling a control plane or network stack into
hoshi-core.

## Decision

1. **SQLite stays the default save.** Existing files and schema version do not
   change.
2. **An optional `chisei` cargo feature** compiles a directory backend selected
   by a `*.chisei` path. Without the feature that path is refused. A
   `chisei = true` line in `config.toml` (off by default) lets the bot arena
   target this backend; the local title screen still creates SQLite slots.
3. **The backend is in-process.** It stores a SQLite `GameState` codec beside a
   versioned object graph (`graph.toml`) and a copy of
   [`hoshi-ontology/v1`](../../crates/hoshi-core/data/ontology/hoshi-v1.json).
   hoshi-core still has no network or server code. A live shared shard remains
   the Chisei-hosted deployment in ADR 0005.
4. **The Chisei product/API version is pinned at 1.3.0**
   (`CHISEI_API_VERSION`). The ontology document is `schema_version: 1` so the
   standalone ontology tool can import it. Governed actions (`build`,
   `research`, `launch`, `colonize`, `invade`) are `propose` then `apply` in
   hoshi-core; a refused proposal leaves live state unchanged.
5. **Reads for another tenant go through markings.** Own planets, allied
   players, and spy-cleared planets are visible; everything else is hidden.

## Consequences

- Default `cargo build` and `cargo test` stay free of the Chisei feature.
- Shared `#25` storage-contract tests run on this backend when the feature is
  on.
- Arena `run --backend chisei` (or `chisei = true` in config) needs the
  feature; see [docs/chisei.md](../chisei.md).

## Rejected alternatives

- **Depend on the Chisei control-plane crate.** That would put gRPC, an async
  runtime, and server code in hoshi-core, which ADR 0005 forbids.
- **Replace SQLite.** Local play and existing saves stay on the default
  backend.
- **Network client in the TUI.** The local binary is not a shard client.
