# Install

The playable crate is `hoshi` (the binary lives in `crates/hoshi-tui`).
`cargo install --git … hoshi` is the cargo path. `hoshi-core` stays unpublished
while an optional git-pinned backend remains in its manifest. This repository's
release workflow publishes GitHub Release binaries, Ubuntu `.deb` packages and the Homebrew formula;
it does not upload crates.

## Git

```sh
cargo install --git https://github.com/Sannrox/hoshi --locked hoshi
```

Optional backends (`--features mikura`, `--features chisei`, `--features bridge`)
are available from this install.

## Prebuilt binaries

Pushing a SemVer tag `vX.Y.Z` runs `.github/workflows/release.yml`. The GitHub
Release attaches five binaries and `SHA256SUMS`:

| File | Target |
| --- | --- |
| `hoshi-x86_64-unknown-linux-gnu` | Linux x86_64 |
| `hoshi-aarch64-unknown-linux-gnu` | Linux aarch64 |
| `hoshi-x86_64-apple-darwin` | macOS Intel |
| `hoshi-aarch64-apple-darwin` | macOS Apple Silicon |
| `hoshi-x86_64-pc-windows-msvc.exe` | Windows x86_64 |

Each binary is started with `--version` in that workflow. Linux x86_64 also
runs a headless `hoshi --save PATH --seed 1 state`. Linux binaries are built on
Ubuntu 22.04, so they need glibc 2.35 or newer.

## Ubuntu

```sh
curl -fsSL https://raw.githubusercontent.com/Sannrox/hoshi/main/scripts/install.sh | sh
```

The script picks `amd64` or `arm64`, downloads `hoshi_X.Y.Z_<arch>.deb` from
the latest GitHub Release, checks it against `SHA256SUMS`, and installs it
with `apt-get` (asks for `sudo`). Pin a release with `HOSHI_VERSION=vX.Y.Z`.
Manual install from a downloaded package:

```sh
sudo apt install ./hoshi_X.Y.Z_amd64.deb
```

Supported on Ubuntu 22.04 and newer. `sudo apt remove hoshi` uninstalls; the
save in `~/.local/share/hoshi/` is kept. The release workflow builds the
packages with `scripts/build-deb.sh` and installs the amd64 one as a smoke test.

## Nightly builds

For alpha testers, `.github/workflows/nightly.yml` builds `main` every night
(03:17 UTC, skipped when `main` has not moved; run it by hand from the Actions
tab). It replaces the `nightly` prerelease with Linux binaries, both `.deb`
packages and `SHA256SUMS`:

```sh
curl -fsSL https://raw.githubusercontent.com/Sannrox/hoshi/main/scripts/install.sh | HOSHI_CHANNEL=nightly sh
```

Rerun the same command to update. `hoshi --version` prints the build, for
example `hoshi 0.1.0-nightly.202610160317.7cee3a2 (nightly 202610160317 7cee3a2)`; include it in bug
reports. The package version is `X.Y.Z~nightly.STAMP.SHA`, which sorts before
the `X.Y.Z` release, so a later stable install replaces it. A nightly can
migrate a save to a schema that an older stable release refuses to open; back
up `~/.local/share/hoshi/` before switching channels.

Release notes are the Unreleased (or matching version) changelog section plus a
**Saves** paragraph: current `SCHEMA_VERSION`, older saves migrate in place,
newer saves are refused.

## Homebrew

```sh
brew tap Sannrox/tap
brew install hoshi
```

