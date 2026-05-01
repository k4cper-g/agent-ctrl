//! Windows-only UIA implementation for [`super::UiaSurface`].
//!
//! UIA is a COM API. The [`IUIAutomation`] handle is `!Send`, so we own a
//! dedicated worker thread that:
//!
//! 1. Initializes COM in the multi-threaded apartment (MTA).
//! 2. Creates the `CUIAutomation` singleton and keeps the `IUIAutomation`
//!    handle alive on this thread.
//! 3. Receives `WorkerCmd`s over a `mpsc` channel and dispatches them.
//! 4. Tears COM down cleanly when the channel closes or `Shutdown` arrives.
//!
//! Concurrent callers are serialized through the channel, which matches
//! UIA's effective threading model anyway. See `docs/uia-mapping.md` §10.

#![cfg(target_os = "windows")]

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use windows::core::Result as WinResult;
use windows::core::{BSTR, VARIANT};
use windows::Win32::Foundation::LPARAM;
use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE, HWND};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    SAFEARRAY,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayDestroy, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded,
    ExpandCollapseState_LeafNode, ExpandCollapseState_PartiallyExpanded, IUIAutomation,
    IUIAutomationCondition, IUIAutomationElement, IUIAutomationExpandCollapsePattern,
    IUIAutomationInvokePattern, IUIAutomationScrollItemPattern, IUIAutomationSelectionItemPattern,
    IUIAutomationSelectionPattern, IUIAutomationTogglePattern, IUIAutomationTreeWalker,
    IUIAutomationValuePattern, IUIAutomationWindowPattern, ToggleState_Indeterminate,
    ToggleState_Off, ToggleState_On, TreeScope, TreeScope_Subtree, UIA_AutomationIdPropertyId,
    UIA_ButtonControlTypeId, UIA_CalendarControlTypeId, UIA_CheckBoxControlTypeId,
    UIA_ComboBoxControlTypeId, UIA_CustomControlTypeId, UIA_DataGridControlTypeId,
    UIA_DataItemControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_ExpandCollapsePatternId, UIA_GroupControlTypeId, UIA_HeaderControlTypeId,
    UIA_HeaderItemControlTypeId, UIA_HyperlinkControlTypeId, UIA_ImageControlTypeId,
    UIA_InvokePatternId, UIA_ListControlTypeId, UIA_ListItemControlTypeId,
    UIA_MenuBarControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId,
    UIA_PaneControlTypeId, UIA_ProgressBarControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_ScrollBarControlTypeId, UIA_ScrollItemPatternId, UIA_SelectionItemPatternId,
    UIA_SelectionPatternId, UIA_SemanticZoomControlTypeId, UIA_SeparatorControlTypeId,
    UIA_SliderControlTypeId, UIA_SpinnerControlTypeId, UIA_SplitButtonControlTypeId,
    UIA_StatusBarControlTypeId, UIA_TabControlTypeId, UIA_TabItemControlTypeId,
    UIA_TableControlTypeId, UIA_TextControlTypeId, UIA_ThumbControlTypeId,
    UIA_TitleBarControlTypeId, UIA_TogglePatternId, UIA_ToolBarControlTypeId,
    UIA_ToolTipControlTypeId, UIA_TreeControlTypeId, UIA_TreeItemControlTypeId, UIA_ValuePatternId,
    UIA_WindowControlTypeId, UIA_WindowPatternId, WindowVisualState_Normal, UIA_CONTROLTYPE_ID,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, VIRTUAL_KEY,
    VK_APPS, VK_BACK, VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME,
    VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_NUMLOCK, VK_PAUSE, VK_PRIOR, VK_RETURN,
    VK_RIGHT, VK_RWIN, VK_SCROLL, VK_SHIFT, VK_SNAPSHOT, VK_SPACE, VK_TAB, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetSystemMetrics, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use agent_ctrl_core::{
    Action, ActionResult, AppContext, Bounds, Checked, Error, NativeHandle, Node, RefEntry, RefId,
    RefMap, Result, Role, Snapshot, SnapshotOptions, State, SurfaceKind, WindowContext,
    WindowTarget,
};

// ---------- Worker thread ----------

/// A unit of work to run on the UIA worker thread. The closure receives the
/// worker's mutable state and is responsible for shipping its own result back
/// to the caller (typically via a captured `oneshot::Sender`).
type WorkerJob = Box<dyn FnOnce(&mut WorkerState) + Send>;

/// Commands the worker thread accepts.
enum WorkerCmd {
    /// Run an arbitrary closure with access to [`WorkerState`].
    Run(WorkerJob),
    /// Cooperative shutdown — drop COM objects and exit the loop.
    Shutdown,
}

/// State held by the UIA worker thread. Owns the live COM objects plus the
/// `RefMap` and target window from the most recent snapshot for action-time
/// re-resolution.
struct WorkerState {
    automation: IUIAutomation,
    /// `RefMap` from the most recent successful snapshot. Replaced wholesale
    /// each time `snapshot()` runs, so action-time resolution can never use
    /// stale entries from older snapshots.
    last_refs: RefMap,
    /// HWND that the most recent snapshot was taken from. `None` until the
    /// first snapshot. Subsequent actions resolve elements relative to this
    /// window, not the foreground (which may have changed in the meantime).
    last_hwnd: Option<HWND>,
}

/// Owns the worker thread that holds the live UIA session.
///
/// Drop sends a `Shutdown` and waits for the worker to exit. If the worker
/// has already died the drop is effectively a no-op.
pub(crate) struct UiaInner {
    sender: Mutex<mpsc::Sender<WorkerCmd>>,
    worker: Option<JoinHandle<()>>,
}

impl UiaInner {
    /// Spawn the worker, wait for COM/UIA init to succeed (or fail fast).
    pub(crate) fn new() -> Result<Self> {
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerCmd>();
        let (ready_tx, ready_rx) = mpsc::channel::<WinResult<()>>();

        let worker = thread::Builder::new()
            .name("agent-ctrl-uia".into())
            .spawn(move || {
                // SAFETY: `CoInitializeEx` is sound to call once per thread. We
                // pass `None` for the reserved pointer (documented usage) and
                // `COINIT_MULTITHREADED` because UIA supports MTA and we never
                // pump a message loop on this thread.
                let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
                if init.is_err() {
                    let _ = ready_tx.send(Err(init.into()));
                    return;
                }

                // SAFETY: `CUIAutomation` and `IUIAutomation` are well-known COM
                // identifiers shipped with Windows. `CLSCTX_INPROC_SERVER` is the
                // documented activation context for the UIA client library.
                let automation: IUIAutomation =
                    match unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) } {
                        Ok(a) => a,
                        Err(e) => {
                            let _ = ready_tx.send(Err(e));
                            // SAFETY: paired with the successful `CoInitializeEx` above.
                            unsafe { CoUninitialize() };
                            return;
                        }
                    };

                let _ = ready_tx.send(Ok(()));

                let mut state = WorkerState {
                    automation,
                    last_refs: RefMap::new(),
                    last_hwnd: None,
                };

                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        WorkerCmd::Run(job) => job(&mut state),
                        WorkerCmd::Shutdown => break,
                    }
                }

                // Order matters: drop COM interfaces BEFORE `CoUninitialize`,
                // since releasing an interface after uninit is undefined behavior.
                drop(state);

                // SAFETY: paired with the successful `CoInitializeEx` above.
                unsafe { CoUninitialize() };
            })
            .map_err(|e| Error::Surface(format!("failed to spawn UIA worker: {e}")))?;

        // Wait for the worker to finish COM init and report status.
        match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self {
                sender: Mutex::new(cmd_tx),
                worker: Some(worker),
            }),
            Ok(Err(e)) => Err(Error::Surface(format!("UIA initialization failed: {e}"))),
            Err(RecvTimeoutError::Timeout) => Err(Error::Surface(
                "UIA worker did not complete init in 5s".into(),
            )),
            Err(RecvTimeoutError::Disconnected) => {
                Err(Error::Surface("UIA worker died during init".into()))
            }
        }
    }

    /// Run a closure on the UIA worker thread and await its result.
    ///
    /// The closure receives a mutable reference to [`WorkerState`], giving it
    /// access to the live `IUIAutomation` and the most recent snapshot's refs.
    /// COM safety: the closure runs *on* the worker thread, so it can use
    /// `!Send` UIA types freely. Only the returned `R` (which must be `Send`)
    /// crosses thread boundaries.
    async fn run<F, R>(&self, f: F) -> Result<R>
    where
        F: FnOnce(&mut WorkerState) -> Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let job: WorkerJob = Box::new(move |state| {
            let _ = reply_tx.send(f(state));
        });

        // Send the job. Brief lock; released before the await.
        let send_result = match self.sender.lock() {
            Ok(sender) => sender.send(WorkerCmd::Run(job)),
            Err(_) => return Err(Error::Surface("UIA worker sender mutex poisoned".into())),
        };
        if send_result.is_err() {
            return Err(Error::Surface("UIA worker is no longer running".into()));
        }

        match reply_rx.await {
            Ok(result) => result,
            Err(_) => Err(Error::Surface("UIA worker dropped reply channel".into())),
        }
    }

    /// Capture a snapshot of the foreground window.
    pub(crate) async fn snapshot(&self, opts: &SnapshotOptions) -> Result<Snapshot> {
        let opts = opts.clone();
        self.run(move |state| capture_foreground(state, &opts))
            .await
    }

    /// Execute an action against the most recent snapshot's refs.
    pub(crate) async fn act(&self, action: Action) -> Result<ActionResult> {
        self.run(move |state| act_dispatch(state, &action)).await
    }
}

