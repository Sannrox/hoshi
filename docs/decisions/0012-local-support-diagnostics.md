# Local support diagnostics

Status: Accepted

Source: [Issue #232](https://github.com/Sannrox/hoshi/issues/232)

## Context

Support needs build, configuration, history and failure evidence without
sharing credentials or uploading a player's world by default. Crash capture
must survive process termination, and collection must not migrate a save.

## Decision

The TUI owns diagnostic collection. Core exposes read-only save diagnostics
and consistent SQLite snapshots; collection never writes gameplay state.
One redactor handles report text, rotating logs and crash files before those
texts reach disk. The report is previewed before publication to a new local
file. Save attachments require explicit archive selection and retain world
state, including the seed. No report is uploaded automatically.

The server exposes the shared Markdown builder through a separately scoped
admin endpoint. Authentication-free access requires a loopback listener and
peer. Operators can disable the endpoint. Ordinary game credentials do not
imply access to diagnostic configuration or logs.

## Consequences

Logs and crashes remain useful across restarts. Each report owns the previewed
bytes, so later state changes cannot silently alter its text. SQLite snapshots
include committed journal data without migration. Unsupported or damaged save
inspection is visible; explicit attachment failure prevents archive creation.
Redaction reduces disclosure but cannot classify arbitrary personal prose;
players retain the final review and sharing decision.

## Rejected alternatives

Automatic issue submission would make local collection a network trust
boundary. Including saves by default would disclose more than diagnostics
require. Separate redactors would allow log and crash behavior to diverge.
