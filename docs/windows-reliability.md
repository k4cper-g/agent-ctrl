# Windows Reliability Guide

This guide is for agents driving native Windows apps through `surface-uia`.
The core loop is:

```bash
agent-ctrl open uia
agent-ctrl snapshot --target-process <process-name>
agent-ctrl find "Save" --role button
agent-ctrl click @eN
agent-ctrl wait-for --stable
agent-ctrl screenshot
```

## Reaching The App

`snapshot --target-process X` pins to the first *visible* top-level window
owned by a process named `X`. Two common ways that fails, and the fix:

- **The app is parked in the system tray.** Slack, Teams, Discord, and many
  others "close to tray" - the process keeps running but the window is hidden
  (`ShowWindow(SW_HIDE)`'d). `snapshot --target-process` errors and says so:
  *"it is running with a hidden window; bring it forward through its own
  tray/taskbar icon or Start menu entry (or `agent-ctrl launch <path>`)"*.
  Use the app's own channels: click its tray icon, or `agent-ctrl launch
  <path>` (for a packaged/Store app, launching its exe still routes through
  the activation broker, which resumes/shows it correctly). Do **not** try to
  un-hide a tray window by hand (`focus-window` on it, etc.) - tray apps
  re-hide a window shown out from under them, leaving it unusable. (`switch-app`
  and `focus-window` *will* un-minimize a window that's only minimized to the
  taskbar; that case is safe.)

- **The app is Chromium/Electron and its accessibility tree isn't built
  yet.** Chromium populates the UIA tree lazily, on the first query - so the
  *first* `snapshot` right after the window appears often shows only the
  window frame (a handful of refs, one big `document`/`generic` child). Use
  `agent-ctrl snapshot --settle` (re-snapshots until the tree's signature
  holds steady, ~8s cap), or chain `agent-ctrl wait-for --stable` then a
  fresh `snapshot`. After that the tree is fully populated.

So the robust opening for a tray-parked Electron app (Slack, say) is:

```bash
agent-ctrl open uia
agent-ctrl launch "C:\...\Slack.exe"            # or click the tray icon
agent-ctrl snapshot --settle --target-process Slack
```

## Deterministic Fixture

Use `agent-ctrl-uia-fixture` for repeatable real-UIA testing. It is a small
native Win32 app with stable common controls: text field, checkbox, combo box,
list, buttons, delayed state, and a dialog trigger.

```powershell
cargo build -p agent-ctrl-cli -p agent-ctrl-uia-fixture
.\target\debug\agent-ctrl-uia-fixture.exe --ready-file "$env:TEMP\agent-ctrl-fixture.ready"
.\target\debug\agent-ctrl.exe open uia --session fixture
.\target\debug\agent-ctrl.exe snapshot --session fixture --target-process agent-ctrl-uia-fixture
```

Required CI should prefer this fixture over OS apps whose UI can vary by
Windows version, locale, installed features, or app redesign.

Run the opt-in end-to-end fixture test with:

```powershell
cargo build -p agent-ctrl-uia-fixture
$env:RUN_UIA_TESTS = "1"
cargo test -p agent-ctrl-cli --test windows_uia_fixture
```

Action output includes method diagnostics where available, for example
`ok method=keyboard-space` for Win32 button activation or
`ok method=toggle-pattern` for checkbox state changes. Use these as clues
when a real app behaves differently from the fixture.

## Dialogs And Sibling Windows

Native dialogs often appear as sibling top-level HWNDs instead of children of
the pinned window. When a command opens a dialog, use:

```bash
agent-ctrl window-list
agent-ctrl focus-window "$(agent-ctrl window-list --first-other)"
agent-ctrl snapshot
```

`focus-window` validates that the HWND still exists before pinning it. After
switching windows, take a fresh snapshot before using refs.

## Refs And Stale Elements

Refs are valid only for the snapshot that produced them. UIA runtime ids and
AutomationId values are used as recovery hints, but the durable fallback is
the snapshot entry's role, name, and nth match. If an app redraws or navigates,
run `snapshot` again before acting.

Good pattern:

```bash
agent-ctrl click @e4
agent-ctrl wait-for --stable
agent-ctrl snapshot
agent-ctrl click "$(agent-ctrl find "OK" --role button --first)"
```

## Foreground Focus And UIPI

Keyboard and mouse fallbacks use `SendInput`, so the target window must be in
the foreground. The UIA surface brings the pinned HWND forward before these
actions and reports a clear error if Windows blocks injection.

Common causes:

- Target app is elevated and `agent-ctrl` is not. Windows UIPI blocks lower
  integrity processes from injecting input into higher integrity windows.
- Secure desktop prompts, UAC consent, lock screen, and credential prompts are
  outside normal UIA automation.
- Remote desktop focus can move while an agent is acting. Re-snapshot and
  verify `get window` or `window-list` if input lands in the wrong app.

Production guidance:

- Run `agent-ctrl` at the same integrity level as the app being driven.
- Avoid mixing manual keyboard/mouse use with an active agent session.
- Prefer UIA-pattern actions (`click`, `check`, `select`, `fill`) over raw
  pointer events when both are available.
- When foreground focus is contested, take a fresh `window-list --json` and
  confirm the intended window is `focused: true` before sending keystrokes.

## Text Input

Use the right text primitive:

- `fill @eN "text"` uses UIA `ValuePattern.SetValue`. Prefer this for model
  level edits, long text, and non-ASCII text.
- `clipboard write "text"` plus `clipboard paste` is a good fallback for
  large or non-ASCII text when the target does not expose `ValuePattern`.
- `type "text"` sends human-like keystrokes. It is useful for shortcut flows
  and controls that need real key events, but IME composition is not modeled.

IME and keyboard layout caveats:

- `type` sends key/input events through the active Windows input stack. It is
  useful for shortcuts and ASCII-ish human typing, but it is not an IME model.
- `fill` bypasses the keyboard layout when the target exposes `ValuePattern`.
- For long or non-ASCII text, prefer clipboard paste when `fill` is
  unsupported. Restore the clipboard yourself if the calling agent needs to
  preserve user clipboard state.

Chromium/Electron text boxes (Slack composer, Teams, Discord) are
`contenteditable` elements with their own React-style input handlers:

- Prefer `type` over `fill` for these. `fill` sets the DOM value directly via
  `ValuePattern.SetValue` and does *not* fire the `input` event the app
  listens for, so the app may not register the text (the Send button stays
  disabled, Enter sends nothing). Synthetic keystrokes (`type`) drive the
  handlers the way a human does.
- An *empty* `contenteditable` reports `value = "\n"`, not `""`. A "is this
  box empty?" check should treat `""` and `"\n"` the same, and `"\n"` is not
  stray content you need to clear.
- Before submitting (sending a message, hitting OK), re-`snapshot` and verify
  the field shows your text and the submit control is enabled. Then `Enter`.

## App Framework Quirks

Win32, WPF, WinUI, Electron, Chromium, and Office expose different UIA trees.
For reliable agents:

- Prefer role plus name queries over index-only assumptions.
- Use `get state` and `is enabled` before clicking controls that may still be
  loading.
- Chain `wait-for --stable` after actions that trigger layout changes.
- Use `screenshot --target ref --ref @eN` when a visual check needs the exact
  element bounds.

Framework notes:

- Win32 common controls are the most predictable and are covered by the
  fixture.
- WinUI and XAML apps can populate their UIA tree lazily. `snapshot --settle`
  (or `wait-for --stable` then `snapshot`), then act.
- Electron and Chromium apps build their UIA tree lazily on first query, so
  the first `snapshot` after the window appears is usually just the frame -
  use `snapshot --settle`. The settled tree is large and document-like; lean
  on `find`, role filters, and `--first` to keep agent prompts compact.
- Office apps may expose rich but deeply nested trees. Prefer named controls
  and stable waits over index-based flows.
- Custom-rendered canvases may appear as opaque regions. Use screenshots for
  visual inspection, but do not expect accessibility refs inside a canvas
  unless the app exposes them.

## Screenshots

PNG is the only screenshot format in this milestone. Supported targets:

- pinned window: `agent-ctrl screenshot`
- desktop: `agent-ctrl screenshot --target desktop`
- region: `agent-ctrl screenshot --region X,Y,W,H`
- element ref: `agent-ctrl screenshot --target ref --ref @eN`

`--annotated` draws labels from the cached accessibility snapshot onto the
PNG. Run `snapshot` first so refs and bounds are current.

## Release Smoke Checklist

Before publishing a Windows binary:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p agent-ctrl-cli -p agent-ctrl-uia-fixture
$env:RUN_UIA_TESTS = "1"
cargo test -p agent-ctrl-cli --test windows_uia_fixture
npm run build --workspace=@agent-ctrl/client
npm run test --workspace=@agent-ctrl/client
```

The TypeScript UIA test also uses the fixture when `RUN_UIA_TESTS=1`, so it
should not depend on Notepad, Calculator, Settings, Explorer, locale-specific
strings, or Windows-version-specific app redesigns.
