# ADR 0020: Shard identity is a host tenant

- Status: Accepted
- Date: 2026-10-24
- Follows: [0005](0005-shared-shard-on-chisei.md),
  [0016](0016-device-pairing-and-sessions.md),
  [0017](0017-local-tui-may-join-a-chisei-shard.md)
- Source: [#389](https://github.com/Sannrox/hoshi/issues/389)

Issue #389 drafted this as 0019; [0019](0019-shard-raids-are-missions-with-flight.md)
was already allocated, so this record is 0020.

## Context

The TUI may join a shard (0017). Pairing files beside a local save
(0016) would tie a tenant to `save.sqlite` and fail the 0004 threat
table if copied. Two sessions on one identity without serialisation
duplicate fleets.

## Decision

1. **Shard identity is issued by the Chisei host**, not by hoshi and
   not by a serve device session. Multi-accounting controls stay on
   that issuer (0005).
2. **Join credentials never enter the local save.** They live in a
   separate file next to config, mode `0600`, same write-and-rename
   pattern as serve sessions. A missing file means no Join. Logout
   deletes that file. Continue / New never read it to load a
   `GameState`.
3. **Reconnect resumes the same tenant.** A second Join with the same
   identity does not mint a second empire.
4. **Two live sessions on one identity serialise Actions** on the
   host (0005). The second submit waits or is refused with a typed
   error; it does not last-write-wins. Reads of the tenant view may
   overlap.
5. **`hoshi serve` pairing is unchanged** and never authenticates to
   a shard.

## Consequences

- Join ([#383](https://github.com/Sannrox/hoshi/issues/383)) stores and
  sends this identity with every intent.
- Title-screen inbound ([#395](https://github.com/Sannrox/hoshi/issues/395))
  may read the credential file without opening `save.sqlite`.
- This ADR does not implement the credential file or Join.

## Rejected alternatives

- Reuse ADR 0016 device sessions as shard login. Wrong save scope.
- Put the token in SQLite. Copied saves steal the tenant.
- Allow two unsynchronised writers. Duplicated fleets; 0004 threat.
