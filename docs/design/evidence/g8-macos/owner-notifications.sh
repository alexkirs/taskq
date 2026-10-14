#!/bin/sh
# G8 (#1051) owner checkpoint: TaskQ.app notification permission, banner, click and Blocked on this Mac.
# Usage from the repository root: sh docs/design/evidence/g8-macos/owner-notifications.sh [candidate taskq binary]
# Disposable install and fixture project under $TMPDIR; the real install, its tray and every board stay untouched.
# Bundle `org.taskq.tray` shares the OS permission with any later real TaskQ.app on this Mac.
set -eu
BIN=${1:-target/release/taskq}
W=${TMPDIR:-/tmp}/taskq-1051-owner
I=$W/inst P=$W/proj
APP=$I/TaskQ.app
rm -rf "$W" && mkdir -p "$APP/Contents/MacOS" "$P/.taskq" "$W/shots"
# no `current`: the tray then never exits over a version mismatch
cat > "$APP/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>org.taskq.tray</string>
<key>CFBundleName</key><string>TaskQ</string>
<key>CFBundleExecutable</key><string>taskq</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>LSUIElement</key><true/>
</dict></plist>
EOF
cp "$BIN" "$APP/Contents/MacOS/taskq"
codesign -f -s - "$APP"
echo "$P" > "$I/projects"
echo "since $(date +%s)" > "$P/.taskq/events"
cat > "$P/.taskq/status.json" <<'EOF'
{"schema":1,"project":"g8-notify-fixture","complete":true,"tasks":[{"n":7,"state":"ask","title":"Fixture question for the notification check","revision":1}],"focus":["all"],"issues":"https://github.com/taskqorg/taskq/issues"}
EOF
log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$W/result.txt"; }
ask() { printf '%s [y/n] ' "$1"; read -r a; log "owner: $1 -> $a"; }

log "start $(shasum -a 256 "$BIN" | cut -c1-64)"
TASKQ_INSTALL=$I "$APP/Contents/MacOS/taskq" tray 2>"$W/tray.log" &
TRAY=$!
echo "1. A macOS prompt asks whether TaskQ may send notifications. Click Allow."
ask "Did the prompt appear and did you click Allow?"
sleep 3
echo "$(date +%s) #7 ask 7001" >> "$P/.taskq/events"
log "appended ask event for #7"
echo "2. A banner 'g8-notify-fixture · Question' with sound appears within ~5 s. Click it."
ask "Did the banner appear?"
ask "Did the click open the TaskQ menu on #7?"
echo "3. System Settings > Notifications > TaskQ: turn Allow notifications off, then come back here."
open "x-apple.systempreferences:com.apple.Notifications-Settings.extension" || true
ask "Is TaskQ notifications now off?"
echo "$(date +%s) #7 failed 7002" >> "$P/.taskq/events"
log "appended failed event for #7 while blocked"
sleep 6
ask "Did any banner appear in the last 6 s? (expected: no)"
kill "$TRAY" 2>/dev/null || true
sleep 1
TASKQ_INSTALL=$I "$APP/Contents/MacOS/taskq" tray --snapshot "$W/shots" >/dev/null 2>>"$W/tray.log" || log "snapshot failed"
cp "$I/tray.json" "$W/tray.json" 2>/dev/null || true
/usr/bin/log show --last 20m --style compact --predicate 'eventMessage CONTAINS "org.taskq.tray"' > "$W/unified.log" 2>&1 || true
log "done: $W (result.txt, shots/popover-*-notifications-16.png, tray.json, unified.log)"
