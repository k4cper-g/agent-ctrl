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
- `IsOffscreen=true` does not mean invisible - it means outside the *viewport*. We map it to `visible=false` anyway because for agents the practical question is "could the user click this right now."
- `HasKeyboardFocus` is per-thread. Reading it across processes is reliable but slightly racy.

## 3. Property mapping (UIA properties → [`Node`](../crates/core/src/node.rs) fields)

| `Node` field   | UIA source |
|---|---|
| `name`         | `Name` as reported by UIA; empty string when the platform exposes none |
| `description`  | `HelpText` when non-empty and ≠ `Name`; else `None` |
| `value`        | `ValuePattern.Value` when present and `IsPassword=false`; else `RangeValuePattern.Value` formatted as a string (sliders, spinners - whole numbers render without a fractional part) |
| `bounds`       | `BoundingRectangle` after DPI normalization (see §6) |
| `level`        | UIA `Level` property when positive (tree items, list items, headings); else `None` |
| `role`         | per §1 |
| `state`        | per §2 |
| `native`       | `NativeHandle::Uia { runtime_id, automation_id }` (see §7) |

**Dropped fields** (intentionally not carried into `Node`):
- `AcceleratorKey`, `AccessKey` - keyboard hints; agents rarely need them and we surface keyboard input separately.
- `FrameworkId` - debugging only.
- `ItemStatus` / `ItemType` - application-specific strings; surface later via a generic annotation map if needed.
- `Orientation` - rarely matters for agents; skip until we have a concrete use case.
- `IsContentElement` / `IsControlElement` - internal UIA tree-pruning hints; we do our own pruning per [`SnapshotOptions`](../crates/core/src/snapshot.rs).

## 4. Action mapping ([`Action`](../crates/core/src/action.rs) → UIA patterns)

For each action we accept, the UIA call we make. Falls back to synthetic
`SendInput` only when the relevant pattern is unavailable.

