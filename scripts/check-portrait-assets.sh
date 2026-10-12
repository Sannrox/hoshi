#!/usr/bin/env bash
# Fail if bundled TUI portraits exceed the size budget from ADR 0003.

set -o errexit
set -o nounset
set -o pipefail

ROOT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
DIR="${ROOT_PATH}/crates/hoshi-tui/assets/portraits"
MAX_FILE=2048
MAX_PACK=32768

if [ ! -d "${DIR}" ]; then
  printf 'portrait check failed: missing %s\n' "${DIR}" >&2
  exit 1
fi

total=0
while IFS= read -r -d '' file; do
  size="$(wc -c < "${file}" | tr -d ' ')"
  if [ "${size}" -gt "${MAX_FILE}" ]; then
    printf 'portrait check failed: %s is %s bytes (max %s)\n' "${file#"${ROOT_PATH}/"}" "${size}" "${MAX_FILE}" >&2
    exit 1
  fi
  total=$((total + size))
done < <(find "${DIR}" -type f -print0)

if [ "${total}" -gt "${MAX_PACK}" ]; then
  printf 'portrait check failed: pack is %s bytes (max %s)\n' "${total}" "${MAX_PACK}" >&2
  exit 1
fi

printf 'portrait assets ok: %s bytes\n' "${total}"
