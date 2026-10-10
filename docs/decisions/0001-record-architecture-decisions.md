# ADR 0001: Record architecture decisions

- Status: Accepted
- Date: 2026-10-10
- Supersedes: —

## Context

hoshi has a few choices that are costly to reverse: the save format, the time
model, and the event format other tools post to.

## Decision

Record architecture decisions as numbered Markdown files in `docs/decisions/`,
with status, date, context, decision, consequences, and rejected alternatives.
Write one when a choice changes the save schema, the tick model, the bridge
event format, or the crate boundaries.

## Consequences

Each such change links an ADR. Superseded ADRs stay and name their successor.
