//! Deterministic macOS AX fixture used by tests and local reliability work.

#![allow(unexpected_cfgs)] // `objc` 0.2 macros still probe the historical `cargo-clippy` cfg.
#![cfg_attr(target_os = "macos", allow(unsafe_code))]

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("agent-ctrl-ax-fixture only opens a GUI on macOS");
}

#[cfg(target_os = "macos")]
fn main() {
    macos_app::run();
}

#[cfg(target_os = "macos")]
mod macos_app {
    // Objective-C APIs use pointer-sized style masks and raw object pointers.
    #![allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]

    use std::ffi::CString;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};
    use std::time::Duration;

    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel, NO, YES};
    use objc::{class, msg_send, sel, sel_impl};

    type Id = *mut Object;
    type CGFloat = f64;
    type NSUInteger = u64;
    type NSInteger = i64;

    const NS_UTF8_STRING_ENCODING: NSUInteger = 4;
    const NS_APPLICATION_ACTIVATION_POLICY_REGULAR: NSInteger = 0;
    const NS_BACKING_STORE_BUFFERED: NSUInteger = 2;
    const NS_WINDOW_STYLE_TITLED: NSUInteger = 1 << 0;
    const NS_WINDOW_STYLE_CLOSABLE: NSUInteger = 1 << 1;
    const NS_WINDOW_STYLE_RESIZABLE: NSUInteger = 1 << 3;
    const NS_BUTTON_TYPE_MOMENTARY_PUSH_IN: NSUInteger = 0;
    const NS_BUTTON_TYPE_SWITCH: NSUInteger = 3;
    const NS_ROUNDED_BEZEL_STYLE: NSUInteger = 1;

    static STATUS_FIELD: AtomicPtr<Object> = AtomicPtr::new(std::ptr::null_mut());
    static CLICK_COUNT: AtomicUsize = AtomicUsize::new(0);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSPoint {
        x: CGFloat,
        y: CGFloat,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSSize {
        width: CGFloat,
        height: CGFloat,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct NSRect {
        origin: NSPoint,
        size: NSSize,
    }

    #[derive(Debug, Default)]
    struct FixtureConfig {
        ready_file: Option<PathBuf>,
        auto_close_ms: Option<u64>,
    }

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {}

    pub(super) fn run() {
        let config = parse_args();
        if let Some(ms) = config.auto_close_ms {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                std::process::exit(0);
            });
        }

        unsafe {
            let _pool: Id = msg_send![class!(NSAutoreleasePool), new];
            let app: Id = msg_send![class!(NSApplication), sharedApplication];
            let _: () =
                msg_send![app, setActivationPolicy: NS_APPLICATION_ACTIVATION_POLICY_REGULAR];

            let target = make_target();
            let window = create_window();
            let content: Id = msg_send![window, contentView];
            build_controls(content, target);

            let _: () = msg_send![window, makeKeyAndOrderFront: std::ptr::null_mut::<Object>()];
            let _: () = msg_send![app, activateIgnoringOtherApps: YES];
            write_ready_file(config.ready_file);
            let _: () = msg_send![app, run];
        }
    }

    fn parse_args() -> FixtureConfig {
        let mut config = FixtureConfig::default();
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--ready-file" {
                config.ready_file = args.next().map(PathBuf::from);
            } else if arg == "--auto-close-ms" {
                config.auto_close_ms = args
                    .next()
                    .and_then(|value| value.to_string_lossy().parse::<u64>().ok());
            }
        }
        config
    }

    unsafe fn create_window() -> Id {
        let frame = rect(160.0, 160.0, 640.0, 360.0);
        let style = NS_WINDOW_STYLE_TITLED | NS_WINDOW_STYLE_CLOSABLE | NS_WINDOW_STYLE_RESIZABLE;
        let window: Id = msg_send![class!(NSWindow), alloc];
        let window: Id = msg_send![
            window,
            initWithContentRect: frame
            styleMask: style
            backing: NS_BACKING_STORE_BUFFERED
            defer: NO
        ];
        let title = nsstring("agent-ctrl AX Fixture");
        let _: () = msg_send![window, setTitle: title];
        let _: () = msg_send![window, setReleasedWhenClosed: NO];
        window
    }

    unsafe fn build_controls(content: Id, target: Id) {
        let status = label("Status: idle", rect(24.0, 292.0, 360.0, 24.0));
        STATUS_FIELD.store(status, Ordering::SeqCst);
        set_identifier(status, "fixture-status");
        add_subview(content, status);

        let field = text_field("fixture text", rect(24.0, 244.0, 320.0, 28.0));
        set_identifier(field, "fixture-text-field");
        add_subview(content, field);

        let button = button("Increment", rect(24.0, 188.0, 140.0, 34.0));
        let _: () = msg_send![button, setTarget: target];
        let _: () = msg_send![button, setAction: sel!(increment:)];
        set_identifier(button, "fixture-increment-button");
        add_subview(content, button);

        let checkbox = checkbox("Enable advanced mode", rect(24.0, 150.0, 220.0, 24.0));
        set_identifier(checkbox, "fixture-advanced-checkbox");
        add_subview(content, checkbox);

        let popup = popup_button(
            &["Apple", "Banana", "Cherry"],
            rect(24.0, 60.0, 200.0, 28.0),
        );
        let _: () = msg_send![popup, setTarget: target];
        let _: () = msg_send![popup, setAction: sel!(selectionChanged:)];
        set_identifier(popup, "fixture-fruit-popup");
        add_subview(content, popup);

        let hint = label(
            "AX fixture ready: use snapshot, find, click, focus, fill, check, type, press",
            rect(24.0, 104.0, 560.0, 24.0),
        );
        add_subview(content, hint);
    }

    unsafe fn set_identifier(view: Id, identifier: &str) {
        let id = nsstring(identifier);
        let _: () = msg_send![view, setAccessibilityIdentifier: id];
    }

    unsafe fn label(text: &str, frame: NSRect) -> Id {
        let field = text_field(text, frame);
        let _: () = msg_send![field, setEditable: NO];
        let _: () = msg_send![field, setSelectable: NO];
        let _: () = msg_send![field, setBezeled: NO];
        let _: () = msg_send![field, setDrawsBackground: NO];
        field
    }

    unsafe fn text_field(text: &str, frame: NSRect) -> Id {
        let field: Id = msg_send![class!(NSTextField), alloc];
        let field: Id = msg_send![field, initWithFrame: frame];
        let value = nsstring(text);
        let _: () = msg_send![field, setStringValue: value];
        field
    }

    unsafe fn button(text: &str, frame: NSRect) -> Id {
        let button: Id = msg_send![class!(NSButton), alloc];
        let button: Id = msg_send![button, initWithFrame: frame];
        let title = nsstring(text);
        let _: () = msg_send![button, setTitle: title];
        let _: () = msg_send![button, setButtonType: NS_BUTTON_TYPE_MOMENTARY_PUSH_IN];
        let _: () = msg_send![button, setBezelStyle: NS_ROUNDED_BEZEL_STYLE];
        button
    }

    unsafe fn checkbox(text: &str, frame: NSRect) -> Id {
        let checkbox: Id = msg_send![class!(NSButton), alloc];
        let checkbox: Id = msg_send![checkbox, initWithFrame: frame];
        let title = nsstring(text);
        let _: () = msg_send![checkbox, setTitle: title];
        let _: () = msg_send![checkbox, setButtonType: NS_BUTTON_TYPE_SWITCH];
        checkbox
    }

    unsafe fn popup_button(items: &[&str], frame: NSRect) -> Id {
        let popup: Id = msg_send![class!(NSPopUpButton), alloc];
        let popup: Id = msg_send![popup, initWithFrame: frame pullsDown: NO];
        for item in items {
            let title = nsstring(item);
            let _: () = msg_send![popup, addItemWithTitle: title];
        }
        popup
    }

    unsafe fn add_subview(content: Id, view: Id) {
        let _: () = msg_send![content, addSubview: view];
    }

    unsafe fn make_target() -> Id {
        let superclass = class!(NSObject);
        let class = if let Some(mut decl) = ClassDecl::new("AgentCtrlAxFixtureTarget", superclass) {
            decl.add_method(
                sel!(increment:),
                increment as extern "C" fn(&Object, Sel, Id),
            );
            decl.add_method(
                sel!(selectionChanged:),
                selection_changed as extern "C" fn(&Object, Sel, Id),
            );
            decl.register()
        } else if let Some(class) = Class::get("AgentCtrlAxFixtureTarget") {
            class
        } else {
            panic!("AgentCtrlAxFixtureTarget class registration failed");
        };
        let target: Id = msg_send![class, new];
        target
    }

    extern "C" fn increment(_this: &Object, _cmd: Sel, _sender: Id) {
        let count = CLICK_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
        let field = STATUS_FIELD.load(Ordering::SeqCst);
        if field.is_null() {
            return;
        }
        unsafe {
            let value = nsstring(&format!("Status: count {count}"));
            let _: () = msg_send![field, setStringValue: value];
        }
    }

    extern "C" fn selection_changed(_this: &Object, _cmd: Sel, sender: Id) {
        let field = STATUS_FIELD.load(Ordering::SeqCst);
        if field.is_null() || sender.is_null() {
            return;
        }
        unsafe {
            let title: Id = msg_send![sender, titleOfSelectedItem];
            if title.is_null() {
                return;
            }
            let utf8: *const std::os::raw::c_char = msg_send![title, UTF8String];
            if utf8.is_null() {
                return;
            }
            let chosen = std::ffi::CStr::from_ptr(utf8)
                .to_string_lossy()
                .into_owned();
            let value = nsstring(&format!("Status: chose {chosen}"));
            let _: () = msg_send![field, setStringValue: value];
        }
    }

    fn write_ready_file(path: Option<PathBuf>) {
        if let Some(path) = path {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, b"ready\n");
        }
    }

    fn rect(x: CGFloat, y: CGFloat, width: CGFloat, height: CGFloat) -> NSRect {
        NSRect {
            origin: NSPoint { x, y },
            size: NSSize { width, height },
        }
    }

    unsafe fn nsstring(value: &str) -> Id {
        let bytes = value
            .as_bytes()
            .iter()
            .copied()
            .filter(|byte| *byte != 0)
            .collect::<Vec<_>>();
        // SAFETY: NUL bytes were removed above, so the vector is a valid CString payload.
        let cstr = unsafe { CString::from_vec_unchecked(bytes) };
        let string: Id = msg_send![class!(NSString), alloc];
        let string: Id = msg_send![
            string,
            initWithBytes: cstr.as_ptr()
            length: cstr.as_bytes().len()
            encoding: NS_UTF8_STRING_ENCODING
        ];
        string
    }
}
