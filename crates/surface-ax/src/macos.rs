//! macOS-specific AX snapshot implementation.

use std::ffi::{c_char, c_void, CString};
use std::process::Command;
use std::time::SystemTime;

use accessibility_sys::{
    kAXButtonRole, kAXCheckBoxRole, kAXChildrenAttribute, kAXComboBoxRole, kAXDescriptionAttribute,
    kAXDialogSubrole, kAXEnabledAttribute, kAXErrorSuccess, kAXExpandedAttribute,
    kAXFocusedAttribute, kAXFocusedWindowAttribute, kAXGroupRole, kAXImageRole, kAXMenuBarItemRole,
    kAXMenuBarRole, kAXMenuButtonRole, kAXMenuItemRole, kAXMenuRole, kAXOutlineRole,
    kAXPopUpButtonRole, kAXPositionAttribute, kAXRadioButtonRole, kAXRoleAttribute,
    kAXScrollAreaRole, kAXSearchFieldSubrole, kAXSelectedAttribute, kAXSizeAttribute,
    kAXSliderRole, kAXStaticTextRole, kAXSubroleAttribute, kAXTabGroupRole, kAXTableRole,
    kAXTextAreaRole, kAXTextFieldRole, kAXTitleAttribute, kAXValueAttribute, kAXValueTypeCGPoint,
    kAXValueTypeCGSize, AXIsProcessTrusted, AXUIElementCopyAttributeValue,
    AXUIElementCreateSystemWide, AXUIElementGetPid, AXUIElementRef, AXValueGetValue, AXValueRef,
};
use agent_ctrl_core::{
    AppContext, Bounds, Error, NativeHandle, Node, RefMap, Result, Role, Snapshot, SnapshotOptions,
    State, SurfaceKind, WindowContext, WindowTarget,
};
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
use core_foundation_sys::base::{kCFAllocatorDefault, CFGetTypeID, CFRelease, CFRetain, CFTypeRef};
use core_foundation_sys::number::{CFBooleanGetTypeID, CFBooleanGetValue};
use core_foundation_sys::string::{
    kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringGetCString,
    CFStringGetMaximumSizeForEncoding, CFStringGetTypeID, CFStringRef,
};

const DEFAULT_DEPTH: usize = 12;

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

pub(super) fn snapshot(opts: &SnapshotOptions) -> Result<Snapshot> {
    let root = match &opts.target {
        WindowTarget::Foreground => focused_window()?,
        WindowTarget::Pid { pid } => app_focused_window(*pid)?,
        WindowTarget::Title { .. } | WindowTarget::ProcessName { .. } => {
            return Err(Error::Unsupported {
                surface: SurfaceKind::Ax.as_str().into(),
                action: "target title/process-name".into(),
            });
        }
    };
    let pid = element_pid(root).unwrap_or_default();
    let title = string_attr(root, kAXTitleAttribute);
    let max_depth = opts.depth.unwrap_or(DEFAULT_DEPTH);
    let mut refs = RefMap::new();
    let node = build_node(root, 0, max_depth, &mut refs);
    // SAFETY: `root` is returned by a create/copy rule helper.
    unsafe { CFRelease(root.cast::<c_void>()) };

    Ok(Snapshot {
        captured_at: SystemTime::now(),
        surface_kind: SurfaceKind::Ax,
        app: AppContext {
            id: format!("pid:{pid}"),
            name: process_name(pid).unwrap_or_else(|| format!("pid {pid}")),
        },
        window: Some(WindowContext {
            id: format!("0x{:x}", root as usize),
            title,
        }),
        root: node,
        refs,
    })
}

fn focused_window() -> Result<AXUIElementRef> {
    // SAFETY: create rule returns a valid system-wide AX object or null.
    let system = unsafe { AXUIElementCreateSystemWide() };
    if system.is_null() {
        return Err(Error::Surface("AX system-wide element was null".into()));
    }
    let window = element_attr(system, kAXFocusedWindowAttribute);
    // SAFETY: release the create-rule system object after copying the window.
    unsafe { CFRelease(system.cast::<c_void>()) };
    let Some(window) = window else {
        return Err(Error::Surface("no focused AX window".into()));
    };
    Ok(window as AXUIElementRef)
}

fn app_focused_window(pid: u32) -> Result<AXUIElementRef> {
    let pid = i32::try_from(pid).map_err(|_| Error::Surface("pid out of range".into()))?;
    // SAFETY: create rule returns an AX application object for pid or null.
    let app = unsafe { accessibility_sys::AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err(Error::Surface(format!(
            "AX application for pid {pid} was null"
        )));
    }
    let window = element_attr(app, kAXFocusedWindowAttribute)
        .or_else(|| first_array_element(app, accessibility_sys::kAXWindowsAttribute));
    // SAFETY: release the create-rule app object after copying the window.
    unsafe { CFRelease(app.cast::<c_void>()) };
    let Some(window) = window else {
        return Err(Error::Surface(format!("pid {pid} has no AX window")));
    };
    Ok(window as AXUIElementRef)
}

fn build_node(element: AXUIElementRef, depth: usize, max_depth: usize, refs: &mut RefMap) -> Node {
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
        checked: None,
        expanded: bool_attr(element, kAXExpandedAttribute),
        required: None,
    };
    let bounds = bounds(element);
    let native = Some(NativeHandle::Ax {
        element_ref: element as u64,
    });
    let ref_id = if role.is_interactive() || role.is_content() {
        Some(refs.insert(role.clone(), name.clone(), 0, native.clone()))
    } else {
        None
    };
    let children = if depth >= max_depth {
        Vec::new()
    } else {
        build_children(element, depth + 1, max_depth, refs)
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
            children.push(build_node(child as AXUIElementRef, depth, max_depth, refs));
        }
    }
    // SAFETY: release the copy-rule array after extracting child pointers.
    unsafe { CFRelease(array_ref.cast::<c_void>()) };
    children
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
