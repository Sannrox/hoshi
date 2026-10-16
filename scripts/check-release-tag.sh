#!/usr/bin/env bash
# Fail when a vX.Y.Z tag does not match [workspace.package].version.
# Usage: scripts/check-release-tag.sh vX.Y.Z

set -euo pipefail

TAG="${1:-}"
if [[ ! "${TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'usage: %s vX.Y.Z\n' "$0" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

export TAG
python3 - << 'PY'
import os
import re
import sys
from pathlib import Path

tag = os.environ["TAG"]
want = tag[1:]
text = Path("Cargo.toml").read_text()
block = re.search(r"(?ms)^\[workspace\.package\]\n(.*?)(?=\n\[|\Z)", text)
if not block:
    sys.exit("check-release-tag: [workspace.package] missing")
match = re.search(r'(?m)^version\s*=\s*"([^"]+)"', block.group(1))
if not match:
    sys.exit("check-release-tag: workspace version missing")
got = match.group(1)
if got != want:
    sys.exit(f"check-release-tag: tag {tag} != crate version {got}")
print(got)
PY
