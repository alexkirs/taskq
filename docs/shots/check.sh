#!/bin/sh
# CI stale check for the README screenshots (docs/design/readme-plan.md, Screenshots): fails when a UI source changed
# since manifest.sourceCommit or README.md references an image the manifest lacks. Fix: docs/shots/update.sh.
# "Changed" compares git blob ids recorded at sourceCommit, so the check survives a squash merge dropping that commit.
set -eu
cd "$(dirname "$0")/../.."
m=docs/shots/manifest.json
ui='src/tray.rs src/tray.html src/icon.rs docs/design/icon-summary.config.json'
rc=0
for f in $ui; do
  grep -q "\"$f\": \"$(git rev-parse "HEAD:$f")\"" $m || { echo "check.sh: $f changed since manifest sourceCommit; run docs/shots/update.sh" >&2; rc=1; }
done
for f in $(grep -o 'docs/shots/[A-Za-z0-9@._-]*\.png' README.md | sort -u); do
  grep -q "\"file\": \"${f##*/}\"" $m || { echo "check.sh: $f is in README.md but not in $m" >&2; rc=1; }
done
exit $rc
