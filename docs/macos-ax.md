# macOS AX Surface

`surface-ax` is the next native surface after Windows UIA. In v0.1 it is a
snapshot preview, not an action-ready automation backend.

## Current Coverage

- Checks `AXIsProcessTrusted()` when opening an AX session.
- Captures the focused window by default.
- Captures a focused or first window for `WindowTarget::Pid`.
- Walks `AXChildren` up to the requested snapshot depth.
- Maps common AX roles to the shared `Role` taxonomy.
- Reads common AX state: enabled, focused, selected, and expanded.
- Reads AX position/size into shared bounds.
- Assigns refs to interactive and content nodes.
- Lists top-level AX windows for the pinned app using session-oriented ids like
  `pid:123:window:0`.
- Supports `focus-window` for those ids through the AX `AXRaise` action.

Element actions, screenshots, title/process-name targeting, stale-ref recovery,
and app switching are still unsupported for AX.

## Permission

macOS requires Accessibility permission before one process can inspect or drive
another process.

Grant the built binary access in:

```text
System Settings > Privacy & Security > Accessibility
```

Add the exact `agent-ctrl` binary you will run. Rebuilding into a different
path may require granting permission again.

## Recommended Validation

From a macOS host:

```bash
cargo build -p agent-ctrl-cli
target/debug/agent-ctrl info
target/debug/agent-ctrl open ax --session ax
target/debug/agent-ctrl snapshot --session ax --json
target/debug/agent-ctrl window-list --session ax --json
target/debug/agent-ctrl close --session ax
```

If `open ax` fails with a permission error, grant Accessibility permission and
retry from a new terminal.

## Roadmap

1. Validate `list_windows` and `focus-window` behavior on real macOS apps.
2. Add action support through `AXUIElementPerformAction`, `AXValue`, and
   keyboard/mouse fallbacks.
3. Add screenshot support through CoreGraphics window capture.
4. Add stale-ref recovery using AX identifier/title/role/nth paths.
5. Add a deterministic macOS fixture app for CI-friendly AX coverage.
