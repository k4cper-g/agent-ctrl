# macOS AX Reliability Guide

This guide is for agents driving native macOS apps through `surface-ax`. The
core loop is:

```bash
agent-ctrl open ax
agent-ctrl snapshot --target-process <process-name>
agent-ctrl find "Save" --role button
agent-ctrl click @eN
agent-ctrl wait-for --stable
agent-ctrl screenshot
```

`docs/macos-ax.md` lists what the surface implements. This guide covers what
to do when a real Cocoa, Catalyst, Electron, or Chromium app behaves
differently from the deterministic fixture.

## Permissions

macOS gates accessibility automation behind two separate Privacy & Security
grants. Both are keyed to the exact `agent-ctrl` binary on disk:

- **Accessibility** is required for every action and snapshot. Without it
  `open ax` returns `Error::PermissionDenied`. Add the binary in
  *System Settings > Privacy & Security > Accessibility*.
- **Screen Recording** is required only for `screenshot` (any target).
  Without it `CGWindowListCreateImage` returns null and the action fails
  with a hint to grant access. Add the binary in
  *System Settings > Privacy & Security > Screen Recording* and restart
  the daemon (`agent-ctrl close` then `open ax`) so the new permission
  takes effect.

When you rebuild into a different path (release vs debug, npm-installed vs
source build), each binary is a separate TCC entry. The toggle sometimes
silently flips off after a relink; if actions or screenshots start failing
right after a `cargo build`, toggle the entry off and back on.

## Deterministic Fixture

Use `agent-ctrl-ax-fixture` for repeatable real-AX testing. It is a Cocoa
app with stable common controls: status label, text field, button,
checkbox, popup button, and a wired-up selection action.

```bash
cargo build -p agent-ctrl-cli -p agent-ctrl-ax-fixture
target/debug/agent-ctrl-ax-fixture --ready-file /tmp/agent-ctrl-ax-fixture.ready &
target/debug/agent-ctrl open ax --session fixture
target/debug/agent-ctrl snapshot --session fixture --target-process agent-ctrl-ax-fixture
```

CI should prefer this fixture over real macOS apps whose UI varies by macOS
version, locale, theme, or app redesign.

Run the opt-in end-to-end fixture test with:

```bash
cargo build -p agent-ctrl-ax-fixture
RUN_AX_TESTS=1 cargo test -p agent-ctrl-cli --test macos_ax_fixture
```

## Method Diagnostics

Every successful action prints a method tag the surface used, for example
`ok method=ax-press` for an AXPress click or `ok method=cg-click` when the
AX path failed and CGEvent took over. The full set:

| Tag | Meaning |
|---|---|
| `ax-press` | `kAXPressAction` succeeded directly. |
| `cg-click` | AXPress failed; CGEvent left mouse click at element center used. |
| `cg-double-click` | Double click via two CGEvent down/up pairs at click counts 1 and 2. |
| `cg-right-click` | CGEvent right mouse down/up at element center. |
| `cg-mouse-moved` | CGEvent move with no button (hover or raw `mouse move`). |
| `cg-mouse-down` / `cg-mouse-up` | Raw `MouseOp::Down` / `Up` events. |
| `cg-drag` | Mouse-down at source, dragged path, up at target. |
| `cg-scroll-wheel` | `CGEventCreateScrollWheelEvent` with vertical and horizontal axes. |
| `cg-keyboard-unicode` | `type` via `CGEventKeyboardSetUnicodeString`. |
| `cg-keyboard-chord` | `press "Cmd+S"` via virtual key + modifier flags. |
| `cg-keyboard-virtual-key` | `key-down` / `key-up` for a single key code. |
| `keyboard-cmd-a` / `cmd-c` / `cmd-v` | `select-all`, `clipboard copy`, `clipboard paste`. |
| `ax-focused` | `setAXFocused = true` succeeded. |
| `ax-value` | `setAXValue` succeeded for `fill`. |
| `ax-value-empty` | `setAXValue = ""` succeeded for `clear`. |
| `ax-scroll-to-visible` | `kAXScrollToVisibleAction` succeeded for `scroll-into-view`. |
| `pbcopy` / `pbpaste` | `clipboard write` / `read` via the system tools. |
| `menu-click` | `select` opened the popup menu and clicked the item via CGEvent. |

