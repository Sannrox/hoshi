# Bot arena

Local bots play the same galaxy seed through the JSON CLI. The game never
starts an arena during normal play, and it never calls a model.

Off until `arena = true` in `config.toml` or `--arena`. Then:

```sh
hoshi --arena arena run --seed 7 --days 3 examples/bots/greedy.sh examples/bots/greedy.py
hoshi --arena arena board
```

`--days` is required. `--actions` (default 32) and `--timeout-ms` (default 2000)
cap each bot. `--board` defaults to `arena-board.json` beside the save.

Each bot is a subprocess. It may read one JSON hint line on stdin (`day`,
`days`, `actions`, `seed`) and must print JSON command lines:

```json
{"cmd":["build","home","metal_mine"]}
```

The runner executes those commands against an isolated deterministic save for
that bot. Default saves are SQLite. `--backend chisei` or `chisei = true` in
`config.toml` (both off by default) uses a `*.chisei` directory when the binary
is built with `--features chisei`; without the feature that target is refused.
Bots cannot tick the clock or point `--save` at another file. A crash,
timeout, or invalid command zeros that bot's score and leaves the others
running.

## Scoring (version 1)

`scoring_version` in the JSON result and leaderboard is `1`.

```
score = prestige
      + 10 × metal_mine
      + 5 × crystal_mine
      + 3 × solar_plant
      + 50 × extra_colonies
      + metal / 1000
```

Failed bots (`crash`, `timeout`, `invalid`) score `0`. Efficiency is
`score / days`. Victory is `1` when a prestige reset is eligible, otherwise
`min(0.99, score / 10000)`.

The leaderboard JSON appends each run (seed, days, scoring version, bot rows).
`hoshi arena board` prints the same table as markdown for sharing. Bump
`scoring_version` when the formula changes; each stored run keeps the version
it was written with.
