# E2E save fixtures

Old-schema sqlite files used by `crates/hoshi-tui/tests/e2e`. Regenerated from
the repository root:

```sh
python3 scripts/gen-e2e-save-fixtures.py
```

The script reads `SCHEMA_VERSION` from `crates/hoshi-core/src/save.rs` and
writes:

- `v1.sqlite` … `v{SCHEMA_VERSION-1}.sqlite` — one save per version we still
  migrate, with a last-tick stamp in the future so a live `state` command is a
  catch-up no-op
- `too_new.sqlite` — `PRAGMA user_version = 999`
- `expedition.sqlite` — schema v9, deterministic, with fighters and deuterium
- `build_research.sqlite` — schema v9, deterministic, with a lab and enough stockpile to queue energy research

When a new save schema lands, rerun the script and keep the new `vN.sqlite`.
Do not hand-edit the binaries.
