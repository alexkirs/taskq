#!/bin/sh
# CI check for the README screenshots (docs/design/readme-plan.md, Screenshots; taskq.md R12). Exit 2: only stale, a
# UI source changed since manifest.sourceCommit (CI warns). Exit 1: the manifest is missing or malformed JSON, a UI source
# is unresolvable, or README.md references an image the manifest lacks (CI fails). Fix: docs/shots/update.sh.
# "Changed" compares git blob ids recorded at sourceCommit, so the check survives a squash merge dropping that commit.
set -eu
cd "$(dirname "$0")/../.."
m=docs/shots/manifest.json
ui='src/tray.rs src/tray.html src/tray-web.js src/icon.rs docs/design/icon-summary.config.json'
# parse the whole manifest first: a malformed one fails even when its sourceCommit line looks valid
python3 -c 'import json, re, sys; m = json.load(open(sys.argv[1])); sys.exit(not re.fullmatch("[0-9a-f]{40}", m["sourceCommit"]))' $m 2>/dev/null ||
  { echo "check.sh: $m is missing, malformed or has no sourceCommit" >&2; exit 1; }
rc=0 stale=0
for f in $ui; do
  blob=$(git rev-parse --verify -q "HEAD:$f") || { echo "check.sh: $f is not in HEAD" >&2; rc=1; continue; }
  grep -q "\"$f\": \"$blob\"" $m || { echo "check.sh: $f changed since manifest sourceCommit; run docs/shots/update.sh" >&2; stale=1; }
done
for f in $(grep -o 'docs/shots/[A-Za-z0-9@._-]*\.png' README.md | sort -u); do
  grep -q "\"file\": \"${f##*/}\"" $m || { echo "check.sh: $f is in README.md but not in $m" >&2; rc=1; }
done
[ $rc = 0 ] || exit 1
[ $stale = 0 ] || exit 2
