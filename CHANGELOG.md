# Changelog

All notable changes to **agent-ctrl** are recorded here. The format is loosely based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches 1.0.

## [Unreleased]

No unreleased changes yet.

## [0.1.1] - 2026-05-06

Production-polish release for the Windows native agent loop. Windows is the
supported action-ready surface; macOS AX now has an initial focused-window
snapshot preview. Browser automation remains out of scope - use the sibling
[agent-browser](https://github.com/vercel-labs/agent-browser) project.

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
- Windows install script and tag-based GitHub Release artifact workflow.
- macOS AX focused-window snapshot preview with role/name/value/state/bounds mapping.

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

### Known Limitations

See README's "Known limitations" section. Highlights:

- Windows is the only action-ready surface. AX can capture a focused-window snapshot on macOS;
  Linux / Android / iOS / browser flows are not implemented in this project yet.
- Local TCP daemon auth is developer-machine scoped. Anyone who can read the session
  file can use that session.
- Refs are valid only against the snapshot that produced them; running `wait-for` in
  parallel with another command on the same session can shift the cached refs mid-flow.
- Modern Win11 file dialogs and popup menus open as sibling top-level HWNDs.
- `type` bypasses IME; non-ASCII text input goes through `fill` or clipboard paste.

## [0.1.0] - 2026-05-01

First public source release. Windows was the supported platform; CDP, macOS AX,
Linux AT-SPI, Android, and iOS were scaffolded.

### Added

- **`Surface` trait** as the cross-platform contract every accessibility backend implements.
  Data-oriented (`snapshot()` returns a unified `Snapshot` schema); refs are stable per
  snapshot and re-resolved at action time via `(role, name, nth)`.
- **Windows UI Automation surface** (`agent-ctrl-surface-uia`), feature-complete:
  full action vocabulary, COM worker thread, DPI-normalized coordinates, locale-independent
  process targeting via `WindowTarget::ProcessName`.
- **Long-running daemon** (`agent-ctrl-daemon`) with both stdio (TS client transport) and
  TCP (CLI transport) modes. Discovered through `~/.agent-ctrl/<session>.json`.
- **CLI verbs**: `open`, `close`, `list`, `info`, `doctor`, `launch`, `snapshot`, `click`,
  `double-click`, `right-click`, `hover`, `focus`, `fill`, `type`, `press`, `key-down`,
  `key-up`, `select`, `select-all`, `scroll`, `scroll-into-view`, `drag`, `switch-app`,
  `focus-window`, `screenshot`, `wait`.
- **`find` verb** and `Snapshot::find` API. Looks up refs in the cached snapshot
  without re-walking the OS tree.
- **`wait-for` verb** with appearance, disappearance, and tree-stability predicates.
- **`window-list` verb** for visible top-level windows owned by the pinned process.
- **`@agent-ctrl/client`** TypeScript wrapper over stdio JSON-RPC.
- **Mock surface** gated by the `mock` Cargo feature.
- **GitHub Actions CI** running fmt, clippy, and tests on Ubuntu, macOS, and Windows.

### Known Limitations

- Windows was the only ready surface. Other surfaces compiled and returned `Unsupported`.
- Local TCP daemon sessions had no authentication.
- Refs were valid only against the snapshot that produced them.
