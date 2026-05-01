# agent-ctrl — handoff for a new agent session

You are continuing work on `agent-ctrl`, a cross-platform computer-use CLI for AI agents — same shape as Vercel Labs' [agent-browser](https://github.com/vercel-labs/agent-browser), but for any OS rather than just the browser. This file gets you productive in ~5 minutes. For deeper background, see `AGENTS.md` (conventions), `README.md` (workspace map + quickstart), and `docs/uia-mapping.md` (Windows UIA design contract).

## What we're building

A unified, accessibility-tree-based schema and runtime that lets an AI agent drive Windows / macOS / Android / iOS / browser through one consistent set of shell commands — `agent-ctrl open uia`, `agent-ctrl snapshot`, `agent-ctrl click @e3`, `agent-ctrl fill @e5 "text"`, `agent-ctrl close`. A long-running daemon per named session holds the platform-specific state (UIA tree, pinned HWND, RefMap) across short-lived CLI invocations.

Stack: **Rust workspace** (engine + daemon + per-platform surfaces) + **npm workspace** (TypeScript client over stdio JSON-RPC, kept around for non-CLI consumers).

## Current state

✅ Windows UIA surface is **feature-complete**. Every `Action` in `agent_ctrl_core::action::Action` has a real implementation. The agent-facing CLI is **live** and verified against live Notepad. `info` and `doctor` give an agent a side-effect-free way to figure out what surfaces are usable on the host.

Verified in the last session:

- `cargo test --workspace` — 26 tests pass (now includes info + doctor unit tests)
- `npm run test --workspace=@agent-ctrl/client` — 3 mock tests pass
- `RUN_UIA_TESTS=1 npm run test ...` — 9 live UIA tests pass against Notepad in ~10s (stdio transport)
- **Live CLI smoke** — full agent flow against Notepad: `open` → `snapshot --target-process Notepad` prints a 23-ref tree, `fill @e0 "..."` updates the document, `screenshot` writes a 52KB PNG, `close` cleanly removes the state file. `doctor` spawns a mock daemon, round-trips a snapshot, shuts it down — all in one go.
- `cargo clippy --workspace --all-targets -- -D warnings` ✓
- `cargo fmt --all -- --check` ✓
- `tsc --noEmit` ✓

## Workspace map

| Path | What |
|---|---|
| `crates/core` | Schema, `Surface` trait, action vocabulary, role taxonomy. No platform deps. |
| `crates/daemon` | Long-running process. Stdio JSON-RPC for the TS client; TCP JSON-RPC for the CLI. Owns `Surface` sessions. Session-discovery state files. Sync TCP client helper. |
| `crates/cli` | The `agent-ctrl` binary. `open` / `close` / `list` plus the full action verb surface. `@eN` ref parsing. Pretty-printed snapshot tree. |
| `crates/surface-uia` | Windows UIA — feature-complete. |
| `crates/surface-cdp` | Chromium-via-CDP — stub. |
| `crates/surface-ax` | macOS AX — stub. |
| `packages/client` | `@agent-ctrl/client` — typed TS wrapper over the stdio transport. |
| `docs/uia-mapping.md` | UIA → schema mapping doc. Update if you change UIA behavior. |
| `agent-browser/` | Reference clone. Not built. `.gitignore`d. |

## Architecture in 60 seconds

- **`Surface` trait** ([crates/core/src/surface.rs](crates/core/src/surface.rs)) — every platform implements `kind()`, `capabilities()`, `snapshot(opts)`, `act(action)`, `shutdown()`. All take `&self` except `shutdown`.
- **`Snapshot` schema** ([crates/core/src/snapshot.rs](crates/core/src/snapshot.rs)) — `app`, `window`, `root: Node` tree, `refs: RefMap`. The `RefMap` is the keystone: agents receive opaque `RefId`s; surfaces re-resolve them at action time. CLI translates these to/from `@eN` for ergonomics.
- **Daemon transports** ([crates/daemon/src/ipc.rs](crates/daemon/src/ipc.rs)) — `run_stdio` (one-daemon-per-process, used by the TS client) and `run_tcp` (long-running, used by the CLI). Both share `handle_line` so the wire shape can't drift. `RequestOp::Shutdown` lets the CLI stop a TCP daemon cleanly.
- **Session discovery** ([crates/daemon/src/session_file.rs](crates/daemon/src/session_file.rs)) — JSON state file at `~/.agent-ctrl/<session>.json` with endpoint + pid + auto-opened daemon-side session id. Liveness is a TCP connect probe; stale files are pruned automatically.
- **CLI client** ([crates/daemon/src/client.rs](crates/daemon/src/client.rs)) — synchronous one-shot connect / write / read; that's all each `agent-ctrl` invocation needs.
- **UIA surface threading** ([crates/surface-uia/src/windows_impl.rs](crates/surface-uia/src/windows_impl.rs)) — `IUIAutomation` is `!Send`, so we own a dedicated worker thread holding the COM objects. Public `run<F, R>` dispatches a closure onto the worker via mpsc + tokio oneshot.

