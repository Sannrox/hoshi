# ADR 0002: Real-time idle game in the terminal with an agent/GitHub event bridge

- Status: Accepted
- Date: 2026-10-10
- Supersedes: —

## Context

Waiting on coding agents and CI leaves short idle gaps. A browser game pulls
attention away from the terminal; a turn-based game demands too much. Classic
browser space-empire games fit idle play well: progress happens on timers, and
you check in, queue the next build and leave.

## Decision

- **Real-time idle model.** All progress is driven by wall-clock timers
  (production, build, research, fleet flights). The game advances while
  closed: on start it catches up the elapsed time in one deterministic step
  that must equal step-by-step ticking.
- **Terminal UI.** A ratatui binary that fits in a split pane, keyboard only.
- **Crate boundaries.** `hoshi-core` owns rules and state and takes an
  injectable clock; `hoshi-tui` renders and sends commands; `hoshi-bridge`
  converts outside events into game events. Only core writes state.
- **Local state in SQLite** (rusqlite, bundled), one file per player, with
  versioned migrations.
- **Standalone first.** hoshi must be a good game on its own. Every system is
  fully playable and balanced without the bridge.
- **Optional event bridge.** Behind the `bridge` cargo feature and a config
  switch, both off by default. It reads only free signals: GitHub events
  (merged PRs, CI results) through the user's own `gh` login or a webhook, and a
  generic local activity feed (file or socket) that any agent tool can write
  to. It never calls AI models or any paid service. Events map to small,
  capped game effects (payouts, boosts, raids) defined in balance config, and
  never unlock anything the normal game cannot.
- **Single player first.** A shared universe is optional research later.

## Consequences

- Deterministic core tests with a fake clock and seeded randomness.
- The bridge is off by default and compiled out unless enabled; the game
  works and is balanced without GitHub or any agent tool.
- Effects are capped so the bridge rewards activity without dominating play.

## Rejected alternatives

- **Turn-based play:** needs attention we do not have while agents run.
- **Browser or GUI client:** leaves the terminal.
- **Plain JSON save:** harder to migrate and to update incrementally.
