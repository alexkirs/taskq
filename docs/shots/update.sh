#!/bin/sh
# Refresh the README screenshots (macOS): build HEAD, render docs/shots/fixture.sh with `taskq tray --snapshot`,
# copy the dark images here and write manifest.json (shape of docs/design/evidence/4.1.9/manifest.json).
set -eu
cd "$(dirname "$0")/../.."
ui='src/tray.rs src/tray.html src/icon.rs docs/design/icon-summary.config.json'
# the images must come from sourceCommit, so UI sources must be committed
git diff --quiet HEAD -- $ui || { echo "update.sh: commit UI source changes first: $ui" >&2; exit 1; }
cargo build --release --locked
bin=target/release/taskq
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
TASKQ_VERSION=$version docs/shots/fixture.sh "$tmp"
TASKQ_INSTALL=$tmp/install "$bin" tray --snapshot "$tmp/shots"
rm -f docs/shots/*.png
cp "$tmp/shots/popover-dark-home-16.png" "$tmp/shots/popover-dark-project-16.png" "$tmp"/shots/icon-summary-run-dark-*.png docs/shots/
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
{
  printf '{\n  "version": "%s",\n  "sourceCommit": "%s",\n  "binarySha256": "%s",\n' "$version" "$(git rev-parse HEAD)" "$(sha "$bin")"
  printf '  "environment": {\n    "os": "macOS %s",\n    "arch": "%s",\n    "renderer": "WKWebView snapshot"\n  },\n' "$(sw_vers -productVersion)" "$(uname -m)"
  printf '  "uiSources": {\n'
  sep=''
  for f in $ui; do printf '%s    "%s": "%s"' "$sep" "$f" "$(git rev-parse "HEAD:$f")"; sep=',
'; done
  printf '\n  },\n'
  printf '  "capture": "Isolated TASKQ_INSTALL using docs/shots/fixture.sh; dark images only.",\n  "images": [\n'
  printf '    {\n      "file": "../header.webp",\n      "sha256": "%s"\n    }' "$(sha docs/header.webp)"
  sep=',
'
  for f in docs/shots/*.png; do
    printf '%s    {\n      "file": "%s",\n      "sha256": "%s"\n    }' "$sep" "${f##*/}" "$(sha "$f")"
    sep=',
'
  done
  printf '\n  ]\n}\n'
} > docs/shots/manifest.json
echo "docs/shots updated from $(git rev-parse --short HEAD)"
