#!/bin/sh
# Install hoshi on Ubuntu/Debian from a GitHub Release .deb.
# Usage: curl -fsSL https://raw.githubusercontent.com/Sannrox/hoshi/main/scripts/install.sh | sh
# HOSHI_CHANNEL=nightly installs the latest nightly build of main.
# HOSHI_VERSION=vX.Y.Z pins a release; the default is the latest stable one.

set -eu

REPO="https://github.com/Sannrox/hoshi"

fail() {
  printf 'hoshi install: %s\n' "$1" >&2
  exit 1
}

managed_install() {
  set -- hoshi update --install-dir "${HOSHI_MANAGED_DIR}" --channel "${HOSHI_CHANNEL:-stable}"
  case "${HOSHI_VERSION:-}" in
    "") ;;
    nightly) set -- hoshi update --install-dir "${HOSHI_MANAGED_DIR}" --channel nightly ;;
    *) version="$(printf '%s' "${HOSHI_VERSION}" | sed 's/~nightly/-nightly/')"; set -- "$@" --version "${version}" ;;
  esac
  if ( : < /dev/tty ) 2>/dev/null; then
    "$@" < /dev/tty
  else
    fail "managed install needs a controlling terminal; run hoshi update --install-dir DIR manually"
  fi
}

# A trusted existing binary can bootstrap a managed installation on any target.
if [ -n "${HOSHI_MANAGED_DIR:-}" ] && command -v hoshi > /dev/null 2>&1; then
  managed_install
  exit 0
fi

command -v dpkg > /dev/null 2>&1 || fail "needs dpkg (Ubuntu or Debian)"
command -v curl > /dev/null 2>&1 || fail "needs curl"

case "$(dpkg --print-architecture)" in
  amd64) ARCH=amd64 ;;
  arm64) ARCH=arm64 ;;
  *) fail "no package for $(dpkg --print-architecture); try: cargo install --git ${REPO} --locked hoshi" ;;
esac

TAG="${HOSHI_VERSION:-}"
if [ -z "${TAG}" ]; then
  case "${HOSHI_CHANNEL:-stable}" in
    stable)
      TAG="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${REPO}/releases/latest")"
      TAG="${TAG##*/}"
      ;;
    nightly) TAG=nightly ;;
    *) fail "HOSHI_CHANNEL must be stable or nightly" ;;
  esac
fi
case "${TAG}" in
  nightly | v[0-9]*.[0-9]*.[0-9]*) ;;
  *) fail "no stable release found; try HOSHI_CHANNEL=nightly" ;;
esac

TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

curl -fsSL -o "${TMP}/SHA256SUMS" "${REPO}/releases/download/${TAG}/SHA256SUMS" \
  || fail "release ${TAG} not found"
DEB="$(sed -n "s/^[0-9a-f]\{64\}  \(hoshi_[^ ]*_${ARCH}\.deb\)\$/\1/p" "${TMP}/SHA256SUMS" | head -n 1)"
[ -n "${DEB}" ] || fail "release ${TAG} has no ${ARCH} package"

printf 'Downloading %s (%s)\n' "${DEB}" "${TAG}"
curl -fsSL -o "${TMP}/${DEB}" "${REPO}/releases/download/${TAG}/${DEB}"
(cd "${TMP}" && grep " ${DEB}\$" SHA256SUMS | sha256sum -c -) || fail "checksum mismatch for ${DEB}"

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"
${SUDO} apt-get install -y --allow-downgrades "${TMP}/${DEB}"

hoshi --version
if [ -n "${HOSHI_MANAGED_DIR:-}" ]; then
  managed_install
fi
