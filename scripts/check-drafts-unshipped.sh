#!/usr/bin/env bash
# Fail if story drafts are wired into the shipped game content index.

set -o errexit
set -o nounset
set -o pipefail

ROOT_PATH="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "${ROOT_PATH}"

fail() {
  printf 'drafts-unshipped check failed: %s\n' "$1" >&2
  exit 1
}

for crate in hoshi-core hoshi-tui hoshi-bridge; do
  hits="$(grep -R -n -E --include='*.rs' --include='*.toml' --include='*.json' \
    'content/drafts' "crates/${crate}" || true)"
  if [ -n "${hits}" ]; then
    printf '%s\n' "${hits}" >&2
    fail "crates/${crate} references content/drafts; drafts are not game content"
  fi
done

includes="$(grep -R -n --include='*.rs' 'include_str!' crates/hoshi-core crates/hoshi-tui crates/hoshi-bridge \
  | grep -E '[Dd]raft' || true)"
if [ -n "${includes}" ]; then
  printf '%s\n' "${includes}" >&2
  fail "game crates include_str a draft path"
fi

if [ -e crates/hoshi-core/data/drafts ] || [ -e crates/hoshi-core/data/content ]; then
  fail "do not place drafts under crates/hoshi-core/data"
fi

printf 'drafts stay unshipped\n'
