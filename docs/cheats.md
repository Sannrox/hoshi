# Local cheats

Cheats are off by default. Enable **cheats** in title Settings, set `cheats = true`
in the config beside your save, or launch with `--cheats`. The deterministic
CLI harness also enables them. Enabling alone does not modify a save.

On any play screen (the bridge included), press backtick to open the console;
with cheats off it says "cheats are disabled". Remap `cheat_console` in
keybindings if needed. Type one command and press Enter; Escape closes it.
The CLI uses the same core action:

```sh
hoshi --cheats cheat grant iron 1000
hoshi --seed 1 cheat skip 60
```

| Code | Effect |
| --- | --- |
| `grant RESOURCE AMOUNT` | Add a positive finite amount to home stock. Resource names match `state`. |
| `finish-build` | Complete the current building job on each owned planet. Queued jobs continue normally. |
| `finish-research` | Complete the current research job. |
| `reveal` | Reveal all configured frontier sectors. |
| `god on` / `god off` | Protect player fleets and planets from damage. |
| `skip SECONDS` | Resolve up to one year of simulation in ordinary event order. |

Successful use permanently marks the save **MODIFIED**. Turning cheats off or
prestiging cannot clear that marker. Modified saves earn no new achievements,
contribute nothing to shared career progression, and cannot enter server mode,
a shared universe, or arena scoring. Servers never accept cheat actions.
Existing achievements remain visible inside that save.

Time advancement persists an offset from the injected wall clock. Play continues
immediately after a skip, including after restarting. Instant completion advances
one second through the normal completion path.

Enabling, disabling and successful commands record actor, arguments and game time
in the save. `state` exposes `cheat_audit`; `log --export PATH` includes durable
`cheat_audit` records alongside ordinary history. No command is interpreted while
cheats are disabled, and refused commands leave gameplay unchanged.

Saves migrate to schema 48. Older binaries refuse that schema. The modified
marker and audit are local integrity rules, not protection against someone
manually rewriting a database.