Use these as clues when a real app behaves differently from the fixture.
Seeing `cg-click` instead of `ax-press` is the typical signal that the
target view does not implement the standard `AXPress` action and you may
want to drive it through coordinate-based clicks for the rest of the flow.

## Refs And Stale Elements

Refs are valid only for the snapshot that produced them. The surface uses
two recovery strategies, in order:

1. **AXIdentifier fast path.** When the snapshot captured a non-empty
   `AXIdentifier` (AppKit `accessibilityIdentifier`) for the element, the
   surface walks the live tree looking for a node with the same identifier
   and uses that node directly. This survives renames, reorderings, and
   value changes.
2. **`(role, name, nth)` walk.** Falls back to a pre-order DFS that
   matches the role and name from the snapshot, picking the n-th match.
   This survives identifier-less apps but is sensitive to label changes.

Apps that set `accessibilityIdentifier` (well-tested AppKit apps, anything
built with `NSAccessibilityCustomAction`-style ergonomics) get the
identifier path automatically. Apps that don't (Electron's default tree,
many third-party Mac apps, anything that exposes plain Cocoa without
identifier annotations) fall back to the role/name walk.

If an app redraws or navigates, run `snapshot` again before acting:

```bash
agent-ctrl click @e4
agent-ctrl wait-for --stable
agent-ctrl snapshot
agent-ctrl click "$(agent-ctrl find "OK" --role button --first)"
```

## Sheets, Dialogs, And Popups

Cocoa modal UI comes in three shapes, and only the last needs the
window-list dance:

1. **Sheets** (`NSSavePanel`, `NSOpenPanel`, alerts started with
   `beginSheetModalForWindow:`) attach to the parent window. They appear
   in the parent's AX tree as another `AXWindow` child; `snapshot` of
   the parent process shows them automatically. Use `find --role
   text-field` etc. on the same session.
2. **App-modal panels** (`runModal`, `NSAlert.runModal`) take over the
   parent app's main thread. AX queries to that app block until the
   modal is dismissed. `snapshot` and `find` from another process still
   work; act on the modal first, then continue.
3. **Standalone modal windows** (popovers, file dialogs that open as
   their own top-level window, popup menus) become sibling top-level
   windows. Use the standard `window-list` flow:

```bash
agent-ctrl window-list
agent-ctrl focus-window "$(agent-ctrl window-list --first-other)"
agent-ctrl snapshot
```

`focus-window` validates the window id still exists and re-pins the
session before the next snapshot.

Popup menus from `NSPopUpButton` are a special case the surface handles
internally. The `select` verb opens the menu via CGEvent, waits past the
system double-click threshold, then CGEvent-clicks the matching menu
item. If you want to drive a popup manually, mirror that pattern: a
direct `click @eN` on the popup followed by an `AXPress` on the menu
item is **silently ignored** by AppKit.

## Foreground Focus And AXRaise

Mouse-driven actions (`click` fallback, `double-click`, `right-click`,
`hover`, `drag`, `scroll`, `select`, raw `mouse`) require the target
window to be foreground; CGEvent posts go to whatever's under the
cursor at OS level, not to a particular AXUIElement. The surface raises
the pinned window via `AXRaise` before each pointer action.

Common foreground gotchas:

- Switching apps mid-flow (Cmd+Tab, focus stolen by a notification) can
  drop our raise. Re-`snapshot` to confirm `window` is still our target.
- App-nap and "hidden" apps may not raise instantly. After
  `switch-app com.example.app`, wait a few hundred ms before the next
  action so the activation completes.
- Some system surfaces (the menu bar, Spotlight, Mission Control,
  fullscreen video players) refuse synthetic input. The surface returns
  the AX or CG error unchanged.
- `agent-ctrl` itself must have the same Accessibility grant as any app
  it drives. If the daemon was launched before the grant, restart it.

## Text Input

Use the right text primitive:

- `fill @eN "text"` calls `setAXValue` on the element. Best for
  programmatic edits, long text, and non-ASCII (no IME involvement).
- `clipboard write "text"` plus `clipboard paste` is the right fallback
  for fields without a settable `AXValue`. Restore the user's clipboard
  yourself if you care about preserving it.
