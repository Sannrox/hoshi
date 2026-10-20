# ADR 0008: Human help from the command catalog

- Status: Accepted
- Date: 2026-10-20
- Source: [#228](https://github.com/Sannrox/hoshi/issues/228)

## Context

The binary has separate launch, server, and JSON command parsers. Human help
must cover these entry points without changing the JSON action contract or
opening a save merely to discover a command.

## Decision

1. Route `--help`, `-h`, and `help` before save access or action parsing.
   Help is human text with exit 0; an unknown topic or export format exits 2.
2. Declare JSON command help alongside each `Command` variant. Export one
   catalog, adding launch topics, flags, and environment descriptions there.
   Existing parsers continue to own execution.
3. Generate the man page, shell completions, and command reference from that
   catalog. CI checks generated-file drift and parser-valid examples.
4. The existing TUI help dialog has Keys and Commands pages. Command examples
   come from the catalog; help never dispatches those actions.

## Consequences

Adding a command requires help metadata. Human help copy may evolve while
JSON responses and exit codes retain their existing contract. Generated files
are committed so cross-target packaging does not need to execute the binary.

## Rejected alternatives

- Independent help lists for every output would preserve documentation drift.
- Replacing the parsers would enlarge the change without improving the
  existing action and save boundaries.
