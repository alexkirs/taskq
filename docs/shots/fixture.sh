#!/bin/sh
# README showcase data for `taskq tray --snapshot` (docs/design/readme-plan.md, Screenshots): two projects with
# plausible tasks in doing, review, ask and ready, one day of history, no errors, fresh status files.
# Edge cases live in docs/design/tray-fixture.sh, not here.
set -eu
d=${1:?usage: fixture.sh DIR}
mkdir -p "$d/install" "$d/storefront/.taskq" "$d/billing-api/.taskq"
d=$(cd "$d" && pwd)
printf '%s\n' "$d/storefront" "$d/billing-api" > "$d/install/projects"
touch "$d/storefront.log" "$d/billing-api.log"
now=$(date +%s) v=${TASKQ_VERSION:-4.2.0}
cat > "$d/storefront/.taskq/status.json" <<EOF
{"project":"storefront","host":"mac","version":"$v","issues":"https://github.com/acme/storefront/issues","log":"$d/storefront.log",
 "focus":["area-checkout"],"error":null,"request":null,"events_since":$((now-86400)),
 "agents":{"at":$((now-60)),"rows":[{"runtime":"claude","path":"/usr/local/bin/claude","version":"2.1.3","login":"logged in","account":"dev@example.com"},
 {"runtime":"codex","path":"/usr/local/bin/codex","version":"0.15.3","login":"logged in","account":"dev@example.com"}]},"tasks":[
 {"n":41,"title":"Keep the cart after sign-in","state":"ask","labels":["taskq","area-checkout"]},
 {"n":42,"title":"Show delivery dates","state":"doing","labels":["taskq","area-checkout"]},
 {"n":43,"title":"Fix the login redirect","state":"review","labels":["taskq","area-checkout"]},
 {"n":44,"title":"Compress product images","state":"ready","labels":["taskq"]}]}
EOF
printf 'since %s\n%s #35 close 201\n%s #36 close 202\n%s #37 close 203\n%s #38 close 204\n%s #39 close 205\n%s #40 close 206\n' \
  $((now-86400)) $((now-82000)) $((now-61000)) $((now-43000)) $((now-20000)) $((now-7000)) $((now-1500)) > "$d/storefront/.taskq/events"
cat > "$d/billing-api/.taskq/status.json" <<EOF
{"project":"billing-api","host":"mac","version":"$v","issues":"https://github.com/acme/billing-api/issues","log":"$d/billing-api.log",
 "focus":["area-invoices"],"error":null,"request":null,"events_since":$((now-86400)),"tasks":[
 {"n":17,"title":"Retry failed card payments once","state":"doing","labels":["taskq","area-invoices"]},
 {"n":18,"title":"Add VAT to PDF invoices","state":"review","labels":["taskq","area-invoices"]},
 {"n":19,"title":"Export invoices as CSV","state":"ready","labels":["taskq","area-invoices"]}]}
EOF
printf 'since %s\n%s #14 close 301\n%s #15 close 302\n%s #16 close 303\n' \
  $((now-86400)) $((now-70000)) $((now-30000)) $((now-9000)) > "$d/billing-api/.taskq/events"