| `Action`            | UIA call                                                           | Fallback |
|---|---|---|
| `Click`             | `InvokePattern.Invoke()` if present; for `Button` roles also `SetFocus` + Space before the pointer fallback | Move cursor to centre of `BoundingRectangle`, send mouse left-down/up. |
| `DoubleClick`       | SendInput double left-click at the element's centre               | n/a |
| `RightClick`        | SendInput secondary-button click at the element's centre          | n/a |
| `Hover`             | Move cursor to the element's centre                                | n/a |
| `Highlight`         | Same as `Hover`, optionally held for a duration                    | n/a |
| `Focus`             | `SetFocus()` on the element                                        | n/a |
| `Type`              | `SendInput` `KEYEVENTF_UNICODE` events at current focus           | n/a |
| `Fill`              | `ValuePattern.SetValue(value)` if pattern is read-write            | none automatic - use `clipboard write` + `clipboard paste`, or `type`, for controls without a writable `ValuePattern`. |
| `Clear`             | `ValuePattern.SetValue("")` (verified), else `SetFocus` + `Ctrl+A` + `Delete` (verified) | n/a |
| `Press`             | `SendInput` key chord                                              | n/a |
| `KeyDown` / `KeyUp` | `SendInput` half-events                                            | n/a |
| `Scroll`            | `SendInput` mouse wheel at the cursor (positioned over the ref's centre when one is given) | n/a (`ScrollPattern.SetScrollPercent` positioning deferred). |
| `ScrollIntoView`    | `ScrollItemPattern.ScrollIntoView()`                               | n/a |
| `Select`            | `SelectionItemPattern.Select()` on the ref, or on the first named `SelectionItem` descendant when the ref is the container | n/a |
| `SelectAll`         | `SetFocus` (when a ref is given) + `SendInput` `Ctrl+A`            | n/a |
| `Check` / `Uncheck` | `TogglePattern.Toggle()` until `ToggleState` matches (bounded retries) | n/a |
| `Toggle`            | `TogglePattern.Toggle()`                                          | n/a |
| `Clipboard`         | Win32 clipboard for read/write; `Ctrl+C` / `Ctrl+V` for copy/paste | n/a |
| `Mouse`             | `SendInput` raw move / button / wheel in screen coordinates        | n/a |
| `Drag`              | `SendInput` mouse-down on `from`, interpolated moves to `to`, mouse-up | n/a |
| `Wait`              | `std::thread::sleep` on the worker (`wait-for` is the better primitive) | n/a |
| `SwitchApp`         | `SetForegroundWindow` of the first visible top-level window owned by the app; re-pins the session | n/a |
| `FocusWindow`       | `WindowPattern.SetWindowVisualState(Normal)` (best-effort) + the `AttachThreadInput` foreground bringer; re-pins the session | n/a |
| `Screenshot`        | `GetWindowDC` + `BitBlt` for the pinned window, screen DC for desktop / region / element-ref targets, optional cached-ref annotations | n/a |

**Decision:** `surface-uia` advertises `snapshot`, `screenshot`, `keyboard`,
`mouse`, `drag`, `multi_app`. Click / double-click / right-click / hover,
drag, and screenshot (window / region / element-ref / desktop, with optional
`@eN` annotations) are all wired. The `CapabilitySet` returned from
`UiaSurface::open()` reflects this and the daemon won't dispatch anything
unsupported. Synthetic input (`SendInput`) requires the pinned window to be
foreground; the surface brings it forward first and reports a clear error if
UIPI/UAC blocks injection.

**Off-screen / virtualized targets.** Every ref-targeted *action* routes
through a resolve step that, when the target element is off-screen, realizes
it (`VirtualizedItemPattern.Realize`), scrolls it into its container's
viewport (`ScrollItemPattern.ScrollIntoView`), and then re-resolves for a
fresh handle (a list/grid commonly rebuilds item elements as it scrolls, so
the pre-scroll handle would report a stale `BoundingRectangle`). So
`click` / `double-click` / `drag` / etc. land correctly on a list or grid
item an agent referenced from a snapshot even if it has since scrolled out of
view. `screenshot --target ref` deliberately skips this so a capture never
scrolls the target app as a side effect.

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

- `Foreground` *(default)* - `GetForegroundWindow()`. Original behavior.
- `Pid { pid }` - first visible top-level window owned by `pid`. Found via `EnumWindows` + `GetWindowThreadProcessId`.
- `Title { title }` - first visible top-level window whose title contains `title` (case-insensitive). Found via `EnumWindows` + `GetWindowTextW`.

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

**Decision:** the UIA worker thread calls
`SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)`
before any window or COM call, so `BoundingRectangle` (physical),
`GetSystemMetrics(SM_*VIRTUALSCREEN)` (used to map screen points into
`MOUSEEVENTF_ABSOLUTE` coordinates), and `SendInput` cursor positioning all
share one coordinate space. For each snapshot we then read the DPI of the
monitor hosting the pinned window (`GetDpiForWindow`) and divide every
`BoundingRectangle` by `dpi / 96.0` before populating `Bounds`. Multi-monitor
setups with mixed DPI where one window spans monitors are edge-cased to the
pinned window's monitor.

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
1. `automation_id` lookup - fast and durable, but only for the first
   occurrence of a `(role, name)` pair (`nth == 0`). AutomationIds are
   duplicated across repeated templates (every list row's "Delete" button
   shares one), so `FindFirst` cannot address a later occurrence; a non-zero
   `nth` falls through to tiers 2-3.
2. `runtime_id` comparison - works within the same UIA session.
3. `(role, name, nth)` walk from the [`RefMap`](../crates/core/src/snapshot.rs) - durable across UIA invalidations.

## 8. Ref-map keying

For each interactive node we emit, the `RefMap` entry stores:

- `role`     - per §1
- `name`     - `Name` after trimming
- `nth`      - 0-based count of preceding siblings with the same `(role, name)` under the same parent
- `native`   - `NativeHandle::Uia` (per §7)

`(role, name, nth)` is the durable lookup tuple. UIA-specific identifiers
are a fast-path hint, never the source of truth.

## 9. Tree walking strategy

UIA has three views: `Raw` (everything), `Control` (only control-typed), and
`Content` (only content-bearing). We use **`Control` view as the default** -
`Raw` is too noisy for agents and `Content` drops buttons.

**Snapshots are cache-backed.** A naive walk is O(nodes x properties)
cross-process COM round-trips, which is seconds of latency on Outlook/Excel-
scale trees. Instead, `snapshot` builds an `IUIAutomationCacheRequest` (the
plain element properties, with a Control-view tree filter), calls
`IUIAutomationElement::BuildUpdatedCache` once to pull the whole window
subtree into a local cache, then walks via `GetCachedChildren` reading every
plain property with in-process `Cached*` getters. Pattern-derived state
(`value`, `checked`, `expanded`, `selected`) and `RuntimeId` are still read
live, but only for roles that plausibly carry them, so a tree of structural
nodes costs roughly one round-trip per node. `SnapshotOptions::depth` of 0 or
1 narrows the cache scope (`TreeScope_Element` / `TreeScope_Children`);
deeper or unbounded requests cache the full subtree and trim during emission.
The **action path** (re-resolving a `RefId`) still uses live `ControlViewWalker`
calls - it stops at the target node, so the per-node cost matters less there.

**`Generic` nodes are emitted by default.** They carry contextual labels
(group headers, sections, panels) that an agent often needs to disambiguate
controls that share names. To keep the tree small for token-budgeted agents,
`SnapshotOptions::compact = true` strips *unnamed, unfocused* `Generic` nodes
from the emitted tree - which covers UIA's `Pane`, `Separator`, `TitleBar`,
and `Thumb` control types, since §1 already maps all of those to `Generic`.
Named `Generic` nodes are kept (the name is the contextual label that earns
their place), as are `Group` nodes. Children of a stripped node are still
walked and reattached to the nearest surviving ancestor.

The CLI `snapshot` command defaults to `compact = true` for terminal
readability. Programmatic clients can opt into full fidelity by passing
`compact: false`.

Walk depth-first, stop at `SnapshotOptions::depth` if set. At each node
decide whether to emit a `RefId`. An element earns one when **any** of these
holds (this is the `qualifies_for_ref` predicate):

- its role is interactive (per `Role::is_interactive`); OR
- it exposes a non-read-only `ValuePattern` (an editable field whose role
  isn't ARIA-interactive - e.g. Win11 Notepad's `Document`-typed canvas); OR
- it is keyboard-focusable (`IsKeyboardFocusable`) and its role is **not**
  purely structural (per `Role::is_structural`).

That last rule catches custom controls that aren't ARIA-classified but the
user can clearly Tab to and act on; excluding structural roles keeps a
focusable pane or group from flooding the ref map.

The action path re-resolves a `RefId` by re-walking the tree counting the
same `(role, name, nth)`, so it **must** apply this identical predicate -
`element_qualifies_as_ref` is the live-read mirror of `qualifies_for_ref`.
If the two drift, refs silently stop resolving; the fixture integration test
round-trips every emitted ref through the action path to guard this.

## 10. Threading and COM

UIA is a COM API. Calls into UIA from a thread that hasn't called
`CoInitializeEx` will fail. Rules for `surface-uia`:

- Initialize each UIA-using thread with `CoInitializeEx(COINIT_MULTITHREADED)`.
- Do **not** marshal UIA elements across threads - re-resolve via the patterns above.
- All `Surface` trait methods take `&self` so they can be called concurrently from the daemon, but each call internally pins itself to a single worker thread for COM safety.
- A worker job that panics is caught (`catch_unwind`); the caller sees an error and the session continues.
- A worker job that doesn't return within ~45s (a COM call to an unresponsive target will never come back) times out: the session is marked wedged, every later call on it fails fast, and the stuck thread is abandoned rather than joined. Recover by closing and re-opening the session.

## 11. Gaps and intentional drops

Things UIA exposes that we are deliberately not surfacing in v0.1:

- **Annotations / live regions** - `AnnotationPattern`, `LiveSetting`. Useful for screen readers; not for agents (yet).
- **Text patterns** - full `TextPattern` access (text ranges, attributes, find). Massive surface, deferred to a separate text-aware iteration.
- **Virtualization (snapshot side)** - a snapshot still captures only the *realised* items of a large virtualized list/grid; off-screen rows a provider hasn't materialised aren't in the tree. *Action-time* virtualization is handled (see §4): a ref-targeted action realises a `VirtualizedItemPattern` placeholder and scrolls an off-screen item into view before acting. Capturing unrealised items into the snapshot itself (walking a container via `ItemContainerPattern.FindItemByProperty`) remains a gap - for now, scroll and re-`snapshot` to bring more rows into the tree.
- **Drag-and-drop** - `DragPattern` / `DropTargetPattern`. Deferred.
- **Custom annotation properties** - UIA lets apps expose arbitrary string properties. Skip until a concrete use case appears.
- **Direct MSAA path.** Older Win32 apps with no native UIA support fall through Windows' built-in UIA→MSAA bridge, which gives reduced-fidelity trees but is "good enough" for v0.1. If a critical real app needs more, we add a parallel `IAccessible` walker; until then, we trust the bridge.
- **Cached action-time resolution.** Snapshots are cache-backed (§9), but re-resolving a `RefId` for an action still walks the Control view with live calls. It stops at the target so the per-node cost is bounded, but a future iteration could share a `BuildUpdatedCache` pass with the snapshot when an action immediately follows it.
- **`ScrollPattern` positioning.** `Scroll` only emits wheel events at the cursor; scrolling to a specific position via `ScrollPattern.SetScrollPercent` is deferred.

## 12. Resolved decisions (was: open questions)

These were open at draft time; recording the calls so we don't relitigate.

1. **Emit `Generic` nodes?** Yes by default, stripped by `compact: true` (which the CLI defaults to). Programmatic clients pass `compact: false` for full fidelity. Rationale: structural labels disambiguate same-named controls; agents that don't need them can flip the flag.
2. **`Custom` class-name promotion?** Yes, small whitelist (see §1.1). Caught the obvious legacy controls (`Edit`, `Static`, `Button`, list/tree views, rich-edit). `WindowsForms10.*` prefix parsing deferred.
3. **MSAA fallback?** No for v0.1 - see §11. Rely on the UIA→MSAA bridge until a real app forces our hand.
