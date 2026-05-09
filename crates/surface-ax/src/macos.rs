//! macOS-specific AX snapshot implementation.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
use std::process::Command;
use std::time::SystemTime;

use accessibility_sys::{
    kAXButtonRole, kAXCheckBoxRole, kAXChildrenAttribute, kAXComboBoxRole, kAXDescriptionAttribute,
    kAXDialogSubrole, kAXEnabledAttribute, kAXErrorSuccess, kAXExpandedAttribute,
    kAXFocusedApplicationAttribute, kAXFocusedAttribute, kAXFocusedWindowAttribute, kAXGroupRole,
    kAXImageRole, kAXMenuBarItemRole, kAXMenuBarRole, kAXMenuButtonRole, kAXMenuItemRole,
    kAXMenuRole, kAXOutlineRole, kAXPopUpButtonRole, kAXPositionAttribute, kAXPressAction,
    kAXRadioButtonRole, kAXRaiseAction, kAXRoleAttribute, kAXScrollAreaRole, kAXSearchFieldSubrole,
    kAXSelectedAttribute, kAXSizeAttribute, kAXSliderRole, kAXStaticTextRole, kAXSubroleAttribute,
    kAXTabGroupRole, kAXTableRole, kAXTextAreaRole, kAXTextFieldRole, kAXTitleAttribute,
    kAXValueAttribute, kAXValueTypeCGPoint, kAXValueTypeCGSize, kAXWindowsAttribute,
    AXIsProcessTrusted, AXUIElementCopyAttributeValue, AXUIElementCreateApplication,
    AXUIElementCreateSystemWide, AXUIElementGetPid, AXUIElementPerformAction, AXUIElementRef,
    AXUIElementSetAttributeValue, AXValueGetValue, AXValueRef, CGKeyCode,
};
use agent_ctrl_core::{
    Action, ActionResult, AppContext, Bounds, Checked, ClipboardOp, Error, MouseButton, MouseOp,
    NativeHandle, Node, RefEntry, RefId, RefMap, Region, Result, Role, ScreenshotTarget, Snapshot,
    SnapshotOptions, State, SurfaceKind, WindowContext, WindowInfo, WindowTarget,
};
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{kCFAllocatorDefault, CFGetTypeID, CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation_sys::number::{
    kCFBooleanTrue, kCFNumberDoubleType, kCFNumberIntType, CFBooleanGetTypeID, CFBooleanGetValue,
    CFNumberGetTypeID, CFNumberGetValue,
};
use core_foundation_sys::string::{
    kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringGetCString,
    CFStringGetMaximumSizeForEncoding, CFStringGetTypeID, CFStringRef,
};

const DEFAULT_DEPTH: usize = 12;
const K_CG_HID_EVENT_TAP: u32 = 0;
const CG_FLAG_SHIFT: u64 = 1 << 17;
const CG_FLAG_CONTROL: u64 = 1 << 18;
const CG_FLAG_OPTION: u64 = 1 << 19;
const CG_FLAG_COMMAND: u64 = 1 << 20;

type CGEventRef = *mut c_void;
type CGImageRef = *mut c_void;
type CGDataProviderRef = *mut c_void;
type CFDataRef = *const c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    origin: CGPoint,
    size: CGSize,
}

const CG_NULL_WINDOW_ID: u32 = 0;
const CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY: u32 = 1 << 0;
const CG_WINDOW_LIST_OPTION_INCLUDING_WINDOW: u32 = 1 << 3;
const CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
const CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING: u32 = 1 << 0;
const CG_WINDOW_IMAGE_BEST_RESOLUTION: u32 = 1 << 3;
const CG_BITMAP_BYTE_ORDER_MASK: u32 = 0x7000;
const CG_BITMAP_BYTE_ORDER_32_LITTLE: u32 = 2 << 12;
const CG_IMAGE_ALPHA_INFO_MASK: u32 = 0x1F;

const CG_EVENT_LEFT_MOUSE_DOWN: u32 = 1;
const CG_EVENT_LEFT_MOUSE_UP: u32 = 2;
const CG_EVENT_RIGHT_MOUSE_DOWN: u32 = 3;
const CG_EVENT_RIGHT_MOUSE_UP: u32 = 4;
const CG_EVENT_MOUSE_MOVED: u32 = 5;
const CG_EVENT_LEFT_MOUSE_DRAGGED: u32 = 6;
const CG_EVENT_OTHER_MOUSE_DOWN: u32 = 25;
const CG_EVENT_OTHER_MOUSE_UP: u32 = 26;

const CG_MOUSE_BUTTON_LEFT: u32 = 0;
const CG_MOUSE_BUTTON_RIGHT: u32 = 1;
const CG_MOUSE_BUTTON_CENTER: u32 = 2;

const CG_SCROLL_UNIT_PIXEL: u32 = 0;

// CGEventField indices.
const CG_MOUSE_EVENT_CLICK_STATE: u32 = 1;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGEventCreateKeyboardEvent(
        source: *const c_void,
        virtualKey: CGKeyCode,
        keyDown: bool,
    ) -> CGEventRef;
    fn CGEventKeyboardSetUnicodeString(
        event: CGEventRef,
        stringLength: usize,
        unicodeString: *const u16,
    );
    fn CGEventSetFlags(event: CGEventRef, flags: u64);
    fn CGEventPost(tap: u32, event: CGEventRef);

    fn CGEventCreateMouseEvent(
        source: *const c_void,
        mouseType: u32,
        position: CGPoint,
        mouseButton: u32,
    ) -> CGEventRef;
    fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);

    // Apple's ARM64 ABI passes variadic args in registers same as fixed args
    // (unlike standard AArch64), so a 5-arg fixed-arity declaration is safe on
    // both x86_64 and aarch64 macOS, the only platforms this surface targets.
    fn CGEventCreateScrollWheelEvent(
        source: *const c_void,
        units: u32,
        wheelCount: u32,
        wheel1: i32,
        wheel2: i32,
    ) -> CGEventRef;

    fn CGWindowListCopyWindowInfo(option: u32, relativeToWindow: u32) -> CFArrayRef;
    fn CGWindowListCreateImage(
        screenBounds: CGRect,
        listOption: u32,
        windowId: u32,
        imageOption: u32,
    ) -> CGImageRef;
    fn CGImageGetWidth(image: CGImageRef) -> usize;
    fn CGImageGetHeight(image: CGImageRef) -> usize;
    fn CGImageGetBytesPerRow(image: CGImageRef) -> usize;
    fn CGImageGetBitsPerPixel(image: CGImageRef) -> usize;
    fn CGImageGetBitmapInfo(image: CGImageRef) -> u32;
    fn CGImageGetDataProvider(image: CGImageRef) -> CGDataProviderRef;
    fn CGImageRelease(image: CGImageRef);
    fn CGDataProviderCopyData(provider: CGDataProviderRef) -> CFDataRef;
    fn CFDataGetLength(data: CFDataRef) -> isize;
    fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
}

