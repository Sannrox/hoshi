#!/usr/bin/env bash
# Regenerate human references from the binary's command catalog.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export CARGO_TARGET_DIR="$ROOT/target"
cargo build --locked -p hoshi --bin hoshi
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
BIN="$CARGO_TARGET_DIR/debug/hoshi"
"$BIN" help --markdown > "$STAGE/commands.md"
"$BIN" help --man > "$STAGE/hoshi.1"
for shell in bash zsh fish; do
  "$BIN" help --completions "$shell" > "$STAGE/hoshi.$shell"
done
mkdir -p docs/generated
mv "$STAGE/commands.md" docs/commands.md
for file in hoshi.1 hoshi.bash hoshi.zsh hoshi.fish; do
  mv "$STAGE/$file" "docs/generated/$file"
done
