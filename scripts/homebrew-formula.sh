#!/usr/bin/env bash
# Write a Homebrew formula for a tagged GitHub Release.
# Usage: scripts/homebrew-formula.sh vX.Y.Z DIST_DIR

set -euo pipefail

TAG="${1:-}"
DIST="${2:-}"
if [[ ! "${TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || [ -z "${DIST}" ]; then
  printf 'usage: %s vX.Y.Z DIST_DIR\n' "$0" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$(cd "${DIST}" && pwd)"
python3 "${ROOT}/scripts/homebrew-formula.py" "${TAG}" "${DIST}"