fn cg_rect_infinite() -> CGRect {
    // CGRectInfinite per CoreGraphics. CGWindowListCreateImage treats this
    // as "the union of every window in the matched list" (the documented
    // sentinel for "give me everything").
    let half = f64::MAX / 2.0;
    CGRect {
        origin: CGPoint { x: -half, y: -half },
        size: CGSize {
            width: f64::MAX,
            height: f64::MAX,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct AxPinnedWindow {
    pub(super) pid: u32,
    pub(super) index: usize,
}

pub(super) struct AxSnapshotCapture {
    pub(super) snapshot: Snapshot,
    pub(super) pinned: AxPinnedWindow,
}

pub(super) struct AxWindowList {
    pub(super) windows: Vec<WindowInfo>,
    pub(super) pinned: Option<AxPinnedWindow>,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGSize {
    width: f64,
    height: f64,
}

pub(super) fn is_process_trusted() -> bool {
    // SAFETY: `AXIsProcessTrusted` reads process accessibility state.
    unsafe { AXIsProcessTrusted() }
}

pub(super) fn snapshot(
    opts: &SnapshotOptions,
    pinned: Option<AxPinnedWindow>,
) -> Result<AxSnapshotCapture> {
    let (root, pinned) = match &opts.target {
        WindowTarget::Foreground => {
            if let Some(pinned) = pinned {
                (window_by_index(pinned.pid, pinned.index)?, pinned)
            } else {
                focused_window_with_pin()?
            }
        }
        WindowTarget::Pid { pid } => app_focused_window(*pid)?,
        WindowTarget::Title { title } => window_by_title(title)?,
        WindowTarget::ProcessName { name } => app_window_by_process_name(name)?,
    };
    let pid = element_pid(root).unwrap_or_default();
    let title = string_attr(root, kAXTitleAttribute);
    let max_depth = opts.depth.unwrap_or(DEFAULT_DEPTH);
    let mut refs = RefMap::new();
    let mut nth_seen = HashMap::new();
    let node = build_node(root, 0, max_depth, &mut refs, &mut nth_seen);
    let window_id = window_id(pinned);
    // SAFETY: `root` is returned by a create/copy rule helper.
    unsafe { CFRelease(root.cast::<c_void>()) };

    Ok(AxSnapshotCapture {
        snapshot: Snapshot {
            captured_at: SystemTime::now(),
            surface_kind: SurfaceKind::Ax,
            app: AppContext {
                id: format!("pid:{pid}"),
                name: process_name(pid).unwrap_or_else(|| format!("pid {pid}")),
            },
            window: Some(WindowContext {
                id: window_id,
                title,
            }),
            root: node,
            refs,
        },
        pinned,
    })
}

pub(super) fn list_windows(pinned: Option<AxPinnedWindow>) -> Result<AxWindowList> {
    let pinned = match pinned {
        Some(pinned) => Some(pinned),
        None => focused_window_pin().ok(),
    };
    let pid = pinned.map_or_else(focused_window_pid, |pinned| Ok(pinned.pid))?;
    let process = process_name(pid).unwrap_or_else(|| format!("pid {pid}"));
    let focused = focused_window_pin().ok();
    let array = app_windows(pid)?;
    // SAFETY: `array` is a valid CFArray copied from AX.
    let count = unsafe { CFArrayGetCount(array) };
    let mut windows = Vec::new();
    for idx in 0..count {
        let Ok(index) = usize::try_from(idx) else {
            continue;
        };
        let Some(window) = array_element_retained(array, idx) else {
            continue;
        };
        let pin = AxPinnedWindow { pid, index };
        let window_ref = window.cast_mut().cast();
        windows.push(WindowInfo {
            id: window_id(pin),
            title: string_attr(window_ref, kAXTitleAttribute),
            process: process.clone(),
            pid,
            focused: focused == Some(pin)
                || bool_attr(window_ref, kAXFocusedAttribute) == Some(true),
            pinned: pinned == Some(pin),
        });
        // SAFETY: release the retained window element.
        unsafe { CFRelease(window.cast::<c_void>()) };
    }
    // SAFETY: release the copy-rule array.
    unsafe { CFRelease(array.cast::<c_void>()) };

    Ok(AxWindowList { windows, pinned })
}

pub(super) fn focus_window(id: &str) -> Result<AxPinnedWindow> {
    let pinned = parse_window_id(id)?;
    let window = window_by_index(pinned.pid, pinned.index)?;
    let action = cf_string(kAXRaiseAction)
        .ok_or_else(|| Error::Surface("failed to allocate AXRaise action string".into()))?;
    // SAFETY: window and action are valid AX/Core Foundation refs.
    let err = unsafe { AXUIElementPerformAction(window, action) };
    // SAFETY: release create/copy-rule refs.
    unsafe {
        CFRelease(action.cast::<c_void>());
        CFRelease(window.cast::<c_void>());
    }
    if err == kAXErrorSuccess {
        Ok(pinned)
    } else {
        Err(Error::Action {
            action: "focus_window".into(),
            reason: format!("AXRaise failed with AX error {err}"),
        })
    }
}

pub(super) fn act(
    action: &Action,
    pinned: Option<AxPinnedWindow>,
    snapshot: Option<&Snapshot>,
) -> Result<ActionResult> {
    let refs = snapshot.map(|s| &s.refs);
    match action {
        Action::Click { ref_id } => act_click(pinned, refs, ref_id),
        Action::DoubleClick { ref_id } => act_double_click(pinned, refs, ref_id),
        Action::RightClick { ref_id } => act_right_click(pinned, refs, ref_id),
        Action::Hover { ref_id } => act_hover(pinned, refs, ref_id),
        Action::Focus { ref_id } => act_focus(pinned, refs, ref_id),
        Action::Fill { ref_id, value } => act_fill(pinned, refs, ref_id, value),
        Action::Check { ref_id } => act_check(pinned, refs, ref_id, true),
        Action::Uncheck { ref_id } => act_check(pinned, refs, ref_id, false),
        Action::Toggle { ref_id } => act_toggle(pinned, refs, ref_id),
        Action::Type { text } => act_type(pinned, text),
        Action::Press { keys } => act_press(pinned, keys),
        Action::KeyDown { key } => act_key(pinned, key, true, "key_down"),
        Action::KeyUp { key } => act_key(pinned, key, false, "key_up"),
        Action::Drag { from, to } => act_drag(pinned, refs, from, to),
        Action::Mouse { op } => act_mouse(*op),
        Action::Select { ref_id, value } => act_select(pinned, refs, ref_id, value),
        Action::SelectAll { ref_id } => act_select_all(pinned, refs, ref_id.as_ref()),
        Action::Clear { ref_id } => act_clear(pinned, refs, ref_id),
        Action::Clipboard { op } => act_clipboard(pinned, op),
        Action::ScrollIntoView { ref_id } => act_scroll_into_view(pinned, refs, ref_id),
        Action::Highlight {
            ref_id,
            duration_ms,
        } => act_highlight(pinned, refs, ref_id, *duration_ms),
        Action::Scroll { ref_id, dx, dy } => act_scroll(pinned, refs, ref_id.as_ref(), *dx, *dy),
        Action::Screenshot {
            region,
            target,
            annotated,
        } => act_screenshot(
            pinned,
            snapshot,
            region.as_ref(),
            target.as_ref(),
            *annotated,
        ),
        Action::Wait { ms } => {
            std::thread::sleep(std::time::Duration::from_millis(*ms));
            Ok(ActionResult::ok())
        }
        _ => Err(Error::Unsupported {
            surface: SurfaceKind::Ax.as_str().into(),
            action: action_name(action).into(),
        }),
    }
}

fn focused_window() -> Result<AXUIElementRef> {
    // SAFETY: create rule returns a valid system-wide AX object or null.
    let system = unsafe { AXUIElementCreateSystemWide() };
    if system.is_null() {
        return Err(Error::Surface("AX system-wide element was null".into()));
    }
    let window = element_attr(system, kAXFocusedWindowAttribute)
        .or_else(|| focused_application_window(system));
    // SAFETY: release the create-rule system object after copying the window.
    unsafe { CFRelease(system.cast::<c_void>()) };
    let Some(window) = window else {
        return Err(Error::Surface("no focused AX window".into()));
    };
    Ok(window as AXUIElementRef)
}

fn focused_application_window(system: AXUIElementRef) -> Option<CFTypeRef> {
    let app = element_attr(system, kAXFocusedApplicationAttribute)?;
    let window = element_attr(app.cast_mut().cast(), kAXFocusedWindowAttribute)
        .or_else(|| first_array_element(app.cast_mut().cast(), kAXWindowsAttribute));
    // SAFETY: release the copy-rule focused app object after copying a window.
    unsafe { CFRelease(app.cast::<c_void>()) };
    window
}

fn focused_window_with_pin() -> Result<(AXUIElementRef, AxPinnedWindow)> {
    let window = focused_window()?;
    let pid = element_pid(window).unwrap_or_default();
    let index = window_index(pid, window).unwrap_or(0);
    Ok((window, AxPinnedWindow { pid, index }))
}

fn focused_window_pin() -> Result<AxPinnedWindow> {
    let (window, pinned) = focused_window_with_pin()?;
    // SAFETY: `window` is returned by a copy-rule helper.
    unsafe { CFRelease(window.cast::<c_void>()) };
    Ok(pinned)
}

fn focused_window_pid() -> Result<u32> {
    let window = focused_window()?;
    let pid = element_pid(window)
        .filter(|pid| *pid != 0)
        .ok_or_else(|| Error::Surface("focused AX window has no pid".into()));
    // SAFETY: `window` is returned by a copy-rule helper.
    unsafe { CFRelease(window.cast::<c_void>()) };
    pid
}

fn app_focused_window(pid: u32) -> Result<(AXUIElementRef, AxPinnedWindow)> {
    let original_pid = pid;
    let pid = i32::try_from(pid).map_err(|_| Error::Surface("pid out of range".into()))?;
    // SAFETY: create rule returns an AX application object for pid or null.
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err(Error::Surface(format!(
            "AX application for pid {pid} was null"
        )));
    }
    let window = element_attr(app, kAXFocusedWindowAttribute)
        .or_else(|| first_array_element(app, kAXWindowsAttribute));
    // SAFETY: release the create-rule app object after copying the window.
    unsafe { CFRelease(app.cast::<c_void>()) };
    let Some(window) = window else {
        return Err(Error::Surface(format!("pid {pid} has no AX window")));
    };
    let index = window_index(original_pid, window.cast_mut().cast()).unwrap_or(0);
    Ok((
        window.cast_mut().cast(),
        AxPinnedWindow {
            pid: original_pid,
            index,
        },
    ))
}

fn app_window_by_process_name(name: &str) -> Result<(AXUIElementRef, AxPinnedWindow)> {
    let pids = process_ids_by_name(name)?;
    for pid in pids {
        if let Ok(window) = app_focused_window(pid) {
            return Ok(window);
        }
    }
    Err(Error::Surface(format!(
        "no AX window found for process name {name:?}"
    )))
}

fn window_by_title(title: &str) -> Result<(AXUIElementRef, AxPinnedWindow)> {
    let needle = title.to_lowercase();
    for pid in process_ids()? {
        let Ok(array) = app_windows(pid) else {
            continue;
        };
        // SAFETY: `array` is a valid CFArray copied from AX.
        let count = unsafe { CFArrayGetCount(array) };
        for idx in 0..count {
            let window = unsafe { CFArrayGetValueAtIndex(array, idx) };
            if window.is_null() {
                continue;
            }
            let window_ref = window.cast_mut().cast();
            let Some(window_title) = string_attr(window_ref, kAXTitleAttribute) else {
                continue;
            };
            if window_title.to_lowercase().contains(&needle) {
                // SAFETY: retain the borrowed array element so it survives array release.
                unsafe { CFRetain(window.cast::<c_void>()) };
                // SAFETY: release copy-rule array before returning retained window.
                unsafe { CFRelease(array.cast::<c_void>()) };
                let index = usize::try_from(idx)
                    .map_err(|_| Error::Surface("window index out of range".into()))?;
                return Ok((window.cast_mut().cast(), AxPinnedWindow { pid, index }));
            }
        }
        // SAFETY: release copy-rule array after scanning this app.
        unsafe { CFRelease(array.cast::<c_void>()) };
    }
    Err(Error::Surface(format!(
        "no AX window title contains {title:?}"
    )))
}

fn build_node(
    element: AXUIElementRef,
    depth: usize,
    max_depth: usize,
    refs: &mut RefMap,
    nth_seen: &mut HashMap<(Role, String), usize>,
) -> Node {
    let role_raw = string_attr(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());
    let subrole = string_attr(element, kAXSubroleAttribute);
    let role = map_role(&role_raw, subrole.as_deref());
    let name = string_attr(element, kAXTitleAttribute)
        .or_else(|| string_attr(element, kAXDescriptionAttribute))
        .or_else(|| value_string(element))
        .unwrap_or_default();
    let value = value_string(element).filter(|value| value != &name);
    let state = State {
        visible: true,
        enabled: bool_attr(element, kAXEnabledAttribute).unwrap_or(true),
        focused: bool_attr(element, kAXFocusedAttribute).unwrap_or(false),
        selected: bool_attr(element, kAXSelectedAttribute),
        checked: checked_state_for_role(&role, element),
        expanded: bool_attr(element, kAXExpandedAttribute),
        required: None,
    };
    let bounds = bounds(element);
    let native = Some(NativeHandle::Ax {
        element_ref: element as u64,
    });
    let ref_id = if is_ref_target(&role) {
        let key = (role.clone(), name.clone());
        let counter = nth_seen.entry(key).or_insert(0);
        let nth = *counter;
        *counter += 1;
        Some(refs.insert(role.clone(), name.clone(), nth, native.clone()))
    } else {
        None
    };
    let children = if depth >= max_depth {
        Vec::new()
    } else {
        build_children(element, depth + 1, max_depth, refs, nth_seen)
    };

    Node {
        ref_id,
        role,
        name,
        description: None,
        value,
        state,
        bounds,
        level: None,
        children,
        opaque: false,
        native,
    }
}

fn map_role(role: &str, subrole: Option<&str>) -> Role {
    if matches!(subrole, Some(value) if value == kAXSearchFieldSubrole)
        && matches!(role, value if value == kAXTextFieldRole || value == kAXTextAreaRole)
    {
        return Role::SearchBox;
    }
    if matches!(subrole, Some(value) if value == kAXDialogSubrole) {
        return Role::Dialog;
    }

    match role {
        v if v == kAXButtonRole || v == kAXPopUpButtonRole || v == kAXMenuButtonRole => {
            Role::Button
        }
        v if v == kAXTextFieldRole || v == kAXTextAreaRole => Role::TextField,
        value if value == kAXCheckBoxRole => Role::Checkbox,
        value if value == kAXRadioButtonRole => Role::Radio,
        value if value == kAXComboBoxRole => Role::ComboBox,
        v if v == kAXMenuItemRole || v == kAXMenuBarItemRole => Role::MenuItem,
        value if value == kAXSliderRole => Role::Slider,
        value if value == kAXStaticTextRole => Role::Region,
        value if value == kAXImageRole => Role::Image,
        value if value == kAXMenuRole => Role::Menu,
        value if value == kAXMenuBarRole => Role::MenuBar,
        value if value == kAXTableRole => Role::Table,
        value if value == kAXOutlineRole => Role::Tree,
        value if value == kAXTabGroupRole => Role::TabList,
        v if v == kAXScrollAreaRole || v == kAXGroupRole => Role::Group,
        "AXWindow" => Role::Window,
        "AXApplication" => Role::Application,
        other => Role::Unknown(other.to_owned()),
    }
}

fn bounds(element: AXUIElementRef) -> Option<Bounds> {
    let position = ax_value_attr::<CGPoint>(element, kAXPositionAttribute, kAXValueTypeCGPoint)?;
    let size = ax_value_attr::<CGSize>(element, kAXSizeAttribute, kAXValueTypeCGSize)?;
    Some(Bounds {
        x: position.x,
        y: position.y,
        w: size.width,
        h: size.height,
    })
}

fn build_children(
    element: AXUIElementRef,
    depth: usize,
    max_depth: usize,
    refs: &mut RefMap,
    nth_seen: &mut HashMap<(Role, String), usize>,
) -> Vec<Node> {
    let Some(array_ref) = array_attr(element, kAXChildrenAttribute) else {
        return Vec::new();
    };
    // SAFETY: `array_ref` is a valid CFArray copied from AX.
    let count = unsafe { CFArrayGetCount(array_ref) };
    let mut children = Vec::new();
    for idx in 0..count {
        // SAFETY: index is within CFArray bounds.
        let child = unsafe { CFArrayGetValueAtIndex(array_ref, idx) };
        if !child.is_null() {
            children.push(build_node(
                child as AXUIElementRef,
                depth,
                max_depth,
                refs,
                nth_seen,
            ));
        }
    }
    // SAFETY: release the copy-rule array after extracting child pointers.
    unsafe { CFRelease(array_ref.cast::<c_void>()) };
    children
}

fn act_click(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
) -> Result<ActionResult> {
    let element = resolve_element(pinned, refs, ref_id, "click")?;
    let result = perform_action(element, kAXPressAction, "click");
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_focus(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
) -> Result<ActionResult> {
    let element = resolve_element(pinned, refs, ref_id, "focus")?;
    let result = set_bool_attr(element, kAXFocusedAttribute, "focus");
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_fill(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
    value: &str,
) -> Result<ActionResult> {
    let element = resolve_element(pinned, refs, ref_id, "fill")?;
    let _ = set_bool_attr(element, kAXFocusedAttribute, "focus");
    let result = set_string_attr(element, kAXValueAttribute, value, "fill");
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_check(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
    desired: bool,
) -> Result<ActionResult> {
    let action = if desired { "check" } else { "uncheck" };
    let element = resolve_element(pinned, refs, ref_id, action)?;
    let result = press_until_checked(element, desired, action);
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_toggle(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
) -> Result<ActionResult> {
    let element = resolve_element(pinned, refs, ref_id, "toggle")?;
    let result = perform_action(element, kAXPressAction, "toggle");
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_double_click(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
) -> Result<ActionResult> {
    let center = element_center(pinned, refs, ref_id, "double_click")?;
    raise_pinned_app(pinned);
    post_mouse(CG_EVENT_MOUSE_MOVED, center, CG_MOUSE_BUTTON_LEFT, None)?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_DOWN,
        center,
        CG_MOUSE_BUTTON_LEFT,
        Some(1),
    )?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_UP,
        center,
        CG_MOUSE_BUTTON_LEFT,
        Some(1),
    )?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_DOWN,
        center,
        CG_MOUSE_BUTTON_LEFT,
        Some(2),
    )?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_UP,
        center,
        CG_MOUSE_BUTTON_LEFT,
        Some(2),
    )?;
    Ok(ActionResult::ok())
}

fn act_right_click(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
) -> Result<ActionResult> {
    let center = element_center(pinned, refs, ref_id, "right_click")?;
    raise_pinned_app(pinned);
    post_mouse(CG_EVENT_MOUSE_MOVED, center, CG_MOUSE_BUTTON_RIGHT, None)?;
    post_mouse(
        CG_EVENT_RIGHT_MOUSE_DOWN,
        center,
        CG_MOUSE_BUTTON_RIGHT,
        Some(1),
    )?;
    post_mouse(
        CG_EVENT_RIGHT_MOUSE_UP,
        center,
        CG_MOUSE_BUTTON_RIGHT,
        Some(1),
    )?;
    Ok(ActionResult::ok())
}

fn act_hover(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
) -> Result<ActionResult> {
    let center = element_center(pinned, refs, ref_id, "hover")?;
    raise_pinned_app(pinned);
    post_mouse(CG_EVENT_MOUSE_MOVED, center, CG_MOUSE_BUTTON_LEFT, None)?;
    Ok(ActionResult::ok())
}

fn act_highlight(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
    duration_ms: Option<u64>,
) -> Result<ActionResult> {
    let center = element_center(pinned, refs, ref_id, "highlight")?;
    raise_pinned_app(pinned);
    post_mouse(CG_EVENT_MOUSE_MOVED, center, CG_MOUSE_BUTTON_LEFT, None)?;
    let duration = duration_ms.unwrap_or(800);
    if duration > 0 {
        std::thread::sleep(std::time::Duration::from_millis(duration));
    }
    Ok(ActionResult {
        ok: true,
        message: Some("highlighted via cursor hover".into()),
        data: None,
    })
}

fn act_drag(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    from: &RefId,
    to: &RefId,
) -> Result<ActionResult> {
    let from_center = element_center(pinned, refs, from, "drag")?;
    let to_center = element_center(pinned, refs, to, "drag")?;
    raise_pinned_app(pinned);
    post_mouse(
        CG_EVENT_MOUSE_MOVED,
        from_center,
        CG_MOUSE_BUTTON_LEFT,
        None,
    )?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_DOWN,
        from_center,
        CG_MOUSE_BUTTON_LEFT,
        Some(1),
    )?;
    // Interpolate a few drag positions so apps that need motion (e.g. selection
    // handles) see a smooth path rather than a single teleport.
    let steps = 8_i32;
    for step in 1..=steps {
        let t = f64::from(step) / f64::from(steps);
        let interp = CGPoint {
            x: from_center.x + (to_center.x - from_center.x) * t,
            y: from_center.y + (to_center.y - from_center.y) * t,
        };
        post_mouse(
            CG_EVENT_LEFT_MOUSE_DRAGGED,
            interp,
            CG_MOUSE_BUTTON_LEFT,
            Some(1),
        )?;
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    post_mouse(
        CG_EVENT_LEFT_MOUSE_UP,
        to_center,
        CG_MOUSE_BUTTON_LEFT,
        Some(1),
    )?;
    Ok(ActionResult::ok())
}

fn act_mouse(op: MouseOp) -> Result<ActionResult> {
    match op {
        MouseOp::Move { x, y } => {
            post_mouse(
                CG_EVENT_MOUSE_MOVED,
                point_from(x, y),
                CG_MOUSE_BUTTON_LEFT,
                None,
            )?;
        }
        MouseOp::Down { x, y, button } => {
            let (event, btn) = mouse_button_down(button);
            post_mouse(event, point_from(x, y), btn, Some(1))?;
        }
        MouseOp::Up { x, y, button } => {
            let (event, btn) = mouse_button_up(button);
            post_mouse(event, point_from(x, y), btn, Some(1))?;
        }
        MouseOp::Wheel { x, y, dx, dy } => {
            post_scroll(point_from(x, y), dx, dy)?;
        }
    }
    Ok(ActionResult::ok())
}

fn act_scroll(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: Option<&RefId>,
    dx: f64,
    dy: f64,
) -> Result<ActionResult> {
    raise_pinned_app(pinned);
    let position = if let Some(ref_id) = ref_id {
        let center = element_center(pinned, refs, ref_id, "scroll")?;
        post_mouse(CG_EVENT_MOUSE_MOVED, center, CG_MOUSE_BUTTON_LEFT, None)?;
        center
    } else {
        // No target ref: scroll wherever the cursor already is. CGEvent scroll
        // wheels are screen-global so we just need a position to anchor to.
        CGPoint { x: 0.0, y: 0.0 }
    };
    let horizontal = round_to_i32(dx);
    let vertical = round_to_i32(dy);
    post_scroll(position, horizontal, vertical)?;
    Ok(ActionResult::ok())
}

fn act_scroll_into_view(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
) -> Result<ActionResult> {
    let element = resolve_element(pinned, refs, ref_id, "scroll_into_view")?;
    // `kAXScrollToVisibleAction` is not exposed by accessibility-sys, but the
    // string constant is stable across macOS versions. Containers (scroll
    // areas, table rows) implement it; on plain controls the AX call returns
    // an error which we surface unchanged.
    let result = perform_action(element, "AXScrollToVisible", "scroll_into_view");
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_select_all(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: Option<&RefId>,
) -> Result<ActionResult> {
    if let Some(ref_id) = ref_id {
        let element = resolve_element(pinned, refs, ref_id, "select_all")?;
        let _ = set_bool_attr(element, kAXFocusedAttribute, "select_all");
        // SAFETY: `resolve_element` returns a retained AX element.
        unsafe { CFRelease(element.cast::<c_void>()) };
    }
    act_press(pinned, "Cmd+A")
}

fn act_select(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
    value: &str,
) -> Result<ActionResult> {
    let element = resolve_element(pinned, refs, ref_id, "select")?;
    let role = string_attr(element, kAXRoleAttribute);
    let result = match role.as_deref() {
        Some(role)
            if role == kAXPopUpButtonRole
                || role == kAXMenuButtonRole
                || role == kAXComboBoxRole =>
        {
            select_via_menu(element, value)
        }
        _ => set_string_attr(element, kAXValueAttribute, value, "select"),
    };
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn select_via_menu(button: AXUIElementRef, value: &str) -> Result<()> {
    // AXPress on a popup button opens its menu, but the menu enters a
    // synthetic-show state where AppKit ignores subsequent AX presses on
    // menu items. Real mouse events sent through the HID event tap, on the
    // other hand, are tracked by NSMenu's normal modal loop just like a
    // user click. So drive the popup the same way a user would: real mouse
    // click on the popup to open the menu, then real mouse click on the
    // matching item to commit the selection.
    let popup_bounds = bounds(button).ok_or_else(|| Error::Action {
        action: "select".into(),
        reason: "popup has no AX bounds".into(),
    })?;
    let popup_center = CGPoint {
        x: popup_bounds.x + popup_bounds.w / 2.0,
        y: popup_bounds.y + popup_bounds.h / 2.0,
    };
    post_mouse(
        CG_EVENT_MOUSE_MOVED,
        popup_center,
        CG_MOUSE_BUTTON_LEFT,
        None,
    )?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_DOWN,
        popup_center,
        CG_MOUSE_BUTTON_LEFT,
        Some(1),
    )?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_UP,
        popup_center,
        CG_MOUSE_BUTTON_LEFT,
        Some(1),
    )?;
    // Give AppKit time to open the menu and set up its modal event tracking.
    // A tight AX poll loop here would serialize through the target's main
    // loop and starve the menu's setup, so we use a fixed sleep instead.
    // The wait also has to be longer than the system double-click threshold
    // (~250ms) so the next click below isn't coalesced with the popup click.
    std::thread::sleep(std::time::Duration::from_millis(400));
    let menu = find_descendant_role(button, kAXMenuRole).ok_or_else(|| Error::Action {
        action: "select".into(),
        reason: "popup menu did not appear after click".into(),
    })?;
    let item = find_menu_item(menu, value);
    // SAFETY: `wait_for_menu` returns a retained AX element.
    unsafe { CFRelease(menu.cast::<c_void>()) };
    let item = item.ok_or_else(|| Error::Action {
        action: "select".into(),
        reason: format!("no menu item titled {value:?}"),
    })?;
    let result = click_menu_item(item);
    // SAFETY: `find_menu_item` returns a retained AX element.
    unsafe { CFRelease(item.cast::<c_void>()) };
    result?;
    // Give AppKit time to dispatch the NSEvent on its main loop, commit the
    // selection, fire the popup's target/action, and dismiss the menu.
    // Polling AX from this thread serializes through the same main loop and
    // would starve the event dispatch, so a fixed sleep is the right tool.
    // 300ms is enough headroom that callers running back-to-back commands
    // (e.g. select then snapshot) don't see a stale tree.
    std::thread::sleep(std::time::Duration::from_millis(300));
    Ok(())
}

fn click_menu_item(item: AXUIElementRef) -> Result<()> {
    let logical = bounds(item).ok_or_else(|| Error::Action {
        action: "select".into(),
        reason: "menu item has no AX bounds".into(),
    })?;
    let center = CGPoint {
        x: logical.x + logical.w / 2.0,
        y: logical.y + logical.h / 2.0,
    };
    post_mouse(CG_EVENT_MOUSE_MOVED, center, CG_MOUSE_BUTTON_LEFT, None)?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_DOWN,
        center,
        CG_MOUSE_BUTTON_LEFT,
        Some(1),
    )?;
    post_mouse(
        CG_EVENT_LEFT_MOUSE_UP,
        center,
        CG_MOUSE_BUTTON_LEFT,
        Some(1),
    )?;
    Ok(())
}

fn find_descendant_role(element: AXUIElementRef, role: &str) -> Option<AXUIElementRef> {
    if string_attr(element, kAXRoleAttribute).as_deref() == Some(role) {
        // SAFETY: retain the matched element so the caller can release.
        unsafe { CFRetain(element.cast::<c_void>()) };
        return Some(element);
    }
    let array = array_attr(element, kAXChildrenAttribute)?;
    // SAFETY: array is a copy-rule CFArray.
    let count = unsafe { CFArrayGetCount(array) };
    let mut found = None;
    for idx in 0..count {
        // SAFETY: idx is in bounds.
        let child = unsafe { CFArrayGetValueAtIndex(array, idx) };
        if !child.is_null() {
            if let Some(hit) = find_descendant_role(child as AXUIElementRef, role) {
                found = Some(hit);
                break;
            }
        }
    }
    // SAFETY: release the copy-rule array.
    unsafe { CFRelease(array.cast::<c_void>()) };
    found
}

fn find_menu_item(menu: AXUIElementRef, value: &str) -> Option<AXUIElementRef> {
    let array = array_attr(menu, kAXChildrenAttribute)?;
    // SAFETY: array is a copy-rule CFArray.
    let count = unsafe { CFArrayGetCount(array) };
    let mut found = None;
    for idx in 0..count {
        // SAFETY: idx is in bounds.
        let child_raw = unsafe { CFArrayGetValueAtIndex(array, idx) };
        if child_raw.is_null() {
            continue;
        }
        let child = child_raw as AXUIElementRef;
        let role = string_attr(child, kAXRoleAttribute);
        let is_menu_item = role
            .as_deref()
            .is_some_and(|role| role == kAXMenuItemRole || role == kAXMenuBarItemRole);
        if !is_menu_item {
            continue;
        }
        let title = string_attr(child, kAXTitleAttribute).unwrap_or_default();
        if title == value {
            // SAFETY: retain the matched element so the caller can release.
            unsafe { CFRetain(child.cast::<c_void>()) };
            found = Some(child);
            break;
        }
    }
    // SAFETY: release the copy-rule array.
    unsafe { CFRelease(array.cast::<c_void>()) };
    found
}

fn act_clear(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
) -> Result<ActionResult> {
    let element = resolve_element(pinned, refs, ref_id, "clear")?;
    let _ = set_bool_attr(element, kAXFocusedAttribute, "clear");
    let result = set_string_attr(element, kAXValueAttribute, "", "clear");
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_clipboard(pinned: Option<AxPinnedWindow>, op: &ClipboardOp) -> Result<ActionResult> {
    match op {
        ClipboardOp::Read => {
            let text = clipboard_read_text()?;
            Ok(ActionResult {
                ok: true,
                message: None,
                data: Some(serde_json::json!({ "text": text })),
            })
        }
        ClipboardOp::Write { text } => {
            clipboard_write_text(text)?;
            Ok(ActionResult::ok())
        }
        ClipboardOp::Copy => act_press(pinned, "Cmd+C"),
        ClipboardOp::Paste => act_press(pinned, "Cmd+V"),
    }
}

fn clipboard_read_text() -> Result<String> {
    // Shelling out to pbpaste avoids pulling Cocoa/objc into surface-ax just
    // for two clipboard verbs. pbpaste ships with every macOS install.
    let output = Command::new("/usr/bin/pbpaste")
        .output()
        .map_err(Error::Io)?;
    if !output.status.success() {
        return Err(Error::Action {
            action: "clipboard_read".into(),
            reason: format!("pbpaste exited with {:?}", output.status.code()),
        });
    }
    String::from_utf8(output.stdout).map_err(|err| Error::Action {
        action: "clipboard_read".into(),
        reason: format!("clipboard text was not valid UTF-8: {err}"),
    })
}

fn clipboard_write_text(text: &str) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new("/usr/bin/pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(Error::Io)?;
    {
        let stdin = child.stdin.as_mut().ok_or_else(|| Error::Action {
            action: "clipboard_write".into(),
            reason: "pbcopy stdin was unavailable".into(),
        })?;
        stdin.write_all(text.as_bytes()).map_err(Error::Io)?;
    }
    let status = child.wait().map_err(Error::Io)?;
    if !status.success() {
        return Err(Error::Action {
            action: "clipboard_write".into(),
            reason: format!("pbcopy exited with {:?}", status.code()),
        });
    }
    Ok(())
}

fn element_center(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
    action: &str,
) -> Result<CGPoint> {
    let element = resolve_element(pinned, refs, ref_id, action)?;
    let logical = bounds(element);
    // SAFETY: `resolve_element` returns a retained AX element.
    unsafe { CFRelease(element.cast::<c_void>()) };
    let logical = logical.ok_or_else(|| Error::Action {
        action: action.into(),
        reason: format!("ref {} has no AX bounds", ref_id.0),
    })?;
    Ok(CGPoint {
        x: logical.x + logical.w / 2.0,
        y: logical.y + logical.h / 2.0,
    })
}

fn point_from(x: i32, y: i32) -> CGPoint {
    CGPoint {
        x: f64::from(x),
        y: f64::from(y),
    }
}

fn mouse_button_down(button: MouseButton) -> (u32, u32) {
    match button {
        MouseButton::Left => (CG_EVENT_LEFT_MOUSE_DOWN, CG_MOUSE_BUTTON_LEFT),
        MouseButton::Right => (CG_EVENT_RIGHT_MOUSE_DOWN, CG_MOUSE_BUTTON_RIGHT),
        MouseButton::Middle => (CG_EVENT_OTHER_MOUSE_DOWN, CG_MOUSE_BUTTON_CENTER),
    }
}

fn mouse_button_up(button: MouseButton) -> (u32, u32) {
    match button {
        MouseButton::Left => (CG_EVENT_LEFT_MOUSE_UP, CG_MOUSE_BUTTON_LEFT),
        MouseButton::Right => (CG_EVENT_RIGHT_MOUSE_UP, CG_MOUSE_BUTTON_RIGHT),
        MouseButton::Middle => (CG_EVENT_OTHER_MOUSE_UP, CG_MOUSE_BUTTON_CENTER),
    }
}

fn post_mouse(
    event_type: u32,
    position: CGPoint,
    button: u32,
    click_state: Option<i64>,
) -> Result<()> {
    // SAFETY: null source asks CG for the default event source.
    let event = unsafe { CGEventCreateMouseEvent(std::ptr::null(), event_type, position, button) };
    if event.is_null() {
        return Err(Error::Action {
            action: "mouse".into(),
            reason: "CGEventCreateMouseEvent returned null".into(),
        });
    }
    if let Some(state) = click_state {
        // SAFETY: event is a valid CGEvent.
        unsafe { CGEventSetIntegerValueField(event, CG_MOUSE_EVENT_CLICK_STATE, state) };
    }
    post_event(event);
    Ok(())
}

fn post_scroll(_anchor: CGPoint, dx: i32, dy: i32) -> Result<()> {
    // Apple's macOS variadic ABI passes args in registers same as fixed args,
    // so calling with wheelCount=2 and two i32 deltas through this fixed-arity
    // declaration matches what the C variadic call would emit. wheel1 is the
    // vertical axis, wheel2 is horizontal.
    // SAFETY: null source asks CG for the default event source.
    let event =
        unsafe { CGEventCreateScrollWheelEvent(std::ptr::null(), CG_SCROLL_UNIT_PIXEL, 2, dy, dx) };
    if event.is_null() {
        return Err(Error::Action {
            action: "scroll".into(),
            reason: "CGEventCreateScrollWheelEvent returned null".into(),
        });
    }
    post_event(event);
    Ok(())
}

fn raise_pinned_app(pinned: Option<AxPinnedWindow>) {
    // Mouse events go to whatever window is under the cursor at the OS level,
    // not to a particular AXUIElement. To make actions on the pinned window
    // deterministic we raise it first; ignore failures because the frontmost
    // app may already be us.
    if let Some(pinned) = pinned {
        let _ = focus_window(&window_id(pinned));
    }
}

fn press_until_checked(element: AXUIElementRef, desired: bool, action: &str) -> Result<()> {
    if checked_state(element).is_some_and(|state| state_matches(state, desired)) {
        return Ok(());
    }
    for _ in 0..3 {
        perform_action(element, kAXPressAction, action)?;
        std::thread::sleep(std::time::Duration::from_millis(30));
        if checked_state(element).is_some_and(|state| state_matches(state, desired)) {
            return Ok(());
        }
    }
    Err(Error::Action {
        action: action.into(),
        reason: "control did not reach requested check state".into(),
    })
}

fn state_matches(state: Checked, desired: bool) -> bool {
    matches!(
        (state, desired),
        (Checked::True, true) | (Checked::False, false)
    )
}

fn act_type(pinned: Option<AxPinnedWindow>, text: &str) -> Result<ActionResult> {
    if text.is_empty() {
        return Ok(ActionResult::ok());
    }
    let app = app_for_keyboard(pinned, "type")?;
    let result = send_text(app, text);
    // SAFETY: `app_for_keyboard` returns a create-rule AX application object.
    unsafe { CFRelease(app.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_press(pinned: Option<AxPinnedWindow>, keys: &str) -> Result<ActionResult> {
    let chord = parse_chord(keys).map_err(|reason| Error::Action {
        action: "press".into(),
        reason,
    })?;
    let app = app_for_keyboard(pinned, "press")?;
    let result = send_chord(app, &chord, "press");
    // SAFETY: `app_for_keyboard` returns a create-rule AX application object.
    unsafe { CFRelease(app.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn act_key(
    pinned: Option<AxPinnedWindow>,
    key: &str,
    key_down: bool,
    action: &str,
) -> Result<ActionResult> {
    let key_code = key_code_from_name(key).ok_or_else(|| Error::Action {
        action: action.into(),
        reason: format!("unknown key name: {key:?}"),
    })?;
    let app = app_for_keyboard(pinned, action)?;
    let result = post_virtual_key(key_code, key_down, 0, action);
    // SAFETY: `app_for_keyboard` returns a create-rule AX application object.
    unsafe { CFRelease(app.cast::<c_void>()) };
    result.map(|()| ActionResult::ok())
}

fn resolve_element(
    pinned: Option<AxPinnedWindow>,
    refs: Option<&RefMap>,
    ref_id: &RefId,
    action: &str,
) -> Result<AXUIElementRef> {
    let refs = refs.ok_or_else(|| Error::Action {
        action: action.into(),
        reason: "no prior snapshot - call snapshot before act".into(),
    })?;
    let entry = refs
        .get(ref_id)
        .cloned()
        .ok_or_else(|| Error::RefNotFound(ref_id.0.clone()))?;
    let pinned = pinned.ok_or_else(|| Error::Action {
        action: action.into(),
        reason: "no prior snapshot - call snapshot before act".into(),
    })?;
    let root = window_by_index(pinned.pid, pinned.index)?;
    let mut nth_seen = HashMap::new();
    let element = find_element(root, &entry, &mut nth_seen);
    // SAFETY: `root` is returned by a copy-rule helper.
    unsafe { CFRelease(root.cast::<c_void>()) };
    element.ok_or_else(|| Error::Action {
        action: action.into(),
        reason: format!(
            "could not rediscover ref {} as role {:?} name {:?} nth {}",
            ref_id.0, entry.role, entry.name, entry.nth
        ),
    })
}

fn app_for_keyboard(pinned: Option<AxPinnedWindow>, action: &str) -> Result<AXUIElementRef> {
    let pinned = pinned.ok_or_else(|| Error::Action {
        action: action.into(),
        reason: "no prior snapshot - call snapshot before keyboard input".into(),
    })?;
    focus_window(&window_id(pinned))?;
    let pid = i32::try_from(pinned.pid).map_err(|_| Error::Surface("pid out of range".into()))?;
    // SAFETY: create rule returns an AX application object for pid or null.
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err(Error::Surface(format!(
            "AX application for pid {pid} was null"
        )));
    }
    Ok(app)
}

fn send_text(app: AXUIElementRef, text: &str) -> Result<()> {
    let _ = app;
    for unit in text.encode_utf16() {
        post_unicode_unit(unit)?;
    }
    Ok(())
}

fn send_chord(_app: AXUIElementRef, chord: &Chord, action: &str) -> Result<()> {
    let mut flags = 0;
    for modifier in &chord.modifiers {
        flags |= modifier.flag;
        post_virtual_key(modifier.key, true, flags, action)?;
    }
    post_virtual_key(chord.key, true, flags, action)?;
    post_virtual_key(chord.key, false, flags, action)?;
    for modifier in chord.modifiers.iter().rev() {
        post_virtual_key(modifier.key, false, flags, action)?;
        flags &= !modifier.flag;
    }
    Ok(())
}

fn post_unicode_unit(unit: u16) -> Result<()> {
    let down = create_keyboard_event(0, true, "type")?;
    // SAFETY: down is a valid CGEvent and unit points to one UTF-16 code unit.
    unsafe { CGEventKeyboardSetUnicodeString(down, 1, &raw const unit) };
    post_event(down);

    let up = create_keyboard_event(0, false, "type")?;
    // SAFETY: up is a valid CGEvent and unit points to one UTF-16 code unit.
    unsafe { CGEventKeyboardSetUnicodeString(up, 1, &raw const unit) };
    post_event(up);
    Ok(())
}

fn post_virtual_key(
    virtual_key: CGKeyCode,
    key_down: bool,
    flags: u64,
    action: &str,
) -> Result<()> {
    let event = create_keyboard_event(virtual_key, key_down, action)?;
    // SAFETY: event is a valid CGEvent and flags are CoreGraphics event flags.
    unsafe { CGEventSetFlags(event, flags) };
    post_event(event);
    Ok(())
}

fn create_keyboard_event(
    virtual_key: CGKeyCode,
    key_down: bool,
    action: &str,
) -> Result<CGEventRef> {
    // SAFETY: null source requests the default event source; virtual_key is a Carbon key code.
    let event = unsafe { CGEventCreateKeyboardEvent(std::ptr::null(), virtual_key, key_down) };
    if event.is_null() {
        Err(Error::Action {
            action: action.into(),
            reason: "CGEventCreateKeyboardEvent returned null".into(),
        })
    } else {
        Ok(event)
    }
}

fn post_event(event: CGEventRef) {
    // SAFETY: event is a valid CGEvent. `CGEventPost` does not take ownership.
    unsafe { CGEventPost(K_CG_HID_EVENT_TAP, event) };
    // SAFETY: CGEvent follows Core Foundation create/copy ownership rules.
    unsafe { CFRelease(event.cast::<c_void>()) };
}

struct Chord {
    modifiers: Vec<ModifierKey>,
    key: CGKeyCode,
}

struct ModifierKey {
    key: CGKeyCode,
    flag: u64,
}

fn parse_chord(keys: &str) -> std::result::Result<Chord, String> {
    let tokens: Vec<&str> = keys.split('+').map(str::trim).collect();
    if tokens.is_empty() || tokens.iter().any(|token| token.is_empty()) {
        return Err(format!("malformed key chord: {keys:?}"));
    }
    let (main, mods) = tokens
        .split_last()
        .ok_or_else(|| format!("empty key chord: {keys:?}"))?;
    let key = key_code_from_name(main).ok_or_else(|| format!("unknown key name: {main:?}"))?;
    let modifiers: Vec<ModifierKey> = mods
        .iter()
        .map(|token| modifier_key_code(token).ok_or_else(|| format!("unknown modifier: {token:?}")))
        .collect::<std::result::Result<_, _>>()?;
    Ok(Chord { modifiers, key })
}

fn modifier_key_code(name: &str) -> Option<ModifierKey> {
    match name.trim().to_ascii_lowercase().as_str() {
        "cmd" | "command" | "meta" | "super" | "win" => Some(ModifierKey {
            key: 0x37,
            flag: CG_FLAG_COMMAND,
        }),
        "shift" => Some(ModifierKey {
            key: 0x38,
            flag: CG_FLAG_SHIFT,
        }),
        "alt" | "option" => Some(ModifierKey {
            key: 0x3a,
            flag: CG_FLAG_OPTION,
        }),
        "ctrl" | "control" => Some(ModifierKey {
            key: 0x3b,
            flag: CG_FLAG_CONTROL,
        }),
        _ => None,
    }
}

fn key_code_from_name(name: &str) -> Option<CGKeyCode> {
    let lower = name.trim().to_ascii_lowercase();
    if lower.len() == 1 {
        let ch = lower.chars().next()?;
        if let Some(code) = ansi_key_code(ch) {
            return Some(code);
        }
    }
    match lower.as_str() {
        "cmd" | "command" | "meta" | "super" | "win" => Some(0x37),
        "shift" => Some(0x38),
        "alt" | "option" => Some(0x3a),
        "ctrl" | "control" => Some(0x3b),
        "enter" | "return" => Some(0x24),
        "tab" => Some(0x30),
        "space" | "spacebar" => Some(0x31),
        "backspace" => Some(0x33),
        "esc" | "escape" => Some(0x35),
        "delete" | "del" => Some(0x75),
        "home" => Some(0x73),
        "end" => Some(0x77),
        "pageup" | "pgup" | "page_up" => Some(0x74),
        "pagedown" | "pgdn" | "page_down" => Some(0x79),
        "left" | "arrowleft" => Some(0x7b),
        "right" | "arrowright" => Some(0x7c),
        "down" | "arrowdown" => Some(0x7d),
        "up" | "arrowup" => Some(0x7e),
        _ => function_key_code(&lower),
    }
}

fn function_key_code(lower: &str) -> Option<CGKeyCode> {
    let rest = lower.strip_prefix('f')?;
    let n = rest.parse::<u16>().ok()?;
    match n {
        1 => Some(0x7a),
        2 => Some(0x78),
        3 => Some(0x63),
        4 => Some(0x76),
        5 => Some(0x60),
        6 => Some(0x61),
        7 => Some(0x62),
        8 => Some(0x64),
        9 => Some(0x65),
        10 => Some(0x6d),
        11 => Some(0x67),
        12 => Some(0x6f),
        13 => Some(0x69),
        14 => Some(0x6b),
        15 => Some(0x71),
        16 => Some(0x6a),
        17 => Some(0x40),
        18 => Some(0x4f),
        19 => Some(0x50),
        20 => Some(0x5a),
        _ => None,
    }
}

fn ansi_key_code(ch: char) -> Option<CGKeyCode> {
    match ch {
        'a' => Some(0x00),
        's' => Some(0x01),
        'd' => Some(0x02),
        'f' => Some(0x03),
        'h' => Some(0x04),
        'g' => Some(0x05),
        'z' => Some(0x06),
        'x' => Some(0x07),
        'c' => Some(0x08),
        'v' => Some(0x09),
        'b' => Some(0x0b),
        'q' => Some(0x0c),
        'w' => Some(0x0d),
        'e' => Some(0x0e),
        'r' => Some(0x0f),
        'y' => Some(0x10),
        't' => Some(0x11),
        '1' => Some(0x12),
        '2' => Some(0x13),
        '3' => Some(0x14),
        '4' => Some(0x15),
        '6' => Some(0x16),
        '5' => Some(0x17),
        '=' => Some(0x18),
        '9' => Some(0x19),
        '7' => Some(0x1a),
        '-' => Some(0x1b),
        '8' => Some(0x1c),
        '0' => Some(0x1d),
        ']' => Some(0x1e),
        'o' => Some(0x1f),
        'u' => Some(0x20),
        '[' => Some(0x21),
        'i' => Some(0x22),
        'p' => Some(0x23),
        'l' => Some(0x25),
        'j' => Some(0x26),
        '\'' => Some(0x27),
        'k' => Some(0x28),
        ';' => Some(0x29),
        '\\' => Some(0x2a),
        ',' => Some(0x2b),
        '/' => Some(0x2c),
        'n' => Some(0x2d),
        'm' => Some(0x2e),
        '.' => Some(0x2f),
        '`' => Some(0x32),
        _ => None,
    }
}

fn find_element(
    element: AXUIElementRef,
    entry: &RefEntry,
    nth_seen: &mut HashMap<(Role, String), usize>,
) -> Option<AXUIElementRef> {
    let role_raw = string_attr(element, kAXRoleAttribute).unwrap_or_else(|| "AXUnknown".into());
    let subrole = string_attr(element, kAXSubroleAttribute);
    let role = map_role(&role_raw, subrole.as_deref());
    let name = string_attr(element, kAXTitleAttribute)
        .or_else(|| string_attr(element, kAXDescriptionAttribute))
        .or_else(|| value_string(element))
        .unwrap_or_default();

    if is_ref_target(&role) {
        let key = (role.clone(), name.clone());
        let counter = nth_seen.entry(key).or_insert(0);
        let nth = *counter;
        *counter += 1;
        if role == entry.role && name == entry.name && nth == entry.nth {
            // SAFETY: retain the matched element so it survives any parent array release.
            unsafe { CFRetain(element.cast::<c_void>()) };
            return Some(element);
        }
    }

    let array_ref = array_attr(element, kAXChildrenAttribute)?;
    // SAFETY: `array_ref` is a valid CFArray copied from AX.
    let count = unsafe { CFArrayGetCount(array_ref) };
    for idx in 0..count {
        // SAFETY: index is within CFArray bounds.
        let child = unsafe { CFArrayGetValueAtIndex(array_ref, idx) };
        if !child.is_null() {
            if let Some(found) = find_element(child as AXUIElementRef, entry, nth_seen) {
                // SAFETY: release the copy-rule array after retaining the match.
                unsafe { CFRelease(array_ref.cast::<c_void>()) };
                return Some(found);
            }
        }
    }
    // SAFETY: release the copy-rule array after traversal.
    unsafe { CFRelease(array_ref.cast::<c_void>()) };
    None
}

fn is_ref_target(role: &Role) -> bool {
    role.is_interactive() || role.is_content()
}

fn perform_action(element: AXUIElementRef, ax_action: &str, action: &str) -> Result<()> {
    let action_ref = cf_string(ax_action)
        .ok_or_else(|| Error::Surface(format!("failed to allocate AX action {ax_action}")))?;
    // SAFETY: element and action_ref are valid AX/Core Foundation refs.
    let err = unsafe { AXUIElementPerformAction(element, action_ref) };
    // SAFETY: release the local CFString created for the action name.
    unsafe { CFRelease(action_ref.cast::<c_void>()) };
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        Err(Error::Action {
            action: action.into(),
            reason: format!("AX action {ax_action} failed with AX error {err}"),
        })
    }
}

fn set_bool_attr(element: AXUIElementRef, attr: &str, action: &str) -> Result<()> {
    let attr_ref = cf_string(attr)
        .ok_or_else(|| Error::Surface(format!("failed to allocate AX attribute {attr}")))?;
    // SAFETY: element, attr_ref, and kCFBooleanTrue are valid Core Foundation refs.
    let err =
        unsafe { AXUIElementSetAttributeValue(element, attr_ref, kCFBooleanTrue.cast::<c_void>()) };
    // SAFETY: release the local CFString created for the attribute name.
    unsafe { CFRelease(attr_ref.cast::<c_void>()) };
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        Err(Error::Action {
            action: action.into(),
            reason: format!("setting AX attribute {attr} failed with AX error {err}"),
        })
    }
}

fn set_string_attr(element: AXUIElementRef, attr: &str, value: &str, action: &str) -> Result<()> {
    let attr_ref = cf_string(attr)
        .ok_or_else(|| Error::Surface(format!("failed to allocate AX attribute {attr}")))?;
    let value_ref = cf_string(value)
        .ok_or_else(|| Error::Surface("failed to allocate AX string value".into()))?;
    // SAFETY: element, attr_ref, and value_ref are valid AX/Core Foundation refs.
    let err =
        unsafe { AXUIElementSetAttributeValue(element, attr_ref, value_ref.cast::<c_void>()) };
    // SAFETY: release local CFStrings created for this call.
    unsafe {
        CFRelease(attr_ref.cast::<c_void>());
        CFRelease(value_ref.cast::<c_void>());
    }
    if err == kAXErrorSuccess {
        Ok(())
    } else {
        Err(Error::Action {
            action: action.into(),
            reason: format!("setting AX attribute {attr} failed with AX error {err}"),
        })
    }
}

fn action_name(action: &Action) -> &'static str {
    match action {
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
        Action::Check { .. } => "check",
        Action::Uncheck { .. } => "uncheck",
        Action::Toggle { .. } => "toggle",
        Action::Clear { .. } => "clear",
        Action::Clipboard { .. } => "clipboard",
        Action::Mouse { .. } => "mouse",
        Action::Highlight { .. } => "highlight",
        Action::ScrollIntoView { .. } => "scroll_into_view",
        Action::Wait { .. } => "wait",
        Action::SwitchApp { .. } => "switch_app",
        Action::FocusWindow { .. } => "focus_window",
        Action::Screenshot { .. } => "screenshot",
    }
}

fn first_array_element(element: AXUIElementRef, attr: &str) -> Option<CFTypeRef> {
    let array = array_attr(element, attr)?;
    // SAFETY: `array` is a valid CFArray copied from AX.
    let count = unsafe { CFArrayGetCount(array) };
    let value = if count > 0 {
        // SAFETY: index 0 exists when count > 0.
        let value = unsafe { CFArrayGetValueAtIndex(array, 0) };
        if !value.is_null() {
            // SAFETY: retain the element so it remains valid after releasing the array.
            unsafe { CFRetain(value.cast::<c_void>()) };
        }
        value
    } else {
        std::ptr::null()
    };
    // SAFETY: release the copy-rule array after reading the first pointer.
    unsafe { CFRelease(array.cast::<c_void>()) };
    if value.is_null() {
        None
    } else {
        Some(value)
    }
}

fn app_windows(pid: u32) -> Result<CFArrayRef> {
    let pid_i32 = i32::try_from(pid).map_err(|_| Error::Surface("pid out of range".into()))?;
    // SAFETY: create rule returns an AX application object for pid or null.
    let app = unsafe { AXUIElementCreateApplication(pid_i32) };
    if app.is_null() {
        return Err(Error::Surface(format!(
            "AX application for pid {pid_i32} was null"
        )));
    }
    let windows = array_attr(app, kAXWindowsAttribute);
    // SAFETY: release app after copying the windows array.
    unsafe { CFRelease(app.cast::<c_void>()) };
    windows.ok_or_else(|| Error::Surface(format!("pid {pid} has no AX windows")))
}

fn window_by_index(pid: u32, index: usize) -> Result<AXUIElementRef> {
    let array = app_windows(pid)?;
    // SAFETY: `array` is a valid CFArray copied from AX.
    let count = unsafe { CFArrayGetCount(array) };
    let idx =
        isize::try_from(index).map_err(|_| Error::Surface("window index out of range".into()))?;
    if idx < 0 || idx >= count {
        // SAFETY: release copy-rule array before returning.
        unsafe { CFRelease(array.cast::<c_void>()) };
        return Err(Error::Surface(format!(
            "pid {pid} has no AX window at index {index}"
        )));
    }
    let window = array_element_retained(array, idx)
        .ok_or_else(|| Error::Surface(format!("pid {pid} window {index} was null")))?;
    // SAFETY: release copy-rule array. The returned element was retained.
    unsafe { CFRelease(array.cast::<c_void>()) };
    Ok(window.cast_mut().cast())
}

fn window_index(pid: u32, target: AXUIElementRef) -> Option<usize> {
    let array = app_windows(pid).ok()?;
    // SAFETY: `array` is a valid CFArray copied from AX.
    let count = unsafe { CFArrayGetCount(array) };
    let target_title = string_attr(target, kAXTitleAttribute);
    let target_bounds = bounds(target);
    let mut found = None;
    for idx in 0..count {
        let window = unsafe { CFArrayGetValueAtIndex(array, idx) };
        if window.is_null() {
            continue;
        }
        if std::ptr::eq(window, target.cast::<c_void>()) {
            found = usize::try_from(idx).ok();
            break;
        }
        let window_ref = window.cast_mut().cast();
        let same_title = string_attr(window_ref, kAXTitleAttribute) == target_title;
        let same_bounds = bounds(window_ref) == target_bounds;
        if same_title && same_bounds {
            found = usize::try_from(idx).ok();
            break;
        }
    }
    // SAFETY: release copy-rule array.
    unsafe { CFRelease(array.cast::<c_void>()) };
    found
}

fn array_element_retained(array: CFArrayRef, idx: isize) -> Option<CFTypeRef> {
    // SAFETY: caller supplies an in-bounds index for a valid CFArray.
    let value = unsafe { CFArrayGetValueAtIndex(array, idx) };
    if value.is_null() {
        None
    } else {
        // SAFETY: retain the borrowed array element so it survives array release.
        unsafe { CFRetain(value.cast::<c_void>()) };
        Some(value)
    }
}

fn window_id(pinned: AxPinnedWindow) -> String {
    format!("pid:{}:window:{}", pinned.pid, pinned.index)
}

fn parse_window_id(id: &str) -> Result<AxPinnedWindow> {
    let parts = id.split(':').collect::<Vec<_>>();
    if parts.len() != 4 || parts[0] != "pid" || parts[2] != "window" {
        return Err(Error::Action {
            action: "focus_window".into(),
            reason: format!("invalid AX window id {id:?}; expected pid:<pid>:window:<index>"),
        });
    }
    let pid = parts[1].parse::<u32>().map_err(|_| Error::Action {
        action: "focus_window".into(),
        reason: format!("invalid AX window id {id:?}; pid was not a u32"),
    })?;
    let index = parts[3].parse::<usize>().map_err(|_| Error::Action {
        action: "focus_window".into(),
        reason: format!("invalid AX window id {id:?}; window index was not a usize"),
    })?;
    Ok(AxPinnedWindow { pid, index })
}

fn string_attr(element: AXUIElementRef, attr: &str) -> Option<String> {
    let value = element_attr(element, attr)?;
    let out = cf_string_to_string(value.cast());
    // SAFETY: release the copy-rule attr value.
    unsafe { CFRelease(value) };
    out
}

fn value_string(element: AXUIElementRef) -> Option<String> {
    string_attr(element, kAXValueAttribute)
}

fn bool_attr(element: AXUIElementRef, attr: &str) -> Option<bool> {
    let value = element_attr(element, attr)?;
    // SAFETY: `value` is a Core Foundation object.
    let is_bool = unsafe { CFGetTypeID(value) == CFBooleanGetTypeID() };
    let out = if is_bool {
        // SAFETY: type was verified as CFBoolean.
        Some(unsafe { CFBooleanGetValue(value.cast::<core_foundation_sys::number::__CFBoolean>()) })
    } else {
        None
    };
    // SAFETY: release the copy-rule attr value.
    unsafe { CFRelease(value) };
    out
}

fn checked_state_for_role(role: &Role, element: AXUIElementRef) -> Option<Checked> {
    role_might_be_checkable(role).then(|| checked_state(element))?
}

fn role_might_be_checkable(role: &Role) -> bool {
    matches!(
        role,
        Role::Checkbox
            | Role::Radio
            | Role::Switch
            | Role::MenuItemCheckbox
            | Role::MenuItemRadio
            | Role::MenuItem
            | Role::Button
            | Role::Unknown(_)
    )
}

fn checked_state(element: AXUIElementRef) -> Option<Checked> {
    let value = element_attr(element, kAXValueAttribute)?;
    let out = checked_from_cf_value(value);
    // SAFETY: release the copy-rule attr value.
    unsafe { CFRelease(value) };
    out
}

fn checked_from_cf_value(value: CFTypeRef) -> Option<Checked> {
    // SAFETY: `value` is a Core Foundation object.
    let type_id = unsafe { CFGetTypeID(value) };
    // SAFETY: Core Foundation type id accessors have no side effects.
    if type_id == unsafe { CFBooleanGetTypeID() } {
        // SAFETY: type was verified as CFBoolean.
        let checked =
            unsafe { CFBooleanGetValue(value.cast::<core_foundation_sys::number::__CFBoolean>()) };
        return Some(if checked {
            Checked::True
        } else {
            Checked::False
        });
    }
    // SAFETY: Core Foundation type id accessors have no side effects.
    if type_id == unsafe { CFNumberGetTypeID() } {
        let mut state = 0;
        // SAFETY: type was verified as CFNumber and `state` is a valid out pointer.
        let ok = unsafe {
            CFNumberGetValue(
                value.cast::<core_foundation_sys::number::__CFNumber>(),
                kCFNumberIntType,
                (&raw mut state).cast(),
            )
        };
        if ok {
            return match state {
                0 => Some(Checked::False),
                1 => Some(Checked::True),
                2 => Some(Checked::Mixed),
                _ => None,
            };
        }
    }
    // SAFETY: Core Foundation type id accessors have no side effects.
    if type_id == unsafe { CFStringGetTypeID() } {
        return checked_from_string(&cf_string_to_string(value.cast())?);
    }
    None
}

fn checked_from_string(value: &str) -> Option<Checked> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "checked" => Some(Checked::True),
        "0" | "false" | "no" | "off" | "unchecked" => Some(Checked::False),
        "2" | "mixed" | "indeterminate" => Some(Checked::Mixed),
        _ => None,
    }
}

fn ax_value_attr<T: Default>(element: AXUIElementRef, attr: &str, ty: u32) -> Option<T> {
    let value = element_attr(element, attr)?;
    let mut out = T::default();
    // SAFETY: AXValueGetValue writes the requested C struct when the attr is that AXValue type.
    let ok = unsafe { AXValueGetValue(value as AXValueRef, ty, (&raw mut out).cast()) };
    // SAFETY: release the copy-rule attr value.
    unsafe { CFRelease(value) };
    ok.then_some(out)
}

fn array_attr(element: AXUIElementRef, attr: &str) -> Option<CFArrayRef> {
    let value = element_attr(element, attr)?;
    Some(value.cast::<core_foundation_sys::array::__CFArray>())
}

fn element_attr(element: AXUIElementRef, attr: &str) -> Option<CFTypeRef> {
    let attr = cf_string(attr)?;
    let mut value: CFTypeRef = std::ptr::null();
    // SAFETY: element and attr are AX/Core Foundation refs; value is an out pointer.
    let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &raw mut value) };
    // SAFETY: release the local CFString created for the attribute name.
    unsafe { CFRelease(attr.cast::<c_void>()) };
    if err == kAXErrorSuccess && !value.is_null() {
        Some(value)
    } else {
        None
    }
}

fn element_pid(element: AXUIElementRef) -> Option<u32> {
    let mut pid = 0;
    // SAFETY: element is an AX element and pid is a valid out pointer.
    let err = unsafe { AXUIElementGetPid(element, &raw mut pid) };
    (err == kAXErrorSuccess)
        .then(|| u32::try_from(pid).ok())
        .flatten()
}

fn process_name(pid: u32) -> Option<String> {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let path = text.trim();
    if path.is_empty() {
        return None;
    }
    Some(
        std::path::Path::new(path)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(path)
            .to_owned(),
    )
}

fn process_ids_by_name(name: &str) -> Result<Vec<u32>> {
    let needle = name.to_lowercase();
    let pids: Vec<u32> = process_rows()?
        .into_iter()
        .filter_map(|(pid, path)| {
            let process = std::path::Path::new(&path)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or(&path)
                .to_lowercase();
            (process == needle).then_some(pid)
        })
        .collect();
    if pids.is_empty() {
        Err(Error::Surface(format!("no process matched name {name:?}")))
    } else {
        Ok(pids)
    }
}

fn process_ids() -> Result<Vec<u32>> {
    Ok(process_rows()?.into_iter().map(|(pid, _)| pid).collect())
}

fn process_rows() -> Result<Vec<(u32, String)>> {
    let output = Command::new("/bin/ps")
        .args(["-axo", "pid=,comm="])
        .output()
        .map_err(Error::Io)?;
    if !output.status.success() {
        return Err(Error::Surface("ps failed while listing processes".into()));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|err| Error::Surface(format!("ps output was not UTF-8: {err}")))?;
    let rows = text
        .lines()
        .filter_map(parse_process_row)
        .collect::<Vec<_>>();
    Ok(rows)
}

fn parse_process_row(line: &str) -> Option<(u32, String)> {
    let trimmed = line.trim();
    let split = trimmed.find(char::is_whitespace)?;
    let (pid, rest) = trimmed.split_at(split);
    let path = rest.trim();
    if path.is_empty() {
        return None;
    }
    Some((pid.parse().ok()?, path.to_owned()))
}

fn cf_string(attr: &str) -> Option<CFStringRef> {
    let cstr = CString::new(attr).ok()?;
    // SAFETY: allocator and UTF-8 C string are valid; returns a create-rule CFString.
    let cf = unsafe {
        CFStringCreateWithCString(kCFAllocatorDefault, cstr.as_ptr(), kCFStringEncodingUTF8)
    };
    (!cf.is_null()).then_some(cf)
}

fn cf_string_to_string(value: CFStringRef) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: `value` is a Core Foundation object.
    let is_string = unsafe { CFGetTypeID(value.cast::<c_void>()) == CFStringGetTypeID() };
    if !is_string {
        return None;
    }
    // SAFETY: `value` is a valid CFString.
    let len = unsafe { core_foundation_sys::string::CFStringGetLength(value) };
    // SAFETY: asks CF for the max bytes needed for UTF-8 plus null terminator.
    let max_len = unsafe { CFStringGetMaximumSizeForEncoding(len, kCFStringEncodingUTF8) } + 1;
    let Ok(buf_len) = usize::try_from(max_len) else {
        return None;
    };
    let mut buf = vec![0_u8; buf_len];
    // SAFETY: `buf` is writable for `buf_len` bytes and `value` is a CFString.
    let ok = unsafe {
        CFStringGetCString(
            value,
            buf.as_mut_ptr().cast::<c_char>(),
            max_len,
            kCFStringEncodingUTF8,
        )
    };
    if ok == 0 {
        return None;
    }
    let nul = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..nul].to_vec()).ok()
}