- `type "text"` uses `CGEventKeyboardSetUnicodeString` to send each
  code unit through the HID event tap. Useful for shortcut flows and
  apps that need real keystrokes, but it does not go through any IME.
  CJK composition and dead-key sequences will not work; use `fill` or
  clipboard paste for those.
- `press "Cmd+S"` posts a virtual key down/up with modifier flags. Best
  for shortcuts and chord-only flows.

The `select-all` and `clear` verbs implement the natural macOS
shortcuts internally (`Cmd+A`, then `setAXValue = ""`).

## App Framework Quirks

Cocoa, Catalyst, Electron, Chromium, and Java apps expose different AX
trees. For reliable agents:

- Prefer role plus name queries over index-only assumptions.
- Use `get state` and `is enabled` before clicking controls that may
  still be loading.
- Chain `wait-for --stable` after actions that trigger layout changes.
- Use `screenshot --target ref --ref @eN` when a visual check needs the
  exact element bounds.

Framework notes:

- **Native Cocoa** (AppKit, anything from Apple) is the most predictable
  and is what the fixture exercises. AXPress, AXValue, AXFocused all
  behave per documentation.
- **Mac Catalyst** apps (apps from iPad ported via Catalyst) expose
  reasonable AX trees but some custom controls may not respond to
  AXPress. Expect `cg-click` fallbacks more often.
- **Electron / Chromium** apps expose a Chromium accessibility tree that
  is rich but uses a lot of `AXGroup` nesting. Use `--depth`, role
  filters, and `--first` to keep agent prompts compact. Identifiers are
  rarely set, so the role/name walk is what runs.
- **Java / Swing / SWT** AX coverage is the weakest. Many controls are
  invisible to AX or report bogus bounds. Prefer screenshots and
  coordinate-based clicks for these.
- **Web content inside Safari / Chrome** is exposed via the browser's
  AX tree. Use the browser-specific surface (`agent-browser`) instead
  for web flows; AX into rendered DOM is far less stable.

## Screenshots

PNG is the only screenshot format. Supported targets:

- pinned window: `agent-ctrl screenshot`
- desktop: `agent-ctrl screenshot --target desktop`
- region: `agent-ctrl screenshot --region X,Y,W,H`
- element ref: `agent-ctrl screenshot --target ref --ref @eN`

The image dimensions are physical pixels (retina-aware): a 640×360
logical-point window captures as 1280×720 on a 2× retina display.
`--annotated` draws ref labels at the same scale, sourced from the
cached snapshot's bounds. Run `snapshot` first so refs and bounds are
current; an annotated screenshot without a prior snapshot is rejected.

If a screenshot returns blank or all-black pixels, Screen Recording
permission is missing for the running binary even though
`CGWindowListCreateImage` returned non-null. Check the Privacy &
Security pane and restart the daemon.

## Switch-App

`agent-ctrl switch-app <app>` accepts either a bundle id or an
executable file stem:

```bash
agent-ctrl switch-app com.apple.Safari
agent-ctrl switch-app TextEdit
agent-ctrl switch-app /Applications/MyApp.app/Contents/MacOS/MyApp
```

Bundle id lookup uses `[NSRunningApplication runningApplicationsWithBundleIdentifier:]`
and is the preferred form. If no bundle match is found, the surface
walks `[NSWorkspace.sharedWorkspace runningApplications]` looking for a
running app whose `executableURL` file stem matches case-insensitively.

After activation, the surface clears its pinned window so the next
`snapshot` discovers the foreground window for the new app. Pair with
`launch` for first-run scenarios:

```bash
agent-ctrl launch /Applications/TextEdit.app --wait 1500
agent-ctrl switch-app com.apple.TextEdit
agent-ctrl snapshot
```

## Release Smoke Checklist

Before publishing a macOS binary:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build -p agent-ctrl-cli -p agent-ctrl-ax-fixture
RUN_AX_TESTS=1 cargo test -p agent-ctrl-cli --test macos_ax_fixture
npm run build --workspace=@agent-ctrl/client
npm run test --workspace=@agent-ctrl/client
```

The opt-in fixture test exercises every action verb, the AXIdentifier
fast path, the `select` popup-click flow, the screenshot path with
annotations, and the `switch-app` round trip between Finder and the
fixture. It runs on a real Cocoa fixture, so it requires both
Accessibility and Screen Recording grants on the local
`agent-ctrl` binary and a logged-in GUI session.