## Load-bearing decisions (non-obvious from the code)

These are invariants that will silently break things if you violate them. Read before editing.

1. **`nth` in `RefMap` is global per snapshot, not per parent.** Pre-order DFS counter, separate counter per `(role, name)` pair. The action-time walker (`find_in_tree`) mirrors this exact ordering.
2. **`element_qualifies_as_ref` must match snapshot's emit predicate exactly.** Snapshot emits a ref when the role is interactive OR the element has an editable `ValuePattern`. Action-time resolution mirrors that. If you change one, change the other.
3. **`promoted_role` is shared between snapshot and action-time.** `MenuItem`+Toggle → `MenuItemCheckbox`, `Window`+IsModal → `Dialog`, `ListItem` whose parent has `Selection` → `Option`. Both `build_node` and `element_qualifies_as_ref` call it so the `(role, name, nth)` tuple stays consistent.
4. **`SnapshotOptions::target` pins the window for subsequent actions.** Snapshot resolves a `WindowTarget` to an HWND, stores it on `WorkerState.last_hwnd`. Actions reuse that HWND, NOT the foreground.
5. **Wire protocol uses `#[serde(flatten)]` + internally-tagged enums.** `Request = { id, #[serde(flatten)] op }` where `RequestOp` is `#[serde(tag = "op")]`. Don't add fields that conflict with the inner enum's variant fields.
6. **COM init is MTA on the UIA worker thread, with strict drop order.** `CoInitializeEx(MULTITHREADED)` → use → drop COM objects → `CoUninitialize`. Never marshal `IUIAutomationElement` across threads.
7. **CLI uses TCP localhost on every platform.** Simpler than per-OS named pipes / Unix sockets, and the localhost firewall blocks remote access.
8. **Daemon writes/removes its own state file.** The CLI's `close` is a courtesy cleanup but the daemon also handles SIGINT and graceful exit. Don't add file management on the CLI side that fights this.

## What works today (UIA surface)

- **Snapshot** of any visible top-level window. Full tree with `Role`, `name`, `state` (visible/enabled/focused/checked/expanded/selected/required), DPI-normalized `bounds`, `value` from `ValuePattern`, `RefId`s for interactive nodes plus elements with editable values. App + window context. `NativeHandle::Uia { runtime_id, automation_id }` populated.
- **Actions** (every variant of `Action`):
  - `Click` — `InvokePattern.Invoke()`
  - `Focus` — `IUIAutomationElement::SetFocus()`
  - `Fill` — `ValuePattern.SetValue(BSTR)` (best path for Unicode)
  - `Type` — SendInput with `KEYEVENTF_UNICODE`. Caveat: Win11 Notepad's WinUI input layer mangles non-ASCII; agents wanting reliable text use Fill.
  - `Press` / `KeyDown` / `KeyUp` — SendInput with VK codes; chord parser handles `"Ctrl+Shift+T"`.
  - `Select` — `SelectionItemPattern.Select()`; falls back to walking children for a named option.
  - `SelectAll` — Focus the field (if ref given), Press `Ctrl+A`.
  - `ScrollIntoView` — `ScrollItemPattern.ScrollIntoView()`.
  - `Scroll` — SendInput `MOUSEEVENTF_WHEEL` / `HWHEEL`. Sign matches CSS (positive `dy` scrolls content down).
  - `DoubleClick` / `RightClick` / `Hover` / `Drag` — SendInput mouse, screen-space center via `BoundingRectangle`, virtual-desktop absolute coords for multi-monitor.
  - `SwitchApp` — finds the first window owned by the app, brings it forward, re-pins `last_hwnd`, clears `last_refs`.
  - `FocusWindow` — restores from minimized via `WindowPattern.SetWindowVisualState(Normal)`, brings forward.
  - `Screenshot` — `GetWindowDC` + `BitBlt` + PNG via the `png` crate; base64 in `ActionResult.data`.
  - `Wait` — sleeps on the worker (single-worker model, blocks subsequent ops).
- **Action-time fast path**: `AutomationId` lookup via `IUIAutomation::CreatePropertyCondition` + `FindFirst(TreeScope_Subtree, ...)`. Falls back to the `(role, name, nth)` walk on miss.
- **Foreground pinning for SendInput**: `ensure_foreground` uses `AttachThreadInput` to bypass `ForegroundLockTimeout`. Keystrokes go to the snapshot's pinned window, not whichever happens to be in front.
- **Window targeting**: `Foreground` / `Pid` / `Title` / `ProcessName`. `ProcessName` is locale-independent — prefer it over title for portable scripts.
- **Win32 class-name promotion** for `Custom` controls: `Edit`/`Static`/`Button`/`ComboBox`/`SysListView32`/`SysTreeView32`/`RichEdit*`.

## What works today (CLI)

`agent-ctrl --help` lists everything. The interesting ones:

