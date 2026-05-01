# Changelog

All notable changes to **agent-ctrl** are recorded here. The format is loosely based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches 1.0.

## [Unreleased]

## [0.1.0] - 2026-05-01

First public release. **Windows is the supported platform**; CDP, macOS AX, Linux AT-SPI,
Android, and iOS are scaffolded.

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

- Windows is the only ready surface. Other surfaces compile and return `Unsupported`.
- The local TCP daemon has no authentication; treat sessions as a developer-machine
  convenience, not a security boundary.
- Refs are valid only against the snapshot that produced them; running `wait-for` in
  parallel with another command on the same session (across two shells) can shift the
  cached refs mid-flow.
- Modern Win11 file dialogs and popup menus open as sibling top-level HWNDs -
  use `window-list` + `focus-window` to reach them.
- `type` bypasses IME; non-ASCII text input goes through `fill` (UIA `ValuePattern`).
