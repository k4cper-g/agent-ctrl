# Linux AT-SPI → agent-ctrl schema mapping

Design doc for the `surface-atspi` implementation - the contract between
[AT-SPI2](https://www.freedesktop.org/wiki/Accessibility/AT-SPI2/) (the Linux
desktop accessibility bus) and the unified schema in [`crates/core`](../crates/core).
The structural analog of [`docs/uia-mapping.md`](uia-mapping.md); read that
first for the cross-platform invariants (the `Surface` trait, the `RefMap`
`(role, name, nth)` rediscovery scheme, `SnapshotOptions`, etc.).

**Scope:** v0.1 of `surface-atspi`. Goal is a working `snapshot` + `click` +
`focus` + `fill` against a GTK app (the `crates/atspi-fixture/` fixture, then
real GTK/Qt apps). Items marked *deferred* are not implemented in v0.1.

## 0. How it talks to AT-SPI

AT-SPI is a D-Bus protocol. A client connects to the session bus, calls
`org.a11y.Bus.GetAddress` to learn the address of the *a11y bus*, connects to
that, and walks `org.a11y.atspi.Registry`'s root accessible. Each accessible
object exposes several interfaces at one object path: `Accessible` (tree nav +
role/name/state), `Component` (geometry, `GrabFocus`), `Action` (`GetActions`,
`DoAction`), `Text`/`EditableText` (`GetText`, `SetTextContents`), `Value`,
`Selection`, `Table`, etc.

