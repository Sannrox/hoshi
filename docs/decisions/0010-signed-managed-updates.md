# ADR 0010: Signed metadata and managed updates

- Status: Accepted
- Date: 2026-10-20
- Source: [#227](https://github.com/Sannrox/hoshi/issues/227)

## Context

Release checks must stay optional and never delay play. An updater must establish
asset integrity independently of the release download, and a crash must leave a
usable installed executable. Saves can advance beyond an older binary's schema.

## Decision

1. A release envelope carries exact UTF-8 JSON payload bytes and an Ed25519
   signature. Release builds compile the public key; signing credentials exist
   only in CI. The same key signs the checksum file. Verification fails closed.
2. Interactive checks run in a background worker with one bounded GET. An atomic
   cache and process-shared attempt lock enforce the daily interval. A separate
   short state lock preserves dismissals during an in-flight response. Harness
   commands do not start checks; servers require explicit opt-in.
3. Managed installs stage and verify a target binary before publishing a single
   atomic pointer containing the active and previous release generations. A
   process-shared update lock fences concurrent installers. Retained directories
   are immutable; abandoned staging is disposable. Rollback verifies retained
   metadata before publishing the previous generation.
4. Core owns read-only schema inspection and SQLite snapshots. Updates check
   compatibility and retain backups before changing the installation. Optional
   backend saves require manual compatibility checks and backup.
5. Config saves preserve unknown keys and sections. Update UI state stays beside
   config, outside game saves. Build and contract metadata are additive server
   fields; update events stay outside game history.

## Consequences

Automatic checks can fail silently without stopping play. Explicit updates show
verification and compatibility failures. Mirrors must preserve signed bytes.
Key rotation needs a separate trust transition. CI cannot publish unsigned
metadata when the release key is missing. Installation changes remain separate
from save migration, which happens only when a newer game opens a save.

## Rejected alternatives

- Checksums fetched beside unsigned assets do not authenticate a release.
- Overwriting a running executable prevents reliable rollback and crash recovery.
- Putting update notices into game history would mix installation state with
  deterministic game events.
