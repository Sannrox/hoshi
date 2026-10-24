# Story event authoring

Writers draft events offline. The game never generates events at runtime and
never calls a model.

Shipped events live in `crates/hoshi-core/data/story.toml` and load through
`StoryCatalog`. Drafts live in `content/drafts/` and are gitignored. CI fails
if a game crate (`hoshi-core`, `hoshi-tui`, `hoshi-bridge`) references that
directory.

## Draft

Skeleton mode needs no model:

```sh
cargo run -p hoshi-author -- events draft --theme salvage --count 3
```

Writes `content/drafts/salvage.toml`. Each event passes the same loader as
shipped content (`StoryCatalog::parse`). Timing fields in the draft file are
placeholders; only the `[[event]]` blocks move into `story.toml`.

Optional local model (loopback HTTP, off unless you pass it):

```sh
cargo run -p hoshi-author -- events draft --theme salvage --count 3 \
  --endpoint http://127.0.0.1:11434/v1 --model llama
```

`--endpoint` can also come from `author.toml` next to the working directory:

```toml
endpoint = "http://127.0.0.1:11434/v1"
model = "llama"
allow_lan = false
```

Public hosts are refused. A private IPv4 URL needs `--allow-lan`. If the
endpoint is down or the reply fails validation, the tool writes the skeleton
instead. The game binary ignores this config.

## Review

1. Edit the draft: unique `id`, player-facing `title` / `text`, two or more
   `[[event.choices]]`, and a `timeout_choice` that names one of them.
2. `StoryCatalog::parse` must succeed. Invalid TOML or unknown effect keys
   are rejected; fix the file, do not patch the loader.
3. Copy the reviewed `[[event]]` blocks into `crates/hoshi-core/data/story.toml`.
   Do not include draft files from `content/drafts/` in any `include_str!`.
4. Playtest the new ids in a throwaway save (`docs/playtest.md`).
5. Leave `content/drafts/` out of the commit unless you are documenting a
   skeleton; generated `*.toml` drafts are gitignored.

## Flags

A choice may `sets` flag ids. An event may `requires` every listed flag,
`requires_any` one of them, and `forbids` any of them. A `[[flag]]` row may
attach a `note` to an id a choice sets. The bridge shows the latest of those
notes until the next arc event is pending or `lane_closed` is set. The
original events leave these keys empty and stay in the weighted draw.
