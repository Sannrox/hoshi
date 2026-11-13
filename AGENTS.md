# Repository Guidelines

<!-- Structure and shared policy ported from Sannrox/shikigami@331897846885e4a55697b0f1006d03b4f2a7495e AGENTS.md (adapted for hoshi). Compare against the source to detect drift. -->

## Project Structure & Module Organization

`hoshi` is a Rust 2024 workspace for a real-time idle space-strategy game that
runs in the terminal, meant to be played while waiting for coding agents. The
design is recorded in [ADR 0002](docs/decisions/0002-terminal-idle-game-with-event-bridge.md).

- `crates/hoshi-core`: game rules, state, timers and offline progression. No
  terminal, network, or database code.
- `crates/hoshi-storage`: save backends (SQLite default; optional Mikura /
  Chisei). The TUI persists through this crate; core never opens a database.
- `crates/hoshi-tui`: the ratatui binary. Renders state and sends commands to
  core; holds no rules.
- `crates/hoshi-bridge`: turns outside events (GitHub, agent activity) into
  game events. Resolves feed paths itself; no workspace crate dependency.
- `crates/hoshi-author`: offline `hoshi-author` CLI that drafts story events
  into `content/drafts/`. The game never depends on it or generates events at
  runtime.
- `crates/hoshi-connector`: optional Chisei shard host. Claims `external_mutate`
  writeback, calls `ShardSession`, and posts signed tenant-view webhooks.
  Depends on `sekai-source-sdk` (git tag `v3.0.0`). Not a game server; sockets
  stay out of hoshi-core.

Do not commit local state (`.state/`, `*.db` saves), logs, or tokens.

Human documentation: [README.md](README.md). Architecture decisions:
[docs/decisions/](docs/decisions/README.md). Design and balance follow
[docs/realism.md](docs/realism.md) (physics and maths only; flavour stays
fictional). `CLAUDE.md` only points here; this file is canonical.

## Dependencies and Ownership

- **hoshi-core is the authority** for rules and state. The TUI calls core APIs
  and persists through `hoshi-storage`. The bridge never writes the save.
- **Time is injected.** Core takes a clock so ticks and offline catch-up are
  deterministic in tests.
- External crates: ratatui and crossterm for the TUI, rusqlite (bundled) in
  `hoshi-storage` for the default save. Optional `mikura` (git tag `v0.2.0`) is
  a second backend behind `--features mikura`. Optional `chisei` (API pin
  1.3.0, off in `config.toml`) is a third backend behind `--features chisei`.
  The bridge uses the `gh` CLI or webhooks; it never stores tokens.

Record cross-repository predecessors in the Issue's `## GitHub dependencies`
section as full `<owner>/<repo>#<n>` references.

## Build, Test, and Development Commands

The compiler is pinned in `rust-toolchain.toml`. Local gates match CI
(`.github/workflows/ci.yml`):

```sh
cargo fmt --all -- --check
cargo build --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
bash scripts/validate-workflow-skills.sh
```

- `cargo fmt --all` formats before review.
- `cargo clippy ... -D warnings` is required for ship-level local gates.
- `scripts/validate-workflow-skills.sh` checks the shared Skill stubs and the
  contract headings of this file against agent-kit `main`.

There is no Makefile yet. When one is added, its targets become the source of
truth for these commands and this section must point to them.

GitHub Issues are the planning source of truth. Read `README.md`,
[docs/realism.md](docs/realism.md) and accepted ADRs under
`docs/decisions/` before changing system boundaries or balance numbers;
record new boundaries, trust-model changes, and public contracts as ADRs
with the `capture-project-decision` Skill. New design Issues name object
types, links and core actions, and they either obey the realism rules or
extend the simplifications list in that file.

## Shared agent skills

