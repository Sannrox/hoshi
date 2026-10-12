# hoshi (星)

A real-time idle space-strategy game for the terminal, built with
[ratatui](https://ratatui.rs). Inspired by the classic browser space-empire
games: mine resources, raise buildings, research technologies, build fleets
and send them on expeditions. Meant to be played in a spare terminal pane
while your coding agents work.

> Status: hobby project, pre-alpha. `hoshi` opens on a title screen (continue, new, load, settings). Tab switches overview, planet, shipyard, wonder, prestige, market and diplomacy. Enter queues a build, a ship, research, or a wonder contribution. Mines run production chains from planet deposits to ship parts. Galaxy seeds are short checksummed codes (`hoshi new --seed`, `y` copies from the overview). Story events offer numbered choices on a cooldown; ignored too long uses the default. The market trades metal, crystal and deuterium at drifting prices. Three NPC factions keep standing that trades, raids, conquest and story choices move. Fleets fly on timers. Colony ships settle empty planets. Pirate outposts take a fleet attack then a troop landing. A prestige reset starts a new galaxy with kept bonuses. Pirate raids land on a countdown. The game saves to SQLite.

## A standalone game first

hoshi is a complete game on its own: economy, research, fleets, colonies,
story events, trade, wonders, prestige, daily missions and achievements. No
account, network connection or AI service is needed. The local game stays
single-player ([ADR 0004](docs/decisions/0004-no-shared-universe.md)); a shared
universe exists only as a shard hosted on Chisei ([ADR 0005](docs/decisions/0005-shared-shard-on-chisei.md)).

## Optional: your agents play along

An optional bridge, behind the `bridge` cargo feature and a config switch,
both off by default, turns your development activity into small, capped game
events. It reads only free signals: local event files or sockets, and GitHub
through your own `gh` login. It never calls AI models or any paid service, and
the game is balanced without it.

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

The save file lives in the user data directory (`hoshi/save.sqlite`). Override the path with `--save PATH` or `HOSHI_SAVE`. Balance numbers load from the bundled `balance.toml`; override with `--balance PATH` or `HOSHI_BALANCE`. `--no-portraits` keeps ASCII drawings and skips RGB. Keys: tab (screens), j/k (select), enter (build or research), x (cancel), q (quit and save), `?` (help for this screen). Remap them with `keys.<action>` lines in `config.toml` or from title Settings → keybindings; a conflict keeps the current keys. Title Achievements lists unlocks and career totals across saves without opening a slot for writing. New game previews a pasted seed before creating the slot; `y` copies the current save's code from the title menu, Load, and the overview. With `--features bridge` and the bridge enabled, a status bar shows the latest feed event and `a` opens the activity log. Notifications are off until Settings (or `notify = true`); they use the terminal bell and OSC 9/777, with per-type toggles and quiet hours in the same file.

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
[docs/decisions/](docs/decisions/README.md). Physics and maths for design
work: [docs/realism.md](docs/realism.md). Portrait art and licenses:
[crates/hoshi-tui/ASSETS.md](crates/hoshi-tui/ASSETS.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
