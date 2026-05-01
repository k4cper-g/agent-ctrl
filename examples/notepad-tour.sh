#!/usr/bin/env bash
# notepad-tour.sh - first-time tour of agent-ctrl's verbs against a real app.
#
# Drives Notepad on Windows. Demonstrates:
#   launch     - start the app under agent-ctrl's control
#   open       - spawn a UIA session (the daemon)
#   snapshot   - capture the a11y tree, pinning to the target window
#   find       - query the cached snapshot without re-walking the OS tree
#   wait-for   - block on UI predicates instead of `sleep N`
#   window-list - enumerate all top-level windows owned by the same app
#   close      - tear down the session
#
# Reads cleanly even if you don't run it. Locale-independent (uses --role
# filters, not Polish/English-specific names).
#
# Requires: agent-ctrl on PATH, Windows.

set -e
SESSION="tour"

cleanup() {
  agent-ctrl close --session "$SESSION" 2>/dev/null || true
}
trap cleanup EXIT

# 1. Start Notepad as a detached child. --wait gives the GUI time to draw
#    its window before our next command tries to find it.
echo "→ launching notepad..."
agent-ctrl launch notepad.exe --wait 1500

# 2. Spawn a UIA daemon under a named session. Each session is one pinned
#    target window; multiple sessions can run side by side.
echo "→ opening UIA session..."
agent-ctrl open uia --session "$SESSION"

# 3. Snapshot pins the session to Notepad's main window. Subsequent verbs
#    on this session target that window. The snapshot's @eN refs are
#    valid until the next snapshot replaces them.
echo "→ snapshotting Notepad..."
agent-ctrl snapshot --target-process Notepad --session "$SESSION" | head -8
echo "  ..."

# 4. find queries the cached snapshot. Cheap (no re-walk) and locale-safe
#    when filtered by role.
echo
echo "→ find every button in the snapshot:"
agent-ctrl find --role button --session "$SESSION"

# 5. wait-for replaces flaky `sleep N`. Three modes:
#    - by name/role (appearance)
#    - --gone (disappearance)
#    - --stable (tree signature unchanged for --idle-ms)
#    Idle Notepad with --idle-ms 400 takes ~1s to settle.
echo
echo "→ wait for the tree to settle (--stable):"
agent-ctrl wait-for --stable --idle-ms 400 --timeout 4000 --session "$SESSION"

# 6. window-list shows every top-level window owned by Notepad's process.
#    With just the main window open, the list is one row tagged [pinned focused].
#    If a Save As dialog were open, it'd show as a sibling - that's the
#    canonical multi-window pattern.
echo
echo "→ all windows owned by Notepad:"
agent-ctrl window-list --session "$SESSION"

# 7. close shuts the daemon down and removes the session file.
echo
echo "→ done - session will close via trap on exit."

# Try this next:
#   - Press Ctrl+S in the Notepad window manually to open Save As.
#   - Re-run `agent-ctrl window-list --session $SESSION` and you'll see
#     the dialog appear as a second row.
#   - `agent-ctrl focus-window "$(agent-ctrl window-list --first-other --session $SESSION)"`
#     re-pins the session onto the dialog.
#   - Re-snapshot and the dialog's full a11y tree shows up.
