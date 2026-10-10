---
name: assess-change-impact
description: Assess a proposed or implemented hoshi change across trust, API, dependency, and operations boundaries. Use when scoping an Issue, planning tests, reviewing a diff, or identifying compatibility, documentation, and security obligations.
---

<!-- Ported from Sannrox/shikigami@331897846885e4a55697b0f1006d03b4f2a7495e .agents/skills/assess-change-impact/SKILL.md (rewritten for hoshi; structure kept). Compare against the source to detect drift. -->

# Assess Change Impact

Build an evidence-backed impact map before implementation or review.

## Procedure

1. Read the linked Issue or request, `README.md`, accepted ADRs under
   `docs/decisions/`, and the relevant code. For a diff, inspect every changed
   file and its direct callers or implementors. Complete when the claimed
   outcome and actual change surface are both known.
2. Trace applicable boundaries:
   - game rules live in hoshi-core; the TUI and bridge never mutate state
     directly;
   - the save format (SQLite schema) and its migrations: old saves must load;
   - time: ticks and offline catch-up use an injectable clock and stay
     deterministic;
   - the bridge event format that agent tools and GitHub feed into the game;
   - balance config and its defaults;
   - pinned upstream versions: ratatui, crossterm, rusqlite;
3. Identify compatibility and failure obligations: save migration and bridge event
   version pins, partial runs (crash after output written but before it is
   reported, an adapter losing its claim mid-run), retries owned by the source, cleanup of scratch
   and processes, and the save database. Complete when data-loss,
   duplicate-write, and partial-failure paths are accounted for.
4. Map evidence to risk: unit tests for pure logic; clock-injected tests for
   timers and offline catch-up; migration tests for saves; fixture tests for
   bridge events; ignored live tests only when real GitHub access is essential. Complete when every material risk has a proposed check
   or an explicit residual uncertainty.
5. Determine durable artifacts that must change: README, docs, examples, an
   ADR, upstream Issues in the owning repository, or a repository Skill.
   Complete when no artifact is proposed merely to record temporary planning.

## Output

Return a compact matrix with columns:

| Surface | Evidence found | Required change/check | Risk if missed |
| --- | --- | --- | --- |

Then list scope boundaries, blocking questions, and the smallest safe PR split.
Do not approve an architecture, perform a full security audit, or claim
behavior parity without inspecting the implementations.