// ---- screenshot --------------------------------------------------------------

fn act_screenshot(
    pinned: Option<AxPinnedWindow>,
    snapshot: Option<&Snapshot>,
    region: Option<&Region>,
    target: Option<&ScreenshotTarget>,
    annotated: bool,
) -> Result<ActionResult> {
    let captured = match target {
        Some(ScreenshotTarget::Window) => capture_pinned_window(pinned)?,
        Some(ScreenshotTarget::Desktop) => capture_desktop()?,
        Some(ScreenshotTarget::Region { region }) => capture_screen_region(region)?,
        Some(ScreenshotTarget::Ref { ref_id }) => capture_ref(snapshot, ref_id)?,
        None => {
            if let Some(region) = region {
                capture_screen_region(region)?
            } else {
                capture_pinned_window(pinned)?
            }
        }
    };

    let mut image = captured.image;
    let origin = captured.origin;
    if annotated {
        if let Some(snap) = snapshot {
            let dpi_scale = if captured.logical_size.0 > 0.0 {
                f64::from(image.width) / captured.logical_size.0
            } else {
                1.0
            };
            let mut labels = Vec::new();
            collect_annotation_labels(
                &snap.root,
                origin,
                dpi_scale,
                image.width,
                image.height,
                &mut labels,
            );
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let label_scale = dpi_scale.round().clamp(1.0, 8.0) as i32;
            draw_annotations(&mut image, &labels, label_scale);
        } else {
            return Err(Error::Action {
                action: "screenshot".into(),
                reason: "annotated screenshot requires a prior snapshot".into(),
            });
        }
    }

    encode_screenshot_result(&image, annotated)
}

