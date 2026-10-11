# hoshi (星)

A real-time idle space-strategy game for the terminal, built with
[ratatui](https://ratatui.rs). Inspired by the classic browser space-empire
games: mine resources, raise buildings, research technologies, build fleets
and send them on expeditions. Meant to be played in a spare terminal pane
while your coding agents work.

hoshi is a fictional game with a second purpose: it is a showcase for how
ontologies are used in business and government work. Its domain model has
clear object types (players, planets, fleets, factions, outposts), explicit
links between them, moves expressed as governed actions, and auditable
outcomes, so the same game can demonstrate modelling, permissions and audit on
a realistic workload. Physics and maths (production, energy, travel times) aim
to be plausible; species, factions and story are pure fiction.

> Status: hobby project, pre-alpha. The terminal has overview, planet and shipyard screens. Tab switches; enter queues a build, a ship, or research. Fleets fly on timers. Colony ships settle empty planets. Pirate raids land on a countdown. The game saves to SQLite.

## The twist: your agents play along

An optional bridge turns your development activity into game events:

- a merged pull request pays out resources;
- green CI boosts production or research;
- red CI or a stuck agent attracts a pirate raid;
- any agent tool can post to a local activity feed shown in the game.

## Layout

| Crate | Role |
| --- | --- |
| `hoshi-core` | Rules, state, timers, offline progression, SQLite save |
| `hoshi-tui` | The `hoshi` binary (ratatui) |
| `hoshi-bridge` | GitHub and agent events into game events |

## Run

```sh
cargo run -p hoshi-tui
```

The save file lives in the user data directory (`hoshi/save.sqlite`). Override the path with `--save PATH` or `HOSHI_SAVE`. Balance numbers load from the bundled `balance.toml`; override with `--balance PATH` or `HOSHI_BALANCE`. Keys: tab (screens), j/k (select), enter (build or research), x (cancel), q (quit and save).

```sh
hoshi simulate --days 3
```

prints a pacing report (first upgrade time, daily mine levels, bridge-effect ratio). Targets live in `docs/balance.md`.

Scriptable JSON commands (`hoshi state`, `build`, `research`, `cancel`, `fleet send`, `tick --advance`) share the TUI's core actions. See `docs/cli.md`.

## Develop

```sh
cargo fmt --all -- --check
cargo build --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
bash scripts/validate-workflow-skills.sh
```

Policy for contributors and agents: [AGENTS.md](AGENTS.md). Decisions:
[docs/decisions/](docs/decisions/README.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
