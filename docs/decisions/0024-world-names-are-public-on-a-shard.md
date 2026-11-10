# ADR 0024: World names are public on a shard

- Status: Accepted
- Date: 2026-11-10
- Source: [#507](https://github.com/Sannrox/hoshi/issues/507)

## Context

Home and colonies showed only as coordinates. Players can now name each
world they hold, and every surface shows `Name (1:3:5)`. On a shard, other
tenants already see which coords are occupied. A name is player text that
reaches other players' terminals, so it is both an intel leak and an input
to every renderer.

## Decision

1. A world name is public wherever its coord is already visible: a shard
   occupancy slot carries the holder's label, and a raid report names the
   world it hit as the defender called it then. No new visibility is
   granted. An inbound raid still names no attacker.
2. Home shows `Home` until renamed, and that default is public too.
3. Core validates every name before it is stored or shown: at rename, on
   save load, and before a host hands one to another tenant. A name is
   trimmed, 1 to 24 terminal columns (wide characters count two), at most
   48 characters, and has no control characters (ANSI escapes, newlines),
   bidi controls or zero-width characters. A name loaded from a save that fails the check
   is dropped, and the world shows its default label.
4. No uniqueness, moderation or rate limit. The coord disambiguates.

## Consequences

- Rivals can link worlds that share a naming pattern, and spot a home that
  kept `Home`, which before needed a spy mission. The player chooses what
  to reveal by naming; the commands reference says so.
- Every renderer can print a name as-is. A later change that relaxes the
  character rules must keep them in core, not per surface.
- Schema 67 adds the `world_names` table and the raid log's `place_name`.
  Older binaries refuse the new save, as for every bump.
