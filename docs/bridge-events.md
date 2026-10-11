# Local agent activity feed

Optional. Off unless the `hoshi-tui` crate is built with `--features bridge` **and** `BridgeConfig.enabled` is true. The game does not open network connections or call models for this feed.

Events are line-delimited JSON in the state directory (`feed.jsonl` next to the save file, including when `--save` or `HOSHI_SAVE` is set). An optional Unix socket `feed.sock` in that directory accepts the same lines when the feed is enabled.

Enable the reader with `enabled = true` in `bridge.toml` next to the save, or by setting `HOSHI_BRIDGE`. The default is off.

```toml
enabled = true
github_repo = "owner/name"
poll_secs = 60
webhook_bind = "127.0.0.1:8765"
```

## Line format

Each line is one JSON object:

| Field | Required | Meaning |
| --- | --- | --- |
| `type` | yes | `agent_started`, `agent_waiting_for_input`, `agent_finished`, `agent_failed` |
| `source` | yes | Who posted (for example `shell` or a hook name) |
| `ts` | yes | Unix timestamp in seconds |
| `payload` | no | JSON object. Never executed; never used as a filesystem path |

Unknown `type` values are ignored. Malformed lines are rejected and skipped. Lines longer than 8192 bytes are rejected.

GitHub merged pull requests and finished workflow runs use `pr_merged` and `workflow_finished` with `source` `github`. They enter the same stream. The reader polls `gh api` for each `github_repo` line (ETag / 304) and follows full pages (`per_page=30`, then `page=2` …) until a short page or ten pages, whichever comes first. New events land on page 1; the ten-page cap is a burst window, not a full-history backfill. It can bind `webhook_bind` for HTTP POST on a loopback address only; a bind failure stays in status and the rest of the bridge still opens. Dedup ids live in `github-seen.json` next to the save. No token is stored. Polling is off when the bridge is off or no repos are listed. If `github_repo` lines are set, webhook events for other repos are dropped.

## Shell

```sh
echo '{"type":"agent_started","source":"shell","ts":1700000000,"payload":{"agent":"local"}}' >> "$(dirname "$HOSHI_SAVE")/feed.jsonl"
```

`HOSHI_SAVE` is the save file; the feed sits next to it. If the path is unset, use the same directory as the default save.

## CLI

Build with the feature, then:

```sh
cargo run -p hoshi-tui --features bridge -- post agent_started shell '{"agent":"local"}'
cargo run -p hoshi-tui --features bridge -- post --save /path/to/save.sqlite --ts 1700000000 agent_finished hook '{"code":"0"}'
```

`hoshi post` appends one valid line. It does not enable the reader; the reader stays off until config sets `enabled`.

## Restart

The reader stores a byte offset in `feed.jsonl.offset` and continues from there after a restart.

## Game effects

When the TUI is built with `--features bridge` and the feed is enabled, accepted events apply at their timestamp during catch-up:

| Event | Effect |
| --- | --- |
| `pr_merged` | Resource payout of configured hours of current production |
| `workflow_finished` with `conclusion` `success` | Temporary production boost |
| `workflow_finished` otherwise | Raid via `GameState::trigger_raid` |
| `agent_finished` with `duration_secs` (or `elapsed`) at or above the threshold | Temporary research speed boost |
| `agent_waiting_for_input` with `waiting_secs` at or above the threshold | Raid |

Each mapping is individually disableable and has a cooldown and a daily cap. `agent_started` and `agent_failed` do not grant effects.
