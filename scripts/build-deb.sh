#!/usr/bin/env bash
# Package a built hoshi binary as a Debian/Ubuntu .deb.
# Usage: scripts/build-deb.sh vX.Y.Z amd64|arm64 BINARY OUT_DIR
# Runs on the build host; the libc6 floor is the host's glibc version.

set -euo pipefail

TAG="${1:-}"
ARCH="${2:-}"
BIN="${3:-}"
OUT="${4:-}"
if [[ ! "${TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || [[ ! "${ARCH}" =~ ^(amd64|arm64)$ ]] \
  || [ ! -f "${BIN}" ] || [ -z "${OUT}" ]; then
  printf 'usage: %s vX.Y.Z amd64|arm64 BINARY OUT_DIR\n' "$0" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${TAG#v}"
GLIBC="$(getconf GNU_LIBC_VERSION | awk '{print $2}')"
mkdir -p "${OUT}"
OUT="$(cd "${OUT}" && pwd)"

STAGE="$(mktemp -d)"
trap 'rm -rf "${STAGE}"' EXIT
PKG="${STAGE}/hoshi_${VERSION}_${ARCH}"

install -Dm755 "${BIN}" "${PKG}/usr/bin/hoshi"
install -Dm644 "${ROOT}/LICENSE" "${PKG}/usr/share/doc/hoshi/copyright"

mkdir -p "${PKG}/DEBIAN"
cat > "${PKG}/DEBIAN/control" << EOF
Package: hoshi
Version: ${VERSION}
Section: games
Priority: optional
Architecture: ${ARCH}
Depends: libc6 (>= ${GLIBC}), libgcc-s1
Installed-Size: $(du -sk "${PKG}/usr" | cut -f1)
Maintainer: Sannrox <noreply@github.com>
Homepage: https://github.com/Sannrox/hoshi
Description: Idle space-strategy game for the terminal
 Mine resources, raise buildings, research technologies, build fleets and
 send them on expeditions. Meant to be played in a spare terminal pane.
EOF

dpkg-deb --build --root-owner-group "${PKG}" "${OUT}/hoshi_${VERSION}_${ARCH}.deb"
