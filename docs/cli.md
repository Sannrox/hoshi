# Scriptable CLI

Harnesses and bots play through the same rules as the TUI. Every command prints
JSON with `schema_version` 2 (see `docs/schema/cli-v1.json`). A breaking change
bumps that version. Resource ids are `iron`, `silicon`, `rare_earths`,
`water_ice`, `helium_3` and `deuterium`; `metal` and `crystal` error with the
replacement name. `state` lists those stockpiles plus `energy` and `heat`.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 2 | Invalid input (unknown command, bad coord, missing flag) |
| 3 | Rule refusal (not enough resources, missing prerequisites, tick on a live save) |
| 4 | Save is locked by another process |
| 5 | Internal error (corrupt save, IO) |

## Commands

```sh
hoshi --version
hoshi serve [--save PATH] [--bind HOST:PORT] [--config DIR]
hoshi --save PATH [--seed N] [--balance PATH] state [--as player|leader:ID] [--json]
hoshi --save PATH intel [coord] [--as player|leader:ID]
hoshi --save PATH intel provenance <coord> <field> [--as player|leader:ID]
hoshi --save PATH classify <kind> <id> --class open|internal|restricted|secret [--compartment ID] [--as player|leader:ID]
hoshi --save PATH share <faction> --class open|internal|restricted|secret [--compartment ID]
hoshi --save PATH incidents [--as player|leader:ID]
hoshi --save PATH advise
hoshi --save PATH log [--type KIND] [--actor ID] [--object ID] [--from UNIX] [--to UNIX] [--lineage ID] [--at UNIX] [--export PATH]
hoshi --save PATH staff
hoshi --save PATH vehicle
hoshi --save PATH vehicle register <planet> <ships...>
hoshi --save PATH vehicle dissolve <id>
hoshi --save PATH mission windows <from> <to> [--transfer hohmann|high_energy|assist]
hoshi --save PATH mission plan <to> --vehicle ID [--transfer hohmann|high_energy|assist] [--customer colony|faction:ID] [--iron N] [--silicon N] [--deuterium N] [--window N]
hoshi --save PATH mission schedule
hoshi --save PATH market [region [coord]]
hoshi --save PATH market order <region> buy|sell <good> <amount> [--limit P] [--ttl SECS]
hoshi --save PATH market contract list|accept <id>|fulfil <id>
hoshi --save PATH market ship <from> <to> [--iron N] [--silicon N] [--deuterium N] [--contract ID]
hoshi --save PATH market history [coord] [--export PATH]
hoshi --save PATH staff recruit [--name TEXT] [--trait ID]
hoshi --save PATH staff assign <id> admiral|governor|minister [--target home|empire|g:s:p]
hoshi --save PATH staff delegate <id> [--permission queue_build|accept_battle|propose_law] [--cap N]
hoshi --save PATH staff revoke <id> [--permission queue_build|accept_battle|propose_law]
hoshi --save PATH staff clearance <id> --class open|internal|restricted|secret [--compartment ID]
hoshi --save PATH build <planet> <building> [--as player|leader:ID]
hoshi --save PATH research <tech>
hoshi --save PATH cancel <queue-item>
hoshi --save PATH fleet send <from> <to> <mission> <ships...>
hoshi --save PATH tick --advance <dur>
hoshi --save PATH prestige
hoshi --save PATH prestige buy <id>
hoshi --save PATH prestige reset
hoshi --save PATH species
hoshi --save PATH species unlock <id>
hoshi --save PATH rivals
hoshi --save PATH transmute <from> <to> <amount>
hoshi --save PATH brew
hoshi --save PATH flagship relocate <coord>
hoshi --save PATH flagship anchor
hoshi --save PATH goals
hoshi --save PATH goals keep
hoshi --save PATH finance
hoshi --save PATH finance set <center> <limit|unlimited>
hoshi --save PATH finance approve <id>
hoshi --save PATH kpis
hoshi --save PATH kpis set <kpi> <value> [owner]
hoshi --save PATH kpis report
hoshi --save PATH kpis generate
hoshi --save PATH incidents
hoshi --save PATH incidents open <kind> [severity]
hoshi --save PATH incidents assign <id> <leader>
hoshi --save PATH incidents resolve <id> [note]
hoshi --save PATH incidents review <id>
hoshi --save PATH maintenance
hoshi --save PATH maintenance schedule <asset-id> [time|usage]
hoshi --save PATH maintenance complete <order-id>
hoshi --save PATH maintenance telemetry [--export PATH]
hoshi --save PATH automation
hoshi --save PATH automation dry-run
hoshi --save PATH automation add stock_below|stock_above <planet> <resource> <amount> queue_build <planet> <building>
hoshi --save PATH automation add stock_below|stock_above <planet> <resource> <amount> transfer <from> <to> <resource> <amount>
hoshi --save PATH automation toggle <id>
hoshi --save PATH automation remove <id>
hoshi --save PATH blueprint list
hoshi --save PATH blueprint show <name>
hoshi --save PATH blueprint capture <planet> <name>
hoshi --save PATH blueprint apply <name> <planet>
hoshi --save PATH blueprint export <name>
hoshi --save PATH blueprint import <path>
hoshi --save PATH law propose <template> [--param N] [--jurisdiction empire|home|g:s:p] [--effective <dur>]
hoshi --save PATH law approve <id>
hoshi --save PATH law repeal <id> [--effective <dur>]
hoshi --save PATH moon build <planet> phalanx|jump_gate
hoshi --save PATH moon jump <from> <to> <ships...>
hoshi --save PATH moon scan <planet>
hoshi --save PATH tutorial skip
hoshi --save PATH tutorial reset
hoshi --save PATH tactics set [formation <id>] [stance <id>] [retreat <0-1>]
hoshi --save PATH tactics action hold|retreat|commit
hoshi --save PATH feedback [note...]
hoshi --self-play self-play [--matrix pr|nightly] [--seed N] [--report PATH] [--bots]
hoshi [--save PATH] --arena arena run --seed N --days N [--actions N] [--timeout-ms N] [--board PATH] bot...
hoshi [--save PATH] --arena arena board [--board PATH]
hoshi [--save PATH] --demo demo governance --seed N [--report PATH]
```

