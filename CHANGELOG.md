# Changelog

All notable changes to **agent-ctrl** are recorded here. The format is loosely based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches 1.0.

## [Unreleased]

### Added

- Mature Windows UIA agent loop:
  - Inspect commands: `get text`, `get value`, `get name`, `get role`, `get state`,
    `get bounds`, `get window`.
  - State checks: `is visible`, `is enabled`, `is focused`, `is selected`,
    `is checked`, `is expanded`.
  - Actions for check state, clear, clipboard read/write/copy/paste, raw mouse,
    highlight, screenshot targets, and ordered `batch` execution.
  - Wait predicates for state, text/value contains, window appears/gone, and
    stable tree signatures.
- Screenshot targets for pinned window, desktop, region, and element refs, plus
  annotated PNG screenshots with cached `@eN` labels.

### Changed

- Rust workspace crates are marked `publish = false`; v0.1 ships via source
  builds, GitHub release binaries, and the npm TypeScript client.
- Per-session TCP auth tokens in session files. The CLI reads and sends the token
  automatically; stdio daemon transport remains exempt for the TypeScript client.
- Protocol version and capability metadata in daemon/session responses.
- CLI `--json` mode across runtime commands, including structured JSON errors
  for parse/runtime failures and redacted session metadata.
- Deterministic Win32 UIA fixture app for repeatable real-UIA coverage.
- Windows UIA fixture integration test gated by `RUN_UIA_TESTS=1`.
- CLI JSON contract tests against the mock surface.
- Windows reliability guide covering dialogs, elevation/UIPI, stale refs,
  foreground focus, IME/non-ASCII text, screenshots, and app framework quirks.

### Changed

- Removed browser/CDP scope from this project. Use agent-ctrl for native UI and
  agent-browser for browser automation.
- Windows button activation now prefers `InvokePattern`, with keyboard Space and
  pointer fallback paths.
- CLI action output keeps human-readable `ok` lines by default and exposes
  structured action results only when `--json` is passed.
- `screenshot --json` writes the PNG to disk and prints metadata rather than
  echoing the base64 image payload.
- README and TypeScript client docs now describe the mature native Windows loop.

### Fixed

- TCP requests without the correct auth token are rejected.
- Stale window handling reports clearer recovery messages and validates HWNDs
  before focus/window-list operations.
- `focus-window` clears cached refs and cached snapshot state after repinning.
- Batch input accepts UTF-8 BOM-prefixed JSON.

### Verification

- Required checks: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --all -- --check`, TypeScript build, and TypeScript tests.
- Optional Windows smoke: `RUN_UIA_TESTS=1 cargo test -p agent-ctrl-cli --test windows_uia_fixture`.

## [0.1.0] - 2026-05-01

First public release. **Windows is the supported platform**; macOS AX is scaffolded,
while Linux AT-SPI, Android, and iOS are planned. Browser automation is intentionally out of scope -
use the sibling [agent-browser](https://github.com/vercel-labs/agent-browser) project.

### Added

- **`Surface` trait** as the cross-platform contract every accessibility backend implements.
  Data-oriented (`snapshot()` returns a unified `Snapshot` schema); refs are stable per
  snapshot and re-resolved at action time via `(role, name, nth)`.
- **Windows UI Automation surface** (`agent-ctrl-surface-uia`), initial implementation:
  full action vocabulary, COM worker thread, DPI-normalized coordinates, locale-independent
  process targeting via `WindowTarget::ProcessName`.
- **Long-running daemon** (`agent-ctrl-daemon`) with both stdio (TS client transport) and
  TCP (CLI transport) modes. Discovered through `~/.agent-ctrl/<session>.json`.
- **CLI verbs**: `open`, `close`, `list`, `info`, `doctor`, `launch`, `snapshot`, `click`,
  `double-click`, `right-click`, `hover`, `focus`, `fill`, `type`, `press`, `key-down`,
  `key-up`, `select`, `select-all`, `scroll`, `scroll-into-view`, `drag`, `switch-app`,
  `focus-window`, `screenshot`, `wait`.
- **`find` verb** (and `Snapshot::find` API). Looks up refs in the cached snapshot
  without re-walking the OS tree. Supports `--role`, `--exact`, `--in @eN`, `--first`
  (bare-ref shell substitution), and `--limit`.
- **`wait-for` verb** with three reliability-tiered predicates: name/role appearance
  (`appears`), disappearance (`--gone`), and tree-signature stability (`--stable`,
  with `--idle-ms`). Daemon-side polling at `--poll` cadence, capped at a 1h timeout.
  Distinct exit codes: 0 satisfied, 1 bad args, 2 timeout.
- **`window-list` verb** mirroring agent-browser's `tab_list`. Enumerates all visible
  top-level windows owned by the pinned process; exposes `--first-other` for shell
  substitution into `focus-window` when a dialog opens as a sibling HWND.
- **`@agent-ctrl/client`** TypeScript wrapper that spawns the Rust daemon and talks
  JSON-RPC over stdio. Surface area matches the CLI: `openSession`, `snapshot`, `act`,
  `find`, `waitFor`, `listWindows`, `closeSession`, `close`.
- **Mock surface** (gated by the `mock` Cargo feature) returning a deterministic two-button
  fake tree. Powers protocol tests on every host without requiring real OS accessibility.
- **GitHub Actions CI** running fmt, clippy (`-D warnings`, pedantic + nursery), and
  tests on Ubuntu, macOS, and Windows runners.

### Known limitations

See README's "Known limitations" section. Highlights:

- Windows is the only ready surface. AX is scaffolded on macOS and returns `Unsupported`;
  Linux / Android / iOS / browser flows are not implemented in this project yet.
- Local TCP daemon auth is developer-machine scoped. Anyone who can read the session
  file can use that session.
- Refs are valid only against the snapshot that produced them; running `wait-for` in
  parallel with another command on the same session (across two shells) can shift the
  cached refs mid-flow.
- Modern Win11 file dialogs and popup menus open as sibling top-level HWNDs -
  use `window-list` + `focus-window` to reach them.
- `type` bypasses IME; non-ASCII text input goes through `fill` (UIA `ValuePattern`).