struct CapturedScreenshot {
    image: CapturedImage,
    /// Screen-space origin of the image's top-left in logical points.
    origin: (f64, f64),
    /// Logical-point size of the captured area, used to compute the
    /// physical-pixel scale for annotation.
    logical_size: (f64, f64),
}

struct CapturedImage {
    width: u32,
    height: u32,
    /// `width * height * 4` bytes. Row-major, top-down, RGBA.
    pixels: Vec<u8>,
}

fn capture_pinned_window(pinned: Option<AxPinnedWindow>) -> Result<CapturedScreenshot> {
    let pinned = pinned.ok_or_else(|| Error::Action {
        action: "screenshot".into(),
        reason: "no pinned window - call snapshot before screenshot".into(),
    })?;
    let window = window_by_index(pinned.pid, pinned.index)?;
    let title = string_attr(window, kAXTitleAttribute);
    let logical = bounds(window).ok_or_else(|| Error::Action {
        action: "screenshot".into(),
        reason: "pinned window has no AX bounds".into(),
    })?;
    // SAFETY: `window` is a copy-rule AX element returned above.
    unsafe { CFRelease(window.cast::<c_void>()) };

    let cg_id = cg_window_id_for(pinned.pid, title.as_deref(), logical)?;
    let image = capture_window_by_id(cg_id, logical)?;
    Ok(CapturedScreenshot {
        image,
        origin: (logical.x, logical.y),
        logical_size: (logical.w, logical.h),
    })
}