Planet is `home` or `galaxy:system:position`. Building and ship ids match the
config keys (`metal_mine`, `light_fighter`). Queue item is `0`, `build:0`, or
`research`. Mission is `attack`, `invade`, `colonize`, `expedition`, or `spy`. Ships look like
`light_fighter:2`. `attack` fights an outpost's defences; `invade` lands troop
transports and is refused while defences remain. `spy` sends espionage probes and
writes a report that arrives after light-time; `state` lists arrived reports with
age, source and confidence and attaches the latest arrived report to each
outpost so attack and invade can read it. `intel` lists every arrived fact;
`intel provenance <coord> <field>` prints the chain for one field. Pending
reports and the internal deceived flag never appear. `colonize` takes exactly `colony_ship:1`. `moon build` spends stockpile on a phalanx or jump gate at that planet's moon. `moon jump` moves parked ships instantly between two moons that both have gates. `moon scan` lists fleets the phalanx can see. Extra arguments
on `build`, `research`, and `cancel` are invalid (`state` may take `--as`). Duration is seconds,
or `30s` / `3m` / `1h` / `1d`.

`advise` prints ranked next-move suggestions from the same player-visible
state the TUI overlay uses. It never enqueues a build, research, or fleet.
Extra arguments on `advise` are invalid.

`log` searches the structured history (build, research, ship, fleet, combat,
trade, activity, prestige). `--lineage ID` walks cause links; `--at UNIX`
returns the recorded empire glance at that time if it is still retained;
`--export PATH` writes JSON lines of the retained window. History keeps the
last 256 events and one snapshot per event. The TUI `log` screen filters by
type (`+`/`-`) and opens lineage (Enter) or the glance (`r`).

