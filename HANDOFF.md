# agent-ctrl — handoff for a new agent session

You are continuing work on `agent-ctrl`, an open-source cross-platform
computer-use framework. This file gets you productive in ~5 minutes. For
deeper background, see `AGENTS.md` (conventions), `README.md` (workspace
map), and `docs/uia-mapping.md` (Windows UIA design contract).

## What we're building

A unified, accessibility-tree-based schema and runtime that lets an AI agent
drive Windows / macOS / Android / iOS / browser through one consistent
interface. Schema is modeled on Vercel Labs' [agent-browser](https://github.com/vercel-labs/agent-browser)
(cloned at `agent-browser/` in this repo as a reference, NOT a dep) and
extended with cross-platform concepts (apps, windows, native handles).

Stack: **Rust workspace** (engine + daemon + per-platform surfaces) +
**npm workspace** (TypeScript client over stdio JSON-RPC). Distribution
target is the agent-browser / Playwright pattern: ship a Rust binary inside
an npm package.

## Current state — v0.1 milestone reached, keyboard input landed

✅ End-to-end TS → Rust → live Windows app loop **works**, including SendInput-based keyboard.

Verified in the last session:

- `cargo test --workspace` — 13 tests pass (core, daemon, surface-uia keyboard helpers)
- `npm run test --workspace=@agent-ctrl/client` — 3 mock tests pass
- `RUN_UIA_TESTS=1 npm run test ...` — **5 UIA tests pass against live Notepad in ~6s**:
  open session → snapshot Notepad → fill via ValuePattern → click an Invoke menu item → type Unicode via SendInput → clear with `Ctrl+A` + `Delete`.
- `cargo clippy --workspace --all-targets -- -D warnings` ✓
- `cargo fmt --all -- --check` ✓
- `tsc --noEmit` ✓

## Workspace map

| Path | What |
|---|---|
| `crates/core` | Schema and `Surface` trait. No platform deps. |
| `crates/daemon` | Long-running process, session registry, JSON-RPC dispatch (stdio). |
| `crates/cli` | The `agent-ctrl` binary (clap). |
| `crates/surface-uia` | Windows UIA — **only platform with real implementation**. |
| `crates/surface-cdp` | Chromium-via-CDP — stub. |
| `crates/surface-ax` | macOS AX — stub. |
| `packages/client` | `@agent-ctrl/client` TypeScript wrapper. |
| `docs/uia-mapping.md` | Authoritative UIA → schema mapping doc. Update if you change behavior. |
| `agent-browser/` | Reference clone. Not built. `.gitignore`d. |

## Architecture in 60 seconds

- **`Surface` trait** ([crates/core/src/surface.rs](crates/core/src/surface.rs)) — every platform implements `kind()`, `capabilities()`, `snapshot(opts)`, `act(action)`, `shutdown()`. All take `&self` except `shutdown` — surfaces use interior mutability if they need it.
- **`Snapshot` schema** ([crates/core/src/snapshot.rs](crates/core/src/snapshot.rs)) — `app`, `window`, `root: Node` (tree), `refs: RefMap`. The `RefMap` is the keystone: agent receives opaque `RefId`s; surfaces re-resolve them to native elements at action time.
- **Daemon** ([crates/daemon/src/dispatcher.rs](crates/daemon/src/dispatcher.rs)) — `Request { id, op }` and `Response { id, body }`, both `serde(flatten)` over an internally-tagged enum. Stdio JSON-RPC: one line in, one line out, correlation by `id`.
- **TS client** ([packages/client/src/client.ts](packages/client/src/client.ts)) — spawns the daemon, maintains a pending-id table, exposes `openSession`, `snapshot`, `act`, `closeSession`, `close`.
- **UIA surface threading** ([crates/surface-uia/src/windows_impl.rs](crates/surface-uia/src/windows_impl.rs)) — `IUIAutomation` is `!Send`, so we own a dedicated worker thread (`UiaInner`) holding the COM objects. Public `run<F, R>` dispatches a closure onto the worker via mpsc + tokio oneshot reply.

## Load-bearing decisions (non-obvious from the code)

These are invariants that will silently break things if you violate them. Read before editing.

1. **`nth` in `RefMap` is global per snapshot, not per parent.** Pre-order DFS counter, separate counter per `(role, name)` pair. The action-time walker (`find_in_tree`) mirrors this exact ordering. See `capture_with_options` in `windows_impl.rs:242` and `find_in_tree` at `:893`.

2. **`find_in_tree`'s "qualifies as ref" predicate must match snapshot's emit predicate exactly.** Snapshot emits a ref when `role.is_interactive() || has_editable_value`. `find_in_tree` uses `element_qualifies_as_ref` which queries both. **Bug fixed last session** — if you change the snapshot predicate, change the resolver predicate too. See `windows_impl.rs:937` (`element_qualifies_as_ref`).