fn capture_desktop() -> Result<CapturedScreenshot> {
    let image = create_image_for(
        cg_rect_infinite(),
        CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
        CG_NULL_WINDOW_ID,
        CG_WINDOW_IMAGE_BEST_RESOLUTION,
    )?;
    let logical = (f64::from(image.width), f64::from(image.height));
    Ok(CapturedScreenshot {
        image,
        origin: (0.0, 0.0),
        logical_size: logical,
    })
}

fn capture_screen_region(region: &Region) -> Result<CapturedScreenshot> {
    let rect = CGRect {
        origin: CGPoint {
            x: f64::from(region.x),
            y: f64::from(region.y),
        },
        size: CGSize {
            width: f64::from(region.w.max(1)),
            height: f64::from(region.h.max(1)),
        },
    };
    let image = create_image_for(
        rect,
        CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
        CG_NULL_WINDOW_ID,
        CG_WINDOW_IMAGE_BEST_RESOLUTION,
    )?;
    Ok(CapturedScreenshot {
        image,
        origin: (rect.origin.x, rect.origin.y),
        logical_size: (rect.size.width, rect.size.height),
    })
}

fn capture_ref(snapshot: Option<&Snapshot>, ref_id: &RefId) -> Result<CapturedScreenshot> {
    let snapshot = snapshot.ok_or_else(|| Error::Action {
        action: "screenshot".into(),
        reason: "no prior snapshot - call snapshot before screenshot --target ref".into(),
    })?;
    let node = snapshot
        .node_by_ref(ref_id)
        .ok_or_else(|| Error::RefNotFound(ref_id.0.clone()))?;
    let logical = node.bounds.ok_or_else(|| Error::Action {
        action: "screenshot".into(),
        reason: format!("ref {} has no bounds", ref_id.0),
    })?;
    let region = Region {
        x: round_to_i32(logical.x),
        y: round_to_i32(logical.y),
        w: round_to_u32(logical.w.max(1.0)),
        h: round_to_u32(logical.h.max(1.0)),
    };
    let mut captured = capture_screen_region(&region)?;
    captured.origin = (logical.x, logical.y);
    captured.logical_size = (logical.w, logical.h);
    Ok(captured)
}

