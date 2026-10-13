# Playtest loop

A short session should feel like a game, not a checklist. Run this after a
fresh install (or `cargo run -p hoshi-tui`) and again after leaving the save
alone for hours.

## Checklist

1. **First launch.** Open the title screen, start a new game, skip or keep the
   opening hint. The overview shows mines ticking.
2. **First build.** On the planet screen, queue a metal mine upgrade. Confirm it
   completes after an offline gap (quit, wait, continue) or with
   `hoshi --save PATH --seed N tick --advance 30m` on a harness save.
3. **First research.** Build a research lab, start energy research from the
   overview. The lab should still be working after a restart.
4. **A raid survived.** Wait for the overview countdown or advance a harness
   save until pirates land. Open the combat report (`r` from the overview).
   Three basic ships (light fighter, small cargo, colony ship) and the shipyard
   are enough to fight back.
5. **Leave and come back after hours.** Quit. Reopen the same slot. Production
   and any finished builds or research should have caught up; nothing should
   error.
6. **Load the save the next day.** Continue from the title screen (or
   `hoshi --continue`). The slot loads; the away summary lists deliveries,
   stalls and completed items, and can be dismissed with `x`.
7. **History log.** Tab to the log screen (or `hoshi log --type build`). A
   finished mine upgrade should show queue then complete, with lineage on
   Enter. Time travel (`r` / `hoshi log --at`) matches the stockpile recorded
   at that event.
8. **Staff.** Tab to the staff screen (or `hoshi staff`). Home has an admiral
   and a governor. Recruiting spends resources; assigning a second role waits
   out the delay; a delegated governor can queue a build with `build --as`.

## Questions

Answer these every round. They are also the GitHub "Playtest feedback" template.

**Fun**

- What made you come back?
- What felt like a chore?

**Pacing**

- How long until the first meaningful choice?
- When were you waiting versus doing?

**Clarity**

- What was confusing?

## Capture

```sh
hoshi --save PATH feedback mines felt slow after the first hour
```

The command appends a timestamped JSON line (game version, optional save
glance: schema, last tick, planet count, fleet strength; never the galaxy seed)
to `PATH` with the `.feedback.log` extension. A missing or locked save still
records the note and version. Do not commit the log.

File follow-ups with the Playtest feedback issue template. Link them from the
round notes on the playtest issue or pull request.
