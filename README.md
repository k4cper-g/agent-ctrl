# agent-ctrl

Computer-use CLI for AI agents. Same shape as [agent-browser](https://github.com/vercel-labs/agent-browser) — but for any OS, not just the browser.

`agent-ctrl` exposes a unified, accessibility-tree based schema across operating systems, so an AI agent can drive Windows, macOS, Android, iOS, and the browser through one consistent set of shell commands. The agent issues `snapshot`, `click @e3`, `fill @e5 "text"` like a human at a terminal; a long-running daemon holds the platform-specific session state across invocations.

> **Status:** Windows UI Automation surface is feature-complete and CLI-driven end-to-end. CDP, macOS AX, Android, and iOS surfaces are scaffolded.

## Quick start (Windows)

```bash
cargo build --release -p agent-ctrl-cli
# put target/release/agent-ctrl on your PATH

agent-ctrl info                                       # what OS am I on, which surfaces are usable
agent-ctrl open uia                                   # spawn a daemon (background)
agent-ctrl snapshot --target-process Notepad          # show Notepad's a11y tree with @eN refs
agent-ctrl fill @e0 "hello from agent-ctrl"           # set the document text
agent-ctrl press "Ctrl+End"                           # move cursor to end
agent-ctrl click @e4                                  # click the File menu
agent-ctrl screenshot result.png                      # save a PNG of the window
agent-ctrl close                                      # stop the daemon
```

### Discovering what works on this machine

Two commands help an agent figure out the environment without running anything destructive:

```bash
agent-ctrl info                  # static facts (OS, recommended surface, active sessions)
agent-ctrl info --json           # parseable for an agent's planner
agent-ctrl doctor                # environment + daemon + live mock-daemon probe
agent-ctrl doctor --json --fix   # JSON-out, prune any stale session files
```

`info` is cheap and side-effect-free — designed to be the first command an agent runs in a fresh session. `doctor` adds a real round-trip probe (spawns a mock daemon, takes a snapshot, shuts it down) to verify the local install works end-to-end. `--quick` skips the probe; `--fix` runs the safe automatic repairs.

The daemon lives at `~/.agent-ctrl/<session>.json` while running. `agent-ctrl list` shows active sessions; `agent-ctrl --session <name> ...` targets a specific one. Default session name is `default`, so most commands need no flag.

Every action follows the same pattern: an agent runs `snapshot` once to learn what's on screen, then issues `click` / `fill` / `type` / `press` / `select` / etc. by ref. Refs are valid only for the snapshot that produced them — re-snapshot before acting on a tree that has changed.

## CLI reference

Use `agent-ctrl --help` for the full list. Highlights:

| Command | Purpose |
|---|---|
| `open <surface> [--session NAME]` | spawn a daemon for the given surface (`uia`, `cdp`, `ax`, `mock`, …) |
| `close [--session NAME]` | stop the daemon and clean up the state file |
| `list` | active sessions across the home directory |
| `snapshot [--target-process X | --target-pid N | --target-title T] [--json]` | capture and print the a11y tree |
| `click @eN` / `double-click` / `right-click` / `hover` / `focus` | pointer + focus on a ref |
| `fill @eN "text"` | replace value via UIA `ValuePattern` (best for Unicode) |
| `type "text"` / `press "Ctrl+A"` / `key-down`/`key-up` | keyboard via SendInput |
| `select @eN "Option"` / `select-all [@eN]` | selection containers |
| `scroll DX DY [--ref @eN]` / `scroll-into-view @eN` | wheel + UIA scroll-item |
| `drag @e1 @e2` | source-to-destination drag |
| `switch-app <app_id>` / `focus-window <hex_id>` | foreground a window |
| `screenshot [PATH] [--region X,Y,W,H]` | PNG of the pinned window or a region |
| `wait MS` | sleep on the daemon worker |
| `info [--json]` | OS / recommended surface / active sessions (no probes) |
| `doctor [--json] [--fix] [--quick]` | environment + daemon + live mock probe |

## Workspace layout

The repository is a **dual workspace** — a Cargo workspace for the Rust engine and an npm workspace for the TypeScript client.

### Rust crates

| Crate | Purpose |
|---|---|
| [`crates/core`](crates/core) | Shared types and the `Surface` trait every platform implements. Schema, role taxonomy, action vocabulary, errors. |
| [`crates/daemon`](crates/daemon) | Long-running process that owns surface sessions and dispatches actions. |
| [`crates/cli`](crates/cli) | The `agent-ctrl` binary — user-facing entrypoint. |
| [`crates/surface-cdp`](crates/surface-cdp) | Chromium-via-CDP surface (browser parity, cross-platform). |
| [`crates/surface-uia`](crates/surface-uia) | Windows UI Automation surface (compiled only on Windows). |
| [`crates/surface-ax`](crates/surface-ax) | macOS Accessibility surface (compiled only on macOS). |

### TypeScript packages

| Package | Purpose |
|---|---|
| [`packages/client`](packages/client) | `@agent-ctrl/client` — typed TS wrapper that spawns the Rust daemon and talks JSON-RPC over stdio. |

## Platforms and surfaces

A **surface** is one accessibility protocol — UIA, AX, CDP, etc. A **platform** is an operating system. The two are not 1-to-1: most platforms can be driven by more than one surface (e.g. on Windows you can use UIA for native apps *and* CDP when you're driving Chrome), and CDP is a single protocol that spans every OS Chrome runs on. That's why the crates are named by surface, not by platform.

| Platform | Native surface | Browser surface | Status |
|---|---|---|---|
| Windows | [`surface-uia`](crates/surface-uia) — UI Automation | [`surface-cdp`](crates/surface-cdp) — Chrome / Edge via CDP | both scaffolded |
| macOS | [`surface-ax`](crates/surface-ax) — Accessibility / AX | [`surface-cdp`](crates/surface-cdp) — Chrome via CDP | both scaffolded |
| Linux | _planned_: `surface-atspi` (AT-SPI / D-Bus) | [`surface-cdp`](crates/surface-cdp) — Chrome via CDP | cdp scaffolded |
| Android | _planned_: `surface-accessibility-service` (AccessibilityService + JNI) | [`surface-cdp`](crates/surface-cdp) — Chrome via CDP | cdp scaffolded |
| iOS | _planned_: `surface-xcuitest` (XCUITest / WebDriverAgent) | [`surface-cdp`](crates/surface-cdp) — limited; iOS Chrome shares WebKit | cdp scaffolded |

Acronyms in one line: **UIA** = Microsoft UI Automation, **AX** = macOS Accessibility, **AT-SPI** = the Linux GNOME accessibility bus, **CDP** = Chrome DevTools Protocol, **XCUITest** = Apple's UI test automation framework.

## Build

Rust:

```bash
cargo check --workspace
cargo build --release -p agent-ctrl-cli
```

Surfaces gated by `target_os` compile to empty crates on other platforms, so the workspace builds on any host.

TypeScript:

```bash
npm install
npm run build --workspace=@agent-ctrl/client
npm run test  --workspace=@agent-ctrl/client
```

The TS test suite spawns the Rust daemon under `cargo run` and exercises the full protocol against the mock surface.

## Mock surface (works on every OS, no permissions needed)

The mock surface returns a fixed two-button window — handy for exercising the protocol without touching platform a11y APIs:

```bash
agent-ctrl open mock
agent-ctrl snapshot
agent-ctrl click @e0
agent-ctrl close
```

## TypeScript client

For agents that prefer a programmatic API, [`@agent-ctrl/client`](packages/client) wraps the daemon over stdio JSON-RPC:

```typescript
import { AgentCtrl } from "@agent-ctrl/client";

const ctrl = new AgentCtrl();
const session = await ctrl.openSession("mock");
const snap = await ctrl.snapshot(session);
console.log(snap.refs.entries);
await ctrl.close();
```

Both transports talk the same wire protocol; agents can mix and match.

## License

Apache-2.0. See [LICENSE](LICENSE).
