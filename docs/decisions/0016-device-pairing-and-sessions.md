# Device pairing and sessions

Status: Accepted. Shard identity is
[0020](0020-shard-identity-is-a-host-tenant.md), not this pairing file.

Source: [Issue #275](https://github.com/Sannrox/hoshi/issues/275)

## Context

A phone on the operator's network or tailnet should stay signed in to the
client that `hoshi serve` hosts. Until now a browser either carried the
operator's shared secret or a minted token that lived only in process memory.
A restart signed every browser out, and shutting out one lost phone meant
rotating the secret every other device used.

## Decision

`hoshi serve` keeps one **device session** per paired browser, scoped to the
save it serves.

- Sessions live in `<save>.devices.json` beside the save, mode `0600`,
  replaced by write-and-rename. They never enter the save. A rollback to a
  build without sessions ignores the file.
- Each session has a random 256-bit secret. Only its SHA-256 is stored; the
  secret appears once, in the response that creates the session.
- A session lasts 30 days after its last use. Use extends it, written at most
  once an hour. Expired sessions are refused and pruned.
- Label, device type, OS, browser and address come from the request
  (User-Agent, peer address), never from the client's own claims. They are for
  display and carry no authorization weight.
- A session authenticates `/v1` like a scoped bearer token: same save scope,
  same rate limits, never admin scope.
- An unreadable or malformed sessions file refuses every device session and is
  never overwritten, so the server fails closed instead of forgetting devices.

Follow-up work builds on this trust model:

- **Pairing.** The operator mints a single-use, short-lived code held in
  memory only; the browser exchanges it once for a new session. The code
  travels in a URL fragment, which browsers never send.
- **Transport.** A browser on the server's own origin carries its session in
  an `HttpOnly`, `SameSite=Lax` cookie that page scripts cannot read. Cookie
  writes require a JSON content type and WebSocket upgrades a matching
  `Origin`. Cookie and bearer are alternatives, never combined.
- **Management.** The admin token lists and revokes sessions; a device can
  revoke only its own.

## Consequences

Paired devices survive restarts, and one device can be shut out without
touching the others. A stolen sessions file reveals metadata but no usable
secret. The operator's shared secret stays for scripts and the CLI. The file
is a new piece of runtime state to back up with the save if devices should
stay paired after a move.

## Rejected alternatives

Storing sessions in the save would leak credentials into every copied or
reported save. Signed stateless tokens cannot be revoked one by one without a
deny list, which is durable state again. Accounts and shared universes stay out
of scope (ADR 0004). Shard Join credentials are a separate file
([0020](0020-shard-identity-is-a-host-tenant.md)) and never this session
file.