```bash
agent-ctrl info [--json]                       # OS / surfaces / active sessions (cheap)
agent-ctrl doctor [--json] [--fix] [--quick]   # diagnostic incl. mock probe
agent-ctrl launch <path> [args...] [--wait MS] # spawn an app detached
agent-ctrl open <surface> [--session NAME]     # spawn detached daemon
agent-ctrl close [--session NAME]              # stop daemon
agent-ctrl list                                # active sessions

agent-ctrl snapshot [--target-process X|--target-pid N|--target-title T] [--json]
agent-ctrl click @eN | double-click | right-click | hover | focus
agent-ctrl fill @eN "value"
agent-ctrl type "text" | press "Ctrl+A" | key-down/key-up KEY
agent-ctrl select @eN "Option" | select-all [--ref @eN]
agent-ctrl scroll DX DY [--ref @eN] | scroll-into-view @eN
agent-ctrl drag @e1 @e2
agent-ctrl switch-app <app_id> | focus-window <hex_id>
agent-ctrl screenshot [PATH] [--region X,Y,W,H]
agent-ctrl wait MS
```

`info` and `doctor` are the agent's "what can I do here?" probes. `info` is static (OS, build, surface compile status, recommended surface, active sessions). `doctor` adds a live mock-daemon round-trip probe and per-check `pass/warn/fail/info` status with optional fix hints — same shape as agent-browser's `doctor`. Add new check categories alongside new surfaces (CDP wants a Chrome check; AX wants `AXIsProcessTrusted`).

Default session name is `default` so most commands don't need `--session`. Snapshot output is a tree with `@eN` refs, role, name, value (truncated to 60 chars), and state annotations like `[focused]` / `[checked=true]` / `[selected]`. `--json` dumps the raw structure.

## What's NOT yet built (in priority order)

1. **CDP surface** — cross-platform browser. After UIA, this is the next biggest demo unlock. Use agent-browser's `cli/src/native/cdp/` as the reference. The CLI's `open cdp` should already plumb through once `factory.rs` knows how to instantiate a CDP surface.
2. **macOS AX surface** — once UIA shape is stable.
3. **Distribution as an npm package** — ship the Rust binary inside an npm package the way agent-browser does (`npm install -g agent-ctrl`). Important for adoption.
4. **`find` semantic locator verb** — `agent-ctrl find role button click --name "Submit"`. Lets agents pick elements without needing a snapshot first.
5. **`batch` verb** — multiple commands in one invocation, avoids daemon round-trip overhead. Mostly important for performance benchmarks.
6. **TS client TCP transport** — currently the TS client always spawns its own stdio daemon. Adding a TCP mode lets it connect to the same daemon the CLI uses.

## Known UIA edges to revisit when motivated

- **Drag interpolation** — `act_drag` sends `(move src, ldown, move dst, lup)` with no intermediate moves. Some drag-and-drop UIs require multiple `MOUSEEVENTF_MOVE` events; if a real app forces our hand, interpolate inside `act_drag`.
- **Screenshot of occluded windows** — uses `GetWindowDC` + `BitBlt` (current pixels but visible-window only). `PrintWindow` works for occluded windows but defers to the app's WM_PRINT handler and some apps return blank frames.
- **`act_wait` blocks the worker** — fine for the single-worker model but a foot-gun if an agent expects parallel timelines.
- **`Type` round-trip vs WinUI 3** — Win11 Notepad's WinUI input layer drops/reorders/substitutes `KEYEVENTF_UNICODE` keystrokes under load. We document the limitation and the integration test asserts the SendInput call completes, not value round-trip equality. Agents needing reliable text should use Fill.

## Workflow / verification

```bash
# Rust
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test --workspace

# TypeScript (stdio transport)
npm install
npm run typecheck --workspace=@agent-ctrl/client
npm run build --workspace=@agent-ctrl/client
npm run test --workspace=@agent-ctrl/client

# Live UIA tests against Notepad
RUN_UIA_TESTS=1 npm run test --workspace=@agent-ctrl/client -- tests/uia.test.ts

# Live CLI smoke (Windows + an open Notepad)
target/debug/agent-ctrl open uia
target/debug/agent-ctrl snapshot --target-process Notepad
target/debug/agent-ctrl close

# Mock surface — works on any OS, no permissions
target/debug/agent-ctrl open mock
target/debug/agent-ctrl snapshot
target/debug/agent-ctrl close
```

## When you start a new session, you can probably skip rebuilding context if

- You can answer: "what's the difference between `run_stdio` and `run_tcp`, and when does each get used?"
- You can find: "where does the action-time walker mirror the snapshot's pre-order DFS?"
- You know: "why is `ensure_foreground` needed for SendInput but not for `Click`?"
- You know: "what does `~/.agent-ctrl/<session>.json` contain and who writes / reads / removes it?"

If any of those are unclear, re-read this doc plus the linked source files.

## One-line ask for a fresh agent

> "Take a look at HANDOFF.md, then continue from the priority list — start with the highest-ranked item that doesn't conflict with whatever I'm asking you to do this session."
