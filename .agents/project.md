# Project profile for agent-kit skills

<!-- Read by the shared skills in https://github.com/Sannrox/agent-kit (pinned in .agents/agent-kit.lock). Keep repo-specific facts here, not in the shared skills. Section headings are a contract: do not rename them. -->

## Project

- Name: hoshi
- Feature means: a new observable gameplay or integration outcome

## Ownership boundaries

Hoshi-core owns game rules and state; the TUI only renders and sends commands; the bridge only turns outside events into game events.

Route work owned by another repository (a runner's job contract, sandbox enforcement) to that repository.

## Affected areas

core/tui/bridge/balance/server

## Shared surfaces

Issues touching the same shared surface run in sequence, never as parallel lanes:
the game state schema and save format, the tick/offline-progression model, the bridge event format, and balance config.

## Impact boundaries

- game rules live in hoshi-core; the TUI and bridge never mutate state directly;
- the save format (SQLite schema) and its migrations: old saves must load;
- time: ticks and offline catch-up use an injectable clock and stay deterministic;
- the bridge event format that agent tools and GitHub feed into the game;
- balance config and its defaults;
- pinned upstream versions: ratatui, crossterm, rusqlite;

## Compatibility and failure obligations

Save format migrations and bridge event versions; a crash mid-tick or mid-write must leave the save database consistent (transactions, no half-applied ticks); offline catch-up after long gaps; bridge events that arrive twice or out of order.

## Evidence map

Unit tests for pure logic; clock-injected tests for timers and offline catch-up; migration tests for saves; fixture tests for bridge events; ignored live tests only when real GitHub access is essential.

## Verification matrix

| Surface | Required evidence |
| --- | --- |
| game rules, timers, offline progression | deterministic tests with an injected clock; catch-up after long offline gaps matches step-by-step ticking |
| TUI rendering and input | ratatui buffer snapshot tests for affected views; key bindings covered |
| save format, bridge events | SQLite migration tests from the previous schema; bridge events parsed from fixtures, malformed input rejected |
| dependency pins (ratatui, rusqlite) | `Cargo.lock` diff reviewed; the pinned tag exists upstream; upstream release notes read |
| balance config | defaults load; property tests that production and costs stay within the documented curves |

## Release audit

- workspace `Cargo.toml` version and lockfile consistency;
- pinned ratatui and rusqlite versions, and their release notes;
- save format migrations, bridge event format and balance changes;
- configuration additions, removals, and defaults;
- balance changes and their effect on existing saves;
- packaging, rollback, and upgrade actions.
