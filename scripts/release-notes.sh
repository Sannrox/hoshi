#!/usr/bin/env bash
# Build GitHub Release notes from CHANGELOG.md and the save schema.
# Usage: scripts/release-notes.sh vX.Y.Z
# Optional: CHANGELOG_PATH overrides the changelog file (tests).

set -euo pipefail

TAG="${1:-}"
if [[ ! "${TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'usage: %s vX.Y.Z\n' "$0" >&2
  exit 2
fi

VERSION="${TAG#v}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT}"

SCHEMA="$(sed -n 's/^pub const SCHEMA_VERSION: u32 = \([0-9][0-9]*\);/\1/p' crates/hoshi-storage/src/save.rs | head -n1)"
if [ -z "${SCHEMA}" ]; then
  printf 'release-notes: SCHEMA_VERSION not found\n' >&2
  exit 1
fi

export VERSION SCHEMA CHANGELOG_PATH="${CHANGELOG_PATH:-CHANGELOG.md}"
python3 - << 'PY'
import os
import re
import sys
from pathlib import Path

version = os.environ["VERSION"]
schema = os.environ["SCHEMA"]
path = Path(os.environ["CHANGELOG_PATH"])
lines = path.read_text().splitlines()

def heading_key(title: str):
    stripped = title.strip()
    lowered = stripped.lower().strip("[]")
    if lowered == "unreleased":
        return "unreleased"
    match = re.match(r"\[?v?(\d+\.\d+\.\d+)\]?", stripped)
    return match.group(1) if match else None

sections = {}
current = None
buf = []
for line in lines:
    if line.startswith("## "):
        if current is not None:
            sections[current] = "\n".join(buf).strip()
        current = heading_key(line[3:])
        buf = []
    elif current is not None:
        buf.append(line)
if current is not None:
    sections[current] = "\n".join(buf).strip()

body = sections.get(version) or sections.get("unreleased") or ""
if not body:
    sys.exit("release-notes: changelog section empty")
print(f"# hoshi {version}")
print()
print(body)
print()
print("## Saves")
print()
print(
    f"This release uses save schema v{schema}. "
    "Opening an older save migrates it in place. "
    "A save written by a newer schema is refused."
)
PY