`vehicle register` assigns parked ships to a named vehicle. `mission windows` lists synodic launch windows with delta-v and coast time. `mission plan` is PlanMission (propose, check, apply): payload mass changes fuel through the rocket equation; `--window N` can book an upcoming window (the vehicle launches when it opens). A missed window or a double-booked vehicle is refused. `mission schedule` lists vehicles, in-flight plans, and double-book / missed-window alerts. Transfer is `hohmann`, `high_energy`, or `assist`. Customer is `colony` (owned dest, cargo credited there), `faction:<id>` (standing moves by that faction's trade delta), or `market` (cargo lands in the destination regional warehouse; refused while the market is off).

`market` is the regional book: silicon and deuterium priced in iron, limit and market orders, delivery and procurement contracts, and shipments that pay fuel and take flight time. `market order` is PlaceOrder; `market contract accept|fulfil` and `market ship` are the same core actions as the TUI. `market history --export` writes JSON lines of per-region price history. NPC traders post through the same PlaceOrder path. Home over-the-counter barter (`trade` on the TUI market screen) is unchanged.

`staff` lists leaders, roles, assignments and delegations. `staff recruit` spends the configured cost. `staff assign` places one role per leader (reassignment waits `reassign_secs`). `staff delegate` lets that leader act with an optional spending cap; `staff revoke` removes it. `staff clearance` grants extra classification and compartments. `build --as leader:ID` runs the same queue checks under that leader's permissions.

`classify` marks a `report`, `law`, `plan` or `fact`. Missing markings are Open. `state --as leader:ID` and `incidents --as` omit objects that role cannot see. Admirals start at Restricted/fleet, governors Internal/planet, ministers Internal/policy; the player is unrestricted. `share` writes an agreement with a faction at favored, honored or truce standing. Detected spy waves and the `disloyal` leader trait open leak incidents. Hidden data never leaves core for the TUI, CLI or a Chisei tenant save; the shared graph does not copy marked object ids.

`--seed` on a new save starts deterministic mode: the wall clock is ignored and
`tick --advance` is allowed. Live saves refuse `tick --advance`. The TUI refuses
a deterministic save so wall-clock catch-up cannot run on it. JSON never
includes RNG state or galaxy seed. Unknown commands and CLI-only flags print
JSON with exit 2; they do not open the TUI. `expedition` dest must be the
configured expedition slot.

`arena run` is off until `arena = true` in `config.toml` or `--arena`. Each bot is a subprocess that prints JSON CLI commands (`{"cmd":["build","home","metal_mine"]}`) against its own deterministic save. Crash, timeout and invalid actions zero that bot and leave the others running. `arena board` prints the appended JSON leaderboard as markdown. `--backend chisei` (or `chisei = true` in config, both off by default) writes `*.chisei` bot saves when built with `--features chisei`. Scoring is versioned in [docs/arena.md](arena.md). Sample bots live in `examples/bots/`.

`demo governance` is off until `demo = true` in `config.toml` or `--demo`, and it needs `--features chisei`. It runs ungoverned vs governed players on the same seed with recorded model replies (no HTTP). JSON and markdown report outcomes, policy blocks, an audit excerpt, cost per win, tokens per move and routing savings. See [docs/demo.md](demo.md). `governance = true` in config makes the TUI ask before an invasion; headless demo runs auto-deny.

`incidents` without a subcommand lists classification leak incidents visible to that actor (`--as` still applies) and the operational board (`open`, `risks`, `reviews`). `incidents open <kind> [severity]` is OpenIncident (`raid`, `failure`, `shortage`, `accident`, `heat`, `contract`, `manual`; severity `low`/`medium`/`high`/`critical`, default medium). `incidents assign <id> <leader>` and `incidents resolve <id> [note]` are AssignIncident and ResolveIncident. `incidents review <id>` archives a post-incident review for a high or critical resolved case. `state` keeps the leak `incidents` array and adds an `incident_board` object (open count, risks, reviews).

`maintenance` lists assets (condition, hours, offline window), plans, open work orders, latest telemetry and failure predictions. `maintenance schedule <asset-id> [time|usage]` is ScheduleMaintenance (default time; spends iron/silicon and offlines the asset). `maintenance complete <order-id>` is CompleteWorkOrder (restores condition). `maintenance telemetry` prints readings; `--export PATH` writes JSON lines. `state` includes a `maintenance` object (asset, order and reading counts). Failures can open operational incidents of kind `failure`.

`kpis` prints live KPI measurements (output, energy_balance, heat_margin, budget_variance, incidents, mission_on_time, market_profit) with targets, variance, progress and source event ids, plus archived period reports and a Markdown export. `kpis set <kpi> <value> [owner]` is SetTarget (`empire`, `colony:g:s:p`, `role:governor|admiral|minister`). `kpis report` is the same snapshot; `kpis generate` archives a period now. `state` includes a `kpis` object (next report time, target count, archived count).
`finance` prints treasury cash, debt, reserve gap, period income/expense, budgets (limit, actual, forecast, variance, run rate) and held spends. `finance set <center> <limit|unlimited>` is SetBudget (`industry`, `research`, `military`, `logistics`, `fleet:home`, `colony:g:s:p`). `finance approve <id>` is ApproveSpend. Over-budget construction, research and ships wait until approved. `state` includes a `finance` object (cash, debt, pending count). `prestige` prints score, currency and owned bonuses. `prestige buy` spends unspent currency. `species` is the current empire overlay (`state` also has a `species` object, `null` on overlay-free saves). `species unlock` spends prestige. `transmute`, `brew`, and `flagship relocate|anchor` are species actions and are refused when that empire does not have them. `prestige reset` is refused until a galaxy victory, a completed wonder, or score reaches the configured minimum; it keeps bonuses and run history and starts a new galaxy. `automation` lists research-gated tiers, control-energy draw and rules with a dry-run preview (`would_fire`). `automation add` writes a stock trigger plus `queue_build` or `transfer`; opposite transfers and the same queue target conflict. `automation toggle` / `remove` change saved rules. `goals` prints the industrial ladder (current tier, next step, completed ids) plus conquest, wonder and diplomacy progress and any victory record; `goals keep` dismisses the victory overlay so the save can keep playing. `frontier` lists revealed frontier worlds with bonuses; `state` includes a `frontier` object (galaxy, revealed sectors, visible worlds). `state` includes a `ladder` object and a `victory` object (homeworld, record, path progress). `state` includes a `leaders` array (traits, assignment, retirement, delegations), a `waste` object (heat and pollution load versus sink, unrest, and the warning flag), a `laws` array (status, approvals, latest expected vs measured samples), a `daily` object (streak, time until reset, and slot progress), and a `tutorial` object (enabled, skipped, pending hint). `tutorial skip` hides remaining hints; `tutorial reset` clears seen ids and shows the opening again. `tactics set` stores the home fleet's formation, stance and auto-retreat threshold; `tactics action` is refused unless a live fight is waiting. `state` includes a `tactics` object. `feedback` appends a timestamped JSON line (version and a save glance, never the galaxy seed) to a `.feedback.log` beside the save; a missing or locked save still records the note. See `docs/playtest.md`. `law propose` writes a proposal; `law approve` records the player approval and enacts when the effective date is due; `law repeal` starts the same flow in reverse and restores prior parameters.

`--save` / `HOSHI_SAVE` and `--balance` / `HOSHI_BALANCE` match the TUI. A `*.mikura` path needs `--features mikura`; a `*.chisei` path needs `--features chisei` ([docs/storage.md](storage.md), [docs/chisei.md](chisei.md)). Build, research, colonize and invade go through propose then apply. `hoshi` with no subcommand opens the title screen. `--save PATH` still opens that file directly. `hoshi --continue` resumes the last slot without the menu; locked slots exit 4, corrupt or newer-schema or harness slots exit 5. `hoshi new --seed <code> [--species ID] [--rivals] [--rival-aggression N] [--rival-efficiency N] [--rival-bonus N] [--rival-personality ID]` writes a new live save (default slot, `--save PATH`, or `HOSHI_SAVE`) and prints the canonical grouped seed code; an existing file exits 2. Default species is `humans`; locked ids are refused until prestige unlocks them. Rivals stay off unless `--rivals` is set. `hoshi rivals` prints the espionage-gated intel list (`state` has a `rivals` object). `hoshi season` lists the current UTC window and shipped rule sets. `hoshi season start <id> [--save PATH]` writes a seasonal save (default `season-<id>-<window>.sqlite` in the save dir) and prints the derived seed; an existing file exits 2. Shareable codes are grouped `XXXX-XXXX-XXXX-XX` with a checksum. Encode always includes the hyphens. Grouped input is a code; ungrouped digits are a raw `u64`; an ungrouped 14-character string with letters is a pasted compact code. JSON `--seed N` on the harness CLI stays a raw integer.

## Bot loop

```sh
save=bot.sqlite
hoshi --save "$save" --seed 1 state >/dev/null
hoshi --save "$save" build home metal_mine
hoshi --save "$save" tick --advance 180
hoshi --save "$save" state
```

Each write takes the save lock. A running TUI on the same file makes the CLI
exit 4 until it quits.

## End-to-end scenarios

Checked-in scripts under `crates/hoshi-tui/tests/e2e/<name>/` drive the `hoshi`
binary (`script.txt`, `expected.json`, optional fixture save). Adding a
scenario needs no Rust: drop a directory with those files next to the others.

Lines in `script.txt` are CLI arguments after `--save` / `--seed` (`build home
metal_mine`). Directives:

| Line | Meaning |
| --- | --- |
| `# expect N` | the following command must exit `N` (default 0) |
| `# restart` | process boundary; the runner also checks that one `tick` of the summed advance matches |
| `# lock` / `# unlock` | take or drop the save lock in the test process |
| `# fixture NAME` | copy `NAME` from the scenario dir or `tests/e2e/fixtures/` over the temp save |
| `# no-seed` | omit `--seed` (old live saves) |
| `# reset` | delete the temp save |
| `# snapshot NAME` | compare that step to `expected-NAME.json` |

Every JSON line is checked against `docs/schema/cli-v1.json`. A mismatch prints
the command and the JSON path. Golden files are pretty JSON from the CLI.
Review golden diffs in the PR; they are not regenerated in CI.

```sh
HOSHI_UPDATE_GOLDEN=1 cargo test -p hoshi --test e2e
```

Old-schema fixtures (`v1.sqlite` through the previous `SCHEMA_VERSION`, plus a
newer-than-supported save) come from `python3 scripts/gen-e2e-save-fixtures.py`.
See `crates/hoshi-tui/tests/e2e/fixtures/README.md`.

