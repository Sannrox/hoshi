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
example `hoshi 0.1.0 (nightly 202610160317 7cee3a2)`; include it in bug
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
