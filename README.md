<p align="center">
  <img src="docs/logo.png" alt="agent-ctrl" height="120">
</p>

<p align="center">
  OS automation CLI for AI agents. Fast native Rust CLI.
</p>

> **Status (v0.1.1):** **Windows is the supported platform today.** The Windows UI Automation surface is implemented and validated end-to-end against the deterministic Win32 fixture. macOS Accessibility (AX) has focused-window snapshots, window-focus preview, first element actions (`click`, `focus`, `fill`), checkable controls, and keyboard input, but it is not at UIA parity yet. Linux AT-SPI, Android, and iOS are planned surfaces and are not implemented yet. Filling them in is the v0.x roadmap.
>
> **Browser automation is out of scope.** agent-ctrl drives native UI; for Chromium-via-CDP use the sibling [agent-browser](https://github.com/vercel-labs/agent-browser) project. The two are designed to compose in the same agent loop.

## Installation

### Windows binary

For tagged releases, download the Windows zip from GitHub Releases or run:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install-windows.ps1
```

The installer downloads the latest `agent-ctrl.exe`, installs it under
`%LOCALAPPDATA%\agent-ctrl\bin`, and adds that directory to the user PATH
unless `-NoPath` is passed.

### From source (recommended for v0.1)

```bash
git clone https://github.com/k4cper-g/agent-ctrl
cd agent-ctrl
cargo build --release -p agent-ctrl-cli
# put target/release/agent-ctrl on your PATH
```

The Rust workspace crates are not published to crates.io in v0.1. The public
distribution path is source builds or GitHub release binaries for `agent-ctrl`,
plus the npm package for TypeScript consumers.

### TypeScript client

```bash
npm install @agent-ctrl/client
# expects `agent-ctrl` on PATH for the daemon transport
```

### Requirements

- **Windows 10/11** for UIA. Other OSes build cleanly; AX has a partial macOS implementation, while Linux / Android / iOS are not implemented yet.
- **Rust 1.85+** (workspace MSRV; rustup will install it from `rust-toolchain.toml`).
- **Node.js 20+** only when using the TypeScript client.

## Quick start

```bash
agent-ctrl info                                  # OS, available surfaces, active sessions
agent-ctrl open uia                              # spawn a daemon (background)
agent-ctrl snapshot --target-process <name>      # tree of refs (@e0, @e1, ...)
agent-ctrl click @e4                             # click by ref
agent-ctrl get name @e4                          # inspect cached snapshot fields
agent-ctrl is enabled @e4                        # boolean state checks
agent-ctrl fill @e0 "hello from agent-ctrl"      # set value via UIA ValuePattern
agent-ctrl press "Ctrl+S"                        # key chord via SendInput
agent-ctrl screenshot result.png                 # PNG of the pinned window
agent-ctrl close                                 # stop the daemon
```

Every action follows the same pattern: `snapshot` once to learn what's on screen, then issue actions by ref. Refs are valid only for the snapshot that produced them - re-snapshot before acting on a tree that has changed.

## Commands

### Core

```bash
agent-ctrl open <surface>                # spawn a daemon (uia, mock, ...)
agent-ctrl close                         # stop the daemon
agent-ctrl list [--json]                 # active sessions
agent-ctrl info [--json]                 # static facts about this binary
agent-ctrl doctor [--json] [--fix] [--quick]  # diagnose the install + live probe
agent-ctrl launch [--json] <path> [--wait MS]  # spawn an app detached from this shell
```

### Snapshot

```bash
agent-ctrl snapshot                              # capture pinned window's a11y tree
agent-ctrl snapshot --target-process <name>      # pin by process executable name
agent-ctrl snapshot --target-pid <pid>           # pin by PID
agent-ctrl snapshot --target-title <substring>   # pin by window title (locale-dependent)
agent-ctrl snapshot --json                       # full JSON for programmatic consumption
agent-ctrl snapshot --compact false              # disable compact-tree filtering
```

The first `snapshot` after `open` pins the session to a target window. Subsequent actions on the session target that window until a `focus-window` re-pins it.

### Pointer / focus

```bash
agent-ctrl click @eN                     # primary-button click on a ref
agent-ctrl double-click @eN              # double-click
agent-ctrl right-click @eN               # secondary-button click
agent-ctrl hover @eN                     # cursor over element, no buttons
agent-ctrl focus @eN                     # UIA SetFocus
agent-ctrl highlight @eN                 # move cursor to element for human debugging
```

### Keyboard

```bash
agent-ctrl type "hello"                  # synthetic Unicode keystrokes
agent-ctrl fill @eN "value"              # native value setting where supported
agent-ctrl clear @eN                     # clear an editable field
agent-ctrl press "Ctrl+S"                # key chord - Enter, Tab, Ctrl+A, Cmd+A, etc.
agent-ctrl key-down "Shift"              # hold a modifier
agent-ctrl key-up "Shift"                # release it
agent-ctrl clipboard read                # read clipboard text
agent-ctrl clipboard write "text"        # replace clipboard text
agent-ctrl clipboard copy                # send Ctrl+C
agent-ctrl clipboard paste               # send Ctrl+V
```

### Selection / scroll

```bash
agent-ctrl select @eN "Option name"      # pick an item in a select / combo / list
agent-ctrl select-all [@eN]              # select all in field; without ref, sends Ctrl+A to focus
agent-ctrl check @eN                     # set a TogglePattern control on
agent-ctrl uncheck @eN                   # set a TogglePattern control off
agent-ctrl toggle @eN                    # toggle a TogglePattern control
agent-ctrl scroll <DX> <DY> [--ref @eN]  # wheel scroll (positive DY = down)
agent-ctrl scroll-into-view @eN          # UIA ScrollItemPattern
agent-ctrl drag @eFROM @eTO              # source-to-destination drag
agent-ctrl mouse move X Y                # raw mouse move
agent-ctrl mouse down X Y --button left  # raw button down
agent-ctrl mouse up X Y --button left    # raw button up
agent-ctrl mouse wheel X Y --dy -120     # raw wheel
```

### Find

```bash
agent-ctrl find "Save"                   # case-insensitive substring on name
agent-ctrl find "Save" --role button     # narrow by role (kebab-case)
agent-ctrl find "Save" --exact           # case-sensitive equality
agent-ctrl find --role menu-item         # all nodes of a role; no name filter
agent-ctrl find "OK" --in @e2            # restrict to subtree under @e2
agent-ctrl find "Save" --first           # bare ref for shell substitution
agent-ctrl find --limit 5                # cap result count
```

`find` queries the *cached* snapshot - it does not re-walk the OS tree. With no match, writes `no match` to stderr and exits non-zero. `--first` prints just `@eN` so the canonical "find then act" pattern composes:

```bash
agent-ctrl click "$(agent-ctrl find "Save" --role button --first)"
```

### Inspect

```bash
agent-ctrl get text @eN                  # value if present, otherwise accessible name
agent-ctrl get value @eN                 # editable/value-bearing field value
agent-ctrl get name @eN                  # accessible name
agent-ctrl get role @eN                  # canonical role
agent-ctrl get state @eN                 # full state object
agent-ctrl get bounds @eN                # logical screen bounds
agent-ctrl get window                    # cached window context

agent-ctrl is visible @eN
agent-ctrl is enabled @eN
agent-ctrl is focused @eN
agent-ctrl is selected @eN
agent-ctrl is checked @eN
agent-ctrl is expanded @eN
```

Inspect commands read the cached snapshot. They are fast and deterministic, but require a prior `snapshot`.

### Wait

```bash
agent-ctrl wait <MS>                                # dumb sleep on the daemon worker
agent-ctrl wait-for "Save" --role button            # wait for a node to appear
agent-ctrl wait-for "Loading..." --gone             # wait for a node to disappear
agent-ctrl wait-for "Agree" --state checked         # wait for a boolean state
agent-ctrl wait-for --role text-field --value-contains ready
agent-ctrl wait-for --window-appears "Dialog title" # wait for a sibling window title
agent-ctrl wait-for --stable [--idle-ms 500]        # wait for the tree signature to settle
agent-ctrl wait-for ... --timeout 10000 --poll 250  # tune the poll loop
```

Three reliability tiers. Use `--stable` after a click to let the UI settle before the next action. Exit codes: 0 satisfied, 1 bad args, 2 timeout - branch on those in shell pipelines instead of parsing strings.

### Windows

```bash
agent-ctrl window-list                            # all top-level windows owned by the pinned process
agent-ctrl window-list --first-other              # bare hex id of the first non-pinned window
agent-ctrl focus-window <hex_id>                  # bring a window to the foreground; re-pins the session
agent-ctrl switch-app <app_id>                    # foreground by app id (path or bare exe name)
```

When a file dialog, confirmation dialog, or popup appears as a sibling top-level window, `window-list` is how you find it. `focus-window` re-pins so subsequent `snapshot` / `find` / actions target the dialog. Mirrors agent-browser's `tab_list` / `tab_switch`.

```bash
agent-ctrl press "Ctrl+S"                                 # may open a sibling dialog HWND
agent-ctrl focus-window "$(agent-ctrl window-list --first-other)"
agent-ctrl snapshot                                       # now sees the dialog
agent-ctrl click "$(agent-ctrl find "OK" --role button --first)"
```

For detailed Windows guidance on dialogs, elevation, stale refs, foreground focus, IME, screenshots, and app framework quirks, see [`docs/windows-reliability.md`](docs/windows-reliability.md).

### Output

```bash
agent-ctrl screenshot                            # PNG of the pinned window to a temp path
agent-ctrl screenshot result.png                 # to a specific path
agent-ctrl screenshot --region X,Y,W,H           # crop in physical screen pixels
agent-ctrl screenshot --target desktop           # virtual desktop
agent-ctrl screenshot --target window            # pinned window
agent-ctrl screenshot --target ref --ref @eN     # element bounds
agent-ctrl screenshot --annotated                # draw @eN labels from cached snapshot bounds
```

`--annotated` draws cached snapshot refs onto the PNG. Run `snapshot` first so the screenshot has a current ref map and bounds.

### Batch

```bash
agent-ctrl batch --file steps.json
Get-Content steps.json | agent-ctrl batch --stdin      # PowerShell-friendly
agent-ctrl batch '[{"op":"find","query":{"name":"Save","limit":1}}]'  # Unix-shell-friendly
```

Batch steps run in order on one daemon session and return structured per-step results. Supported step ops: `act`, `find`, `get`, `is`, `wait`, and `list_windows`.

### JSON mode

```bash
agent-ctrl list --json
agent-ctrl find "Save" --role button --json
agent-ctrl get state @eN --json
agent-ctrl is enabled @eN --json
agent-ctrl click @eN --json
agent-ctrl wait-for --stable --json
agent-ctrl window-list --json
agent-ctrl screenshot out.png --json
```

Most runtime commands accept `--json` for machine-readable output. `snapshot --json` returns the full snapshot; `get --json`, `is --json`, action commands, `wait-for --json`, and `window-list --json` return structured protocol results. `batch` output is always JSON, so `batch --json` is accepted as a compatibility no-op.

Session commands redact the TCP auth token from JSON output. `screenshot --json` writes the PNG to disk and prints file metadata (`path`, `width`, `height`, `bytes`, `annotated`) instead of echoing the base64 image payload.

When `--json` is present, parse and runtime failures are emitted as one structured object with `ok: false`, `error.code`, `error.message`, and, when available, `error.hint`. Exit codes still matter: 0 means success, 1 means command/request failure, and `wait-for --json` keeps exit 2 for timeouts while printing the structured wait outcome.

## Sessions

Run multiple isolated UIA sessions side by side:

```bash
agent-ctrl open uia --session app1
agent-ctrl open uia --session app2

agent-ctrl snapshot --session app1 --target-process <process-a>
agent-ctrl snapshot --session app2 --target-process <process-b>

agent-ctrl list
# SESSION         SURFACE   PID         ENDPOINT
# app1            uia       12345       127.0.0.1:54001
# app2            uia       12346       127.0.0.1:54002

agent-ctrl close --session app1
agent-ctrl close --session app2
```

The default session is `default`, so most commands need no flag. Each session has its own daemon process, pinned target window, cached snapshot, and refs. Session metadata lives at `~/.agent-ctrl/<name>.json` while the daemon is running. TCP session files include a random per-session auth token, and every CLI TCP request sends it automatically. Stdio daemon clients, including the TypeScript client, do not need a token.

## Mock surface

The `mock` surface returns a fixed two-button window - handy for testing the protocol without UIA permissions or a target app:

```bash
agent-ctrl open mock
agent-ctrl snapshot
agent-ctrl click @e0
agent-ctrl close
```

Available on every OS, no setup required. Used by the integration tests under `packages/client/tests/`.

## TypeScript client

```typescript
import { AgentCtrl } from "@agent-ctrl/client";

const ctrl = new AgentCtrl();              // spawns `agent-ctrl daemon` over stdio
const session = await ctrl.openSession("uia");

await ctrl.snapshot(session, {
  target: { by: "process-name", name: "target-app" },
});

const matches = await ctrl.find(session, {
  name: "Save",
  role: "button",
});

await ctrl.act(session, { kind: "click", ref_id: matches[0]!.ref_id });

const outcome = await ctrl.waitFor(session, {
  predicate: { kind: "stable", idle_ms: 500 },
  timeout_ms: 5000,
  poll_ms: 250,
});

await ctrl.closeSession(session);
await ctrl.close();
```

Method surface: `openSession`, `snapshot`, `act`, `find`, `waitFor`, `listWindows`, `closeSession`, `close`. Both transports (shell CLI and stdio TypeScript) talk the same wire protocol; agents can mix and match.

See [packages/client/README.md](packages/client/README.md) for the full API.

## Architecture

agent-ctrl uses a client-daemon architecture mirroring agent-browser:

1. **Rust CLI** (`crates/cli`) - parses commands, dials the daemon, prints results.
2. **Rust daemon** (`crates/daemon`) - long-running process that owns surface sessions and dispatches snapshot / action / find / wait / list-windows requests.
3. **Surface trait** (`crates/core`) - cross-platform contract every backend implements. Per-platform crates (`crates/surface-uia`, `surface-ax`) provide the implementations, gated by `target_os`.

The daemon starts via `agent-ctrl open <surface>` and persists across CLI invocations for fast subsequent operations. Each session has its own daemon process and writes a discovery file at `~/.agent-ctrl/<session>.json`.

## Workspace layout

The repository is a **dual workspace** - a Cargo workspace for the Rust engine and an npm workspace for the TypeScript client.

| Crate / package | Purpose |
|---|---|
| [`crates/core`](crates/core) | Shared types and the `Surface` trait. Schema, role taxonomy, action vocabulary, errors. |
| [`crates/daemon`](crates/daemon) | Long-running process that owns surface sessions and dispatches actions. |
| [`crates/cli`](crates/cli) | The `agent-ctrl` binary - user-facing entrypoint. |
| [`crates/surface-uia`](crates/surface-uia) | Windows UI Automation surface (Windows-only). |
| [`crates/uia-fixture`](crates/uia-fixture) | Deterministic native Win32 fixture app for UIA reliability tests. |
| [`crates/surface-ax`](crates/surface-ax) | macOS Accessibility surface (partial automation; macOS-only). |
| [`crates/ax-fixture`](crates/ax-fixture) | Deterministic native Cocoa fixture app for AX reliability tests. |
| [`packages/client`](packages/client) | `@agent-ctrl/client` - typed TypeScript wrapper over stdio JSON-RPC. |

Surfaces gated by `target_os` compile to empty crates on other platforms, so the workspace builds on any host.

## Platforms

A **surface** is one accessibility protocol - UIA, AX, AT-SPI, etc. A **platform** is an operating system. They aren't 1-to-1: most platforms can be driven by more than one surface.

| Platform | Native surface | Status |
|---|---|---|
| Windows | [`surface-uia`](crates/surface-uia) - UI Automation | **ready** |
| macOS | [`surface-ax`](crates/surface-ax) - Accessibility / AX | partial |
| Linux | _planned_ `surface-atspi` (AT-SPI / D-Bus) | not started |
| Android | _planned_ `surface-accessibility-service` (JNI) | not started |
| iOS | _planned_ `surface-xcuitest` (WebDriverAgent) | not started |

For browsers, run agent-ctrl alongside [agent-browser](https://github.com/vercel-labs/agent-browser); the two are complementary, not competing.

Acronyms in one line: **UIA** = Microsoft UI Automation, **AX** = macOS Accessibility, **AT-SPI** = the Linux GNOME accessibility bus, **XCUITest** = Apple's UI test framework.

AX feature coverage is in [docs/macos-ax.md](docs/macos-ax.md); production
guidance for macOS lives in [docs/macos-ax-reliability.md](docs/macos-ax-reliability.md).
Windows production guidance lives in [docs/windows-reliability.md](docs/windows-reliability.md).

## Build

```bash
cargo check --workspace                          # fast type-check
cargo build --release -p agent-ctrl-cli          # the binary
cargo test --workspace                           # all unit + integration tests
cargo clippy --workspace --all-targets -- -D warnings   # lint, fail on warnings
cargo fmt --all -- --check                       # format check
```

Windows UIA fixture:

```powershell
cargo build -p agent-ctrl-cli -p agent-ctrl-uia-fixture
.\target\debug\agent-ctrl-uia-fixture.exe --ready-file "$env:TEMP\agent-ctrl-fixture.ready"
.\target\debug\agent-ctrl.exe open uia --session fixture
.\target\debug\agent-ctrl.exe snapshot --session fixture --target-process agent-ctrl-uia-fixture
```

The fixture is the preferred real-UIA test target. It exposes common native controls through stable Win32/UIA patterns so tests do not depend on Notepad, Calculator, localized strings, or Windows-version-specific app redesigns.

Opt-in fixture integration test:

```powershell
cargo build -p agent-ctrl-uia-fixture
$env:RUN_UIA_TESTS = "1"
cargo test -p agent-ctrl-cli --test windows_uia_fixture
```

Successful UIA actions may print a method diagnostic such as `ok method=keyboard-space`, `ok method=selection-item-pattern`, or `ok method=toggle-pattern`. These are intended for agents and humans debugging cross-app behavior.

macOS AX fixture:

```bash
cargo build -p agent-ctrl-cli -p agent-ctrl-ax-fixture
target/debug/agent-ctrl-ax-fixture --ready-file /tmp/agent-ctrl-ax-fixture.ready &
target/debug/agent-ctrl open ax --session fixture
target/debug/agent-ctrl snapshot --session fixture --target-process agent-ctrl-ax-fixture
```

Opt-in AX fixture integration test:

```bash
cargo build -p agent-ctrl-ax-fixture
RUN_AX_TESTS=1 cargo test -p agent-ctrl-cli --test macos_ax_fixture
```

The AX fixture covers the deterministic macOS loop for snapshots, `find`,
`click`, `fill`, `check`, `uncheck`, `toggle`, and `window-list`. Keyboard
actions exist, but are still validated manually because host focus and event-tap
behavior can vary under the Rust test harness.

TypeScript client:

```bash
npm install
npm run build --workspace=@agent-ctrl/client
npm run test  --workspace=@agent-ctrl/client     # spawns the Rust daemon under cargo run
```

The TS test suite spawns the Rust daemon under `cargo run` and exercises the full protocol against the mock surface - including `find`, `waitFor`, and `listWindows`.

## Usage with AI agents

### Just ask the agent

The simplest approach - tell your agent it can use it:

```
Use agent-ctrl to drive Windows apps. Run `agent-ctrl --help` to see the command list,
and `agent-ctrl info` to check what's available on this machine.
```

The `--help` output is comprehensive and most modern agents can figure out the rest from there.

### AGENTS.md / CLAUDE.md

For consistent results, add to your project or global instructions:

```markdown
## OS automation

Use `agent-ctrl` for native UI automation on Windows. Core workflow:

1. `agent-ctrl open uia` - spawn a daemon
2. `agent-ctrl snapshot --target-process <name>` - pin to the app and capture refs
3. `agent-ctrl find "Save" --role button --first` - discover refs by name/role
4. `agent-ctrl click @eN` / `fill @eN "text"` / `press "Ctrl+S"` - interact
5. `agent-ctrl wait-for --stable` - let the UI settle before the next action
6. `agent-ctrl window-list` + `focus-window <id>` - switch to dialogs / popups
7. Re-`snapshot` after the tree changes
```

### Example flow

The recommended pattern is app-agnostic: launch or focus the target, snapshot by
process/window, find by role/name, act, wait for stability, and re-snapshot after
the tree changes. A concrete Notepad walkthrough is available in
[examples/notepad-tour.sh](examples/notepad-tour.sh), but production agents should
prefer the generic loop above over app-specific assumptions.

## Known limitations

These are real today - the goal is to fix or document them as the project matures.

- **Windows is the only fully action-ready surface.** AX can capture the focused macOS window, raise listed windows, run first element actions (`click`, `focus`, `fill`), drive checkable controls, and send keyboard input when Accessibility permission is granted; Linux / Android / iOS / browser flows are not implemented in this project yet.
- **Local TCP daemon auth is developer-machine scoped.** TCP session files include a random bearer token and the daemon rejects missing or incorrect tokens, but anyone who can read `~/.agent-ctrl/<session>.json` can still use that session. Treat sessions as a local developer-machine boundary, not a multi-user security sandbox.
- **Refs are valid only against the snapshot that produced them.** If `wait-for` runs in parallel with another command on the same session (across two shells), the wait loop refreshes the cached refs on each poll, and a previously-issued ref may resolve to a different element. Sequential CLI usage in one shell - the realistic flow - doesn't trip this.
- **Modern Win11 file dialogs and popup menus open as sibling top-level windows**, not as children of the app's main window. Use `window-list` + `focus-window` to discover and switch to them.
- **`type` bypasses IME.** Synthetic Unicode keystrokes via `SendInput` are reliable for ASCII; CJK with IME composition is not supported yet. `fill` (UIA `ValuePattern`) is the right escape hatch for non-ASCII text input.
- **HWND recycling.** Windows reassigns numeric HWNDs after a window closes; `window-list` shows whatever currently holds an id, with no UIA-runtime-id verification. Theoretical, never observed in practice.

## License

Apache-2.0. See [LICENSE](LICENSE).
