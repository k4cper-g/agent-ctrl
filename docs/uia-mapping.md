# Windows UIA → agent-ctrl schema mapping

Design doc for the `surface-uia` implementation. This is the contract between
[Windows UI Automation](https://learn.microsoft.com/en-us/windows/win32/winauto/entry-uiauto-win32)
and the unified schema in [`crates/core`](../crates/core). Settle the
ambiguous calls here, then write the Rust against this doc.

**Scope:** v0.1 of `surface-uia`. Goal is a working `snapshot` + `click` +
`fill` + `focus` against any modern Win32 / WPF / WinUI app. Items marked
*deferred* are not implemented in v0.1.

## 1. Role mapping (`UIA_ControlType` → [`Role`](../crates/core/src/role.rs))

UIA exposes ~40 standard `ControlType`s. Mapping is mostly mechanical because
both UIA and our `Role` derive from the same ARIA-equivalent vocabulary.

| UIA `ControlType` | `Role`            | Notes |
|---|---|---|
| `Button`          | `Button`          | |
| `Calendar`        | `Group`           | No native ARIA equivalent; treated as a group of `Cell`s. |
| `CheckBox`        | `Checkbox`        | |
| `ComboBox`        | `ComboBox`        | |
| `Custom`          | `Unknown(class)`  | Use `ClassName` as the hint, e.g. `Unknown("Edit")`. |
| `DataGrid`        | `Grid`            | |
| `DataItem`        | `Row`             | When inside a `DataGrid`. |
| `Document`        | `Document`        | |
| `Edit`            | `TextField`       | |
| `Group`           | `Group`           | |
| `Header`          | `RowGroup`        | Could also be `Group`; `RowGroup` better reflects intent in tables. |
| `HeaderItem`      | `ColumnHeader`    | |
| `Hyperlink`       | `Link`            | |
| `Image`           | `Image`           | |
| `List`            | `List`            | |
| `ListItem`        | `ListItem`        | When parent has `Selection` pattern, also acts as `Option`. |
| `Menu`            | `Menu`            | |
| `MenuBar`         | `MenuBar`         | |
| `MenuItem`        | `MenuItem`        | Promote to `MenuItemCheckbox` / `MenuItemRadio` when the `Toggle` pattern is present. |
| `Pane`            | `Generic`         | UIA's catch-all container; carries little semantic value. |
| `ProgressBar`     | `Generic`         | No interactive role; emit but mark `state.enabled = false`. |
| `RadioButton`     | `Radio`           | |
| `ScrollBar`       | `Generic`         | Skip in `interactive_only` snapshots. |
| `SemanticZoom`    | `Group`           | Rare; deferred. |
| `Separator`       | `Generic`         | Drop in `compact` snapshots. |
| `Slider`          | `Slider`          | |
| `Spinner`         | `SpinButton`      | |
| `SplitButton`     | `Button`          | Has both `Invoke` and `ExpandCollapse` patterns. |
| `StatusBar`       | `Region`          | Landmark-ish. |
| `Tab`             | `TabList`         | UIA names the *container* `Tab`. |
| `TabItem`         | `Tab`             | UIA names the *child* `TabItem`. (Yes, this is confusing.) |
| `Table`           | `Table`           | |
| `Text`            | `Generic`         | If parent is `Heading`-like, prefer `Heading` (rare in UIA). |
| `Thumb`           | `Generic`         | Internal slider piece; usually skipped. |
| `TitleBar`        | `Generic`         | Captured but not interactive. |
| `ToolBar`         | `Toolbar`         | |
| `ToolTip`         | `Generic`         | Captured but rarely useful for agents. |
| `Tree`            | `Tree`            | |
| `TreeItem`        | `TreeItem`        | |
| `Window`          | `Window`          | Top-level frames; nested dialogs use `Dialog`. |

**Promotion rules** (a node's `ControlType` is not always sufficient):
- `MenuItem` + `Toggle` pattern → `MenuItemCheckbox` (or `MenuItemRadio` if `IsRadio`)
- `ListItem` inside a parent with `Selection` pattern → `Option`
- `Window` whose `IsModal` is true → `Dialog`
- `Edit` with `IsPassword=true` → still `TextField` (we don't have a separate role; flag elsewhere later)

### 1.1 Win32 class-name promotion table

When `ControlType` is `Custom`, before falling back to `Unknown(class)` we
promote a small whitelist of well-known Win32 class names back to canonical
roles. This is the difference between "useful in Notepad / older apps" and
"opaque blob for the agent".

| `ClassName` (case-insensitive)       | Promoted `Role`  |
|---|---|
| `Edit`                               | `TextField`      |
| `Static`                             | `Generic`        |
| `Button`                             | `Button`         |
| `ComboBox`                           | `ComboBox`       |
| `SysListView32`                      | `List`           |
| `SysTreeView32`                      | `Tree`           |
| `RichEdit*` (any class starting with `RichEdit`) | `TextField` |

Anything else falls through to `Role::Unknown(class_name)` so the agent can
still see what the underlying control type is.

`WindowsForms10.*` class-name parsing (extracting the underlying control name
from the prefix) is intentionally deferred to v0.2.

## 2. State mapping (UIA properties → [`NodeState`](../crates/core/src/node.rs))

| `NodeState` field | UIA source |
|---|---|
| `visible`  | `!IsOffscreen` |
| `enabled`  | `IsEnabled` |
| `focused`  | `HasKeyboardFocus` |
| `selected` | `SelectionItemPattern.IsSelected` if pattern present, else `None` |
| `checked`  | `TogglePattern.ToggleState` (`On`→`True`, `Off`→`False`, `Indeterminate`→`Mixed`) |
| `expanded` | `ExpandCollapsePattern.ExpandCollapseState` (`Expanded` or `PartiallyExpanded` → `true`, else `false`) |
| `required` | `IsRequiredForForm` (rare, but exposed) |

**Caveats:**
- `IsOffscreen=true` does not mean invisible — it means outside the *viewport*. We map it to `visible=false` anyway because for agents the practical question is "could the user click this right now."
- `HasKeyboardFocus` is per-thread. Reading it across processes is reliable but slightly racy.

## 3. Property mapping (UIA properties → [`Node`](../crates/core/src/node.rs) fields)

| `Node` field   | UIA source |
|---|---|
| `name`         | `Name` (after trimming control characters; if empty, fall back to `LocalizedControlType` for landmarks) |
| `description`  | `HelpText` if non-empty and ≠ `Name`; else `None` |
| `value`        | `ValuePattern.Value` if present and not a password field; else `RangeValuePattern.Value.to_string()` |
| `bounds`       | `BoundingRectangle` after DPI normalization (see §6) |
| `level`        | `Level` property where applicable (tree items, headings); else `None` |
| `role`         | per §1 |
| `state`        | per §2 |
| `native`       | `NativeHandle::Uia { runtime_id, automation_id }` (see §7) |

**Dropped fields** (intentionally not carried into `Node`):
- `AcceleratorKey`, `AccessKey` — keyboard hints; agents rarely need them and we surface keyboard input separately.
- `FrameworkId` — debugging only.
- `ItemStatus` / `ItemType` — application-specific strings; surface later via a generic annotation map if needed.
- `Orientation` — rarely matters for agents; skip until we have a concrete use case.
- `IsContentElement` / `IsControlElement` — internal UIA tree-pruning hints; we do our own pruning per [`SnapshotOptions`](../crates/core/src/snapshot.rs).

## 4. Action mapping ([`Action`](../crates/core/src/action.rs) → UIA patterns)

For each action we accept, the UIA call we make. Falls back to synthetic
`SendInput` only when the relevant pattern is unavailable.

| `Action`            | UIA call                                                           | Fallback |
|---|---|---|
| `Click`             | `InvokePattern.Invoke()` if present                                | Move cursor to centre of `BoundingRectangle`, send mouse left-down/up. |
| `DoubleClick`       | _Deferred to v0.2_                                                 | SendInput double-click. |
| `RightClick`        | _Deferred to v0.2_                                                 | SendInput right-click. |
| `Hover`             | _Deferred to v0.2_                                                 | Move cursor only. |
| `Focus`             | `SetFocus()` on the element                                        | n/a |
| `Type`              | `SendInput` keyboard events for each character at current focus    | n/a |
| `Fill`              | `ValuePattern.SetValue(value)` if pattern is read-write            | `Focus` → `SelectAll` (`Ctrl+A`) → `SendInput` characters. |
| `Press`             | `SendInput` key chord                                              | n/a |
| `KeyDown` / `KeyUp` | `SendInput` half-events                                            | n/a |
| `Scroll`            | `ScrollPattern.Scroll(...)` or `ScrollPattern.SetScrollPercent(...)` | SendInput mouse wheel. |
| `ScrollIntoView`    | `ScrollItemPattern.ScrollIntoView()`                               | n/a |
| `Select`            | `SelectionItemPattern.Select()` (or `AddToSelection` for multi-select) | n/a |
| `SelectAll`         | `SendInput` `Ctrl+A`                                               | n/a |
| `Drag`              | _Deferred to v0.2_                                                 | SendInput mouse-down on `from`, move to `to`, mouse-up. |
| `Wait`              | `tokio::time::sleep`                                               | n/a |
| `SwitchApp`         | `SetForegroundWindow(hwnd_of_app)`; we'll need a process→hwnd lookup | n/a |
| `FocusWindow`       | `WindowPattern.SetWindowVisualState(Normal)` + `SetFocus`          | `SetForegroundWindow`. |
| `Screenshot`        | `BitBlt` of the desktop or of the window's HWND                    | n/a |

**Decision:** for v0.1, `surface-uia` advertises `snapshot`, `keyboard`,
`mouse`, `multi_app`. Drag, double/right-click, hover, screenshot are wired
in v0.2. The `CapabilitySet` returned from `UiaSurface::open()` will reflect
this and the daemon won't dispatch anything unsupported.

## 5. App and window context

| Field | Source |
|---|---|
| `app.id`         | Application User Model ID (AUMID) when available; else process executable basename. AUMID lookup uses `SHGetPropertyStoreForWindow` + `PKEY_AppUserModel_ID`. |
| `app.name`       | Process executable's `FileDescription` from version resources, falling back to executable basename. |
| `window.id`      | Top-level `HWND` rendered as a hex string. |
| `window.title`   | The top-level window's UIA `Name` (which on Windows is the title bar). |

### 5.1 Window targeting

`SnapshotOptions::target` (a [`WindowTarget`](../crates/core/src/snapshot.rs))
selects which window the snapshot captures. Three variants in v0.1:

- `Foreground` *(default)* — `GetForegroundWindow()`. Original behavior.
- `Pid { pid }` — first visible top-level window owned by `pid`. Found via `EnumWindows` + `GetWindowThreadProcessId`.
- `Title { title }` — first visible top-level window whose title contains `title` (case-insensitive). Found via `EnumWindows` + `GetWindowTextW`.

Once a snapshot resolves a target, the worker stores the HWND on `WorkerState`
and **subsequent actions reuse the same HWND**, even if the user changes focus.
This makes the surface usable for background automation and immune to Windows'
`ForegroundLockTimeout` policy when driving non-foreground apps.

Multi-app desktop snapshots (capturing several windows in one tree) are
deferred. For now, snapshot one window; switch sessions / re-snapshot to
target another.

## 6. Coordinate handling

UIA's `BoundingRectangle` is in **physical pixels**. Our [`Bounds`](../crates/core/src/node.rs)
is documented as logical / DPI-normalized.

**Decision:** at session open we capture the DPI of the monitor the
foreground window is on (`GetDpiForWindow`) and divide all `BoundingRectangle`
values by `dpi / 96.0` before populating `Bounds`. Multi-monitor setups with
mixed DPI are edge-cased to the foreground monitor for v0.1.

## 7. `NativeHandle::Uia`

```rust
NativeHandle::Uia {
    runtime_id: Vec<u8>,        // UIA RuntimeId, packed as bytes
    automation_id: Option<String>,
}
```

We populate both. `automation_id` is the most stable identifier UIA exposes
(set by the developer at design time on WPF / WinUI controls); `runtime_id`
is what UIA itself uses to compare elements but is unstable across runs.

At action-time re-resolution we try in order:
1. `automation_id` lookup if set — fast and durable.
2. `runtime_id` comparison — works within the same UIA session.
3. `(role, name, nth)` walk from the [`RefMap`](../crates/core/src/snapshot.rs) — durable across UIA invalidations.

## 8. Ref-map keying

For each interactive node we emit, the `RefMap` entry stores:

- `role`     — per §1
- `name`     — `Name` after trimming
- `nth`      — 0-based count of preceding siblings with the same `(role, name)` under the same parent
- `native`   — `NativeHandle::Uia` (per §7)

`(role, name, nth)` is the durable lookup tuple. UIA-specific identifiers
are a fast-path hint, never the source of truth.

## 9. Tree walking strategy

UIA has three views: `Raw` (everything), `Control` (only control-typed), and
`Content` (only content-bearing). We use **`Control` view as the default** —
`Raw` is too noisy for agents and `Content` drops buttons.

**`Generic` nodes are emitted by default.** They carry contextual labels
(group headers, sections, panels) that an agent often needs to disambiguate
controls that share names. To keep the tree small for token-budgeted agents,
`SnapshotOptions::compact = true` strips `Generic`, `Pane`, `Group`,
`Separator`, `TitleBar`, and `Thumb` from the emitted tree (children are
still walked; structural ancestors are reattached to the nearest non-stripped
ancestor).

The CLI `snapshot` command defaults to `compact = true` for terminal
readability. Programmatic clients can opt into full fidelity by passing
`compact: false`.

Walk depth-first, stop at `SnapshotOptions::depth` if set. At each node
decide whether to emit a `RefId` based on:

- Role is interactive (per `Role::is_interactive`), OR
- Has the `Invoke` or `Value` pattern, OR
- `IsKeyboardFocusable` is true and role is not purely structural.

That last rule catches custom controls that aren't ARIA-classified but the
user can clearly act on.

## 10. Threading and COM

UIA is a COM API. Calls into UIA from a thread that hasn't called
`CoInitializeEx` will fail. Rules for `surface-uia`:

- Initialize each UIA-using thread with `CoInitializeEx(COINIT_MULTITHREADED)`.
- Do **not** marshal UIA elements across threads — re-resolve via the patterns above.
- All `Surface` trait methods take `&self` so they can be called concurrently from the daemon, but each call internally pins itself to a single worker thread for COM safety.

## 11. Gaps and intentional drops

Things UIA exposes that we are deliberately not surfacing in v0.1:

- **Annotations / live regions** — `AnnotationPattern`, `LiveSetting`. Useful for screen readers; not for agents (yet).
- **Text patterns** — full `TextPattern` access (text ranges, attributes, find). Massive surface, deferred to a separate text-aware iteration.
- **Virtualization** — `VirtualizedItemPattern` realisation. We will hit this when snapshotting large list/grid controls (Outlook, Excel). Track as a known gap; for v0.1 we capture only realised items.
- **Drag-and-drop** — `DragPattern` / `DropTargetPattern`. Deferred.
- **Custom annotation properties** — UIA lets apps expose arbitrary string properties. Skip until a concrete use case appears.
- **Direct MSAA path.** Older Win32 apps with no native UIA support fall through Windows' built-in UIA→MSAA bridge, which gives reduced-fidelity trees but is "good enough" for v0.1. If a critical real app needs more, we add a parallel `IAccessible` walker; until then, we trust the bridge.

## 12. Resolved decisions (was: open questions)

These were open at draft time; recording the calls so we don't relitigate.

1. **Emit `Generic` nodes?** Yes by default, stripped by `compact: true` (which the CLI defaults to). Programmatic clients pass `compact: false` for full fidelity. Rationale: structural labels disambiguate same-named controls; agents that don't need them can flip the flag.
2. **`Custom` class-name promotion?** Yes, small whitelist (see §1.1). Caught the obvious legacy controls (`Edit`, `Static`, `Button`, list/tree views, rich-edit). `WindowsForms10.*` prefix parsing deferred.
3. **MSAA fallback?** No for v0.1 — see §11. Rely on the UIA→MSAA bridge until a real app forces our hand.