fn capture_window_by_id(window_id: u32, logical: Bounds) -> Result<CapturedImage> {
    // Passing the window's actual logical bounds as screenBounds (instead of
    // CGRectInfinite) is what tells CG "give me an image sized to this rect,
    // with the window composited at its real screen position." Combined with
    // IncludingWindow + windowID, that crops the result to just our window,
    // so dpi_scale = image.width / logical.w resolves to the host's retina
    // factor and annotation offsets line up.
    let rect = CGRect {
        origin: CGPoint {
            x: logical.x,
            y: logical.y,
        },
        size: CGSize {
            width: logical.w.max(1.0),
            height: logical.h.max(1.0),
        },
    };
    create_image_for(
        rect,
        CG_WINDOW_LIST_OPTION_INCLUDING_WINDOW,
        window_id,
        CG_WINDOW_IMAGE_BOUNDS_IGNORE_FRAMING | CG_WINDOW_IMAGE_BEST_RESOLUTION,
    )
}

fn create_image_for(
    rect: CGRect,
    list_option: u32,
    window_id: u32,
    image_option: u32,
) -> Result<CapturedImage> {
    // SAFETY: CG entry point; the returned image (if non-null) follows
    // create-rule ownership and is released via CGImageRelease below.
    let image = unsafe { CGWindowListCreateImage(rect, list_option, window_id, image_option) };
    if image.is_null() {
        return Err(Error::Action {
            action: "screenshot".into(),
            reason: "CGWindowListCreateImage returned null - check Screen Recording permission in System Settings > Privacy & Security".into(),
        });
    }
    let result = cg_image_to_rgba(image);
    // SAFETY: release the create-rule image.
    unsafe { CGImageRelease(image) };
    result
}