The workflow Skills (`advance-issue-frontier`, `assess-change-impact`,
`capture-project-decision`, `deliver-ready-issue`, `prepare-release`,
`refactor-docs`, `shape-work-item`, `technical-documentation`, `verify-change`)
are canonical in [Sannrox/agent-kit](https://github.com/Sannrox/agent-kit).
There is no pin: this repository always uses the latest `main`.
`.agents/skills/<name>/` holds only a generated stub that points to the shared
skill; read the installed skill under `~/.agents/agent-kit/` (or
`$AGENT_KIT_HOME`). Run `bash scripts/validate-workflow-skills.sh` to clone or
update it.

- Repository-specific facts the shared Skills need come from this file: the
  ownership, structure, build, parallel-lane and testing sections, plus
  `## Agent skill profile` below for anything they do not already say. Edit
  this file, not the stubs.
- Never edit the stubs or the vendored `scripts/validate-workflow-skills.sh`
  here; change them upstream in agent-kit, then run
  `bash scripts/validate-workflow-skills.sh --render` here when CI reports
  drift.
- Repository-only Skills live next to the stubs as full folders and stay local.

## Agent skill profile

Facts the shared agent-kit skills read that the sections above do not already
state.

Feature: a new observable player or operator outcome.

### Compatibility and failure obligations

- save schema and migrations: old saves must load; older binaries cannot open
  a newer schema;
- the bridge event format that agent tools and GitHub feed into the game;
- ticks and offline catch-up stay deterministic on the injected clock;
- optional `mikura` git tag `v0.2.0` and `chisei` API pin 1.3.0;
- optional shard host connector pin: `sekai-source-sdk` git tag `v3.0.0`;
- a crash or restart must not corrupt the save.

### Additional impact boundaries

- the `/v1` HTTP and WebSocket contract `hoshi serve` publishes
  (`docs/schema/`);
- signed managed-update metadata (ADR 0010);
- portrait assets and the human help catalog;
- the optional `ui` embedded web client pin.

### Verification matrix

Rows beyond `## Testing Guidelines`:

| Surface | Required evidence |
| --- | --- |
| Rust source | focused tests plus the local gates in this file |
| optional mikura / chisei / ui | the matching feature-test job in `.github/workflows/ci.yml` |
| portrait assets | `bash scripts/check-portrait-assets.sh` |
| story drafts | `bash scripts/check-drafts-unshipped.sh` |
| docs, templates, Skills | `bash scripts/validate-workflow-skills.sh` |
| release metadata | `python3 scripts/test-release-metadata.py` |
| dependency pins | `Cargo.lock` diff reviewed; the pinned tag exists upstream |

### Release audit

- the version source is the workspace `Cargo.toml`; check it against the
  lockfile;
- pinned ratatui, rusqlite, mikura tag, chisei API pin, and
  `sekai-source-sdk` git tag `v3.0.0`;
- save format migrations, bridge event format, and balance changes;
- configuration additions, removals, and defaults;
- packaging: deb, Homebrew tap, signed managed updates;
- release notes: draft with `bash scripts/changelog-draft.sh`; fold them into
  `CHANGELOG.md` only in the version-bump Pull Request. Agents never push
  `v*` tags.

## Agent delivery closeout (required)

`cargo test` / Clippy / green CI are **not** a substitute for structured code
review. For non-trivial implementation work (any behavior, security, settings,
or public-API change) that will be **committed, pushed, or opened as a PR**:

1. Run the local gates above via the `verify-change` Skill.
2. Run **`autoreview`** before the ship commit (or before push if the commit
   already exists). **Do not vendor** the autoreview skill into this repo;
   resolve the helper from the shared skill install (first match wins):

   ```bash
   # Preferred: shared agent-skills checkout or global skills home
   if [ -x "${AUTOREVIEW:-}" ]; then :; \
   elif [ -x "$HOME/Projects/agent-skills/skills/autoreview/scripts/autoreview" ]; then
     export AUTOREVIEW="$HOME/Projects/agent-skills/skills/autoreview/scripts/autoreview"
   elif [ -x "${AGENTS_HOME:-$HOME/.agents}/skills/autoreview/scripts/autoreview" ]; then
     export AUTOREVIEW="${AGENTS_HOME:-$HOME/.agents}/skills/autoreview/scripts/autoreview"
   else
     echo "autoreview helper not found; install shared agent-skills or set AUTOREVIEW" >&2
     exit 1
   fi
   # Dirty uncommitted work:
   "$AUTOREVIEW" --mode local
   # Topic branch / open PR (preferred after commit):
   "$AUTOREVIEW" --mode branch --base origin/main
   # Already committed on a clean tree:
   "$AUTOREVIEW" --mode commit --commit HEAD
   ```

3. Treat helper output as advisory: verify each accepted finding in the real
   code, fix actionable ones, rerun tests and autoreview until the helper exits
   0 with no accepted/actionable findings (or document a maintainer judgment
   blocker in the PR).
4. Report in the PR or handoff: commands run, tests, autoreview result
   (clean / findings fixed / consciously rejected).

**Do not skip autoreview** because CI is green, the change is “small,” or the
session is optimizing for throughput unless the user explicitly waived it.
Docs-only typo fixes and pure formatting may skip structured review; state the
waiver. The `deliver-ready-issue` Skill requires this same closeout before
publish/land. Read the shared `autoreview` skill’s `SKILL.md` next to the
resolved helper for engines and findings policy.

## Parallel delivery lanes

A delivery lane is one Issue, one branch, one isolated checkout, one Pull
Request, and one owner. Several agents may deliver several Issues at the same
time, on one machine or on many, only as separate lanes. One Issue is never
split across lanes, and one lane never carries a second Issue.

### Claims live on GitHub

Machines cannot see each other's worktrees, so a claim is only what GitHub
shows. An Issue is claimed, and therefore active, when any of the following
exists:

1. an open Pull Request, draft or ready, that references the Issue;
2. a branch `<type>/<issue>`, or any branch matching `*/<issue>-*`;
3. the authenticated login assigned to the Issue less than six hours ago.

An assignment older than six hours with neither branch nor Pull Request is an
ownership hint, not a veto: report it and ask the maintainer before claiming.
A claim is stale once its Issue is closed or its Pull Request has merged; its
owner or the maintainer removes it. Any other takeover first preserves the
existing branch, Pull Request, and evidence and requires the maintainer's
confirmation, unless the lead of the same parallel run owns the stalled lane.

A lane claims before it implements, under Publish or Land authority or with an
explicit claim authorization under Implement, using
`bash "${AGENT_KIT_HOME:-$HOME/.agents/agent-kit}"/skills/deliver-ready-issue/scripts/issue-lane.sh claim <issue>`:

- the branch `<type>/<issue>` is created on GitHub from the default branch by
  an atomic ref creation, so when two machines race exactly one claim
  succeeds and the other sees `claimed`;
- the authenticated login is assigned to the Issue, which shows the claim in
  the Issue list and timestamps it in the Issue timeline.

`<type>` is derived deterministically so that every machine computes the same
branch: the Conventional Commit type in the Issue title, with `bug` mapped to
`fix`; otherwise the type label (`bug` → `fix`, `enhancement` → `feat`,
`documentation` → `docs`); otherwise `chore`. Lane branches carry no slug
because the name is the claim; the Pull Request title carries the description.
Human topic branches may keep `<type>/<issue>-<slug>` and are detected as
claims by the same rule.

An Implement-only lane cannot claim and is invisible to other machines. Say so
in the report, or ask for claim authority before starting.

### Isolation on each machine

Local isolation protects lanes that share a machine; it is not a claim.

- Each lane checks out its claim branch in `.worktrees/issue-<issue>` inside
  the repository, ignored by Git. A fresh clone dedicated to the lane, as on a
  cloud agent, is equivalent. The base SHA is recorded in the lane report.
- The primary checkout belongs to the maintainer. Agents never switch its
  branch, reset it, stash it, or run long jobs in it while another lane is
  active.
- A worktree serves exactly one Issue. Finished lanes are removed; a worktree is
  never reused for a different Issue or renamed to hide its origin.
- Shared Git state is mutated in short, serialized slots: `git fetch --prune`,
  worktree creation and removal, local branch deletion, and merging belong to
  the lead of a parallel run or happen one lane at a time on a shared machine.
  A lane commits and publishes its own branch from its own worktree; that is
  not a shared mutation. Nobody holds a slot across implementation, test runs,
  or a remote wait.

### Limits, collisions, and landing

The three-lane limit counts claims visible on GitHub per repository: open
implementation Pull Requests plus claim branches without one. Assigned or
planned work with neither is not a running lane.

Parallel lanes must not collide. Collision surfaces in this repository are
the game state schema and save format, the tick/offline-progression model, the bridge event format, and balance config. Lanes that would
both change one of these surfaces run in sequence, not in parallel.

A lane publishes its first Verified commit to the claim branch with
`bash "${AGENT_KIT_HOME:-$HOME/.agents/agent-kit}"/skills/deliver-ready-issue/scripts/issue-lane.sh push <issue>`
and opens a draft Pull Request that closes the Issue. Public GitHub lane fields may list only: claim
branch, repo-relative worktree (for example `.worktrees/issue-N` — never an
absolute path), base SHA, and published SHA. Keep absolute paths and
machine/host names in the private agent session only. It marks the Pull
Request ready when verification and review are complete. Immediately before
publishing, the lane fetches and confirms that the default branch is an
ancestor of its head; it refreshes onto `main` only for a conflict, a failing
gate, an explicit request, or a sibling landing on a shared surface, not
merely because `main` advanced.

Each lane lands its own reviewed head with
`bash "${AGENT_KIT_HOME:-$HOME/.agents/agent-kit}"/skills/deliver-ready-issue/scripts/issue-lane.sh land <issue> --pr <pr>`.
Lanes may watch concurrently, and a lane that conflicts with a sibling's
landing stops instead of merging. After each merge, fetch, recompute the
frontier, and let the remaining lanes recheck `mergeable` against the new
`main`.

The lead of a parallel run keeps a ledger per lane: Issue, branch, base SHA,
owner, state, Pull Request, evidence, blockers, and cleanup. Keep hostnames
and absolute checkout paths in the session with the maintainer, never on
GitHub. Report verified outcomes, not launched work. The executable lead
procedure is `skills/deliver-ready-issue/references/parallel-delivery.md` in
the installed agent-kit.

## Coding Style & Naming Conventions

Follow standard Rust formatting with `cargo fmt`. Use `snake_case` for files,
modules, functions, and variables; `PascalCase` for types and traits;
`SCREAMING_SNAKE_CASE` for constants. Keep rules pure and data-driven (balance
values come from config, not literals in code). Never read the wall clock
outside the clock abstraction. Save schema changes always ship a migration.

## Testing Guidelines

Add focused tests for production, build and research timers, offline catch-up
(long gaps must equal step-by-step ticking), combat resolution with seeded
randomness, save migrations, and bridge event parsing. Prefer deterministic
tests with a fake clock and fixed RNG seeds. Tests that need GitHub access are
ignored by default. Offline `cargo test` must stay green.

## Commit & Pull Request Guidelines

Use short imperative subjects, often Conventional Commit style:
`feat(core): catch up production after an offline gap`,
`fix(tui): keep the build queue visible on small terminals`,
`docs: describe the bridge event format`. Keep commits narrow and describe the affected
subsystem when useful. Pull requests should include a concise behavior
summary, **tests run**, **autoreview result** (for non-trivial code), linked
issue or context, and any configuration, compatibility, or security
implications. `.agents/skills/` is tracked; do not treat it as gitignored.
Pull requests do not edit `CHANGELOG.md`; it is release-owned. The PR title
and body carry the change note.

### Public hygiene

This repository is public. Never put hostnames, FQDNs, IP addresses, home
directories, absolute worktree paths, LAN or employer network names, deployment
or environment state, image or artifact digests, tenant or customer names,
release-hold or rollout-hold language, or other private environment inventory on
Issues, Pull Requests, comments, commit messages, ADRs, or docs. Describe
behavior and versions instead (for example "after the sandbox release" rather
than an environment name, a digest, or a hold).

### Verified commits on GitHub

Lane branches are published with GitHub-verified commits and landed through
the shared lane tool, never with a plain `git push` or a hand-run merge:

1. Implement and commit locally as usual (`commit.gpgsign` may still apply).
   Never pass `--no-gpg-sign`; if GPG fails, stop and fix it.
2. Publish from the lane worktree:
   `bash "${AGENT_KIT_HOME:-$HOME/.agents/agent-kit}"/skills/deliver-ready-issue/scripts/issue-lane.sh push <issue>`.
   It creates the Verified commit through GraphQL `createCommitOnBranch` and
   resets the worktree to it, so local `HEAD` is the Pull Request head.
   Confirm it reports `verification.verified=true`.
3. Land with
   `bash "${AGENT_KIT_HOME:-$HOME/.agents/agent-kit}"/skills/deliver-ready-issue/scripts/issue-lane.sh land <issue> --pr <pr>`.
   It squash-merges with the reviewed head as the precondition, so the land
   commit on `main` is Verified and history stays linear.

Never queue GitHub auto-merge and avoid GitHub **rebase** merges: rebase-merge
rewrites commits and drops signatures. Do not rewrite protected `main` after
merging unless the user explicitly approves; if protection is temporarily
relaxed, restore force-push and status-check settings immediately after the
correction.

## Security & Configuration Tips

Never commit secrets, tokens, credentials, save files, logs, or local state.
The bridge reads GitHub through the user's existing `gh` login or a webhook
secret supplied at runtime; it never writes tokens to disk or the save. Local
event sockets and files accept only the documented event format and reject
everything else. Report security issues privately (GitHub private
vulnerability reporting or the maintainer). Never put hostnames, IP addresses,
home directories, absolute paths, or other private environment details on
public Issues, Pull Requests, comments, or commit messages.
