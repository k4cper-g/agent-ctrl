# macOS AX Surface

`surface-ax` is the next native surface after Windows UIA. It is now a partial
automation backend: snapshots, window listing, window focus, first element
actions, checkable controls, and keyboard input are implemented, but it is not
at Windows UIA parity.

## Current Coverage

- Checks `AXIsProcessTrusted()` when opening an AX session.
- Captures the focused window by default.
- Captures a focused or first window for `WindowTarget::Pid`.
- Captures a focused or first window for `WindowTarget::ProcessName`.
- Captures the first window whose title contains `WindowTarget::Title`.
- Walks `AXChildren` up to the requested snapshot depth.
- Maps common AX roles to the shared `Role` taxonomy.
- Reads common AX state: enabled, focused, selected, checked, and expanded.
- Reads AX position/size into shared bounds.
- Assigns refs to interactive and content nodes.
- Lists top-level AX windows for the pinned app using session-oriented ids like
  `pid:123:window:0`.
- Supports `focus-window` for those ids through the AX `AXRaise` action.
- Stores the latest snapshot refs on the session and rediscover elements by
  `(role, name, nth)` before acting.
- Supports `click` through `AXPress`.
- Supports `focus` by setting `AXFocused`.
- Supports `fill` by setting `AXValue`.
- Supports `check`, `uncheck`, and `toggle` for AX controls with readable
  `AXValue` check state.
- Supports `type`, `press`, `key-down`, and `key-up` through
  CoreGraphics keyboard events.
- Captures `screenshot` for `--target window` (default), `--target desktop`,
  `--target region`, and `--target ref` through `CGWindowListCreateImage`,
  including `--annotated` overlays driven by the cached snapshot bounds.
- Drives `double-click`, `right-click`, `hover`, `highlight`, `drag`,
  `scroll`, and raw `mouse move/down/up/wheel` through CoreGraphics events.
  These actions raise the pinned window first, then post the event at the
  element's center (or the supplied screen coordinates).
- Supports `select-all` (focus + `Cmd+A`), `clear` (focus + `AXValue=""`),
  `scroll-into-view` (`AXScrollToVisible` on the resolved element), and
  `clipboard` read/write through `pbpaste`/`pbcopy` plus copy/paste through
  the platform `Cmd+C` / `Cmd+V` chord.

Combo-box / popup `select`, richer stale-ref recovery, and app switching are
still unsupported for AX.

## Permission

macOS requires Accessibility permission before one process can inspect or drive
another process.

Grant the built binary access in:

```text
System Settings > Privacy & Security > Accessibility
```

Add the exact `agent-ctrl` binary you will run. Rebuilding into a different
path may require granting permission again.

`screenshot` additionally requires Screen Recording permission on macOS 10.15
and later. Without it, `CGWindowListCreateImage` returns null or a blank
image; grant access in:

```text
System Settings > Privacy & Security > Screen Recording
```

Add the same `agent-ctrl` binary and restart the daemon (`agent-ctrl close`
then `open ax`) for the new permission to take effect.

## Recommended Validation

From a macOS host:

```bash
cargo build -p agent-ctrl-cli -p agent-ctrl-ax-fixture
target/debug/agent-ctrl info
target/debug/agent-ctrl open ax --session ax
target/debug/agent-ctrl-ax-fixture --ready-file /tmp/agent-ctrl-ax-fixture.ready &
target/debug/agent-ctrl snapshot --target-process agent-ctrl-ax-fixture --session ax --json
target/debug/agent-ctrl window-list --session ax --json
TEXT_REF="$(target/debug/agent-ctrl find --role text-field --first --session ax)"
target/debug/agent-ctrl fill "$TEXT_REF" "hello from ax" --session ax
CHECK_REF="$(target/debug/agent-ctrl find "Enable advanced mode" --role checkbox --first --session ax)"
target/debug/agent-ctrl check "$CHECK_REF" --session ax
target/debug/agent-ctrl screenshot /tmp/agent-ctrl-ax.png --annotated --session ax
target/debug/agent-ctrl close --session ax
```

For automated local coverage:

```bash
cargo build -p agent-ctrl-ax-fixture
RUN_AX_TESTS=1 cargo test -p agent-ctrl-cli --test macos_ax_fixture
```

If `open ax` fails with a permission error, grant Accessibility permission and
retry from a new terminal.

## Roadmap

1. Stabilize keyboard-action validation under the Rust test harness.
2. Add `select` (popup-button / combo-box) through AX menu traversal.
3. Add richer stale-ref recovery using AX identifier/title/role/nth paths.
4. Add `switch-app` through NSWorkspace bundle ids.
5. Expand the fixture with popup-button, scroll view, and dialog controls.
