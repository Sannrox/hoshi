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
hoshi --save PATH build <planet> <building>
hoshi --save PATH research <tech>
hoshi --save PATH cancel <queue-item>
hoshi --save PATH fleet send <from> <to> <mission> <ships...>
hoshi --save PATH tick --advance <dur>
hoshi --save PATH prestige
hoshi --save PATH prestige buy <id>
hoshi --save PATH prestige reset
```

Planet is `home` or `galaxy:system:position`. Building and ship ids match the
config keys (`metal_mine`, `light_fighter`). Queue item is `0`, `build:0`, or
`research`. Mission is `attack`, `invade`, `colonize`, `expedition`, or `spy`. Ships look like
`light_fighter:2`. `attack` fights an outpost's defences; `invade` lands troop
transports and is refused while defences remain. `spy` sends espionage probes and
writes a report; `state` lists reports with age and attaches the latest report to
each outpost so attack and invade can read it. `colonize` takes exactly `colony_ship:1`. Extra arguments
on `state`, `build`, `research`, and `cancel` are invalid. Duration is seconds,
or `30s` / `3m` / `1h` / `1d`.

`--seed` on a new save starts deterministic mode: the wall clock is ignored and
`tick --advance` is allowed. Live saves refuse `tick --advance`. The TUI refuses
a deterministic save so wall-clock catch-up cannot run on it. JSON never
includes RNG state or galaxy seed. Unknown commands and CLI-only flags print
JSON with exit 2; they do not open the TUI. `expedition` dest must be the
configured expedition slot.

`prestige` prints score, currency and owned bonuses. `prestige buy` spends unspent currency. `prestige reset` is refused until a wonder is complete or score reaches the configured minimum; it keeps bonuses and run history.

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