fn cg_image_to_rgba(image: CGImageRef) -> Result<CapturedImage> {
    // SAFETY: image is a valid create-rule CGImage from the caller.
    let width = unsafe { CGImageGetWidth(image) };
    let height = unsafe { CGImageGetHeight(image) };
    let bits_per_pixel = unsafe { CGImageGetBitsPerPixel(image) };
    let bytes_per_row = unsafe { CGImageGetBytesPerRow(image) };
    let bitmap_info = unsafe { CGImageGetBitmapInfo(image) };
    if bits_per_pixel != 32 {
        return Err(Error::Action {
            action: "screenshot".into(),
            reason: format!("unexpected CGImage bits_per_pixel: {bits_per_pixel}"),
        });
    }
    if width == 0 || height == 0 {
        return Err(Error::Action {
            action: "screenshot".into(),
            reason: "CGImage has zero dimensions".into(),
        });
    }
    // SAFETY: image is non-null.
    let provider = unsafe { CGImageGetDataProvider(image) };
    if provider.is_null() {
        return Err(Error::Action {
            action: "screenshot".into(),
            reason: "CGImageGetDataProvider returned null".into(),
        });
    }
    // SAFETY: provider is non-null. Returned CFData follows create-rule.
    let data = unsafe { CGDataProviderCopyData(provider) };
    if data.is_null() {
        return Err(Error::Action {
            action: "screenshot".into(),
            reason: "CGDataProviderCopyData returned null".into(),
        });
    }
    // SAFETY: data is a valid CFData; length and byte ptr are read-only.
    let len = unsafe { CFDataGetLength(data) };
    let ptr = unsafe { CFDataGetBytePtr(data) };
    let src_len = usize::try_from(len).unwrap_or(0);
    if ptr.is_null() || src_len < bytes_per_row.saturating_mul(height) {
        // SAFETY: release create-rule data before returning.
        unsafe { CFRelease(data) };
        return Err(Error::Action {
            action: "screenshot".into(),
            reason: "CGImage backing data was shorter than declared".into(),
        });
    }
    // SAFETY: src_len bytes at ptr are valid for reads as a flat byte slice.
    let src = unsafe { std::slice::from_raw_parts(ptr, src_len) };

    let byte_order = bitmap_info & CG_BITMAP_BYTE_ORDER_MASK;
    let alpha = bitmap_info & CG_IMAGE_ALPHA_INFO_MASK;
    // Most macOS screenshots come back as 32-bit BGRA premultiplied
    // (kCGBitmapByteOrder32Little | kCGImageAlphaPremultipliedFirst). Treat
    // any 32-Little encoding as B,G,R,A in memory; everything else we copy
    // verbatim and force opaque alpha.
    let swap_br = byte_order == CG_BITMAP_BYTE_ORDER_32_LITTLE;
    let _ = alpha;

    let pixel_count = width.saturating_mul(height).saturating_mul(4);
    let mut pixels = vec![0_u8; pixel_count];
    let row_dst = width.saturating_mul(4);
    for y in 0..height {
        let src_off = y.saturating_mul(bytes_per_row);
        let dst_off = y.saturating_mul(row_dst);
        let src_row = &src[src_off..src_off + row_dst];
        let dst_row = &mut pixels[dst_off..dst_off + row_dst];
        for (sp, dp) in src_row.chunks_exact(4).zip(dst_row.chunks_exact_mut(4)) {
            if swap_br {
                dp[0] = sp[2];
                dp[1] = sp[1];
                dp[2] = sp[0];
            } else {
                dp[0] = sp[0];
                dp[1] = sp[1];
                dp[2] = sp[2];
            }
            dp[3] = 0xFF;
        }
    }

    // SAFETY: release create-rule data.
    unsafe { CFRelease(data) };

    Ok(CapturedImage {
        width: u32::try_from(width).unwrap_or(u32::MAX),
        height: u32::try_from(height).unwrap_or(u32::MAX),
        pixels,
    })
}

