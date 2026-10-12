#!/bin/sh
# Fixture for `taskq tray` (docs/design/verification.md): three projects in DIR, then
#   TASKQ_INSTALL=DIR/install taskq tray --snapshot DIR/shots
# alpha admits (long titles, ask, failed, a newer version to update to, history over the last day: 3 closes and
# 3 failed in a day, the first and last of 12 bins not empty, Agents rows); beta is paused with a saved focus, has a service error and the
# same issue number #12 in another repo; gamma is observation only with a stale status.json. No board, no service.
set -eu
d=${1:?usage: tray-fixture.sh DIR}
mkdir -p "$d/install" "$d/alpha/.taskq" "$d/beta/.taskq" "$d/gamma/.taskq"
d=$(cd "$d" && (pwd -W 2>/dev/null || pwd))
printf '%s\n' "$d/alpha" "$d/beta" "$d/gamma" > "$d/install/projects"
now=$(date +%s)
long='A very long task title that never fits in one line of a 320 px popover at sixteen pixels, so it must ellipsize'
cat > "$d/alpha/.taskq/status.json" <<EOF
{"project":"alpha","host":"mac","version":"4.1.0","issues":"https://github.com/o/alpha/issues","log":"$d/alpha.log",
 "focus":["area-ui"],"error":null,"request":"Work on the UI","update":"4.1.1","events_since":$((now-86400)),
 "agents":{"at":$((now-120)),"rows":[{"runtime":"claude","path":"/bin/claude","version":"2.1.3","login":"logged in","account":"owner@example.com"},
 {"runtime":"codex","path":"/bin/codex","version":"0.15.3","login":"not logged in","account":null},
 {"runtime":"codex-app","path":null,"version":null,"login":"not installed","account":null}]},"tasks":[
 {"n":12,"title":"$long","state":"ask","labels":["taskq","area-ui"]},
 {"n":13,"title":"Fix <b>escaping</b> & quotes \" '","state":"doing","labels":["taskq","area-ui"]},
 {"n":14,"title":"Broken build","state":"failed","labels":["taskq","area-maps"]},
 {"n":15,"title":"Next up","state":"ready","labels":["taskq"]}]}
EOF
printf 'since %s\n%s #9 close 101\n%s #10 close 102\n%s #11 failed 103\n%s #9 close 101\n%s #8 failed 99\n%s #7 close 98\n%s #6 failed 97\n' \
  $((now-86400)) $((now-3000)) $((now-600)) $((now-300)) $((now-3000)) $((now-86000)) $((now-80000)) $((now-50000)) > "$d/alpha/.taskq/events"
cat > "$d/beta/.taskq/status.json" <<EOF
{"project":"beta","host":"mac","version":"4.1.0","issues":"https://gitlab.example.com/o/beta/-/issues","log":"$d/beta.log",
 "focus":[],"error":"board read: HTTP 502","request":null,"events_since":null,"tasks":[
 {"n":12,"title":"Same number, other repo","state":"ask","labels":["taskq","area-api"]},
 {"n":20,"title":"Waiting on #12","state":"waiting","labels":["taskq"]}]}
EOF
echo area-api > "$d/beta/.taskq/focus.paused"
cat > "$d/gamma/.taskq/status.json" <<EOF
{"project":"gamma","host":"mac","version":"4.1.0","issues":"https://github.com/o/gamma/issues","log":null,
 "focus":[],"error":null,"request":null,"events_since":null,"tasks":[
 {"n":3,"title":"Observed only","state":"ready","labels":["taskq","area-docs"]}]}
EOF
touch -t 202001010000 "$d/gamma/.taskq/status.json"
