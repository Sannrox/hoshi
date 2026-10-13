# Scriptable CLI

Harnesses and bots play through the same rules as the TUI. Every command prints
JSON with `schema_version` (see `docs/schema/cli-v1.json`). A breaking change
bumps that version.

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
hoshi --save PATH [--seed N] [--balance PATH] state [--json]
hoshi --save PATH advise
hoshi --save PATH log [--type KIND] [--actor ID] [--object ID] [--from UNIX] [--to UNIX] [--lineage ID] [--at UNIX] [--export PATH]
hoshi --save PATH staff
hoshi --save PATH vehicle
hoshi --save PATH vehicle register <planet> <ships...>
hoshi --save PATH vehicle dissolve <id>
hoshi --save PATH mission windows <from> <to> [--transfer hohmann|high_energy|assist]
hoshi --save PATH mission plan <to> --vehicle ID [--transfer hohmann|high_energy|assist] [--customer colony|faction:ID] [--metal N] [--crystal N] [--deuterium N] [--window N]
hoshi --save PATH mission schedule
hoshi --save PATH staff recruit [--name TEXT] [--trait ID]
hoshi --save PATH staff assign <id> admiral|governor|minister [--target home|empire|g:s:p]
hoshi --save PATH staff delegate <id> [--permission queue_build|accept_battle|propose_law] [--cap N]
hoshi --save PATH staff revoke <id> [--permission queue_build|accept_battle|propose_law]
hoshi --save PATH build <planet> <building> [--as player|leader:ID]
hoshi --save PATH research <tech>
hoshi --save PATH cancel <queue-item>
hoshi --save PATH fleet send <from> <to> <mission> <ships...>
hoshi --save PATH tick --advance <dur>
hoshi --save PATH prestige
hoshi --save PATH prestige buy <id>
hoshi --save PATH prestige reset
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
hoshi [--save PATH] --arena arena run --seed N --days N [--actions N] [--timeout-ms N] [--board PATH] bot...
hoshi [--save PATH] --arena arena board [--board PATH]
```

Planet is `home` or `galaxy:system:position`. Building and ship ids match the
config keys (`metal_mine`, `light_fighter`). Queue item is `0`, `build:0`, or
`research`. Mission is `attack`, `invade`, `colonize`, `expedition`, or `spy`. Ships look like
`light_fighter:2`. `attack` fights an outpost's defences; `invade` lands troop
transports and is refused while defences remain. `spy` sends espionage probes and
writes a report; `state` lists reports with age and attaches the latest report to
each outpost so attack and invade can read it. `colonize` takes exactly `colony_ship:1`. `moon build` spends stockpile on a phalanx or jump gate at that planet's moon. `moon jump` moves parked ships instantly between two moons that both have gates. `moon scan` lists fleets the phalanx can see. Extra arguments
on `state`, `build`, `research`, and `cancel` are invalid. Duration is seconds,
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

`vehicle register` assigns parked ships to a named vehicle. `mission windows` lists synodic launch windows with delta-v and coast time. `mission plan` is PlanMission (propose, check, apply): payload mass changes fuel through the rocket equation; `--window N` can book an upcoming window (the vehicle launches when it opens). A missed window or a double-booked vehicle is refused. `mission schedule` lists vehicles, in-flight plans, and double-book / missed-window alerts. Transfer is `hohmann`, `high_energy`, or `assist`. Customer is `colony` (owned dest, cargo credited there) or `faction:<id>` (cargo is delivered to the faction and standing moves by that faction's trade delta); `market` is refused.

`staff` lists leaders, roles, assignments and delegations. `staff recruit` spends the configured cost. `staff assign` places one role per leader (reassignment waits `reassign_secs`). `staff delegate` lets that leader act with an optional spending cap; `staff revoke` removes it. `build --as leader:ID` runs the same queue checks under that leader's permissions.

`--seed` on a new save starts deterministic mode: the wall clock is ignored and
`tick --advance` is allowed. Live saves refuse `tick --advance`. The TUI refuses
a deterministic save so wall-clock catch-up cannot run on it. JSON never
includes RNG state or galaxy seed. Unknown commands and CLI-only flags print
JSON with exit 2; they do not open the TUI. `expedition` dest must be the
configured expedition slot.

`arena run` is off until `arena = true` in `config.toml` or `--arena`. Each bot is a subprocess that prints JSON CLI commands (`{"cmd":["build","home","metal_mine"]}`) against its own deterministic save. Crash, timeout and invalid actions zero that bot and leave the others running. `arena board` prints the appended JSON leaderboard as markdown. Scoring is versioned in [docs/arena.md](arena.md). Sample bots live in `examples/bots/`.

`prestige` prints score, currency and owned bonuses. `prestige buy` spends unspent currency. `prestige reset` is refused until a wonder is complete or score reaches the configured minimum; it keeps bonuses and run history. `state` includes a `leaders` array (traits, assignment, retirement, delegations), a `waste` object (heat and pollution load versus sink, unrest, and the warning flag), a `laws` array (status, approvals, latest expected vs measured samples), a `daily` object (streak, time until reset, and slot progress), and a `tutorial` object (enabled, skipped, pending hint). `tutorial skip` hides remaining hints; `tutorial reset` clears seen ids and shows the opening again. `tactics set` stores the home fleet's formation, stance and auto-retreat threshold; `tactics action` is refused unless a live fight is waiting. `state` includes a `tactics` object. `feedback` appends a timestamped JSON line (version and a save glance, never the galaxy seed) to a `.feedback.log` beside the save; a missing or locked save still records the note. See `docs/playtest.md`. `law propose` writes a proposal; `law approve` records the player approval and enacts when the effective date is due; `law repeal` starts the same flow in reverse and restores prior parameters.

`--save` / `HOSHI_SAVE` and `--balance` / `HOSHI_BALANCE` match the TUI. `hoshi` with no subcommand opens the title screen. `--save PATH` still opens that file directly. `hoshi --continue` resumes the last slot without the menu; locked slots exit 4, corrupt or newer-schema or harness slots exit 5. `hoshi new --seed <code>` writes a new live save (default slot, `--save PATH`, or `HOSHI_SAVE`) and prints the canonical grouped seed code; an existing file exits 2. Shareable codes are grouped `XXXX-XXXX-XXXX-XX` with a checksum. Encode always includes the hyphens. Grouped input is a code; ungrouped digits are a raw `u64`; an ungrouped 14-character string with letters is a pasted compact code. JSON `--seed N` on the harness CLI stays a raw integer.

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
HOSHI_UPDATE_GOLDEN=1 cargo test -p hoshi-tui --test e2e
```

Old-schema fixtures (`v1.sqlite` through the previous `SCHEMA_VERSION`, plus a
newer-than-supported save) come from `python3 scripts/gen-e2e-save-fixtures.py`.
See `crates/hoshi-tui/tests/e2e/fixtures/README.md`.