The formula (`Formula/hoshi.rb` in [Sannrox/homebrew-tap](https://github.com/Sannrox/homebrew-tap))
is regenerated on each tag. `brew test` runs `hoshi --version`. If the tap token
is not set, the same `hoshi.rb` is attached to the GitHub Release.

## Crate registry

After `hoshi` and `hoshi-core` are on a crate registry:

```sh
cargo install hoshi --locked
```

Until then, use git, a GitHub Release binary, the Ubuntu package, or Homebrew.

## Command help, man page and completions

`hoshi --help` lists commands and launch flags. `hoshi help state`,
`hoshi help fleet send`, and `hoshi serve --help` show usage and examples.
In the TUI, `?` opens Keys; Tab switches to Commands for the current hub.
Title Settings → Help opens the same two pages.

Release assets include `hoshi.1`, `hoshi.bash`, `hoshi.zsh`, and `hoshi.fish`.
Ubuntu packages install the man page and all three completion scripts.
For another installation, generate the files from the installed binary:

```sh
hoshi help --man > hoshi.1
man -l ./hoshi.1
hoshi help --completions bash > hoshi.bash
source ./hoshi.bash
hoshi help --completions zsh > _hoshi
hoshi help --completions fish > hoshi.fish
```

For persistent Bash completion, source `hoshi.bash` from your shell startup
file. For Zsh, put `_hoshi` in a directory on `$fpath` before `compinit`.
For Fish, put `hoshi.fish` in `~/.config/fish/completions/`.

Contributors run `scripts/generate-help.sh` after editing the catalog.
CI tests parser-valid examples, catalog coverage, golden help text, and drift
in `docs/commands.md`, the man page, and completion scripts. Update and changelog
entries are explicitly marked planned until their commands ship.

## Update checks and release log

Interactive play checks for a signed release in the background. The title screen
renders immediately. One GET has a ten-second timeout, no retry, and a
`User-Agent: hoshi/<version> (<os>; <arch>)`. It sends no save, identifier or
usage data. Results and attempts are cached for 24 hours beside `config.toml`.
The JSON harness and scripts never start this check. Servers require opt-in.

Disable automatic requests with `--offline` or `HOSHI_OFFLINE=1`. Disable just
release checks with `HOSHI_SKIP_VERSION_CHECK=1` or this config:

```toml
update_check = false
update_endpoint = ""
collapse_changelog = false
```

`update_endpoint` can be an HTTP(S) mirror or `file:///path/latest.json`.
Mirrors must retain the signed envelope unchanged. Cached verified notices work
offline. The first interactive start discloses the request policy once. Press
`?`, then `u` to dismiss an available version. Press `?`, then `c` for the bundled
Changelog, or run `hoshi changelog` without accessing a save or the network.
After upgrading, the release log shows sections newer than the last seen build.
`collapse_changelog = true` reduces it to an update line.

## Explicit updates and rollback

Nothing installs automatically:

```sh
hoshi update
hoshi update --yes
hoshi update --version vX.Y.Z
hoshi update --channel nightly
hoshi update --force
hoshi update --rollback
```

The command verifies metadata, displays the version and maintainer note, and
asks before installing. A manual metadata request allows two retries. Homebrew
and cargo installs invoke their package manager after confirmation. Ubuntu
packages print the administrator command. Unknown installs print their path
and refuse to replace it.

The install script accepts `HOSHI_MANAGED_DIR` to hand off to the managed
updater after installing a trusted binary. You can also create a managed
standalone install directly from a trusted release binary:

```sh
hoshi update --install-dir "$XDG_DATA_HOME/hoshi"
```

Choose a concrete writable data directory if `XDG_DATA_HOME` is unset. Put its
`bin/` directory on `PATH`. The launcher follows the atomic `current` pointer.
The updater locks the install, downloads to `staging/`, checks the signed asset
hash, and runs `--version` before publishing a new immutable release directory.
It keeps the active and previous generations. Interrupted staging leaves the
old pointer usable. A second updater fails immediately. `--rollback` verifies
the previous metadata and reverses the pointer. An open session reports that a
restart is needed when another terminal updates the installation.

Use `--save-dir DIR` when saves live outside the default directory. SQLite saves
are checked before installation; an older target schema is refused. Backups use
SQLite snapshots, including committed WAL data, under `update-backups/`; the
last three completed backups are retained. To downgrade after migration, close
play and restore a matching backup first. Optional backend saves require manual
backup and compatibility checks. Unknown config keys and sections survive
saves; interactive play warns once when it sees them.

## Release signing

Release builds compile `HOSHI_RELEASE_PUBLIC_KEY` as 64 hexadecimal characters.
Source builds must supply that public value at build time to enable trusted
release checks and updates. A build without a key cannot accept metadata.
`HOSHI_RELEASE_SIGNING_KEY` is an Ed25519 PEM stored only as a CI secret.
Release jobs sign exact JSON payload bytes and `SHA256SUMS`, and refuse to
publish when the private and compiled public keys do not match.

The optional manual key-provisioning workflow generates the private key only
in CI and stores it directly in the repository secret. It requires an operator
credential with repository secrets and variables write permission in the
`release-signing` environment; remove that credential after provisioning. It
reuses the existing private key when retrying an interrupted setup and refuses
to rotate an existing identity. Key rotation needs a reviewed trust
transition and must keep existing installations able to verify releases.

Signed metadata is attached to each release and published atomically on the
`update-metadata` branch. Stable and nightly have separate latest files. Each
version also has an immutable metadata file. Nightly keeps versioned assets in
addition to its moving prerelease, so a pin never resolves to another build.

During explicit installation or rollback, a directory lock fences new SQLite
save creation and migration while compatibility and backups are checked again.
Already-open sessions continue. Linked save entries require manual compatibility
checks and backups; automatic updating refuses them rather than skipping them.
