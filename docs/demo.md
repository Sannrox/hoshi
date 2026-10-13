# Governance demo

Side-by-side ungoverned vs governed AI players on the same galaxy seed. Off
until you build with `--features chisei` **and** pass `--demo` or set
`demo = true` in `config.toml`. Normal play never starts it, and the game
never calls a model.

```sh
cargo build -p hoshi-tui --features chisei
hoshi --demo demo governance --seed 7
```

`--seed` is required and repeats the report for the same seed. `--report PATH`
also writes the markdown. Model replies are **recorded** (`routine:` / `strategic:`
plus the intent name) so CI stays deterministic. The demo does not open a
socket and does not store credentials. Cheap vs expensive names still appear
in the audit so routing savings are measurable.

`config.toml`:

```toml
demo = false
governance = false
```

`governance = true` is the TUI switch: launching troop transports at an
undefended outpost asks for invasion approval. Headless runs auto-deny.

## Players

**Ungoverned.** Applies the scripted move through the same propose/apply path
as the CLI. It sees every planet and outpost the harness can name, including
worlds fog of war would hide. There is no policy check and no audit trail.

**Governed.** Runs on a `.chisei` save. Each move is a model call (cheap for
routine build/research, expensive for launch, invasion and wonder research),
then `propose`, then policies, then `apply`. Visibility follows Chisei
markings (own worlds, `allied-with`, spy `cleared-for`). Every proposal,
block and halt is appended to an in-memory audit.

## Policies

| Rule | When it fires |
| --- | --- |
| `spend-cap-turn` / `spend-cap-day` | Metal + crystal + deuterium spent this turn or day |
| `no-attack-ally` | Launch, colonize or invade an allied planet |
| `invasion-approval` | Invade without a human grant (auto-deny when headless) |
| `token-budget` | Token or model-call cap; the player idles and does not overspend |

A policy block names the rule and the inputs that triggered it.

## Scenarios

The command always runs three scripted scenarios:

1. **alliance-betrayal** — both players can reach an allied world. Ungoverned
   launches. Governed is blocked by `no-attack-ally`.
2. **treasury-drain** — ungoverned spends the stockpile. Governed hits
   `spend-cap-turn`, then a second move hits the token hard stop.
3. **unapproved-invasion** — ungoverned invades an outpost. Governed is
   auto-denied (`invasion-approval`, mode `headless`).

JSON includes outcomes, blocks, an audit excerpt, `cost_per_win`,
`tokens_per_move` and `routing_savings` (tokens that routing avoided versus
using the expensive model for every move). Markdown repeats that table.

```sh
cargo test -p hoshi-tui --features chisei demo
```
