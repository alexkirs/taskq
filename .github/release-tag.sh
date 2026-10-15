#!/bin/sh
# release.yml tag job (taskq.md § 8 item 1): prints `tag=v<version>` for GITHUB_OUTPUT when this run must release.
# Tag push: that tag. Main push: only when the Cargo.toml version changed since BEFORE and tag v<version> is absent;
# an unreadable or invalid version on either side fails without a tag. An existing tag never moves.
set -eu
if [ "$GITHUB_REF_TYPE" = tag ]; then echo "tag=$GITHUB_REF_NAME"; exit 0; fi
ver() { sed -n 's/^version = "\([0-9][0-9A-Za-z.+-]*\)"$/\1/p' | head -1; }
new=$(ver < Cargo.toml)
[ -n "$new" ] || { echo "::error::no valid version in Cargo.toml; no tag" >&2; exit 1; }
case "$BEFORE" in ''|*[!0]*) ;; *) echo "::error::push has no previous commit; push v$new by hand; no tag" >&2; exit 1;; esac
raw=$(gh api -H 'Accept: application/vnd.github.raw' "repos/$GITHUB_REPOSITORY/contents/Cargo.toml?ref=$BEFORE") ||
  { echo "::error::cannot read Cargo.toml at $BEFORE; no tag" >&2; exit 1; }
old=$(printf '%s\n' "$raw" | ver)
[ -n "$old" ] || { echo "::error::no valid version in Cargo.toml at $BEFORE; no tag" >&2; exit 1; }
[ "$old" != "$new" ] || { echo "version v$new unchanged; no release" >&2; exit 0; }
if gh api "repos/$GITHUB_REPOSITORY/git/refs" -f ref="refs/tags/v$new" -f sha="$GITHUB_SHA" > /dev/null; then
  echo "tag=v$new"
else
  # Creation failed: succeed quietly only when the tag already exists, else fail.
  gh api "repos/$GITHUB_REPOSITORY/git/ref/tags/v$new" > /dev/null
  echo "v$new exists; no release" >&2
fi