3. **`compact` mode hoists children, doesn't change `nth`.** When an unnamed `Generic` is dropped, its children are spliced into the caller's child list. This is purely a presentation concern — refs and `nth` were already allocated against the raw tree. Don't move ref allocation around without understanding this.

4. **`SnapshotOptions::target` pins the window for subsequent actions.** Snapshot resolves a `WindowTarget` (`Foreground`/`Pid`/`Title`/`ProcessName`) to an HWND, stores it on `WorkerState.last_hwnd`. Actions reuse that HWND, NOT the foreground. This is what lets us drive non-foreground apps reliably.

5. **Wire protocol uses `#[serde(flatten)]` + internally-tagged enums.** `Request = { id: String, #[serde(flatten)] op: RequestOp }` where `RequestOp` is `#[serde(tag = "op")]`. Same for `Response`. JSON shape is `{"id":"...","op":"snapshot",...}`. Don't add fields that conflict with the inner enum's variant fields.

6. **COM init is MTA on the worker thread, with strict drop order.** `CoInitializeEx(MULTITHREADED)` → use → drop COM objects → `CoUninitialize`. Never marshal `IUIAutomationElement` across threads — they're `!Send` for a reason.

7. **`SurfaceKind::Mock`** lives in core and is feature-gated by `mock` (default off). The daemon enables it. Don't expose `MockSurface` from a non-test code path without the feature.

## What works today (UIA surface specifically)

- **Snapshot** of any visible top-level window. Walks the Control view, builds the unified `Node` tree with `Role`, `name`, `state`, `bounds` (DPI-normalized via `GetDpiForWindow`), `value` (from `ValuePattern` when present), allocates `RefId`s for interactive nodes plus elements with editable `ValuePattern`. App context (exe path + name from PID) and window context (HWND + title) populated.
- **Action: Click** — `IUIAutomationInvokePattern.Invoke()`.
- **Action: Focus** — `IUIAutomationElement::SetFocus()`.
- **Action: Fill** — `IUIAutomationValuePattern.SetValue(BSTR)`.
- **Action: Type** — `SendInput` with `KEYEVENTF_UNICODE`, one (down,up) pair per UTF-16 code unit. Bypasses keyboard layout entirely; emoji and non-Latin scripts work.
- **Action: Press / KeyDown / KeyUp** — `SendInput` with virtual-key codes. Chord parser handles `"Ctrl+Shift+T"` style strings; modifiers are pressed in order and released in reverse. Key-name table covers letters, digits, `F1..F24`, modifiers (`Ctrl`/`Shift`/`Alt`/`Win`), navigation (`Home`/`End`/arrows/`PageUp`/`PageDown`), `Enter`/`Tab`/`Space`/`Escape`/`Backspace`/`Delete`/`Insert`, lock keys, `PrintScreen`/`Pause`/`Apps`.
- **Foreground pinning for SendInput**: every keyboard helper calls `ensure_foreground` first, which uses the `AttachThreadInput` workaround to bypass `ForegroundLockTimeout`. Without it, keystrokes go to whatever window happened to be foreground (typically the IDE), not the snapshot's pinned HWND.
- **Window targeting**: `Foreground` / `Pid` / `Title` / `ProcessName`. `ProcessName` is locale-independent and the right default for tests.
- **Win32 class-name promotion** for `Custom` controls: `Edit`/`Static`/`Button`/`ComboBox`/`SysListView32`/`SysTreeView32`/`RichEdit*` get promoted back to canonical roles.

## What's NOT yet built (in priority order)

1. **Populate `NativeHandle::Uia`** with `RuntimeId` + `AutomationId`. Currently `None`. With handles, action-time resolution can use `IUIAutomation::FindFirstBuildCache` with a `UIA_AutomationIdPropertyId` condition — O(1) instead of O(tree). See `docs/uia-mapping.md` §7.
2. **Pattern-based state extraction**: `Toggle.ToggleState` → `state.checked`, `ExpandCollapse.State` → `state.expanded`, `SelectionItem.IsSelected` → `state.selected`. See `docs/uia-mapping.md` §2.
3. **Pattern-based role promotion**: MenuItem+Toggle → MenuItemCheckbox, Window+IsModal → Dialog, ListItem-in-Selection → Option. See `docs/uia-mapping.md` §1 promotion rules.
4. **Remaining actions** per `docs/uia-mapping.md` §4: DoubleClick, RightClick, Hover, Scroll, ScrollIntoView, Select, SelectAll, Drag, SwitchApp, FocusWindow, Screenshot.
5. **CDP surface implementation** — cross-platform browser surface. Use agent-browser's `cli/src/native/cdp/` as the reference. After UIA, this is the next biggest demo unlock.
6. **macOS AX surface** — once UIA shape is stable.

## Known pitfalls (you will hit these)