impl Drop for UiaInner {
    fn drop(&mut self) {
        if let Ok(sender) = self.sender.lock() {
            let _ = sender.send(WorkerCmd::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

// ---------- Snapshot pieces ----------

/// Build a [`Snapshot`] for the targeted window and store its refs + HWND
/// on the [`WorkerState`] for subsequent action-time re-resolution.
fn capture_foreground(state: &mut WorkerState, opts: &SnapshotOptions) -> Result<Snapshot> {
    let hwnd = resolve_target_hwnd(&opts.target)?;
    let snap = capture_with_options(&state.automation, opts, hwnd)?;
    state.last_refs = snap.refs.clone();
    state.last_hwnd = Some(hwnd);
    Ok(snap)
}

fn capture_with_options(
    automation: &IUIAutomation,
    opts: &SnapshotOptions,
    hwnd: HWND,
) -> Result<Snapshot> {
    // SAFETY: `hwnd` is either a valid window handle or null; `ElementFromHandle`
    // is documented to return an error in the latter case rather than dereferencing.
    let root_element = unsafe { automation.ElementFromHandle(hwnd) }
        .map_err(|e| Error::Snapshot(format!("ElementFromHandle: {e}")))?;

    // Capture per-window DPI once; we apply the same factor to every node's
    // bounds since they all live in the same window. See docs §6.
    let dpi_scale = window_dpi_scale(hwnd);

    // The Control view walker is the right tradeoff for agents (see docs §9):
    // skips the lowest-level rendering noise without hiding interactive controls.
    let walker: IUIAutomationTreeWalker = unsafe { automation.ControlViewWalker() }
        .map_err(|e| Error::Snapshot(format!("ControlViewWalker: {e}")))?;

    let mut refs = RefMap::new();
    // `nth_seen` tracks `(role, name) → count` *globally* across the whole
    // snapshot in pre-order DFS. The action-time walker mirrors this exactly
    // so it can rediscover any element the agent references.
    let mut nth_seen: HashMap<(Role, String), usize> = HashMap::new();

    let (mut root, root_editable) = build_node(&root_element, None, dpi_scale);
    if root.role.is_interactive() || root_editable {
        let key = (root.role.clone(), root.name.clone());
        let counter = nth_seen.entry(key).or_insert(0);
        let nth = *counter;
        *counter += 1;
        let id = refs.insert(
            root.role.clone(),
            root.name.clone(),
            nth,
            root.native.clone(),
        );
        root.ref_id = Some(id);
    }
    root.children = walk_children(
        &walker,
        &root_element,
        &mut refs,
        &mut nth_seen,
        1,
        opts,
        dpi_scale,
    )?;

    let window_title = if root.name.is_empty() {
        None
    } else {
        Some(root.name.clone())
    };

    // SAFETY: `root_element` is a valid COM interface.
    let pid_signed = unsafe { root_element.CurrentProcessId() }
        .map_err(|e| Error::Snapshot(format!("CurrentProcessId: {e}")))?;
    let pid = u32::try_from(pid_signed)
        .map_err(|_| Error::Snapshot(format!("invalid PID from UIA: {pid_signed}")))?;

    let (app_id, app_name) = process_info(pid)?;

    Ok(Snapshot {
        captured_at: SystemTime::now(),
        surface_kind: SurfaceKind::Uia,
        app: AppContext {
            id: app_id,
            name: app_name,
        },
        window: Some(WindowContext {
            id: format!("{:#x}", hwnd.0 as usize),
            title: window_title,
        }),
        root,
        refs,
    })
}

/// Walk every Control-view child of `parent` and emit `Node`s.
///
/// Pre-order DFS. `nth_seen` is the *global* per-snapshot counter described
/// in `capture_with_options`. Honors `opts.compact` (drop unnamed `Generic`
/// nodes; their children are hoisted into the caller's list) and `opts.depth`.
fn walk_children(
    walker: &IUIAutomationTreeWalker,
    parent: &IUIAutomationElement,
    refs: &mut RefMap,
    nth_seen: &mut HashMap<(Role, String), usize>,
    depth: usize,
    opts: &SnapshotOptions,
    dpi_scale: f64,
) -> Result<Vec<Node>> {
    if let Some(max) = opts.depth {
        if depth > max {
            return Ok(Vec::new());
        }
    }

    let mut result: Vec<Node> = Vec::new();

    // SAFETY: `parent` is a valid COM interface; walker methods return Err
    // (which we treat as "no more children") when there are no more siblings.
    let mut maybe_child = unsafe { walker.GetFirstChildElement(parent) }.ok();
    while let Some(child) = maybe_child {
        let (mut node, has_editable_value) = build_node(&child, Some(parent), dpi_scale);

        // Allocate ref BEFORE recursing so `nth` follows pre-order DFS.
        // `find_in_tree` mirrors this exact ordering at action time.
        // Refs are emitted for ARIA-interactive roles AND for any element
        // that exposes an editable `ValuePattern` (catches Document-typed
        // text editors like Win11 Notepad's main canvas).
        if node.role.is_interactive() || has_editable_value {
            let key = (node.role.clone(), node.name.clone());
            let counter = nth_seen.entry(key).or_insert(0);
            let nth = *counter;
            *counter += 1;
            let id = refs.insert(
                node.role.clone(),
                node.name.clone(),
                nth,
                node.native.clone(),
            );
            node.ref_id = Some(id);
        }

        node.children = walk_children(walker, &child, refs, nth_seen, depth + 1, opts, dpi_scale)?;

        if opts.compact && is_compactable(&node) {
            // Hoist the children into our caller's list — the empty Generic
            // wrapper is dropped, but its descendants survive.
            result.extend(std::mem::take(&mut node.children));
        } else {
            result.push(node);
        }

        // SAFETY: `child` is a valid COM interface obtained from the walker.
        maybe_child = unsafe { walker.GetNextSiblingElement(&child) }.ok();
    }

    Ok(result)
}

/// Roles that may carry an editable value via UIA's `ValuePattern`. We only
/// query the pattern for these to keep snapshot cost bounded — querying every
/// node would add a COM round-trip per element on trees of thousands.
fn role_might_have_value(role: &Role) -> bool {
    matches!(
        role,
        Role::TextField
            | Role::Document
            | Role::ComboBox
            | Role::SearchBox
            | Role::SpinButton
            | Role::Slider
            | Role::Unknown(_)
    )
}

/// Read the element's `ValuePattern` value when present.
///
/// Returns `(value, is_read_only)`. `None` if the pattern isn't supported.
fn read_value_pattern(element: &IUIAutomationElement) -> Option<(String, bool)> {
    // SAFETY: `element` is a valid COM interface; UIA pattern IDs are well-known.
    let pattern: IUIAutomationValuePattern =
        unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId) }.ok()?;
    let value = unsafe { pattern.CurrentValue() }
        .ok()
        .map(|b| b.to_string())
        .unwrap_or_default();
    let read_only = unsafe { pattern.CurrentIsReadOnly() }.is_ok_and(BOOL::as_bool);
    Some((value, read_only))
}

/// Map an [`IUIAutomationElement`] to a [`Node`] (no children populated).
///
/// Returns `(node, has_editable_value)`. Each property read is best-effort:
/// if a particular UIA call fails (some controls just don't implement them
/// all), we substitute a defensible default rather than failing the whole
/// node. `has_editable_value` is `true` when the element exposes a
/// non-read-only `ValuePattern`; the caller uses it to decide whether to
/// allocate a `RefId` even when the role isn't ARIA-interactive (the typical
/// case for Win11 Notepad's `Document`-typed edit area).
fn build_node(
    element: &IUIAutomationElement,
    parent: Option<&IUIAutomationElement>,
    dpi_scale: f64,
) -> (Node, bool) {
    // SAFETY: each Current* getter dereferences only the COM `this` pointer
    // (the receiver), which is valid for the lifetime of `element`.
    let control_type = unsafe { element.CurrentControlType() }.unwrap_or(UIA_CONTROLTYPE_ID(0));
    let class_name = unsafe { element.CurrentClassName() }
        .ok()
        .map(|b| b.to_string())
        .unwrap_or_default();
    let role = promoted_role(element, control_type, &class_name, parent);

    let name = unsafe { element.CurrentName() }
        .ok()
        .map(|b| b.to_string())
        .unwrap_or_default();

    let is_enabled = unsafe { element.CurrentIsEnabled() }.is_ok_and(BOOL::as_bool);
    let is_offscreen = unsafe { element.CurrentIsOffscreen() }.is_ok_and(BOOL::as_bool);
    let has_focus = unsafe { element.CurrentHasKeyboardFocus() }.is_ok_and(BOOL::as_bool);

    let bounds = unsafe { element.CurrentBoundingRectangle() }
        .ok()
        .map(|r| Bounds {
            x: f64::from(r.left) / dpi_scale,
            y: f64::from(r.top) / dpi_scale,
            w: f64::from(r.right - r.left) / dpi_scale,
            h: f64::from(r.bottom - r.top) / dpi_scale,
        });

    let (value, has_editable_value) = if role_might_have_value(&role) {
        match read_value_pattern(element) {
            Some((v, read_only)) => (if v.is_empty() { None } else { Some(v) }, !read_only),
            None => (None, false),
        }
    } else {
        (None, false)
    };

    let checked = if role_might_be_toggleable(&role) {
        read_toggle_state(element)
    } else {
        None
    };
    let expanded = if role_might_expand(&role) {
        read_expand_state(element)
    } else {
        None
    };
    let selected = if role_might_be_selectable(&role) {
        read_selection_state(element)
    } else {
        None
    };
    let required = if role_might_be_required(&role) {
        read_required_for_form(element)
    } else {
        None
    };

    let native = build_native_handle(element);

    let node = Node {
        ref_id: None,
        role,
        name,
        description: None,
        value,
        state: State {
            visible: !is_offscreen,
            enabled: is_enabled,
            focused: has_focus,
            selected,
            checked,
            expanded,
            required,
        },
        bounds,
        level: None,
        children: Vec::new(),
        opaque: false,
        native,
    };
    (node, has_editable_value)
}

/// Roles plausibly hosting `TogglePattern`. Cheap pre-filter so we don't
/// add a COM round-trip per element on trees of thousands.
fn role_might_be_toggleable(role: &Role) -> bool {
    matches!(
        role,
        Role::Checkbox
            | Role::Switch
            | Role::MenuItem
            | Role::MenuItemCheckbox
            | Role::Button
            | Role::Unknown(_)
    )
}

/// Roles plausibly hosting `ExpandCollapsePattern`. Includes ComboBox (the
/// drop-down expands), TreeItem (subtree disclosure), MenuItem (sub-menus),
/// and Group (collapsibles).
fn role_might_expand(role: &Role) -> bool {
    matches!(
        role,
        Role::ComboBox
            | Role::TreeItem
            | Role::MenuItem
            | Role::Group
            | Role::ListItem
            | Role::Unknown(_)
    )
}

/// Roles plausibly hosting `SelectionItemPattern`. Tabs, list options, and
/// tree items all expose `IsSelected`.
fn role_might_be_selectable(role: &Role) -> bool {
    matches!(
        role,
        Role::Tab
            | Role::ListItem
            | Role::TreeItem
            | Role::Radio
            | Role::Option
            | Role::MenuItemRadio
            | Role::Cell
            | Role::Row
            | Role::Unknown(_)
    )
}

/// Roles plausibly carrying `IsRequiredForForm`. Limited to form input
/// controls — most other elements never set it.
fn role_might_be_required(role: &Role) -> bool {
    matches!(
        role,
        Role::TextField | Role::ComboBox | Role::SearchBox | Role::SpinButton | Role::Unknown(_)
    )
}

/// Read `TogglePattern.ToggleState` if supported. Maps UIA's tristate
/// (`Off` / `On` / `Indeterminate`) to our [`Checked`] enum.
fn read_toggle_state(element: &IUIAutomationElement) -> Option<Checked> {
    // SAFETY: `element` is a valid COM interface; UIA pattern IDs are well-known.
    let pattern: IUIAutomationTogglePattern =
        unsafe { element.GetCurrentPatternAs(UIA_TogglePatternId) }.ok()?;
    let state = unsafe { pattern.CurrentToggleState() }.ok()?;
    Some(if state == ToggleState_On {
        Checked::True
    } else if state == ToggleState_Indeterminate {
        Checked::Mixed
    } else if state == ToggleState_Off {
        Checked::False
    } else {
        // Unknown future state — bias toward "off" rather than dropping the
        // signal entirely. A new ToggleState variant is unlikely but safer
        // to handle than to crash the snapshot.
        Checked::False
    })
}

/// Read `ExpandCollapsePattern.ExpandCollapseState` if supported. Per
/// `docs/uia-mapping.md` §2: `Expanded` and `PartiallyExpanded` map to
/// `true`; `Collapsed` and `LeafNode` map to `false`.
fn read_expand_state(element: &IUIAutomationElement) -> Option<bool> {
    // SAFETY: `element` is a valid COM interface.
    let pattern: IUIAutomationExpandCollapsePattern =
        unsafe { element.GetCurrentPatternAs(UIA_ExpandCollapsePatternId) }.ok()?;
    let state = unsafe { pattern.CurrentExpandCollapseState() }.ok()?;
    // Collapsed, LeafNode, and any future variant all map to `false` — we
    // only care whether children are currently showing. Reference the
    // remaining constants explicitly so an upstream rename surfaces here.
    let _ = (ExpandCollapseState_Collapsed, ExpandCollapseState_LeafNode);
    Some(state == ExpandCollapseState_Expanded || state == ExpandCollapseState_PartiallyExpanded)
}

/// Read `SelectionItemPattern.IsSelected` if supported.
fn read_selection_state(element: &IUIAutomationElement) -> Option<bool> {
    // SAFETY: `element` is a valid COM interface.
    let pattern: IUIAutomationSelectionItemPattern =
        unsafe { element.GetCurrentPatternAs(UIA_SelectionItemPatternId) }.ok()?;
    let is_selected = unsafe { pattern.CurrentIsSelected() }.ok()?;
    Some(is_selected.as_bool())
}

/// Read `IsRequiredForForm`. Always available as an element property; the
/// role gate keeps us from paying for it on elements that are never
/// form-required (panels, separators, etc.).
fn read_required_for_form(element: &IUIAutomationElement) -> Option<bool> {
    // SAFETY: `element` is a valid COM interface.
    let required = unsafe { element.CurrentIsRequiredForForm() }.ok()?;
    // Most apps never set this — only emit `Some(true)` to avoid littering
    // every form field with `required: false`.
    if required.as_bool() {
        Some(true)
    } else {
        None
    }
}

/// Build the platform handle stored on every emitted [`Node`]. The handle is
/// later cloned into the `RefMap` entry for elements that get a `RefId`, so
/// action-time resolution can take a fast path through UIA's property index
/// rather than walking the full subtree.
///
/// Returns `None` only when both `RuntimeId` extraction and `AutomationId`
/// reads fail — the element is then so degenerate that there's nothing
/// useful to record.
fn build_native_handle(element: &IUIAutomationElement) -> Option<NativeHandle> {
    let runtime_id = extract_runtime_id(element).unwrap_or_default();
    let automation_id = extract_automation_id(element);
    if runtime_id.is_empty() && automation_id.is_none() {
        return None;
    }
    Some(NativeHandle::Uia {
        runtime_id,
        automation_id,
    })
}

/// Pull the UIA `RuntimeId` out of an element and pack it as little-endian
/// bytes (4 bytes per `i32` slot). RuntimeIds are unstable across UIA
/// sessions but stable within one; we expose them so downstream code (and a
/// future fast-path resolver) can compare elements without re-walking.
fn extract_runtime_id(element: &IUIAutomationElement) -> Option<Vec<u8>> {
    // SAFETY: `element` is a valid COM interface; `GetRuntimeId` returns a
    // SAFEARRAY pointer we own and must destroy.
    let array_ptr = unsafe { element.GetRuntimeId() }.ok()?;
    if array_ptr.is_null() {
        return None;
    }
    let ids = read_i32_safearray(array_ptr);
    let mut bytes = Vec::with_capacity(ids.len() * 4);
    for id in ids {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    if bytes.is_empty() {
        None
    } else {
        Some(bytes)
    }
}

/// Read the `AutomationId` property. Most Win32 controls don't set it; WPF
/// and WinUI controls do. Empty strings are normalized to `None` so a present
/// `Some("")` never tricks the fast-path resolver into matching the wrong
/// element.
fn extract_automation_id(element: &IUIAutomationElement) -> Option<String> {
    // SAFETY: `element` is a valid COM interface.
    let bstr = unsafe { element.CurrentAutomationId() }.ok()?;
    let s = bstr.to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Read a one-dimensional SAFEARRAY of `i32` (the only shape `GetRuntimeId`
/// returns) into a `Vec<i32>`. Always destroys the SAFEARRAY before returning,
/// even on read failure — leaking would slowly bloat the UIA process's heap
/// across thousands of nodes.
fn read_i32_safearray(array: *mut SAFEARRAY) -> Vec<i32> {
    if array.is_null() {
        return Vec::new();
    }
    // SAFETY: `array` is a valid SAFEARRAY pointer obtained from UIA. Each
    // SafeArray* call below is safe given a valid pointer; we destroy the
    // array on every exit path. Bounds come back as i32s; data is a slice
    // of i32 we copy out before unaccessing.
    unsafe {
        let (Ok(lbound), Ok(ubound)) = (SafeArrayGetLBound(array, 1), SafeArrayGetUBound(array, 1))
        else {
            let _ = SafeArrayDestroy(array);
            return Vec::new();
        };
        let count = ubound.checked_sub(lbound).and_then(|d| d.checked_add(1));
        let Some(count) = count
            .filter(|c| *c > 0)
            .and_then(|c| usize::try_from(c).ok())
        else {
            let _ = SafeArrayDestroy(array);
            return Vec::new();
        };

        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        if SafeArrayAccessData(array, &raw mut data).is_err() || data.is_null() {
            let _ = SafeArrayDestroy(array);
            return Vec::new();
        }
        let slice = std::slice::from_raw_parts(data.cast::<i32>(), count);
        let out = slice.to_vec();
        let _ = SafeArrayUnaccessData(array);
        let _ = SafeArrayDestroy(array);
        out
    }
}

/// Compact-mode predicate: drop unnamed structural-only wrapper nodes.
fn is_compactable(node: &Node) -> bool {
    matches!(node.role, Role::Generic) && node.name.is_empty() && !node.state.focused
}

/// UIA ControlType → canonical [`Role`]. Per `docs/uia-mapping.md` §1.
fn role_from_control_type(ct: UIA_CONTROLTYPE_ID, class_name: &str) -> Role {
    // Mappings are listed grouped by target Role so that ControlTypes which
    // share a Role (e.g. Pane / ProgressBar / ScrollBar all become Generic)
    // can be collapsed without confusing clippy.
    if ct == UIA_ButtonControlTypeId || ct == UIA_SplitButtonControlTypeId {
        Role::Button
    } else if ct == UIA_CheckBoxControlTypeId {
        Role::Checkbox
    } else if ct == UIA_ComboBoxControlTypeId {
        Role::ComboBox
    } else if ct == UIA_CustomControlTypeId {
        promote_class_name(class_name)
    } else if ct == UIA_DataGridControlTypeId {
        Role::Grid
    } else if ct == UIA_DataItemControlTypeId {
        Role::Row
    } else if ct == UIA_DocumentControlTypeId {
        Role::Document
    } else if ct == UIA_EditControlTypeId {
        Role::TextField
    } else if ct == UIA_GroupControlTypeId
        || ct == UIA_CalendarControlTypeId
        || ct == UIA_SemanticZoomControlTypeId
    {
        Role::Group
    } else if ct == UIA_HeaderControlTypeId {
        Role::RowGroup
    } else if ct == UIA_HeaderItemControlTypeId {
        Role::ColumnHeader
    } else if ct == UIA_HyperlinkControlTypeId {
        Role::Link
    } else if ct == UIA_ImageControlTypeId {
        Role::Image
    } else if ct == UIA_ListControlTypeId {
        Role::List
    } else if ct == UIA_ListItemControlTypeId {
        Role::ListItem
    } else if ct == UIA_MenuControlTypeId {
        Role::Menu
    } else if ct == UIA_MenuBarControlTypeId {
        Role::MenuBar
    } else if ct == UIA_MenuItemControlTypeId {
        Role::MenuItem
    } else if ct == UIA_PaneControlTypeId
        || ct == UIA_ProgressBarControlTypeId
        || ct == UIA_ScrollBarControlTypeId
        || ct == UIA_SeparatorControlTypeId
        || ct == UIA_TextControlTypeId
        || ct == UIA_ThumbControlTypeId
        || ct == UIA_TitleBarControlTypeId
        || ct == UIA_ToolTipControlTypeId
    {
        Role::Generic
    } else if ct == UIA_RadioButtonControlTypeId {
        Role::Radio
    } else if ct == UIA_SliderControlTypeId {
        Role::Slider
    } else if ct == UIA_SpinnerControlTypeId {
        Role::SpinButton
    } else if ct == UIA_StatusBarControlTypeId {
        Role::Region
    } else if ct == UIA_TabControlTypeId {
        Role::TabList
    } else if ct == UIA_TabItemControlTypeId {
        Role::Tab
    } else if ct == UIA_TableControlTypeId {
        Role::Table
    } else if ct == UIA_ToolBarControlTypeId {
        Role::Toolbar
    } else if ct == UIA_TreeControlTypeId {
        Role::Tree
    } else if ct == UIA_TreeItemControlTypeId {
        Role::TreeItem
    } else if ct == UIA_WindowControlTypeId {
        Role::Window
    } else {
        Role::Unknown(format!("uia_ct_{}", ct.0))
    }
}

/// Apply the pattern-based role promotions from `docs/uia-mapping.md` §1 on
/// top of the raw `ControlType → Role` mapping.
///
/// The promotions:
/// - `MenuItem` + `TogglePattern` → `MenuItemCheckbox`. UIA doesn't expose a
///   reliable "is radio" hint, so we don't distinguish radio menu items —
///   anything checkable becomes `MenuItemCheckbox`.
/// - `Window` + `WindowPattern.IsModal=true` → `Dialog`. Top-level frames
///   stay as `Window`; modal popups become `Dialog`.
/// - `ListItem` whose `parent` exposes `SelectionPattern` → `Option`. List
///   items inside a selection container act like `<option>`s in a select.
///
/// Both the snapshot path (`build_node`) and the action-time qualifies-as-
/// ref check use this so the `(role, name, nth)` lookup tuple stays
/// consistent across snapshot and resolution.
fn promoted_role(
    element: &IUIAutomationElement,
    ct: UIA_CONTROLTYPE_ID,
    class_name: &str,
    parent: Option<&IUIAutomationElement>,
) -> Role {
    let base = role_from_control_type(ct, class_name);
    match base {
        Role::MenuItem if has_toggle_pattern(element) => Role::MenuItemCheckbox,
        Role::Window if is_modal_window(element) => Role::Dialog,
        Role::ListItem if parent.is_some_and(parent_has_selection_pattern) => Role::Option,
        other => other,
    }
}

/// Returns `true` when the element exposes `TogglePattern`. Cheap to check —
/// one COM call. Used both for role promotion (MenuItem→MenuItemCheckbox)
/// and could feed into `state.checked` extraction, though the existing
/// `read_toggle_state` does its own pattern probe.
fn has_toggle_pattern(element: &IUIAutomationElement) -> bool {
    // SAFETY: `element` is a valid COM interface; UIA pattern IDs are well-known.
    unsafe { element.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) }
        .is_ok()
}

/// Returns `true` when the element is a modal window per `WindowPattern`.
/// Non-window elements and windows that don't support the pattern return
/// `false` — those stay as `Role::Window`.
fn is_modal_window(element: &IUIAutomationElement) -> bool {
    // SAFETY: `element` is a valid COM interface.
    let Ok(pattern) =
        (unsafe { element.GetCurrentPatternAs::<IUIAutomationWindowPattern>(UIA_WindowPatternId) })
    else {
        return false;
    };
    // SAFETY: `pattern` is a valid `IUIAutomationWindowPattern`.
    unsafe { pattern.CurrentIsModal() }.is_ok_and(BOOL::as_bool)
}

/// Returns `true` when the parent element exposes `SelectionPattern` —
/// i.e. a selection container like a list box or tab list.
fn parent_has_selection_pattern(parent: &IUIAutomationElement) -> bool {
    // SAFETY: `parent` is a valid COM interface.
    unsafe { parent.GetCurrentPatternAs::<IUIAutomationSelectionPattern>(UIA_SelectionPatternId) }
        .is_ok()
}

/// Promotion table for legacy Win32 class names. Per `docs/uia-mapping.md` §1.1.
fn promote_class_name(class_name: &str) -> Role {
    let lower = class_name.to_ascii_lowercase();
    if lower.starts_with("richedit") {
        return Role::TextField;
    }
    match lower.as_str() {
        "edit" => Role::TextField,
        "static" => Role::Generic,
        "button" => Role::Button,
        "combobox" => Role::ComboBox,
        "syslistview32" => Role::List,
        "systreeview32" => Role::Tree,
        _ => Role::Unknown(class_name.to_string()),
    }
}

/// Returns the DPI scale factor (1.0 = 96 DPI) for the monitor hosting `hwnd`.
/// Used to convert UIA's physical-pixel `BoundingRectangle` into the logical
/// pixels our schema documents. Per `docs/uia-mapping.md` §6.
fn window_dpi_scale(hwnd: HWND) -> f64 {
    // SAFETY: `GetDpiForWindow` accepts any HWND; it returns 0 if the API is
    // unavailable or the window is on an older OS, in which case we fall back
    // to 1.0 (no scaling).
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        f64::from(dpi) / 96.0
    }
}

/// Resolve a PID to `(executable path, executable file stem)`.
fn process_info(pid: u32) -> Result<(String, String)> {
    /// Capacity of the WCHAR buffer used for the executable path. 1024 chars
    /// is comfortably above MAX_PATH (260) and the extended path limit (~32k
    /// is theoretical; in practice almost no executable lives near it).
    const BUFFER_LEN: u32 = 1024;

    // SAFETY: `OpenProcess` is sound; we close the returned handle below.
    // `PROCESS_QUERY_LIMITED_INFORMATION` is the minimum right needed to call
    // `QueryFullProcessImageNameW` on most processes (including elevated ones
    // for an unelevated client).
    let handle: HANDLE = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map_err(|e| Error::Snapshot(format!("OpenProcess(pid={pid}): {e}")))?;

    let mut buffer = [0u16; BUFFER_LEN as usize];
    let mut size = BUFFER_LEN;

    // SAFETY: `handle` is a valid process handle obtained from OpenProcess.
    // `buffer` outlives the call; `&raw mut size` is a valid pointer to a u32
    // holding the buffer capacity in WCHARs.
    let query_result = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &raw mut size,
        )
    };

    // SAFETY: `handle` is valid; CloseHandle is required even on query failure.
    let _ = unsafe { CloseHandle(handle) };

    query_result.map_err(|e| Error::Snapshot(format!("QueryFullProcessImageNameW: {e}")))?;

    let path = String::from_utf16_lossy(&buffer[..size as usize]);
    let name = std::path::Path::new(&path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map_or_else(|| path.clone(), str::to_owned);

    Ok((path, name))
}

// ---------- Action pieces ----------

/// Top-level dispatch: route a typed `Action` to the right UIA call.
fn act_dispatch(state: &mut WorkerState, action: &Action) -> Result<ActionResult> {
    match action {
        Action::Click { ref_id } => act_click(state, ref_id),
        Action::Focus { ref_id } => act_focus(state, ref_id),
        Action::Fill { ref_id, value } => act_fill(state, ref_id, value),
        Action::Type { text } => act_type(state, text),
        Action::Press { keys } => act_press(state, keys),
        Action::KeyDown { key } => act_key_down(state, key),
        Action::KeyUp { key } => act_key_up(state, key),
        Action::Select { ref_id, value } => act_select(state, ref_id, value),
        Action::ScrollIntoView { ref_id } => act_scroll_into_view(state, ref_id),
        Action::SelectAll { ref_id } => act_select_all(state, ref_id.as_ref()),
        Action::DoubleClick { ref_id } => act_double_click(state, ref_id),
        Action::RightClick { ref_id } => act_right_click(state, ref_id),
        Action::Hover { ref_id } => act_hover(state, ref_id),
        Action::Scroll { ref_id, dx, dy } => act_scroll(state, ref_id.as_ref(), *dx, *dy),
        Action::Drag { from, to } => act_drag(state, from, to),
        Action::SwitchApp { app_id } => act_switch_app(state, app_id),
        Action::FocusWindow { window_id } => act_focus_window(state, window_id),
        other => Err(Error::Unsupported {
            surface: SurfaceKind::Uia.as_str().into(),
            action: action_name(other).into(),
        }),
    }
}

fn act_click(state: &WorkerState, ref_id: &RefId) -> Result<ActionResult> {
    let entry = lookup_ref(state, ref_id)?;
    let hwnd = state.last_hwnd.ok_or_else(|| Error::Action {
        action: "resolve".into(),
        reason: "no prior snapshot — call snapshot before act".into(),
    })?;
    let element = resolve_element(&state.automation, hwnd, &entry)?;

    // SAFETY: `element` is a valid COM interface; pattern interface IDs are
    // well-known UIA constants.
    let pattern: IUIAutomationInvokePattern =
        unsafe { element.GetCurrentPatternAs(UIA_InvokePatternId) }.map_err(|e| Error::Action {
            action: "click".into(),
            reason: format!("element does not support InvokePattern: {e}"),
        })?;

    // SAFETY: `pattern` is a valid IUIAutomationInvokePattern.
    unsafe { pattern.Invoke() }.map_err(|e| Error::Action {
        action: "click".into(),
        reason: format!("Invoke: {e}"),
    })?;

    Ok(ActionResult::ok())
}

fn act_focus(state: &WorkerState, ref_id: &RefId) -> Result<ActionResult> {
    let entry = lookup_ref(state, ref_id)?;
    let hwnd = state.last_hwnd.ok_or_else(|| Error::Action {
        action: "resolve".into(),
        reason: "no prior snapshot — call snapshot before act".into(),
    })?;
    let element = resolve_element(&state.automation, hwnd, &entry)?;

    // SAFETY: `element` is a valid COM interface.
    unsafe { element.SetFocus() }.map_err(|e| Error::Action {
        action: "focus".into(),
        reason: format!("SetFocus: {e}"),
    })?;

    Ok(ActionResult::ok())
}

fn act_fill(state: &WorkerState, ref_id: &RefId, value: &str) -> Result<ActionResult> {
    let entry = lookup_ref(state, ref_id)?;
    let hwnd = state.last_hwnd.ok_or_else(|| Error::Action {
        action: "resolve".into(),
        reason: "no prior snapshot — call snapshot before act".into(),
    })?;
    let element = resolve_element(&state.automation, hwnd, &entry)?;

    // SAFETY: `element` is a valid COM interface.
    let pattern: IUIAutomationValuePattern =
        unsafe { element.GetCurrentPatternAs(UIA_ValuePatternId) }.map_err(|e| Error::Action {
            action: "fill".into(),
            reason: format!("element does not support ValuePattern: {e}"),
        })?;

    let bstr = BSTR::from(value);
    // SAFETY: `pattern` is valid; `&bstr` outlives the call.
    unsafe { pattern.SetValue(&bstr) }.map_err(|e| Error::Action {
        action: "fill".into(),
        reason: format!("SetValue: {e}"),
    })?;

    Ok(ActionResult::ok())
}

fn lookup_ref(state: &WorkerState, ref_id: &RefId) -> Result<RefEntry> {
    state
        .last_refs
        .get(ref_id)
        .cloned()
        .ok_or_else(|| Error::RefNotFound(ref_id.0.clone()))
}

// ---------- Pattern-based actions ----------

/// Choose an option in a select / combo box / list box.
///
/// Resolution rules, in order:
///
/// 1. If the resolved element itself supports `SelectionItemPattern` and its
///    `Name` matches `value` (or `value` is empty), call `Select` on it.
/// 2. Otherwise treat the resolved element as a container and walk its
///    Control-view subtree for the first descendant whose `Name == value`
///    AND that supports `SelectionItemPattern`.
///
/// This covers both the "agent already has a ref to the option" case
/// (Tab, ListItem, etc.) and the "agent has the container, names the
/// option" case (ComboBox + dropdown options).
fn act_select(state: &WorkerState, ref_id: &RefId, value: &str) -> Result<ActionResult> {
    let entry = lookup_ref(state, ref_id)?;
    let hwnd = require_hwnd(state, "select")?;
    let element = resolve_element(&state.automation, hwnd, &entry)?;

    if let Some(pattern) = selection_item_pattern_if_named(&element, value) {
        // SAFETY: `pattern` is a valid IUIAutomationSelectionItemPattern.
        unsafe { pattern.Select() }.map_err(|e| Error::Action {
            action: "select".into(),
            reason: format!("SelectionItemPattern.Select: {e}"),
        })?;
        return Ok(ActionResult::ok());
    }

    // Treat as container — walk for a named SelectionItem descendant.
    // SAFETY: `automation` is valid; ControlViewWalker returns Err if the
    // automation singleton can't allocate a walker (essentially OOM).
    let walker = unsafe { state.automation.ControlViewWalker() }.map_err(|e| Error::Action {
        action: "select".into(),
        reason: format!("ControlViewWalker: {e}"),
    })?;
    let option =
        find_named_selection_item(&walker, &element, value).ok_or_else(|| Error::Action {
            action: "select".into(),
            reason: format!(
                "no descendant named {value:?} supporting SelectionItemPattern under ref {}",
                ref_id.0
            ),
        })?;
    let pattern: IUIAutomationSelectionItemPattern =
        // SAFETY: `option` is valid; pattern probe returns Err if unsupported.
        unsafe { option.GetCurrentPatternAs(UIA_SelectionItemPatternId) }.map_err(|e| {
            Error::Action {
                action: "select".into(),
                reason: format!("SelectionItemPattern unavailable on matched option: {e}"),
            }
        })?;
    // SAFETY: `pattern` is valid.
    unsafe { pattern.Select() }.map_err(|e| Error::Action {
        action: "select".into(),
        reason: format!("SelectionItemPattern.Select: {e}"),
    })?;
    Ok(ActionResult::ok())
}

/// Scroll the element into view via `ScrollItemPattern.ScrollIntoView()`.
/// Doesn't move focus or change selection — just makes the element visible
/// in its scroll container.
fn act_scroll_into_view(state: &WorkerState, ref_id: &RefId) -> Result<ActionResult> {
    let entry = lookup_ref(state, ref_id)?;
    let hwnd = require_hwnd(state, "scroll_into_view")?;
    let element = resolve_element(&state.automation, hwnd, &entry)?;

    // SAFETY: `element` is valid; pattern probe returns Err if unsupported.
    let pattern: IUIAutomationScrollItemPattern = unsafe {
        element.GetCurrentPatternAs(UIA_ScrollItemPatternId)
    }
    .map_err(|e| Error::Action {
        action: "scroll_into_view".into(),
        reason: format!("element does not support ScrollItemPattern: {e}"),
    })?;
    // SAFETY: `pattern` is valid.
    unsafe { pattern.ScrollIntoView() }.map_err(|e| Error::Action {
        action: "scroll_into_view".into(),
        reason: format!("ScrollIntoView: {e}"),
    })?;
    Ok(ActionResult::ok())
}

/// Select all content in the focused field, or in the referenced field.
/// When `ref_id` is `Some`, focus the element first via UIA `SetFocus`.
/// Then send `Ctrl+A` via the existing `act_press` plumbing.
fn act_select_all(state: &WorkerState, ref_id: Option<&RefId>) -> Result<ActionResult> {
    if let Some(rid) = ref_id {
        // Reuse Focus's resolution + SetFocus path.
        act_focus(state, rid)?;
    }
    act_press(state, "Ctrl+A")
}

/// Look up the snapshot's pinned HWND, mapping the missing-snapshot case to a
/// uniform action error. Most action helpers need this; pulling it out
/// removes the boilerplate duplication.
fn require_hwnd(state: &WorkerState, action: &str) -> Result<HWND> {
    state.last_hwnd.ok_or_else(|| Error::Action {
        action: action.into(),
        reason: "no prior snapshot — call snapshot before act".into(),
    })
}

/// Return the element's `SelectionItemPattern` when it both supports the
/// pattern AND its Name matches `expected_name` (or `expected_name` is
/// empty, meaning "any selectable option"). Used by `act_select` to decide
/// whether the resolved ref is itself the option to select.
fn selection_item_pattern_if_named(
    element: &IUIAutomationElement,
    expected_name: &str,
) -> Option<IUIAutomationSelectionItemPattern> {
    // SAFETY: `element` is valid.
    let pattern: IUIAutomationSelectionItemPattern =
        unsafe { element.GetCurrentPatternAs(UIA_SelectionItemPatternId) }.ok()?;
    if expected_name.is_empty() {
        return Some(pattern);
    }
    // SAFETY: `element` is valid; failed Name read falls back to empty.
    let name = unsafe { element.CurrentName() }
        .ok()
        .map(|b| b.to_string())
        .unwrap_or_default();
    if name == expected_name {
        Some(pattern)
    } else {
        None
    }
}

/// Pre-order DFS for the first descendant whose `Name == value` AND that
/// supports `SelectionItemPattern`. Mirrors how snapshot walks the tree, so
/// the search order is predictable from the snapshot output.
fn find_named_selection_item(
    walker: &IUIAutomationTreeWalker,
    parent: &IUIAutomationElement,
    value: &str,
) -> Option<IUIAutomationElement> {
    // SAFETY: `parent` is valid; walker.Get*ChildElement returns Err for
    // "no more children" which we treat as the end of iteration.
    let mut maybe_child = unsafe { walker.GetFirstChildElement(parent) }.ok();
    while let Some(child) = maybe_child {
        // SAFETY: `child` is valid.
        let name = unsafe { child.CurrentName() }
            .ok()
            .map(|b| b.to_string())
            .unwrap_or_default();
        if name == value
            && unsafe {
                child
                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                    .is_ok()
            }
        {
            return Some(child);
        }
        if let Some(found) = find_named_selection_item(walker, &child, value) {
            return Some(found);
        }
        // SAFETY: `child` is valid.
        maybe_child = unsafe { walker.GetNextSiblingElement(&child) }.ok();
    }
    None
}

// ---------- Keyboard input via SendInput ----------
//
// `SendInput` injects events into the system input queue; they are routed to
// whichever window currently has keyboard focus. Each helper first calls
// `ensure_foreground` to bring the snapshot's pinned HWND forward, so that
// keystrokes land in the same window the agent took its snapshot of —
// matching the design promise that "actions reuse the pinned HWND".

/// Type a literal string at the current focus. Each UTF-16 code unit becomes
/// a `KEYEVENTF_UNICODE` keystroke pair (down + up) — this bypasses keyboard
/// layout translation entirely, so emoji and non-Latin scripts work without
/// any per-locale handling. Surrogate pairs are sent as two events as the OS
/// expects.
///
/// Caveat: some WinUI 3 / XAML controls (Win11 Notepad's Document is the one
/// we hit in tests) don't honor `VK_PACKET` / `WM_CHAR` for non-ASCII
/// codepoints and substitute a fallback char. ASCII works everywhere; for
/// guaranteed Unicode delivery against an editable field, prefer
/// `Fill { ref_id, value }`, which goes through `ValuePattern.SetValue` and
/// updates the model directly.
fn act_type(state: &WorkerState, text: &str) -> Result<ActionResult> {
    if text.is_empty() {
        return Ok(ActionResult::ok());
    }
    ensure_foreground(state, "type")?;
    let mut inputs: Vec<INPUT> = Vec::with_capacity(text.encode_utf16().count() * 2);
    for unit in text.encode_utf16() {
        inputs.push(make_unicode_input(unit, false));
        inputs.push(make_unicode_input(unit, true));
    }
    send_inputs(&inputs, "type")
}

/// Press a chord like `"Enter"`, `"Ctrl+A"`, or `"Ctrl+Shift+T"`. Modifiers
/// are pressed in declaration order, the main key is tapped, then modifiers
/// are released in reverse order — matching how a human keyboard handles it.
fn act_press(state: &WorkerState, keys: &str) -> Result<ActionResult> {
    let parsed = parse_chord(keys).map_err(|reason| Error::Action {
        action: "press".into(),
        reason,
    })?;
    ensure_foreground(state, "press")?;

    let mut inputs: Vec<INPUT> = Vec::with_capacity(parsed.modifiers.len() * 2 + 2);
    for &m in &parsed.modifiers {
        inputs.push(make_vk_input(m, false));
    }
    inputs.push(make_vk_input(parsed.key, false));
    inputs.push(make_vk_input(parsed.key, true));
    for &m in parsed.modifiers.iter().rev() {
        inputs.push(make_vk_input(m, true));
    }
    send_inputs(&inputs, "press")
}

fn act_key_down(state: &WorkerState, key: &str) -> Result<ActionResult> {
    let vk = vk_from_name(key).ok_or_else(|| Error::Action {
        action: "key_down".into(),
        reason: format!("unknown key name: {key:?}"),
    })?;
    ensure_foreground(state, "key_down")?;
    send_inputs(&[make_vk_input(vk, false)], "key_down")
}

fn act_key_up(state: &WorkerState, key: &str) -> Result<ActionResult> {
    let vk = vk_from_name(key).ok_or_else(|| Error::Action {
        action: "key_up".into(),
        reason: format!("unknown key name: {key:?}"),
    })?;
    ensure_foreground(state, "key_up")?;
    send_inputs(&[make_vk_input(vk, true)], "key_up")
}

// ---------- Mouse input via SendInput ----------
//
// Pointer-driven actions that UIA doesn't expose a pattern for. Each helper
// resolves the ref to a live element, computes the element's screen-space
// center from `CurrentBoundingRectangle` (which is in physical pixels), then
// builds a sequence of `MOUSEINPUT` events. Cursor positioning uses the
// virtual-desktop absolute mode so multi-monitor setups Just Work.
//
// All helpers go through `ensure_foreground` first, like the keyboard
// helpers, so the events land on the snapshot's pinned HWND rather than
// whichever window happens to be in front of the test runner.

/// Double primary-button click at the element's center.
fn act_double_click(state: &WorkerState, ref_id: &RefId) -> Result<ActionResult> {
    let entry = lookup_ref(state, ref_id)?;
    let hwnd = require_hwnd(state, "double_click")?;
    let element = resolve_element(&state.automation, hwnd, &entry)?;
    let (cx, cy) = element_center_physical(&element)?;
    let (ax, ay) = screen_to_absolute(cx, cy);
    ensure_foreground(state, "double_click")?;
    send_inputs(
        &[
            make_mouse_move_absolute(ax, ay),
            make_mouse_button(MouseButton::Left, true),
            make_mouse_button(MouseButton::Left, false),
            make_mouse_button(MouseButton::Left, true),
            make_mouse_button(MouseButton::Left, false),
        ],
        "double_click",
    )
}

/// Secondary-button (right) click at the element's center.
fn act_right_click(state: &WorkerState, ref_id: &RefId) -> Result<ActionResult> {
    let entry = lookup_ref(state, ref_id)?;
    let hwnd = require_hwnd(state, "right_click")?;
    let element = resolve_element(&state.automation, hwnd, &entry)?;
    let (cx, cy) = element_center_physical(&element)?;
    let (ax, ay) = screen_to_absolute(cx, cy);
    ensure_foreground(state, "right_click")?;
    send_inputs(
        &[
            make_mouse_move_absolute(ax, ay),
            make_mouse_button(MouseButton::Right, true),
            make_mouse_button(MouseButton::Right, false),
        ],
        "right_click",
    )
}

/// Move the cursor to the element's center; no button events. Useful for
/// triggering hover-only UI (tooltips, dropdown anchors).
fn act_hover(state: &WorkerState, ref_id: &RefId) -> Result<ActionResult> {
    let entry = lookup_ref(state, ref_id)?;
    let hwnd = require_hwnd(state, "hover")?;
    let element = resolve_element(&state.automation, hwnd, &entry)?;
    let (cx, cy) = element_center_physical(&element)?;
    let (ax, ay) = screen_to_absolute(cx, cy);
    ensure_foreground(state, "hover")?;
    send_inputs(&[make_mouse_move_absolute(ax, ay)], "hover")
}

/// Scroll by `(dx, dy)` logical pixels. When `ref_id` is `Some`, the cursor
/// is positioned over the element's center first so the wheel events go to
/// the right scroll container (most apps route mouse wheel to whatever's
/// under the cursor). When `None`, the wheel events fire at the current
/// cursor position.
///
/// Sign convention follows CSS / DOM: positive `dy` = scroll content
/// downward (which is wheel-toward-the-user, a *negative* `WHEEL_DELTA`).
/// Pixel-to-wheel conversion is approximate — we round `dy` to the nearest
/// integer wheel delta. One traditional notch is 120 units; modern smooth-
/// scrolling apps accept any integer.
fn act_scroll(
    state: &WorkerState,
    ref_id: Option<&RefId>,
    dx: f64,
    dy: f64,
) -> Result<ActionResult> {
    let hwnd = require_hwnd(state, "scroll")?;
    let mut inputs: Vec<INPUT> = Vec::new();

    if let Some(rid) = ref_id {
        let entry = lookup_ref(state, rid)?;
        let element = resolve_element(&state.automation, hwnd, &entry)?;
        let (cx, cy) = element_center_physical(&element)?;
        let (ax, ay) = screen_to_absolute(cx, cy);
        inputs.push(make_mouse_move_absolute(ax, ay));
    }

    let vertical = scroll_delta_from_pixels(dy);
    if vertical != 0 {
        // Negate: positive dy (scroll content down) → wheel toward user → negative delta.
        inputs.push(make_mouse_wheel(WheelAxis::Vertical, -vertical));
    }
    let horizontal = scroll_delta_from_pixels(dx);
    if horizontal != 0 {
        // HWHEEL: positive delta = scroll right (content moves left).
        inputs.push(make_mouse_wheel(WheelAxis::Horizontal, horizontal));
    }

    if inputs.is_empty() {
        return Ok(ActionResult::ok());
    }

    ensure_foreground(state, "scroll")?;
    send_inputs(&inputs, "scroll")
}

/// Drag from one element to another. Sends a left-button press at the
/// source's center, an absolute move to the destination's center, and a
/// release. Some drag-and-drop UIs require intermediate movement to
/// recognize the gesture; if a real app turns out to need that, we'll
/// interpolate here. For v0.2 the two-point drag is enough for the common
/// pattern (drag a tab, drag a list item).
fn act_drag(state: &WorkerState, from: &RefId, to: &RefId) -> Result<ActionResult> {
    let hwnd = require_hwnd(state, "drag")?;
    let from_entry = lookup_ref(state, from)?;
    let to_entry = lookup_ref(state, to)?;

    let from_element = resolve_element(&state.automation, hwnd, &from_entry)?;
    let to_element = resolve_element(&state.automation, hwnd, &to_entry)?;

    let (fx, fy) = element_center_physical(&from_element)?;
    let (tx, ty) = element_center_physical(&to_element)?;
    let (fax, fay) = screen_to_absolute(fx, fy);
    let (tax, tay) = screen_to_absolute(tx, ty);

    ensure_foreground(state, "drag")?;
    send_inputs(
        &[
            make_mouse_move_absolute(fax, fay),
            make_mouse_button(MouseButton::Left, true),
            make_mouse_move_absolute(tax, tay),
            make_mouse_button(MouseButton::Left, false),
        ],
        "drag",
    )
}

// ---------- Window / process targeting ----------

/// Bring the app identified by `app_id` to the foreground, and re-pin
/// future actions on its window.
///
/// `app_id` is matched against the executable file stem (case-insensitive).
/// If `app_id` looks like a path (e.g. the `app.id` we emit on Windows is
/// the full exe path), the file stem is extracted first. This keeps the
/// schema consistent: the agent can pass the `app.id` it received from a
/// snapshot, or just the short name like `"Notepad"`.
///
/// After the switch, `state.last_hwnd` points to the new window and
/// `state.last_refs` is cleared — refs from a previous snapshot can no
/// longer resolve, so the agent must take a fresh snapshot before its next
/// ref-bearing action.
fn act_switch_app(state: &mut WorkerState, app_id: &str) -> Result<ActionResult> {
    let process_name = process_name_from_app_id(app_id);
    let hwnd = find_window_by_process_name(process_name).ok_or_else(|| Error::Action {
        action: "switch_app".into(),
        reason: format!("no visible top-level window owned by app {app_id:?}"),
    })?;
    bring_window_to_foreground(hwnd);
    state.last_hwnd = Some(hwnd);
    state.last_refs = RefMap::new();
    Ok(ActionResult::ok())
}

/// Bring a specific top-level window forward by its `window_id` (the same
/// hex-formatted HWND `WindowContext` carries). When the window supports
/// `WindowPattern`, restore from minimized first via
/// `SetWindowVisualState(Normal)`. Then run the AttachThreadInput-backed
/// foreground bringer.
///
/// Like `SwitchApp`, this re-pins `state.last_hwnd` and clears `last_refs`.
fn act_focus_window(state: &mut WorkerState, window_id: &str) -> Result<ActionResult> {
    let hwnd = parse_window_id(window_id).ok_or_else(|| Error::Action {
        action: "focus_window".into(),
        reason: format!("invalid window_id {window_id:?}; expected hex like 0x10edc"),
    })?;

    // Best-effort restore. Windows whose pattern doesn't support the call,
    // or which aren't minimized, just skip this — Set/Bring foreground does
    // the rest.
    // SAFETY: `automation` is a valid COM interface; `hwnd` may be invalid,
    // in which case `ElementFromHandle` returns Err and we skip the restore.
    if let Ok(elem) = unsafe { state.automation.ElementFromHandle(hwnd) } {
        if let Ok(pattern) =
            unsafe { elem.GetCurrentPatternAs::<IUIAutomationWindowPattern>(UIA_WindowPatternId) }
        {
            // SAFETY: `pattern` is valid.
            let _ = unsafe { pattern.SetWindowVisualState(WindowVisualState_Normal) };
        }
    }

    bring_window_to_foreground(hwnd);
    state.last_hwnd = Some(hwnd);
    state.last_refs = RefMap::new();
    Ok(ActionResult::ok())
}

/// Reduce an `app_id` to the executable file stem `find_window_by_process_name`
/// expects. Accepts both the full path we emit on Windows (e.g.
/// `"C:\Path\To\Notepad.exe"`) and a bare name (e.g. `"Notepad"`).
fn process_name_from_app_id(app_id: &str) -> &str {
    std::path::Path::new(app_id)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(app_id)
}

/// Parse a `window_id` string back to an HWND. The schema emits these as
/// `format!("{:#x}", hwnd.0 as usize)`, e.g. `"0x10edc"`. We accept both
/// `0x`-prefixed and bare hex.
fn parse_window_id(s: &str) -> Option<HWND> {
    let trimmed = s.trim();
    let hex = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    let n = usize::from_str_radix(hex, 16).ok()?;
    if n == 0 {
        return None;
    }
    // SAFETY-equivalent: the cast preserves bits; HWND is an opaque pointer
    // wrapper, and we don't dereference here.
    #[allow(clippy::cast_possible_wrap)]
    Some(HWND(n as *mut std::ffi::c_void))
}

#[derive(Clone, Copy)]
enum WheelAxis {
    Vertical,
    Horizontal,
}

fn make_mouse_wheel(axis: WheelAxis, delta: i32) -> INPUT {
    let flag = match axis {
        WheelAxis::Vertical => MOUSEEVENTF_WHEEL,
        WheelAxis::Horizontal => MOUSEEVENTF_HWHEEL,
    };
    // `mouseData` is documented as "signed" but typed `u32` in the Win32
    // headers; cast preserves the bit pattern so the receiver reads the
    // correct signed delta.
    #[allow(clippy::cast_sign_loss)]
    let mouse_data = delta as u32;
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data,
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Map a pixel-space scroll amount to an integer `WHEEL_DELTA`. We treat
/// 1 pixel ≈ 1 wheel-delta unit, which gives ~120 pixels per traditional
/// notch — a reasonable agent-facing default. Sub-pixel inputs round to
/// the nearest integer; values whose absolute magnitude is below 0.5 round
/// to zero and short-circuit the wheel event.
fn scroll_delta_from_pixels(px: f64) -> i32 {
    let rounded = px.round();
    if rounded.abs() < 0.5 {
        return 0;
    }
    // Clamp to i32 range. Inputs above ~2.1B pixels are pathological; we
    // saturate rather than UB on the cast.
    #[allow(clippy::cast_possible_truncation)]
    let v = rounded.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    v
}

#[derive(Clone, Copy)]
enum MouseButton {
    Left,
    Right,
}

fn make_mouse_button(button: MouseButton, down: bool) -> INPUT {
    let flag = match (button, down) {
        (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
    };
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn make_mouse_move_absolute(x: i32, y: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: x,
                dy: y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Read the element's UIA `BoundingRectangle` and return the center point
/// in physical screen pixels. Used to position the cursor before button
/// events. Surfaces a clean action error when the element doesn't have a
/// rectangle (rare — typically only when the element has been destroyed).
fn element_center_physical(element: &IUIAutomationElement) -> Result<(i32, i32)> {
    // SAFETY: `element` is a valid COM interface.
    let r = unsafe { element.CurrentBoundingRectangle() }.map_err(|e| Error::Action {
        action: "mouse".into(),
        reason: format!("CurrentBoundingRectangle: {e}"),
    })?;
    Ok(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
}

/// Convert a physical-pixel screen-space point to UIA's absolute-cursor
/// coordinate space (`0..=65535` over the virtual desktop). Multi-monitor
/// setups with negative virtual coords (secondary monitor to the left of
/// primary) work because we anchor at `SM_X/YVIRTUALSCREEN`. Used together
/// with `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` on the move event.
fn screen_to_absolute(x_phys: i32, y_phys: i32) -> (i32, i32) {
    // SAFETY: `GetSystemMetrics` is always sound. The virtual-screen metrics
    // never return zero on a working desktop session, but we clamp to 1 to
    // make the divisions safe in the impossible case.
    let xv = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let yv = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let cx = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) }.max(1);
    let cy = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) }.max(1);
    let nx = (i64::from(x_phys - xv) * 65535 + i64::from(cx) / 2) / i64::from(cx);
    let ny = (i64::from(y_phys - yv) * 65535 + i64::from(cy) / 2) / i64::from(cy);
    let nx = nx.clamp(0, 65535);
    let ny = ny.clamp(0, 65535);
    // Casts are safe after clamp; the clamp ensures the values fit in i32.
    #[allow(clippy::cast_possible_truncation)]
    (nx as i32, ny as i32)
}

/// Bring the snapshot's pinned HWND to the foreground so subsequent
/// `SendInput` events reach it.
fn ensure_foreground(state: &WorkerState, action: &str) -> Result<()> {
    let hwnd = state.last_hwnd.ok_or_else(|| Error::Action {
        action: action.into(),
        reason: "no prior snapshot — call snapshot before keyboard input".into(),
    })?;
    bring_window_to_foreground(hwnd);
    Ok(())
}

/// Force `target` to the foreground using the `AttachThreadInput` workaround
/// for Windows' `ForegroundLockTimeout` policy — without it,
/// `SetForegroundWindow` from a non-foreground process is silently rejected,
/// which is exactly the case we hit when the test runner / IDE is in front
/// and we want to drive another app.
///
/// Best-effort: failures (target window destroyed, attach denied) are
/// swallowed; the caller's next operation will surface the symptom (an
/// action that lands in the wrong window, a SendInput sent count mismatch).
/// Returning a hard error here would just turn intermittent OS quirks into
/// loud user-facing failures without giving the caller anything to do.
fn bring_window_to_foreground(target: HWND) {
    if target.0.is_null() {
        return;
    }
    // SAFETY: `GetForegroundWindow` is always sound; null is allowed.
    let current = unsafe { GetForegroundWindow() };
    if current.0 == target.0 {
        return;
    }

    // SAFETY: `GetCurrentThreadId` is always sound. `GetWindowThreadProcessId`
    // accepts any HWND (null returns 0). We never dereference the returned
    // thread ids directly.
    let me_thread = unsafe { GetCurrentThreadId() };
    let fg_thread = if current.0.is_null() {
        0
    } else {
        unsafe { GetWindowThreadProcessId(current, None) }
    };

    // Attach our input queue to the foreground thread's queue (if there is
    // one and it isn't already us) so SetForegroundWindow bypasses the
    // foreground-lock policy. Detaching is mandatory regardless of outcome.
    let attached = fg_thread != 0 && fg_thread != me_thread && {
        // SAFETY: thread ids come from valid Win32 calls above.
        unsafe { AttachThreadInput(fg_thread, me_thread, true) }.as_bool()
    };

    // SAFETY: target is non-null per the early return; SetForegroundWindow
    // returns FALSE if blocked but is otherwise sound.
    let _ = unsafe { SetForegroundWindow(target) };

    if attached {
        // SAFETY: paired detach for the AttachThreadInput above.
        let _ = unsafe { AttachThreadInput(fg_thread, me_thread, false) };
    }

    // Give the foreground change a moment to propagate before any
    // immediately-following input. 50ms is enough on every Windows version
    // we target without being a noticeable wait for an agent.
    std::thread::sleep(Duration::from_millis(50));
}

/// Build a virtual-key `INPUT` event. `key_up = false` is a press; `true` is
/// a release. Extended-key handling is delegated to the OS — when `wVk` is
/// set and `wScan = 0` Windows sets the EXTENDED flag itself for navigation
/// keys, so we don't need to special-case arrow / Home / End / etc.
fn make_vk_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    let flags = if key_up {
        KEYEVENTF_KEYUP
    } else {
        KEYBD_EVENT_FLAGS(0)
    };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Build a UTF-16 `INPUT` event. `wVk = 0` and `KEYEVENTF_UNICODE` tells
/// Windows to deliver `wScan` as a character to the focused control rather
/// than as a scancode.
fn make_unicode_input(code_unit: u16, key_up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: code_unit,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Submit a batch of `INPUT` events. Returns an error if the OS reports it
/// inserted fewer events than we sent (typically because UIPI / UAC blocked
/// us, or the input desktop changed mid-call).
fn send_inputs(inputs: &[INPUT], action: &str) -> Result<ActionResult> {
    if inputs.is_empty() {
        return Ok(ActionResult::ok());
    }
    let expected = u32::try_from(inputs.len()).map_err(|_| Error::Action {
        action: action.into(),
        reason: format!("input batch too large ({} events)", inputs.len()),
    })?;
    let cb_size = i32::try_from(std::mem::size_of::<INPUT>()).map_err(|_| Error::Action {
        action: action.into(),
        reason: "INPUT struct size does not fit in i32".into(),
    })?;
    // SAFETY: `inputs` is a valid slice of `INPUT` for the duration of the
    // call; `cb_size` is `sizeof(INPUT)` as required by the Win32 contract.
    let sent = unsafe { SendInput(inputs, cb_size) };
    if sent != expected {
        return Err(Error::Action {
            action: action.into(),
            reason: format!(
                "SendInput inserted {sent} of {expected} events (likely blocked by UIPI/UAC)"
            ),
        });
    }
    Ok(ActionResult::ok())
}

/// Parsed key chord: zero or more modifiers plus exactly one main key.
struct Chord {
    modifiers: Vec<VIRTUAL_KEY>,
    key: VIRTUAL_KEY,
}

/// Split `"Ctrl+Shift+T"` into modifiers + main key. The last `+`-separated
/// token is the main key; everything before it is treated as a modifier.
/// Each token is run through [`vk_from_name`].
fn parse_chord(keys: &str) -> std::result::Result<Chord, String> {
    let tokens: Vec<&str> = keys.split('+').map(str::trim).collect();
    if tokens.is_empty() || tokens.iter().any(|t| t.is_empty()) {
        return Err(format!("malformed key chord: {keys:?}"));
    }
    // `split` always yields at least one element, so `split_last` is safe.
    let (main, mods) = tokens
        .split_last()
        .ok_or_else(|| format!("empty key chord: {keys:?}"))?;
    let key = vk_from_name(main).ok_or_else(|| format!("unknown key name: {main:?}"))?;
    let modifiers: Vec<VIRTUAL_KEY> = mods
        .iter()
        .map(|t| vk_from_name(t).ok_or_else(|| format!("unknown modifier: {t:?}")))
        .collect::<std::result::Result<_, _>>()?;
    Ok(Chord { modifiers, key })
}

/// Map a key name to its Windows virtual-key code. Case-insensitive.
///
/// Single ASCII letters and digits map to their VK directly (`VK_A == 0x41`,
/// `VK_0 == 0x30`). Function keys `F1..F24` map to `VK_F1 + n - 1`. Everything
/// else is a name in the table below.
fn vk_from_name(name: &str) -> Option<VIRTUAL_KEY> {
    let lower = name.trim().to_ascii_lowercase();

    // Single ASCII letter / digit → its VK is the ASCII code itself.
    if lower.len() == 1 {
        let c = lower.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(VIRTUAL_KEY(u16::from(c.to_ascii_uppercase() as u8)));
        }
        if c.is_ascii_digit() {
            return Some(VIRTUAL_KEY(u16::from(c as u8)));
        }
    }

    // `F1` .. `F24` → `VK_F1` (0x70) + n - 1.
    if let Some(rest) = lower.strip_prefix('f') {
        if let Ok(n) = rest.parse::<u16>() {
            if (1..=24).contains(&n) {
                return Some(VIRTUAL_KEY(0x70 + n - 1));
            }
        }
    }

    let vk = match lower.as_str() {
        "ctrl" | "control" => VK_CONTROL,
        "shift" => VK_SHIFT,
        "alt" => VK_MENU,
        "win" | "meta" | "cmd" | "super" | "lwin" => VK_LWIN,
        "rwin" => VK_RWIN,
        "enter" | "return" => VK_RETURN,
        "tab" => VK_TAB,
        "space" | "spacebar" => VK_SPACE,
        "esc" | "escape" => VK_ESCAPE,
        "backspace" => VK_BACK,
        "delete" | "del" => VK_DELETE,
        "insert" | "ins" => VK_INSERT,
        "home" => VK_HOME,
        "end" => VK_END,
        "pageup" | "pgup" | "page_up" => VK_PRIOR,
        "pagedown" | "pgdn" | "page_down" => VK_NEXT,
        "left" | "arrowleft" => VK_LEFT,
        "right" | "arrowright" => VK_RIGHT,
        "up" | "arrowup" => VK_UP,
        "down" | "arrowdown" => VK_DOWN,
        "capslock" => VK_CAPITAL,
        "numlock" => VK_NUMLOCK,
        "scrolllock" => VK_SCROLL,
        "printscreen" | "prtsc" | "prtscn" => VK_SNAPSHOT,
        "pause" | "break" => VK_PAUSE,
        "apps" | "menu" | "contextmenu" => VK_APPS,
        _ => return None,
    };
    Some(vk)
}

/// Translate a [`WindowTarget`] into the HWND the snapshot/action will operate on.
fn resolve_target_hwnd(target: &WindowTarget) -> Result<HWND> {
    match target {
        WindowTarget::Foreground => {
            // SAFETY: `GetForegroundWindow` is always sound; null check below.
            let h = unsafe { GetForegroundWindow() };
            if h.0.is_null() {
                Err(Error::Snapshot("no foreground window".into()))
            } else {
                Ok(h)
            }
        }
        WindowTarget::Pid { pid } => find_window_by_pid(*pid)
            .ok_or_else(|| Error::Snapshot(format!("no visible top-level window for PID {pid}"))),
        WindowTarget::Title { title } => find_window_by_title(title).ok_or_else(|| {
            Error::Snapshot(format!(
                "no visible top-level window with title containing {title:?}"
            ))
        }),
        WindowTarget::ProcessName { name } => find_window_by_process_name(name).ok_or_else(|| {
            Error::Snapshot(format!(
                "no visible top-level window owned by a process named {name:?}"
            ))
        }),
    }
}

/// State threaded through `EnumWindows` callbacks for finding a target window.
struct EnumState {
    /// Predicate evaluated against each enumerated visible top-level window.
    predicate: Box<dyn FnMut(HWND) -> bool>,
    /// First HWND for which `predicate` returned `true`.
    found: Option<HWND>,
}

extern "system" fn enum_windows_proc(
    hwnd: HWND,
    lparam: LPARAM,
) -> windows::Win32::Foundation::BOOL {
    // SAFETY: `lparam` was set in the caller to a `*mut EnumState` with a
    // valid lifetime. `EnumWindows` is single-threaded with respect to this
    // callback, and the caller blocks until enumeration completes.
    let state = unsafe { &mut *(lparam.0 as *mut EnumState) };
    // SAFETY: `IsWindowVisible` accepts any HWND.
    if unsafe { IsWindowVisible(hwnd) }.as_bool() && (state.predicate)(hwnd) {
        state.found = Some(hwnd);
        windows::Win32::Foundation::BOOL(0) // stop enumeration
    } else {
        windows::Win32::Foundation::BOOL(1) // continue
    }
}

fn find_window_by_pid(target_pid: u32) -> Option<HWND> {
    let mut state = EnumState {
        predicate: Box::new(move |hwnd| {
            let mut pid: u32 = 0;
            // SAFETY: `hwnd` is valid (provided by EnumWindows); writing to a
            // local u32 via &raw mut is sound.
            let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
            pid == target_pid
        }),
        found: None,
    };
    enumerate_top_level(&mut state);
    state.found
}

fn find_window_by_title(needle: &str) -> Option<HWND> {
    let needle = needle.to_ascii_lowercase();
    let mut state = EnumState {
        predicate: Box::new(move |hwnd| {
            // SAFETY: `GetWindowTextLengthW` accepts any HWND.
            let len = unsafe { GetWindowTextLengthW(hwnd) };
            let Ok(len) = usize::try_from(len) else {
                return false;
            };
            if len == 0 {
                return false;
            }
            // +1 for the null terminator GetWindowTextW writes.
            let mut buf = vec![0u16; len + 1];
            // SAFETY: buffer is sized for `len + 1` u16s.
            let written = unsafe { GetWindowTextW(hwnd, &mut buf) };
            let Ok(written) = usize::try_from(written) else {
                return false;
            };
            if written == 0 {
                return false;
            }
            let title = String::from_utf16_lossy(&buf[..written]).to_ascii_lowercase();
            title.contains(&needle)
        }),
        found: None,
    };
    enumerate_top_level(&mut state);
    state.found
}

/// Find the first visible top-level window whose owning process executable's
/// file stem matches `target_name` (case-insensitive). Locale-independent,
/// unlike [`find_window_by_title`].
fn find_window_by_process_name(target_name: &str) -> Option<HWND> {
    let target = target_name.to_ascii_lowercase();
    let mut state = EnumState {
        predicate: Box::new(move |hwnd| {
            let mut pid: u32 = 0;
            // SAFETY: `hwnd` is valid; writing to a local u32 via raw pointer is sound.
            let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
            if pid == 0 {
                return false;
            }
            let Ok((_path, name)) = process_info(pid) else {
                return false;
            };
            name.to_ascii_lowercase() == target
        }),
        found: None,
    };
    enumerate_top_level(&mut state);
    state.found
}

fn enumerate_top_level(state: &mut EnumState) {
    let lparam = LPARAM(std::ptr::from_mut::<EnumState>(state) as isize);
    // SAFETY: callback is `extern "system"`; `lparam` points to a valid `EnumState`
    // for the duration of the call (this fn blocks until enumeration completes).
    // We ignore the return code (False means a callback returned false to stop).
    let _ = unsafe { EnumWindows(Some(enum_windows_proc), lparam) };
}

/// Re-resolve a [`RefEntry`] to a live [`IUIAutomationElement`].
///
/// Resolution priority, per `docs/uia-mapping.md` §7:
///
/// 1. Fast path: if the entry carries an `AutomationId`, ask UIA itself for
///    the matching subtree element. UIA evaluates the property condition in
///    its own indexed structures, which avoids cross-process COM round-trips
///    per node and is what makes WPF/WinUI apps with thousands of controls
///    snappy to drive.
/// 2. Fall back to a `(role, name, nth)` walk of the Control view, mirroring
///    the snapshot's pre-order DFS. This is the durable path that survives
///    UIA tree mutations like virtualization realising/unrealising rows.
fn resolve_element(
    automation: &IUIAutomation,
    hwnd: HWND,
    target: &RefEntry,
) -> Result<IUIAutomationElement> {
    if hwnd.0.is_null() {
        return Err(Error::Action {
            action: "resolve".into(),
            reason: "snapshot target window is no longer valid".into(),
        });
    }

    // SAFETY: hwnd is non-null per the check above.
    let root = unsafe { automation.ElementFromHandle(hwnd) }.map_err(|e| Error::Action {
        action: "resolve".into(),
        reason: format!("ElementFromHandle: {e}"),
    })?;

    if let Some(NativeHandle::Uia {
        automation_id: Some(aid),
        ..
    }) = &target.native
    {
        if let Some(elem) = find_by_automation_id(automation, &root, aid, &target.role) {
            return Ok(elem);
        }
    }

    // SAFETY: `automation` is a valid COM interface.
    let walker = unsafe { automation.ControlViewWalker() }.map_err(|e| Error::Action {
        action: "resolve".into(),
        reason: format!("ControlViewWalker: {e}"),
    })?;

    find_in_tree(&walker, &root, target).ok_or_else(|| Error::Action {
        action: "resolve".into(),
        reason: format!(
            "could not find element matching role={:?} name={:?} nth={}",
            target.role, target.name, target.nth
        ),
    })
}

/// Fast-path resolver: ask UIA for the subtree element with this
/// `AutomationId`. Returns `None` if the property condition can't be
/// constructed, the search returns nothing, or the found element's role no
/// longer matches what we captured (a buggy app could reuse the same
/// `AutomationId` across role changes; the role check keeps us honest).
fn find_by_automation_id(
    automation: &IUIAutomation,
    root: &IUIAutomationElement,
    automation_id: &str,
    expected_role: &Role,
) -> Option<IUIAutomationElement> {
    let value: VARIANT = BSTR::from(automation_id).into();
    // SAFETY: `automation` is a valid COM interface; `value` outlives the call.
    let condition: IUIAutomationCondition =
        unsafe { automation.CreatePropertyCondition(UIA_AutomationIdPropertyId, &value) }.ok()?;

    // SAFETY: `root` and `condition` are valid COM interfaces.
    let element = unsafe { root.FindFirst(TreeScope(TreeScope_Subtree.0), &condition) }.ok()?;

    // Defensive: confirm the role still matches what the snapshot recorded.
    // SAFETY: `element` is valid; failed reads fall back to a sentinel role.
    let ct = unsafe { element.CurrentControlType() }.unwrap_or(UIA_CONTROLTYPE_ID(0));
    let class = unsafe { element.CurrentClassName() }
        .ok()
        .map(|b| b.to_string())
        .unwrap_or_default();
    // We don't have the parent on this fast path (FindFirst returned the
    // element directly, not a path). Per-element promotions still apply;
    // the parent-aware ListItem→Option promotion does not, so a ref whose
    // recorded role is `Option` will deliberately miss the fast path here
    // and fall through to `find_in_tree`, which threads parent context.
    if &promoted_role(&element, ct, &class, None) == expected_role {
        Some(element)
    } else {
        None
    }
}

/// Walk the Control view in the same pre-order DFS the snapshot used,
/// returning the element matching the target's `(role, name, nth)` triple.
///
/// `nth` is global across the snapshot, mirroring the snapshot-time scheme.
/// Crucially, the predicate that decides whether an element "counts" toward
/// `nth` must match snapshot's ref-emission predicate exactly (interactive
/// role OR editable `ValuePattern`) — otherwise an element matching role+name
/// that didn't get a ref in the snapshot would still bump the counter here,
/// causing action-time resolution to land on the wrong element.
fn find_in_tree(
    walker: &IUIAutomationTreeWalker,
    root: &IUIAutomationElement,
    target: &RefEntry,
) -> Option<IUIAutomationElement> {
    let mut counter: usize = 0;

    // Check the root itself first to mirror the snapshot's root handling.
    // The root has no parent; promotion rules that depend on parent context
    // (ListItem→Option) trivially don't apply.
    if element_qualifies_as_ref(root, None, &target.role, &target.name) {
        if counter == target.nth {
            return Some(root.clone());
        }
        counter += 1;
    }
    descend(walker, root, target, &mut counter)
}

fn descend(
    walker: &IUIAutomationTreeWalker,
    parent: &IUIAutomationElement,
    target: &RefEntry,
    counter: &mut usize,
) -> Option<IUIAutomationElement> {
    // SAFETY: `parent` is a valid COM interface; walker.Get*ChildElement
    // returns Err for "no more children".
    let mut maybe_child = unsafe { walker.GetFirstChildElement(parent) }.ok();
    while let Some(child) = maybe_child {
        // Pre-order: check this element first. Pass `parent` so the qualifies
        // check applies the same parent-aware promotions the snapshot used.
        if element_qualifies_as_ref(&child, Some(parent), &target.role, &target.name) {
            if *counter == target.nth {
                return Some(child);
            }
            *counter += 1;
        }
        // Then recurse into its subtree.
        if let Some(found) = descend(walker, &child, target, counter) {
            return Some(found);
        }
        // SAFETY: `child` is a valid COM interface obtained from the walker.
        maybe_child = unsafe { walker.GetNextSiblingElement(&child) }.ok();
    }
    None
}

/// Mirrors snapshot's ref-emission predicate: returns `true` iff the element
/// matches `(role, name)` AND would have been allocated a `RefId` during
/// snapshot (interactive role, or editable `ValuePattern`). Uses the same
/// `promoted_role` the snapshot did, so refs that captured a promoted role
/// (e.g. `MenuItemCheckbox` from a `MenuItem` with `TogglePattern`) still
/// resolve correctly.
fn element_qualifies_as_ref(
    element: &IUIAutomationElement,
    parent: Option<&IUIAutomationElement>,
    role: &Role,
    name: &str,
) -> bool {
    // SAFETY: `element` is valid; failed reads fall back to defaults.
    let ct = unsafe { element.CurrentControlType() }.unwrap_or(UIA_CONTROLTYPE_ID(0));
    let class = unsafe { element.CurrentClassName() }
        .ok()
        .map(|b| b.to_string())
        .unwrap_or_default();
    let r = promoted_role(element, ct, &class, parent);
    if &r != role {
        return false;
    }
    let n = unsafe { element.CurrentName() }
        .ok()
        .map(|b| b.to_string())
        .unwrap_or_default();
    if n != name {
        return false;
    }

    // Final gate: the snapshot only allocates a ref when the element is
    // ARIA-interactive OR exposes a non-read-only `ValuePattern`. We check
    // ValuePattern only for roles that might have one (avoids per-element
    // COM calls during the cold portion of the walk).
    if r.is_interactive() {
        return true;
    }
    if role_might_have_value(&r) {
        if let Some((_, read_only)) = read_value_pattern(element) {
            if !read_only {
                return true;
            }
        }
    }
    false
}

fn action_name(a: &Action) -> &'static str {
    match a {
        Action::Click { .. } => "click",
        Action::DoubleClick { .. } => "double_click",
        Action::RightClick { .. } => "right_click",
        Action::Hover { .. } => "hover",
        Action::Focus { .. } => "focus",
        Action::Type { .. } => "type",
        Action::Fill { .. } => "fill",
        Action::Press { .. } => "press",
        Action::KeyDown { .. } => "key_down",
        Action::KeyUp { .. } => "key_up",
        Action::Scroll { .. } => "scroll",
        Action::Drag { .. } => "drag",
        Action::Select { .. } => "select",
        Action::SelectAll { .. } => "select_all",
        Action::ScrollIntoView { .. } => "scroll_into_view",
        Action::Wait { .. } => "wait",
        Action::SwitchApp { .. } => "switch_app",
        Action::FocusWindow { .. } => "focus_window",
        Action::Screenshot { .. } => "screenshot",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{parse_chord, vk_from_name};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_A, VK_CONTROL, VK_DELETE, VK_F1, VK_F12, VK_RETURN, VK_SHIFT, VK_T,
    };

    #[test]
    fn vk_letters_and_digits_match_ascii_codes() {
        assert_eq!(vk_from_name("a"), Some(VK_A));
        assert_eq!(vk_from_name("A"), Some(VK_A));
        // Digits use the same VK encoding as their ASCII codepoint.
        assert_eq!(vk_from_name("0").map(|v| v.0), Some(0x30));
        assert_eq!(vk_from_name("9").map(|v| v.0), Some(0x39));
    }

    #[test]
    fn vk_function_keys_span_f1_to_f24() {
        assert_eq!(vk_from_name("F1"), Some(VK_F1));
        assert_eq!(vk_from_name("f12"), Some(VK_F12));
        assert_eq!(vk_from_name("F24").map(|v| v.0), Some(0x70 + 23));
        assert_eq!(vk_from_name("F25"), None);
        assert_eq!(vk_from_name("F0"), None);
    }

    #[test]
    fn vk_named_keys_are_case_insensitive() {
        assert_eq!(vk_from_name("Enter"), Some(VK_RETURN));
        assert_eq!(vk_from_name("RETURN"), Some(VK_RETURN));
        assert_eq!(vk_from_name("delete"), Some(VK_DELETE));
        assert_eq!(vk_from_name("Del"), Some(VK_DELETE));
        assert_eq!(vk_from_name("ctrl"), Some(VK_CONTROL));
        assert_eq!(vk_from_name("Control"), Some(VK_CONTROL));
        assert_eq!(vk_from_name("nonsense"), None);
    }

    #[test]
    fn chord_parses_modifiers_then_key() {
        let c = parse_chord("Ctrl+Shift+T").unwrap();
        assert_eq!(c.modifiers, vec![VK_CONTROL, VK_SHIFT]);
        assert_eq!(c.key, VK_T);
    }

    #[test]
    fn chord_accepts_single_key_with_no_modifiers() {
        let c = parse_chord("Enter").unwrap();
        assert!(c.modifiers.is_empty());
        assert_eq!(c.key, VK_RETURN);
    }

    #[test]
    fn chord_rejects_empty_or_unknown_tokens() {
        assert!(parse_chord("").is_err());
        assert!(parse_chord("Ctrl+").is_err());
        assert!(parse_chord("+A").is_err());
        assert!(parse_chord("Ctrl+Bogus").is_err());
    }

    /// Mirror of `screen_to_absolute`'s arithmetic with caller-supplied
    /// virtual-screen metrics. Lets us unit-test the conversion without
    /// pulling in the live `GetSystemMetrics` values, which vary by host.
    fn screen_to_absolute_for(
        x_phys: i32,
        y_phys: i32,
        xv: i32,
        yv: i32,
        cx: i32,
        cy: i32,
    ) -> (i32, i32) {
        let cx = cx.max(1);
        let cy = cy.max(1);
        let nx = (i64::from(x_phys - xv) * 65535 + i64::from(cx) / 2) / i64::from(cx);
        let ny = (i64::from(y_phys - yv) * 65535 + i64::from(cy) / 2) / i64::from(cy);
        let nx = nx.clamp(0, 65535);
        let ny = ny.clamp(0, 65535);
        #[allow(clippy::cast_possible_truncation)]
        (nx as i32, ny as i32)
    }

    #[test]
    fn screen_to_absolute_maps_corners() {
        // Primary 1920x1080 at origin: (0,0) → (0,0); (1919,1079) → (~65535,~65535).
        let (x0, y0) = screen_to_absolute_for(0, 0, 0, 0, 1920, 1080);
        assert_eq!((x0, y0), (0, 0));
        let (xn, yn) = screen_to_absolute_for(1919, 1079, 0, 0, 1920, 1080);
        // Last *pixel* (not last fractional point) maps to roughly (N-1)/N
        // of the absolute range — at 1080 rows that's ~65474. The cursor
        // lands within one pixel of the corner, which is the accuracy
        // contract MOUSEEVENTF_ABSOLUTE provides.
        assert!(xn >= 65400, "right edge x mapped to {xn}");
        assert!(yn >= 65400, "bottom edge y mapped to {yn}");
    }

    #[test]
    fn screen_to_absolute_handles_negative_virtual_origin() {
        // Secondary monitor 1920x1080 to the LEFT of primary: virtual desktop
        // starts at x=-1920. The point at the left edge of the secondary
        // (physical x=-1920) should map to nx=0.
        let virtual_w = 3840;
        let (nx, _) = screen_to_absolute_for(-1920, 0, -1920, 0, virtual_w, 1080);
        assert_eq!(nx, 0);
        // The seam between the two monitors (physical x=0) should map to
        // roughly the midpoint of the absolute range.
        let (nmid, _) = screen_to_absolute_for(0, 0, -1920, 0, virtual_w, 1080);
        assert!((32700..=32800).contains(&nmid), "seam mapped to {nmid}");
    }

    #[test]
    fn scroll_delta_rounds_and_clamps() {
        use super::scroll_delta_from_pixels;
        // Ordinary cases round to the nearest integer.
        assert_eq!(scroll_delta_from_pixels(0.0), 0);
        assert_eq!(scroll_delta_from_pixels(120.0), 120);
        assert_eq!(scroll_delta_from_pixels(-120.0), -120);
        assert_eq!(scroll_delta_from_pixels(119.6), 120);
        // Sub-pixel values short-circuit so we don't emit no-op wheel events.
        assert_eq!(scroll_delta_from_pixels(0.4), 0);
        assert_eq!(scroll_delta_from_pixels(-0.4), 0);
        // Pathological inputs saturate rather than UB on the cast.
        assert_eq!(scroll_delta_from_pixels(1e15), i32::MAX);
        assert_eq!(scroll_delta_from_pixels(-1e15), i32::MIN);
    }

    #[test]
    fn screen_to_absolute_clamps_off_screen() {
        // Pathological inputs (cursor outside the virtual desktop) clamp
        // rather than overflow into negative or >65535 values, which
        // `MOUSEEVENTF_ABSOLUTE` documents as undefined behavior.
        let (nx, ny) = screen_to_absolute_for(-10_000, -10_000, 0, 0, 1920, 1080);
        assert_eq!((nx, ny), (0, 0));
        let (nx, ny) = screen_to_absolute_for(100_000, 100_000, 0, 0, 1920, 1080);
        assert_eq!((nx, ny), (65535, 65535));
    }

    #[test]
    fn process_name_from_app_id_handles_paths_and_bare_names() {
        use super::process_name_from_app_id;
        // Bare name passes through.
        assert_eq!(process_name_from_app_id("Notepad"), "Notepad");
        // Full Windows path → file stem.
        assert_eq!(
            process_name_from_app_id("C:\\Program Files\\WindowsApps\\Foo\\Notepad.exe"),
            "Notepad"
        );
        // Forward slashes still work (Path is platform-aware on Windows but
        // also tolerates `/`).
        assert_eq!(process_name_from_app_id("/usr/bin/code"), "code");
        // Empty input passes through harmlessly.
        assert_eq!(process_name_from_app_id(""), "");
    }

    #[test]
    fn parse_window_id_accepts_hex_with_or_without_prefix() {
        use super::parse_window_id;
        // Standard form emitted by snapshot.
        assert!(parse_window_id("0x10edc").is_some());
        // Bare hex also works.
        assert!(parse_window_id("10edc").is_some());
        // Uppercase prefix.
        assert!(parse_window_id("0X10EDC").is_some());
        // Whitespace tolerated.
        assert!(parse_window_id("  0x10edc  ").is_some());
        // Zero is invalid (null HWND has no useful meaning here).
        assert!(parse_window_id("0x0").is_none());
        assert!(parse_window_id("0").is_none());
        // Garbage rejected.
        assert!(parse_window_id("not-hex").is_none());
        assert!(parse_window_id("").is_none());
    }

    /// `RuntimeId` packing must keep the i32 sequence reconstructable: a
    /// downstream consumer (or a future runtime-id-based fast-path) needs to
    /// be able to read the bytes back as little-endian i32s in order.
    #[test]
    fn runtime_id_bytes_round_trip_as_le_i32() {
        let ids: [i32; 3] = [42, -1, 0x7FFF_FFFF];
        let mut bytes: Vec<u8> = Vec::new();
        for id in ids {
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        assert_eq!(bytes.len(), 12);

        let reconstructed: Vec<i32> = bytes
            .chunks_exact(4)
            .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(reconstructed, ids);
    }
}
