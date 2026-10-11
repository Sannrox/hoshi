# Local agent activity feed

Optional. Off unless the `hoshi-tui` crate is built with `--features bridge` **and** `BridgeConfig.enabled` is true. The game does not open network connections or call models for this feed.

Events are line-delimited JSON in the state directory (`feed.jsonl` next to the save file, including when `--save` or `HOSHI_SAVE` is set). An optional Unix socket `feed.sock` in that directory accepts the same lines when the feed is enabled.

Enable the reader with `enabled = true` in `bridge.toml` next to the save, or by setting `HOSHI_BRIDGE`. The default is off.

## Line format

Each line is one JSON object:

| Field | Required | Meaning |
| --- | --- | --- |
| `type` | yes | `agent_started`, `agent_waiting_for_input`, `agent_finished`, `agent_failed` |
| `source` | yes | Who posted (for example `shell` or a hook name) |
| `ts` | yes | Unix timestamp in seconds |
| `payload` | no | JSON object. Never executed; never used as a filesystem path |

Unknown `type` values are ignored. Malformed lines are rejected and skipped. Lines longer than 8192 bytes are rejected.

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