fn cg_window_id_for(pid: u32, title: Option<&str>, logical: Bounds) -> Result<u32> {
    // SAFETY: CG entry point. Returns a copy-rule CFArray when non-null.
    let array = unsafe {
        CGWindowListCopyWindowInfo(
            CG_WINDOW_LIST_OPTION_ON_SCREEN_ONLY | CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
            CG_NULL_WINDOW_ID,
        )
    };
    if array.is_null() {
        return Err(Error::Action {
            action: "screenshot".into(),
            reason: "CGWindowListCopyWindowInfo returned null".into(),
        });
    }
    // SAFETY: array is a valid CFArray copied above.
    let count = unsafe { CFArrayGetCount(array) };

    let mut by_title: Option<u32> = None;
    let mut by_bounds: Option<(u32, f64)> = None;
    let mut by_pid: Option<u32> = None;

    for idx in 0..count {
        // SAFETY: index in [0, count).
        let dict = unsafe { CFArrayGetValueAtIndex(array, idx) }.cast::<c_void>();
        if dict.is_null() {
            continue;
        }
        let dict = dict as CFDictionaryRef;
        let owner = dict_int(dict, "kCGWindowOwnerPID").unwrap_or(-1);
        if u32::try_from(owner).ok() != Some(pid) {
            continue;
        }
        if dict_int(dict, "kCGWindowLayer").unwrap_or(-1) != 0 {
            continue;
        }
        let number = match dict_int(dict, "kCGWindowNumber") {
            Some(n) if n > 0 => match u32::try_from(n) {
                Ok(n) => n,
                Err(_) => continue,
            },
            _ => continue,
        };
        let name = dict_string(dict, "kCGWindowName");
        if let (Some(want), Some(got)) = (title, name.as_deref()) {
            if !got.is_empty() && got == want {
                by_title.get_or_insert(number);
            }
        }
        if let Some(window_bounds) = dict_bounds(dict, "kCGWindowBounds") {
            let delta = bounds_distance(logical, window_bounds);
            match by_bounds {
                Some((_, best)) if delta >= best => {}
                _ => by_bounds = Some((number, delta)),
            }
        }
        by_pid.get_or_insert(number);
    }
    // SAFETY: release the create-rule array.
    unsafe { CFRelease(array.cast::<c_void>()) };

    if let Some(id) = by_title {
        return Ok(id);
    }
    if let Some((id, dist)) = by_bounds {
        if dist < 8.0 {
            return Ok(id);
        }
        return Ok(id);
    }
    by_pid.ok_or_else(|| Error::Action {
        action: "screenshot".into(),
        reason: format!("no on-screen CGWindow found for pid {pid}"),
    })
}

fn bounds_distance(a: Bounds, b: Bounds) -> f64 {
    (a.x - b.x).abs() + (a.y - b.y).abs() + (a.w - b.w).abs() + (a.h - b.h).abs()
}

fn dict_int(dict: CFDictionaryRef, key: &str) -> Option<i64> {
    let value = dict_value(dict, key)?;
    // SAFETY: `value` is a Core Foundation object.
    let is_number = unsafe { CFGetTypeID(value) == CFNumberGetTypeID() };
    if !is_number {
        return None;
    }
    let mut out: i64 = 0;
    // SAFETY: type was verified as CFNumber.
    let ok = unsafe {
        CFNumberGetValue(
            value.cast::<core_foundation_sys::number::__CFNumber>(),
            kCFNumberIntType,
            (&raw mut out).cast(),
        )
    };
    ok.then_some(out)
}

fn dict_string(dict: CFDictionaryRef, key: &str) -> Option<String> {
    let value = dict_value(dict, key)?;
    cf_string_to_string(value.cast())
}

fn dict_bounds(dict: CFDictionaryRef, key: &str) -> Option<Bounds> {
    let value = dict_value(dict, key)?;
    let bounds_dict = value as CFDictionaryRef;
    let x = dict_double(bounds_dict, "X")?;
    let y = dict_double(bounds_dict, "Y")?;
    let w = dict_double(bounds_dict, "Width")?;
    let h = dict_double(bounds_dict, "Height")?;
    Some(Bounds { x, y, w, h })
}

fn dict_double(dict: CFDictionaryRef, key: &str) -> Option<f64> {
    let value = dict_value(dict, key)?;
    // SAFETY: `value` is a Core Foundation object.
    let is_number = unsafe { CFGetTypeID(value) == CFNumberGetTypeID() };
    if !is_number {
        return None;
    }
    let mut out: f64 = 0.0;
    // SAFETY: type was verified as CFNumber.
    let ok = unsafe {
        CFNumberGetValue(
            value.cast::<core_foundation_sys::number::__CFNumber>(),
            kCFNumberDoubleType,
            (&raw mut out).cast(),
        )
    };
    ok.then_some(out)
}

fn dict_value(dict: CFDictionaryRef, key: &str) -> Option<CFTypeRef> {
    let key_ref = cf_string(key)?;
    // SAFETY: dict is a valid CFDictionary; key_ref is a freshly created CFString.
    // CFDictionaryGetValue returns a borrowed pointer (no retain).
    let value = unsafe { CFDictionaryGetValue(dict, key_ref.cast::<c_void>()) };
    // SAFETY: release the local CFString created for the key.
    unsafe { CFRelease(key_ref.cast::<c_void>()) };
    if value.is_null() {
        None
    } else {
        Some(value)
    }
}

fn encode_screenshot_result(image: &CapturedImage, annotated: bool) -> Result<ActionResult> {
    use base64::Engine;
    let png = encode_png(image)?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
    Ok(ActionResult {
        ok: true,
        message: annotated.then(|| "annotated with cached snapshot refs".into()),
        data: Some(serde_json::json!({
            "format": "png",
            "encoding": "base64",
            "width": image.width,
            "height": image.height,
            "annotated": annotated,
            "data": encoded,
        })),
    })
}

fn encode_png(image: &CapturedImage) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut buf, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| Error::Action {
            action: "screenshot".into(),
            reason: format!("png header: {e}"),
        })?;
        writer
            .write_image_data(&image.pixels)
            .map_err(|e| Error::Action {
                action: "screenshot".into(),
                reason: format!("png write: {e}"),
            })?;
    }
    Ok(buf)
}

// ---- annotation --------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct AnnotationLabel {
    text: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

fn collect_annotation_labels(
    node: &Node,
    origin: (f64, f64),
    dpi_scale: f64,
    image_w: u32,
    image_h: u32,
    out: &mut Vec<AnnotationLabel>,
) {
    if let (Some(ref_id), Some(bounds)) = (&node.ref_id, node.bounds) {
        if let Some(label) = annotation_label(ref_id, bounds, origin, dpi_scale, image_w, image_h) {
            out.push(label);
        }
    }
    for child in &node.children {
        collect_annotation_labels(child, origin, dpi_scale, image_w, image_h, out);
    }
}

fn annotation_label(
    ref_id: &RefId,
    bounds: Bounds,
    origin: (f64, f64),
    dpi_scale: f64,
    image_w: u32,
    image_h: u32,
) -> Option<AnnotationLabel> {
    let x = round_to_i32((bounds.x - origin.0) * dpi_scale);
    let y = round_to_i32((bounds.y - origin.1) * dpi_scale);
    let w = round_to_i32(bounds.w * dpi_scale).max(1);
    let h = round_to_i32(bounds.h * dpi_scale).max(1);
    let image_w = i32::try_from(image_w).unwrap_or(i32::MAX);
    let image_h = i32::try_from(image_h).unwrap_or(i32::MAX);
    if x >= image_w || y >= image_h || x + w <= 0 || y + h <= 0 {
        return None;
    }
    let label_x = x.clamp(0, image_w.saturating_sub(1));
    let label_y = y.clamp(0, image_h.saturating_sub(1));
    Some(AnnotationLabel {
        text: display_ref_label(ref_id),
        x: label_x,
        y: label_y,
        w: w.min(image_w),
        h: h.min(image_h),
    })
}

fn display_ref_label(ref_id: &RefId) -> String {
    ref_id
        .0
        .strip_prefix("ref_")
        .map_or_else(|| ref_id.0.clone(), |n| format!("@e{n}"))
}

fn round_to_i32(v: f64) -> i32 {
    #[allow(clippy::cast_possible_truncation)]
    let rounded = v.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32;
    rounded
}

fn round_to_u32(v: f64) -> u32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let rounded = v.round().clamp(0.0, f64::from(u32::MAX)) as u32;
    rounded
}

fn draw_annotations(image: &mut CapturedImage, labels: &[AnnotationLabel], scale: i32) {
    let scale = scale.max(1);
    for label in labels {
        draw_rect_outline(
            image,
            label.x,
            label.y,
            label.w,
            label.h,
            scale,
            [255, 45, 85, 255],
        );
        let text_w = text_width(&label.text, scale);
        let bg_w = text_w + 4 * scale;
        let bg_h = 11 * scale;
        let bg_x = label
            .x
            .min(i32::try_from(image.width).unwrap_or(i32::MAX) - bg_w);
        let bg_y = label
            .y
            .min(i32::try_from(image.height).unwrap_or(i32::MAX) - bg_h);
        let bg_x = bg_x.max(0);
        let bg_y = bg_y.max(0);
        fill_rect(image, bg_x, bg_y, bg_w, bg_h, [255, 45, 85, 235]);
        draw_text(
            image,
            bg_x + 2 * scale,
            bg_y + 2 * scale,
            &label.text,
            scale,
            [255, 255, 255, 255],
        );
    }
}

fn text_width(text: &str, scale: i32) -> i32 {
    let chars = i32::try_from(text.chars().count()).unwrap_or(i32::MAX);
    chars.saturating_mul(6 * scale).saturating_sub(scale).max(0)
}

fn draw_rect_outline(
    image: &mut CapturedImage,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    thickness: i32,
    color: [u8; 4],
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let stroke = thickness.max(1);
    fill_rect(image, x, y, w, stroke, color);
    fill_rect(image, x, y + h - stroke, w, stroke, color);
    fill_rect(image, x, y, stroke, h, color);
    fill_rect(image, x + w - stroke, y, stroke, h, color);
}

fn fill_rect(image: &mut CapturedImage, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    for py in y..y.saturating_add(h) {
        for px in x..x.saturating_add(w) {
            blend_pixel(image, px, py, color);
        }
    }
}

fn draw_text(image: &mut CapturedImage, x: i32, y: i32, text: &str, scale: i32, color: [u8; 4]) {
    let mut cursor = x;
    for ch in text.chars() {
        draw_glyph(image, cursor, y, ch, scale, color);
        cursor = cursor.saturating_add(6 * scale);
    }
}

fn draw_glyph(image: &mut CapturedImage, x: i32, y: i32, ch: char, scale: i32, color: [u8; 4]) {
    let glyph = glyph_5x7(ch);
    for (row_idx, row) in glyph.iter().enumerate() {
        let row_y = y + i32::try_from(row_idx).unwrap_or(0).saturating_mul(scale);
        for col in 0..5 {
            if row & (1 << (4 - col)) != 0 {
                fill_rect(image, x + col * scale, row_y, scale, scale, color);
            }
        }
    }
}

fn glyph_5x7(ch: char) -> [u8; 7] {
    match ch {
        '@' => [
            0b01110, 0b10001, 0b10111, 0b10101, 0b10111, 0b10000, 0b01110,
        ],
        'e' | 'E' => [
            0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110, 0b00000,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b00001, 0b00001, 0b11110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        _ => [0; 7],
    }
}

fn blend_pixel(image: &mut CapturedImage, x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 {
        return;
    }
    let Ok(xu) = u32::try_from(x) else {
        return;
    };
    let Ok(yu) = u32::try_from(y) else {
        return;
    };
    if xu >= image.width || yu >= image.height {
        return;
    }
    let idx = (usize::try_from(yu).unwrap_or(0) * usize::try_from(image.width).unwrap_or(0)
        + usize::try_from(xu).unwrap_or(0))
        * 4;
    let alpha = u16::from(color[3]);
    let inv = 255_u16.saturating_sub(alpha);
    for (channel, value) in color.iter().take(3).enumerate() {
        let dst = u16::from(image.pixels[idx + channel]);
        let src = u16::from(*value);
        image.pixels[idx + channel] = u8::try_from((src * alpha + dst * inv) / 255).unwrap_or(255);
    }
    image.pixels[idx + 3] = 255;
}
