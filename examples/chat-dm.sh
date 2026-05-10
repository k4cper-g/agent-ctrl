#!/usr/bin/env bash
# chat-dm.sh - drive a Slack/Teams-style chat app to a DM and compose a message.
#
# Demonstrates the pattern for web-based desktop apps (Electron/Chromium):
#   switch-app          - bring the app forward, even if it's parked in the tray
#   snapshot --settle    - wait out Chromium's lazy a11y tree (the first plain
#                          snapshot is usually just the window frame)
#   press "Ctrl+K"       - the app's quick switcher / "jump to"
#   type                 - synthetic keystrokes, so the app's React input
#                          handlers actually register the text (fill / SetValue
#                          would not fire the `input` event these apps listen for)
#   re-snapshot + verify - confirm the composer holds your text and the Send
#                          control is enabled BEFORE the irreversible step
#   press "Enter"        - send (gated behind SEND=1; off by default)
#
# Defaults to Slack. Teams: set APP=ms-teams and SWITCHER="Ctrl+E".
#
# Reads cleanly even if you don't run it. By default it stops at the verify
# step and leaves the message in the composer for you to review and send -
# pass SEND=1 to actually send it.
#
# Requires: agent-ctrl on PATH, Windows, and the app already open with a
# window (visible or minimized). If it's parked in the system tray, click its
# tray icon first, or set APP_PATH=<full exe path> so this script can
# `agent-ctrl launch` it (which routes through the proper activation broker;
# tray apps re-hide a window you'd try to un-hide by hand, so don't).

set -e

APP="${APP:-Slack}"               # process executable file stem
APP_PATH="${APP_PATH:-}"          # optional: full exe path to `launch` if not visible
RECIPIENT="${RECIPIENT:-Kacper Gadomski}"
MESSAGE="${MESSAGE:-agent-ctrl test message}"
SWITCHER="${SWITCHER:-Ctrl+K}"    # the app's quick-switcher shortcut
SEND="${SEND:-0}"                 # set to 1 to actually press Enter
SESSION="chat-dm"

cleanup() {
  agent-ctrl close --session "$SESSION" 2>/dev/null || true
}
trap cleanup EXIT

# 1. Spawn a UIA daemon for this session.
echo "→ opening UIA session..."
agent-ctrl open uia --session "$SESSION"

# 2. Bring the app forward. switch-app foregrounds (and un-minimizes) the app's
#    visible window. If it has none - parked in the tray, or not running -
#    `launch` it via APP_PATH, or bail with guidance.
echo "→ bringing $APP forward..."
if ! agent-ctrl switch-app "$APP" --session "$SESSION" 2>/dev/null; then
  if [ -n "$APP_PATH" ]; then
    echo "  ($APP has no visible window; launching $APP_PATH ...)"
    agent-ctrl launch "$APP_PATH" --wait 2000
  else
    echo "✗ $APP has no visible window. Click its tray icon to bring it up," >&2
    echo "  or re-run with APP_PATH=<full exe path> so I can launch it." >&2
    exit 1
  fi
fi

# 3. Snapshot with --settle: Chromium builds its UIA tree lazily on first
#    query, so a plain snapshot right after switching is usually just the
#    window frame. --settle re-snapshots until the tree stops changing.
echo "→ snapshotting $APP (settling)..."
agent-ctrl snapshot --settle --target-process "$APP" --session "$SESSION" | head -6
echo "  ..."

# 4. Open the quick switcher and type the recipient's name. Ctrl+A first so
#    the keystrokes replace whatever the box started with.
echo "→ opening the quick switcher and searching for \"$RECIPIENT\"..."
agent-ctrl press "$SWITCHER" --session "$SESSION"
agent-ctrl press "Ctrl+A" --session "$SESSION"
agent-ctrl type "$RECIPIENT" --session "$SESSION"

# 5. Wait for the result to show up, snapshot, and click the DM option. find
#    does a case-insensitive substring match, so "$RECIPIENT" matches an entry
#    labelled "$RECIPIENT, Direct Message".
agent-ctrl wait-for "$RECIPIENT" --role option --timeout 5000 --session "$SESSION"
agent-ctrl snapshot --session "$SESSION" >/dev/null
DM_REF="$(agent-ctrl find "$RECIPIENT" --role option --first --session "$SESSION")"
echo "→ opening DM ($DM_REF)..."
agent-ctrl click "$DM_REF" --session "$SESSION"

# 6. Snapshot the DM view (settle again - opening a conversation re-renders),
#    then grab the message composer. It's a text-field named "Message ...".
agent-ctrl snapshot --settle --target-process "$APP" --session "$SESSION" >/dev/null
COMPOSER="$(agent-ctrl find "Message" --role text-field --first --session "$SESSION")"
echo "→ composer is $COMPOSER; typing message..."
agent-ctrl focus "$COMPOSER" --session "$SESSION"
agent-ctrl press "Ctrl+A" --session "$SESSION"
agent-ctrl press "Delete" --session "$SESSION"
agent-ctrl type "$MESSAGE" --session "$SESSION"

# 7. CHECKPOINT. Re-snapshot, re-find the composer (refs are per-snapshot), and
#    confirm it actually holds the message before we do anything irreversible.
agent-ctrl snapshot --session "$SESSION" >/dev/null
COMPOSER="$(agent-ctrl find "Message" --role text-field --first --session "$SESSION")"
VALUE="$(agent-ctrl get value "$COMPOSER" --session "$SESSION")"
echo "→ composer value: $VALUE"
case "$VALUE" in
  *"$MESSAGE"*) : ;;  # good, the message is in there
  *) echo "✗ composer does not contain the message - bailing without sending"; exit 1 ;;
esac

# 8. Send - only if explicitly asked. By default we stop here with the message
#    sitting in the composer for you to review.
if [ "$SEND" = "1" ]; then
  echo "→ sending..."
  agent-ctrl press "Enter" --session "$SESSION"
  agent-ctrl snapshot --session "$SESSION" >/dev/null
  echo "✓ sent."
else
  echo "✓ message composed and verified, not sent (re-run with SEND=1 to send)."
fi

# 9. close runs via the EXIT trap.