The Rust [`atspi`](https://crates.io/crates/atspi) crate wraps this over
`zbus` and is **async** - so unlike `surface-uia` (which needs a worker thread
because COM is `!Send`), `surface-atspi` can `await` the proxies directly from
the `Surface` trait's `async fn`s. The `AccessibilityConnection` is held on the
`AtSpiSurface` value; the daemon's per-session `Mutex` serializes calls.

On non-Linux hosts the crate compiles to a stub that returns
`Error::PermissionDenied` / `Error::Unsupported`, like `surface-uia` does off
Windows.

## 1. Role mapping (AT-SPI `Role` → [`Role`](../crates/core/src/role.rs))

AT-SPI has ~75 roles; most map mechanically. Mapping by *target* `Role`:

| AT-SPI role(s) | `Role` | Notes |
|---|---|---|
| `frame`, `window` | `Window` | Top-level frame; `dialog`/`alert` → `Dialog`. |
| `dialog`, `alert`, `file chooser`, `color chooser`, `font chooser` | `Dialog` | |
| `push button` | `Button` | |
| `toggle button` | `Button` | (Has `Action` "click"; toggle state via `StateSet::Checked`.) |
| `check box`, `check menu item` | `Checkbox` / `MenuItemCheckbox` | The menu variant → `MenuItemCheckbox`. |
| `radio button`, `radio menu item` | `Radio` / `MenuItemRadio` | |
| `toggle button` w/ switch styling | `Switch` | (GTK4 `GtkSwitch` reports `toggle button`; can't always distinguish - default `Button`.) |
| `combo box` | `ComboBox` | |
| `entry`, `password text`, `text` (single-line, editable) | `TextField` / `SearchBox` | `password text` stays `TextField`. A `search entry` → `SearchBox` when distinguishable. |
| `text` (multi-line / document body), `document text`, `document web` | `Document` | |
| `label`, `static`, `caption` | `Generic` | Carries text via `name`. |
| `link`, `hyperlink` | `Link` | |
| `image`, `icon` | `Image` | |
| `list`, `list box` | `List` | |
| `list item`, `option` | `ListItem` / `Option` | `option` (or a `list item` whose parent is a selection container) → `Option`. |
| `tree`, `tree table` | `Tree` / `Grid` | |
| `tree item` | `TreeItem` | |
| `table` | `Table` | |
| `table row` | `Row` | |
| `table cell`, `table column header`, `table row header` | `Cell` / `ColumnHeader` / `RowHeader` | |
| `menu bar` | `MenuBar` | |
| `menu`, `popup menu` | `Menu` | |
| `menu item` | `MenuItem` | Promote to `MenuItemCheckbox`/`MenuItemRadio` per the `check/radio menu item` rows. |
| `page tab list` | `TabList` | |
| `page tab` | `Tab` | |
| `tool bar` | `Toolbar` | |
| `slider` | `Slider` | |
| `spin button` | `SpinButton` | |
| `progress bar`, `level bar` | `Generic` | Emit; `state.enabled = false`. |
| `scroll bar`, `scroll pane`, `separator`, `filler`, `panel`, `section`, `redundant object`, `unknown` | `Generic` | Structural; stripped in `compact` mode. |
| `heading` | `Heading` | `level` from the `level` attribute. |
| `status bar` | `Region` | Landmark-ish. |
| `tool tip` | `Generic` | Captured, rarely useful. |
| `application` | `Application` (the snapshot root's parent) | Not emitted as a tree node - it's the `app` context. |
| anything else | `Unknown("atspi_<role_name>")` | Keep the raw role visible. |

**Promotion rules:** `list item` / `menu item` whose parent reports the
`Selectable`/selection semantics → `Option`/`MenuItemRadio` as above; `frame`
with the `Modal` state → `Dialog`.

## 2. State mapping (`StateSet` → [`State`](../crates/core/src/node.rs))

| `State` field | AT-SPI source |
|---|---|
| `visible` | `Showing` AND `Visible` (an off-screen-but-visible widget maps to `false`, matching the UIA `IsOffscreen` choice). |
| `enabled` | `Enabled` AND `Sensitive`. |
| `focused` | `Focused`. |
| `selected` | `Selected` when `Selectable` is set, else `None`. |
| `checked` | `Checked` → `True` / `Indeterminate` → `Mixed` / (checkable but unchecked) → `False`; else `None`. "checkable" = role is checkbox/radio/toggle/check-menu-item etc. |
| `expanded` | `Expanded` when `Expandable`, else `None`. |
| `required` | `Required` → `Some(true)`, else `None`. |

## 3. Property mapping (→ [`Node`](../crates/core/src/node.rs) fields)

| `Node` field | AT-SPI source |
|---|---|
| `name` | `Accessible.Name` (trimmed). |
| `description` | `Accessible.Description` if non-empty and ≠ `name`. |
| `value` | `EditableText`/`Text.GetText(0, char_count)` for editable text roles; else `Value.CurrentValue` (`f64` → string) for slider/spin/progress; else `None`. |
| `bounds` | `Component.GetExtents(ATSPI_COORD_TYPE_SCREEN)` → `(x, y, w, h)` in screen pixels. (HiDPI: AT-SPI extents are already in the toolkit's pixel space; we pass them through. Per-monitor scaling refinement deferred - X11 headless is 1x anyway.) |
| `level` | `level` entry of `Accessible.GetAttributes()` for headings/tree items; else `None`. |
| `role`, `state` | per §1, §2. |
| `native` | `NativeHandle::AtSpi { bus_name, path }` (see §5). |

**Dropped:** the `Accessible.GetRelationSet` graph, `GetAttributes` beyond
`level` (app-specific strings), `Hyperlink` ranges, `Text` attribute runs -
deferred to a text-aware iteration. `Action` keybindings are dropped (we
surface keyboard input separately).

## 4. Action mapping ([`Action`](../crates/core/src/action.rs) → AT-SPI calls)

| `Action` | AT-SPI call | Fallback |
|---|---|---|
| `Click` | `Action.DoActionName("click")` (or `DoAction(i)` for the `click`/`press`/`activate` action) | Synthetic pointer at `Component.GetExtents` centre (via `Atspi.GenerateMouseEvent`, or X11 `XTEST` - *deferred*; v0.1 errors if there's no `click` action). |
| `Focus` | `Component.GrabFocus()` | n/a |
| `Fill` | `EditableText.SetTextContents(value)` | `Focus` → select-all → `EditableText.InsertText` / synthetic keys (*deferred*). |
| `Clear` | `EditableText.SetTextContents("")`, verified | `Focus` + synthetic `Ctrl+A`/`Delete` (*deferred*). |
| `Check` / `Uncheck` / `Toggle` | `Action.DoActionName("click"/"toggle")` until `StateSet::Checked` matches (bounded retries) | n/a |
| `Select` | `Selection.SelectChild(i)` on the container for the named child; or `Action.DoActionName` on the option | n/a |
| `Type` / `Press` / `KeyDown` / `KeyUp` | `Atspi.GenerateKeyboardEvent` (the registry's device-event API) | *deferred* - v0.1 may not implement these; `Fill` is the editable-text path. |
| `Hover` / `DoubleClick` / `RightClick` / `Mouse` / `Drag` | `Atspi.GenerateMouseEvent` | *deferred* (needs the device-event API or XTEST; v0.1 errors). |
| `Scroll` / `ScrollIntoView` | `Component.ScrollTo` / `ScrollToPoint`; wheel via device events | `ScrollIntoView` first; wheel *deferred*. |
| `Wait` | `tokio::time::sleep` | n/a |
| `SwitchApp` | enumerate the registry's children for an `application` whose name matches; focus its active frame | n/a |
| `FocusWindow` | `Component.GrabFocus()` on the frame addressed by id | n/a |
| `Screenshot` | *deferred* - no portable AT-SPI screenshot; would shell to `gnome-screenshot` / `grim` / `import`, or capture the X11/Wayland framebuffer. v0.1 advertises no `screenshot` capability. |

**v0.1 capabilities:** `snapshot`, plus `keyboard`/`mouse` only if the
device-event API works headlessly (TBD). Realistically v0.1 advertises
`snapshot` and the pattern-driven actions (`click`, `focus`, `fill`, `check`,
`select`), and *not* `screenshot`, `mouse`, raw `keyboard` - the
`CapabilitySet` reflects that and the daemon won't dispatch the rest.

## 5. `NativeHandle::AtSpi`

```rust
NativeHandle::AtSpi {
    bus_name: String,   // the app's unique D-Bus name, e.g. ":1.42"
    path: String,       // the accessible's object path, e.g. "/org/a11y/atspi/accessible/12"
}
```

The `(bus_name, path)` pair is the AT-SPI handle and is **stable within a
session** (a GTK widget keeps its path while it exists), so it's the fast-path
hint at action-time re-resolution - the analog of UIA's `RuntimeId`:

1. `(bus_name, path)` lookup if the object still exists and its role matches → fast.
2. `(role, name, nth)` walk of the snapshot's pre-order DFS, from the `RefMap` - durable across tree mutations.

Object paths are *not* stable across app restarts, so they're a hint, never
the source of truth - exactly the `RefMap` discipline the other surfaces use.

## 6. Tree walking & refs

Walk `Accessible.GetChildren()` depth-first from the snapshot root (the app's
active top-level frame, picked from `org.a11y.atspi.Registry` per
`SnapshotOptions::target`). Same ref-emission rule as `surface-uia`: emit a
`RefId` for ARIA-interactive roles OR elements exposing editable text
(`EditableText`); `nth` is the global per-snapshot pre-order count of preceding
elements with the same `(role, name)`. `compact` strips `Generic`/`panel`/
`filler`/`separator`/`section` wrappers. `depth` bounds the walk.

There is no AT-SPI equivalent of a property-batch cache like UIA's
`BuildUpdatedCache`; each `Accessible`/`Component`/`State` read is its own D-Bus
round-trip. On a deep GTK tree that's many round-trips per snapshot - acceptable
for v0.1; a future optimisation is to use `GetChildren` (returns the whole child
list in one call) aggressively and parallelise per-node property reads with
`futures::join!`. Track as a known cost, like UIA virtualization.

## 7. Targeting & windows

`SnapshotOptions::target`:
- `Foreground` *(default)* - the `application` whose active frame has the
  `Active` state; if none, the most recently active. (No single "foreground
  window" concept on the bus; `wnck`/the window manager would be more precise -
  *deferred*.)
- `ProcessName { name }` - the `application` accessible whose name (or whose
  `toolkit`/`process` attribute) matches `name`.
- `Pid { pid }` - the `application` whose `process-id` attribute is `pid`.
- `Title { title }` - the first top-level `frame` whose `name` contains `title`.

`list_windows` returns one `WindowInfo` per top-level `frame` under the pinned
app, mirroring `surface-uia` (`focus-window` re-pins). Cross-app desktop
snapshots: deferred; snapshot one app, switch sessions / re-snapshot for another.

## 8. Gaps & intentional drops (v0.1)

- **No screenshots** - see §4. Add a backend-shell or framebuffer capture later.
- **Synthetic input** (`type`/`press`/`mouse`/`drag`/`hover`/`double/right-click`) - depends on the AT-SPI device-event API working headlessly under Xvfb, which is uncertain; v0.1 may ship without it. `fill` (`EditableText.SetTextContents`) is the text path; `click`/`check`/`select` go through `Action`.
- **Wayland vs X11** - the AT-SPI bus is the same on both; only synthetic input and screenshots differ. v0.1 (and CI) run under Xvfb (X11).
- **Qt apps** - need `QT_ACCESSIBILITY=1`; otherwise the same bus. Untested in v0.1; GTK is the reference.
- **Per-monitor HiDPI scaling** of `Component` extents - passed through as-is for now (headless is 1x).
- **Text patterns / relations** - `Text` attribute runs, `GetRelationSet` (label-for, member-of, ...). Deferred.
