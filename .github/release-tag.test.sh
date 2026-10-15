#!/bin/sh
# Chain check for the release.yml tag job (taskq.md § 8 item 1, R12): a main push tags and releases only when the
# Cargo.toml version changed and the tag is absent; an unreadable or invalid prior version fails without a tag;
# an existing tag is never written; a manual tag push releases that tag. gh is a stub; nothing leaves the runner.
set -eu
src=$(cd "$(dirname "$0")" && pwd)/release-tag.sh
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
cd "$t"
mkdir bin
cat > bin/gh <<'SH'
#!/bin/sh
# Stub: OLD = prior Cargo.toml body or "503"; TAGGED = 1 when the tag exists. Writes are logged.
case "$*" in
  *contents/Cargo.toml*) [ "$OLD" = 503 ] && { echo 'HTTP 503' >&2; exit 1; }; printf '%s\n' "$OLD";;
  *git/refs*) echo "$*" >> writes; [ "$TAGGED" = 1 ] && exit 1; exit 0;;
  *git/ref/tags/*) [ "$TAGGED" = 1 ];;
  *) exit 2;;
esac
SH
chmod +x bin/gh
export PATH="$t/bin:$PATH" GITHUB_REPOSITORY=o/r GITHUB_SHA=abc GITHUB_REF_NAME=main GITHUB_REF_TYPE=branch
fails=0
# $1 name, $2 new version line, $3 prior body, $4 tagged, $5 expected exit, $6 expected output, $7 expected writes
case_() {
  printf '%s\n' "$2" > Cargo.toml; rm -f writes; touch writes
  rc=0; out=$(OLD=$3 TAGGED=$4 BEFORE=${BEFORE:-def} sh "$src" 2>/dev/null) || rc=$?
  w=$(wc -l < writes | tr -d ' ')
  if [ "$rc" = "$5" ] && [ "$out" = "$6" ] && [ "$w" = "$7" ]; then echo "ok   $1"
  else echo "FAIL $1: exit $rc out '$out' writes $w; want $5 '$6' $7"; fails=1; fi
}
case_ 'changed, absent: tags' 'version = "1.1.0"' 'version = "1.0.0"' 0 0 tag=v1.1.0 1
case_ 'changed, existing: no release' 'version = "1.1.0"' 'version = "1.0.0"' 1 0 '' 1
case_ 'unchanged, absent: nothing' 'version = "1.1.0"' 'version = "1.1.0"' 0 0 '' 0
case_ 'unchanged, existing: nothing' 'version = "1.1.0"' 'version = "1.1.0"' 1 0 '' 0
case_ 'prior read 503: fails' 'version = "1.1.0"' 503 0 1 '' 0
case_ 'prior empty: fails' 'version = "1.1.0"' '' 0 1 '' 0
case_ 'prior invalid: fails' 'version = "1.1.0"' 'version = ""' 0 1 '' 0
case_ 'new invalid: fails' 'version = "x"' 'version = "1.0.0"' 0 1 '' 0
BEFORE=0000000000000000000000000000000000000000 case_ 'no prior commit: fails' 'version = "1.1.0"' 'version = "1.0.0"' 0 1 '' 0
GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v9.9.9 case_ 'manual tag push: releases it' 'version = "1.1.0"' 503 1 0 tag=v9.9.9 0
exit $fails