- **Win11 Notepad's edit area is a `Document` with `ValuePattern`**, NOT an `Edit` / `TextField`. The "would have been a TextField" promotion is via `has_editable_value` flag on `build_node` — that's why we emit refs for Documents that have editable value patterns. Don't "fix" the role mapping to coerce Document → TextField.
- **Window titles are localized.** This is a Polish Windows install — Notepad's title is `"Bez tytułu — Notatnik"`, not `"Untitled - Notepad"`. Tests use `WindowTarget::ProcessName` (locale-independent) instead.
- **Windows `ForegroundLockTimeout`** silently rejects `WScript.Shell.AppActivate` from non-foreground processes. We worked around this by NOT depending on foreground for tests — `WindowTarget::ProcessName` finds Notepad's HWND directly.
- **HWND can be reused** after a window closes. We don't currently validate that `last_hwnd` still points at the same window. If it's been reused, you'd walk a different tree and `find_in_tree` would fail to find the target with a clean error. Acceptable for v0.1, worth tightening later.
- **`OpenProcess` can fail on protected processes** (LSASS, etc.). `find_window_by_process_name` enumerates ALL visible top-level windows and calls `process_info` on each — that's O(N_windows) `OpenProcess` calls. Slow on busy desktops, fails silently for protected processes (which is fine — we just skip them).
- **Pedantic clippy is on workspace-wide.** Every PR must pass `cargo clippy --workspace --all-targets -- -D warnings`. Allow lints with comments, never silently.
- **`unsafe_code = "warn"` workspace, `allow` per-crate** for surfaces that need FFI. Keep `unsafe` blocks small with `// SAFETY:` justifications.

## Workflow / verification

```bash
# Rust
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test --workspace

# TypeScript
npm install                                                   # one-time
npm run typecheck --workspace=@agent-ctrl/client
npm run build --workspace=@agent-ctrl/client
npm run test --workspace=@agent-ctrl/client                   # mock tests only

# UIA integration test (Windows + an open Notepad)
RUN_UIA_TESTS=1 npm run test --workspace=@agent-ctrl/client -- tests/uia.test.ts

# Smoke a snapshot from CLI
cargo run -q -p agent-ctrl-cli -- snapshot --surface uia --target-process Notepad
cargo run -q -p agent-ctrl-cli -- snapshot --surface mock         # works on any OS
```

## Deferred issues from last session's code review

These are real issues but didn't block v0.1. Pick when relevant:

- **Serial stdio loop in daemon** ([ipc.rs](crates/daemon/src/ipc.rs)) — `dispatch().await` blocks reading the next line. The TS client's "concurrent calls are matched by id" promise is technically misleading; concurrent requests get serialized at the daemon. Fix: spawn each `dispatch` on `tokio::spawn` with an mpsc-fed writer task for ordered stdout.
- **`u64`/`i64` precision in `NativeHandle` TS types** ([types.ts:51-54](packages/client/src/types.ts#L51-L54)) — JS `number` is 53-bit. Becomes a real bug the moment we populate native handles on macOS AX (above 2^53 is common). Fix: type as `bigint` with custom JSON parse, or serialize as string.
- **Loose TS `Role` type** — `string | { unknown: string }` instead of an enumerated kebab-case string union. Aesthetic.
- **`Error::PermissionDenied` for wrong-platform** ([factory.rs:27-30](crates/daemon/src/factory.rs)) — misleading message. Add `Error::WrongPlatform`.
- **`RefMap.entries`/`next` exposed via serde without `#[serde(rename = ...)]`** — silently breaks the wire if anyone renames the private fields. Add explicit renames + a roundtrip test.
- **CLI `let _ = dispatch(... CloseSession ...)`** silently swallows close errors. Wrap with `tracing::warn!`.
- **`MockSurface::act` poisons mutex on panic, `actions()` recovers** — inconsistent. Use `parking_lot::Mutex` or recover in both.

## Auto-memory (already loaded into Claude sessions)

The user has these stored in their auto-memory directory:

- `project_agent_ctrl.md` — what agent-ctrl is, why
- `user_role.md` — k4cper-g is the founder; TS/Python background, learning Rust
- `reference_agent_browser.md` — pointer to the cloned reference repo
- `project_architecture_mapping.md` — concrete mapping from agent-browser's structures to ours

If your session loads memory, those will be available. If not, this file is self-contained.

## When you start a new session, you can probably skip rebuilding context if

- You can answer: "what is `Surface::snapshot`'s contract w.r.t. the `RefMap`?"
- You can find: "where does the action-time walker mirror the snapshot's pre-order DFS?"
- You know: "why is `nth` global instead of per-parent?"
- You know: "why do we have a worker thread on the UIA surface specifically?"

If any of those are unclear, re-read this doc plus the linked source files.

## One-line ask for a fresh agent

> "Take a look at HANDOFF.md, then continue from the priority list — start with the highest-ranked item that doesn't conflict with whatever I'm asking you to do this session."
