#!/bin/sh
# Install hoshi on Ubuntu/Debian from the GitHub Release .deb.
# Usage: curl -fsSL https://raw.githubusercontent.com/Sannrox/hoshi/main/scripts/install.sh | sh
# HOSHI_VERSION=vX.Y.Z pins a release; the default is the latest one.

set -eu

REPO="https://github.com/Sannrox/hoshi"

fail() {
  printf 'hoshi install: %s\n' "$1" >&2
  exit 1
}

command -v dpkg > /dev/null 2>&1 || fail "needs dpkg (Ubuntu or Debian)"
command -v curl > /dev/null 2>&1 || fail "needs curl"

case "$(dpkg --print-architecture)" in
  amd64) ARCH=amd64 ;;
  arm64) ARCH=arm64 ;;
  *) fail "no package for $(dpkg --print-architecture); try: cargo install --git ${REPO} --locked hoshi" ;;
esac

TAG="${HOSHI_VERSION:-}"
if [ -z "${TAG}" ]; then
  TAG="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${REPO}/releases/latest")"
  TAG="${TAG##*/}"
fi
case "${TAG}" in
  v[0-9]*.[0-9]*.[0-9]*) ;;
  *) fail "could not resolve a release tag (got '${TAG}')" ;;
esac

DEB="hoshi_${TAG#v}_${ARCH}.deb"
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

printf 'Downloading %s (%s)\n' "${DEB}" "${TAG}"
curl -fsSL -o "${TMP}/${DEB}" "${REPO}/releases/download/${TAG}/${DEB}"
curl -fsSL -o "${TMP}/SHA256SUMS" "${REPO}/releases/download/${TAG}/SHA256SUMS"
(cd "${TMP}" && grep " ${DEB}\$" SHA256SUMS | sha256sum -c -) || fail "checksum mismatch for ${DEB}"

SUDO=""
[ "$(id -u)" -eq 0 ] || SUDO="sudo"
${SUDO} apt-get install -y "${TMP}/${DEB}"

hoshi --version
