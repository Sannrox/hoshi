# Repository Guidelines

<!-- Structure and shared policy ported from Sannrox/shikigami@331897846885e4a55697b0f1006d03b4f2a7495e AGENTS.md (adapted for hoshi). Compare against the source to detect drift. -->

## Project Structure & Module Organization

`hoshi` is a Rust 2024 workspace for a real-time idle space-strategy game that
runs in the terminal, meant to be played while waiting for coding agents. The
design is recorded in [ADR 0002](docs/decisions/0002-terminal-idle-game-with-event-bridge.md).

- `crates/hoshi-core`: game rules, state, timers, offline progression and the
  save backends (SQLite default; optional Mikura / Chisei). No terminal or network code.
- `crates/hoshi-tui`: the ratatui binary. Renders state and sends commands to
  core; holds no rules.
- `crates/hoshi-bridge`: turns outside events (GitHub, agent activity) into
  game events for core.
- `crates/hoshi-author`: offline `hoshi-author` CLI that drafts story events
  into `content/drafts/`. The game never depends on it or generates events at
  runtime.

Do not commit local state (`.state/`, `*.db` saves), logs, or tokens.

Human documentation: [README.md](README.md). Architecture decisions:
[docs/decisions/](docs/decisions/README.md). Design and balance follow
[docs/realism.md](docs/realism.md) (physics and maths only; flavour stays
fictional). `CLAUDE.md` only points here; this file is canonical.

## Dependencies and Ownership

- **hoshi-core is the authority** for rules and state. The TUI and the bridge
  call core APIs; they never write the save directly.
- **Time is injected.** Core takes a clock so ticks and offline catch-up are
  deterministic in tests.
- External crates: ratatui and crossterm for the TUI, rusqlite (bundled) for
  the default save. Optional `mikura` (git tag `v0.2.0`) is a second backend
  behind `--features mikura`. Optional `chisei` (API pin 1.3.0, off in
  `config.toml`) is a third backend behind `--features chisei`. The bridge uses the `gh` CLI or webhooks; it
  never stores tokens.

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
- `scripts/validate-workflow-skills.sh` checks that the delivery Skills and
  this file stay linked.

There is no Makefile yet. When one is added, its targets become the source of
truth for these commands and this section must point to them.

GitHub Issues are the planning source of truth. Project-specific Skills live
under `.agents/skills/`. Read `README.md`, [docs/realism.md](docs/realism.md)
and accepted ADRs under `docs/decisions/` before changing system boundaries
or balance numbers; record new boundaries, trust-model changes, and public
contracts as ADRs with the `capture-project-decision` Skill. New design
Issues name object types, links and core actions, and they either obey the
realism rules or extend the simplifications list in that file.

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
`bash .agents/skills/deliver-ready-issue/scripts/issue-lane.sh claim <issue>`:

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

A lane publishes its first Verified commit to the claim branch as a draft Pull
Request that closes the Issue. Public GitHub lane fields may list only: claim
branch, repo-relative worktree (for example `.worktrees/issue-N` — never an
absolute path), base SHA, and published SHA. Keep absolute paths and
machine/host names in the private agent session only. It marks the Pull
Request ready when verification and review are complete. Immediately before
publishing, the lane fetches and confirms that the default branch is an
ancestor of its head; it refreshes onto `main` only for a conflict, a failing
gate, an explicit request, or a sibling landing on a shared surface, not
merely because `main` advanced.

Landing is sequential. After each merge, fetch, recompute the frontier, and let
the remaining lanes recheck `mergeable` against the new `main`. A failed or
timed-out merge response may still have merged; reconcile the remote state
before retrying.

The lead of a parallel run keeps a ledger per lane: Issue, branch, base SHA,
owner, state, Pull Request, evidence, blockers, and cleanup. Keep hostnames
and absolute checkout paths in the session with the maintainer, never on
GitHub. Report verified outcomes, not launched work. The executable lead
procedure is `.agents/skills/deliver-ready-issue/references/parallel-delivery.md`.

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
Record user-visible changes in `CHANGELOG.md` under `Unreleased`.

### Public hygiene

This repository is public. Never put hostnames, FQDNs, IP addresses, home
directories, absolute worktree paths, LAN or employer network names, deployment
or environment state, image or artifact digests, tenant or customer names,
release-hold or rollout-hold language, or other private environment inventory on
Issues, Pull Requests, comments, commit messages, ADRs, or docs. Describe
behavior and versions instead (for example "after the sandbox release" rather
than an environment name, a digest, or a hold).

### Verified commits on GitHub

Prefer publishing PR branch tips with GitHub-signed commits so GitHub shows
**Verified**:

1. Implement and commit locally as usual (`commit.gpgsign` may still apply).
2. Publish the branch tip with `bash scripts/gh-verified-push.sh` instead of a
   plain `git push` when you want the hosted commit Verified (GraphQL
   `createCommitOnBranch`). That path creates one server-side commit with the
   local `HEAD` tree; committer is typically **GitHub**.
3. New branch:
   `bash scripts/gh-verified-push.sh --create-branch-from origin/main --branch <topic> --sync-local`
4. Existing PR branch:
   `bash scripts/gh-verified-push.sh --branch <topic> --sync-local`
   (uses the current remote tip as `expectedHeadOid`).
5. Never pass `--no-gpg-sign` for local commits; if GPG fails, stop and fix it.
6. After publish, confirm `verification.verified=true` (the script prints this).

When merging PRs, prefer **squash** (`gh pr merge --squash --delete-branch`) so
the land commit on `main` is also GitHub-signed/Verified and history stays
linear. Use `gh pr merge --merge` only when multi-commit history must be kept
(original SHAs preserved). Avoid GitHub **rebase** merges when Verified history
matters: rebase-merge rewrites commits and drops signatures. Do not rewrite
protected `main` after merging unless the user explicitly approves; if
protection is temporarily relaxed, restore force-push and status-check settings
immediately after the correction.

## Security & Configuration Tips

Never commit secrets, tokens, credentials, save files, logs, or local state.
The bridge reads GitHub through the user's existing `gh` login or a webhook
secret supplied at runtime; it never writes tokens to disk or the save. Local
event sockets and files accept only the documented event format and reject
everything else. Report security issues privately (GitHub private
vulnerability reporting or the maintainer). Never put hostnames, IP addresses,
home directories, absolute paths, or other private environment details on
public Issues, Pull Requests, comments, or commit messages.
