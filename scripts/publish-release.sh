#!/usr/bin/env bash
# Retry a release publication without replacing already published signed assets.
set -euo pipefail
if [ "$#" -ne 4 ]; then
  echo 'usage: publish-release.sh TAG TITLE NOTES stable|nightly' >&2
  exit 2
fi
tag="$1"
title="$2"
notes="$3"
channel="$4"
case "$channel" in stable|nightly) ;; *) exit 2 ;; esac
if gh release view "$tag" > /dev/null 2>&1; then
  stage="$(mktemp -d)"
  trap 'rm -rf "$stage"' EXIT
  draft="$(gh release view "$tag" --json isDraft --jq .isDraft)"
  case "$draft" in true|false) ;; *) echo 'Cannot determine release publication status.' >&2; exit 1 ;; esac
  gh release download "$tag" --dir "$stage"
  for asset in dist/*; do
    name="$(basename "$asset")"
    if [ -f "$stage/$name" ]; then
      if ! cmp -s "$asset" "$stage/$name"; then
        echo 'Published release assets differ; use a new version rather than replacing signed assets.' >&2
        exit 1
      fi
    elif [ "$draft" = true ]; then
      gh release upload "$tag" "$asset"
    else
      echo 'Published release is incomplete; refusing to publish update metadata.' >&2
      exit 1
    fi
  done
  if [ "$draft" = true ]; then
    gh release edit "$tag" --draft=false
  fi
  echo 'Release already contains this signed manifest.'
else
  extra=()
  if [ "$channel" = nightly ]; then
    extra=(--prerelease --latest=false --target "$GITHUB_SHA")
  fi
  gh release create "$tag" dist/* --draft --title "$title" --notes-file "$notes" "${extra[@]}"
  # Reuse the completeness check before publishing a newly uploaded draft.
  exec bash "$0" "$tag" "$title" "$notes" "$channel"
fi
