//! Deterministic Windows UIA fixture used by tests and local reliability work.

// The fixture is a tiny Win32 app, so it needs direct FFI calls.
#![cfg_attr(target_os = "windows", allow(unsafe_code))]

#[cfg(not(target_os = "windows"))]
fn main() {
    println!("agent-ctrl-uia-fixture only opens a GUI on Windows");
}

#[cfg(target_os = "windows")]
fn main() -> windows::core::Result<()> {
    windows_app::run()
}

#[cfg(target_os = "windows")]
mod windows_app {
    // Win32 APIs use pointer-sized handles and message parameters.
    #![allow(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        clippy::cast_sign_loss
    )]

    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::time::Duration;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Controls::{
        InitCommonControlsEx, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetDlgItem, GetMessageW,
        LoadCursorW, PostQuitMessage, RegisterClassW, SendMessageW, SetTimer, SetWindowTextW,
        ShowWindow, TranslateMessage, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, CBS_DROPDOWNLIST,
        CB_ADDSTRING, CB_SETCURSEL, CW_USEDEFAULT, ES_AUTOHSCROLL, ES_LEFT, GWLP_USERDATA, HMENU,
        IDC_ARROW, LBS_NOTIFY, LB_ADDSTRING, LB_SETCURSEL, MSG, SW_SHOW, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_BORDER, WS_CHILD,
        WS_EX_CLIENTEDGE, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
    };

    const CLASS_NAME: PCWSTR = w!("AgentCtrlUiaFixtureWindow");
    const DIALOG_CLASS_NAME: PCWSTR = w!("AgentCtrlUiaFixtureDialog");
    const TITLE: PCWSTR = w!("agent-ctrl UIA Fixture");
    const DIALOG_TITLE: PCWSTR = w!("Fixture Secondary Dialog");

    const ID_STATUS: i32 = 100;
    const ID_TEXT: i32 = 101;
    const ID_INCREMENT: i32 = 102;
    const ID_CHECKBOX: i32 = 103;
    const ID_COMBO: i32 = 104;
    const ID_LIST: i32 = 105;
    const ID_DIALOG: i32 = 106;
    const ID_DELAY: i32 = 107;
    const ID_DIALOG_OK: i32 = 201;
    const TIMER_DELAY_READY: usize = 1;
    const TIMER_AUTO_CLOSE: usize = 2;

    #[derive(Debug, Default)]
    struct FixtureConfig {
        ready_file: Option<PathBuf>,
        auto_close_ms: Option<u32>,
    }

    #[derive(Debug, Default)]
    struct FixtureState {
        click_count: u32,
        ready_file: Option<PathBuf>,
        auto_close_ms: Option<u32>,
    }

    #[derive(Clone, Copy)]
    struct ControlSpec {
        class_name: PCWSTR,
        text: &'static str,
        style: WINDOW_STYLE,
        ex_style: WINDOW_EX_STYLE,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        id: i32,
    }

    pub(super) fn run() -> windows::core::Result<()> {
        let config = parse_args();
        unsafe {
            init_common_controls();
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hInstance: instance.into(),
                lpszClassName: CLASS_NAME,
                lpfnWndProc: Some(wnd_proc),
                hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                ..Default::default()
            };
            RegisterClassW(&raw const class);
            let dialog_class = WNDCLASSW {
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                hInstance: instance.into(),
                lpszClassName: DIALOG_CLASS_NAME,
                lpfnWndProc: Some(dialog_wnd_proc),
                hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                ..Default::default()
            };
            RegisterClassW(&raw const dialog_class);

            let state = Box::new(FixtureState {
                ready_file: config.ready_file,
                auto_close_ms: config.auto_close_ms,
                ..Default::default()
            });
            let state_ptr = Box::into_raw(state);
            let hwnd_result = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                CLASS_NAME,
                TITLE,
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                760,
                520,
                None,
                None,
                instance,
                Some(state_ptr.cast()),
            );
            let hwnd = match hwnd_result {
                Ok(hwnd) => hwnd,
                Err(err) => {
                    drop(Box::from_raw(state_ptr));
                    return Err(err);
                }
            };

            let _ = ShowWindow(hwnd, SW_SHOW);
            write_ready_file(hwnd);

            let mut msg = MSG::default();
            while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&raw const msg);
                DispatchMessageW(&raw const msg);
            }
        }
        Ok(())
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
                    .and_then(|value| value.to_string_lossy().parse::<u32>().ok());
            }
        }
        config
    }

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                let createstruct =
                    lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::CREATESTRUCTW;
                let state_ptr = (*createstruct).lpCreateParams.cast::<FixtureState>();
                set_window_user_data(hwnd, state_ptr);
                let _ = create_controls(hwnd);
                if let Some(ms) = state(hwnd).and_then(|s| s.auto_close_ms) {
                    SetTimer(hwnd, TIMER_AUTO_CLOSE, ms, None);
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                handle_command(hwnd, low_word(wparam.0));
                LRESULT(0)
            }
            WM_TIMER => {
                match wparam.0 {
                    TIMER_DELAY_READY => set_child_text(hwnd, ID_STATUS, "Status: ready"),
                    TIMER_AUTO_CLOSE => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                if let Some(ptr) = take_window_user_data(hwnd) {
                    drop(Box::from_raw(ptr));
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn create_controls(hwnd: HWND) -> windows::core::Result<()> {
        create_text_controls(hwnd)?;
        create_choice_controls(hwnd)?;
        create_command_controls(hwnd)?;
        Ok(())
    }

    unsafe fn create_text_controls(hwnd: HWND) -> windows::core::Result<()> {
        create_child(
            hwnd,
            ControlSpec {
                class_name: w!("STATIC"),
                text: "Status: idle",
                style: WINDOW_STYLE::default(),
                ex_style: WINDOW_EX_STYLE::default(),
                x: 24,
                y: 24,
                width: 300,
                height: 24,
                id: ID_STATUS,
            },
        )?;
        create_child(
            hwnd,
            ControlSpec {
                class_name: w!("EDIT"),
                text: "fixture text",
                style: control_style(ES_LEFT | ES_AUTOHSCROLL) | WS_BORDER | WS_TABSTOP,
                ex_style: WS_EX_CLIENTEDGE,
                x: 24,
                y: 64,
                width: 320,
                height: 28,
                id: ID_TEXT,
            },
        )?;
        Ok(())
    }

    unsafe fn create_choice_controls(hwnd: HWND) -> windows::core::Result<()> {
        create_child(
            hwnd,
            ControlSpec {
                class_name: w!("BUTTON"),
                text: "Enable advanced mode",
                style: control_style(BS_AUTOCHECKBOX) | WS_TABSTOP,
                ex_style: WINDOW_EX_STYLE::default(),
                x: 184,
                y: 112,
                width: 210,
                height: 32,
                id: ID_CHECKBOX,
            },
        )?;
        let combo = create_child(
            hwnd,
            ControlSpec {
                class_name: w!("COMBOBOX"),
                text: "",
                style: control_style(CBS_DROPDOWNLIST) | WS_TABSTOP,
                ex_style: WINDOW_EX_STYLE::default(),
                x: 24,
                y: 168,
                width: 220,
                height: 120,
                id: ID_COMBO,
            },
        )?;
        add_combo_item(combo, "Alpha");
        add_combo_item(combo, "Beta");
        add_combo_item(combo, "Gamma");
        SendMessageW(combo, CB_SETCURSEL, WPARAM(0), LPARAM(0));

        let list = create_child(
            hwnd,
            ControlSpec {
                class_name: w!("LISTBOX"),
                text: "",
                style: control_style(LBS_NOTIFY) | WS_BORDER | WS_TABSTOP,
                ex_style: WS_EX_CLIENTEDGE,
                x: 280,
                y: 168,
                width: 180,
                height: 96,
                id: ID_LIST,
            },
        )?;
        add_list_item(list, "First");
        add_list_item(list, "Second");
        add_list_item(list, "Third");
        SendMessageW(list, LB_SETCURSEL, WPARAM(0), LPARAM(0));
        Ok(())
    }

    unsafe fn create_command_controls(hwnd: HWND) -> windows::core::Result<()> {
        create_child(
            hwnd,
            ControlSpec {
                class_name: w!("BUTTON"),
                text: "Increment",
                style: control_style(BS_DEFPUSHBUTTON) | WS_TABSTOP,
                ex_style: WINDOW_EX_STYLE::default(),
                x: 24,
                y: 112,
                width: 140,
                height: 32,
                id: ID_INCREMENT,
            },
        )?;
        create_child(
            hwnd,
            ControlSpec {
                class_name: w!("BUTTON"),
                text: "Open dialog",
                style: WS_TABSTOP,
                ex_style: WINDOW_EX_STYLE::default(),
                x: 24,
                y: 304,
                width: 140,
                height: 32,
                id: ID_DIALOG,
            },
        )?;
        create_child(
            hwnd,
            ControlSpec {
                class_name: w!("BUTTON"),
                text: "Delay ready",
                style: WS_TABSTOP,
                ex_style: WINDOW_EX_STYLE::default(),
                x: 184,
                y: 304,
                width: 140,
                height: 32,
                id: ID_DELAY,
            },
        )?;
        Ok(())
    }

    unsafe fn create_child(parent: HWND, spec: ControlSpec) -> windows::core::Result<HWND> {
        let instance = GetModuleHandleW(None)?;
        let text_wide = wide(spec.text);
        CreateWindowExW(
            spec.ex_style,
            spec.class_name,
            PCWSTR(text_wide.as_ptr()),
            WS_CHILD | WS_VISIBLE | spec.style,
            spec.x,
            spec.y,
            spec.width,
            spec.height,
            parent,
            control_id_menu(spec.id),
            instance,
            None,
        )
    }

    unsafe fn handle_command(hwnd: HWND, id: usize) {
        match i32::try_from(id).ok() {
            Some(ID_INCREMENT) => {
                if let Some(state) = state_mut(hwnd) {
                    state.click_count = state.click_count.saturating_add(1);
                    set_child_text(
                        hwnd,
                        ID_STATUS,
                        &format!("Status: count {}", state.click_count),
                    );
                }
            }
            Some(ID_DIALOG) => {
                let _ = create_fixture_dialog(hwnd);
            }
            Some(ID_DELAY) => {
                set_child_text(hwnd, ID_STATUS, "Status: waiting");
                SetTimer(
                    hwnd,
                    TIMER_DELAY_READY,
                    timer_ms(Duration::from_millis(300)),
                    None,
                );
            }
            _ => {}
        }
    }

    unsafe extern "system" fn dialog_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CREATE => {
                let _ = create_dialog_controls(hwnd);
                LRESULT(0)
            }
            WM_COMMAND => {
                if i32::try_from(low_word(wparam.0)).ok() == Some(ID_DIALOG_OK) {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    unsafe fn create_fixture_dialog(owner: HWND) -> windows::core::Result<HWND> {
        let instance = GetModuleHandleW(None)?;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            DIALOG_CLASS_NAME,
            DIALOG_TITLE,
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            420,
            220,
            owner,
            None,
            instance,
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        Ok(hwnd)
    }

    unsafe fn create_dialog_controls(hwnd: HWND) -> windows::core::Result<()> {
        create_child(
            hwnd,
            ControlSpec {
                class_name: w!("STATIC"),
                text: "Dialog status: open",
                style: WINDOW_STYLE::default(),
                ex_style: WINDOW_EX_STYLE::default(),
                x: 24,
                y: 24,
                width: 240,
                height: 24,
                id: 200,
            },
        )?;
        create_child(
            hwnd,
            ControlSpec {
                class_name: w!("BUTTON"),
                text: "Dialog OK",
                style: control_style(BS_DEFPUSHBUTTON) | WS_TABSTOP,
                ex_style: WINDOW_EX_STYLE::default(),
                x: 24,
                y: 72,
                width: 120,
                height: 32,
                id: ID_DIALOG_OK,
            },
        )?;
        Ok(())
    }

    unsafe fn init_common_controls() {
        let size = u32::try_from(std::mem::size_of::<INITCOMMONCONTROLSEX>()).unwrap_or_default();
        let controls = INITCOMMONCONTROLSEX {
            dwSize: size,
            dwICC: ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&raw const controls);
    }

    unsafe fn add_combo_item(hwnd: HWND, text: &str) {
        let text_wide = wide(text);
        SendMessageW(
            hwnd,
            CB_ADDSTRING,
            WPARAM(0),
            LPARAM(PCWSTR(text_wide.as_ptr()).0 as isize),
        );
    }

    unsafe fn add_list_item(hwnd: HWND, text: &str) {
        let text_wide = wide(text);
        SendMessageW(
            hwnd,
            LB_ADDSTRING,
            WPARAM(0),
            LPARAM(PCWSTR(text_wide.as_ptr()).0 as isize),
        );
    }

    unsafe fn set_text(hwnd: HWND, text: &str) {
        let text_wide = wide(text);
        let _ = SetWindowTextW(hwnd, PCWSTR(text_wide.as_ptr()));
    }

    unsafe fn set_child_text(parent: HWND, id: i32, text: &str) {
        if let Ok(hwnd) = GetDlgItem(parent, id) {
            set_text(hwnd, text);
        }
    }

    unsafe fn write_ready_file(hwnd: HWND) {
        if let Some(path) = state(hwnd).and_then(|s| s.ready_file.as_ref()) {
            let _ = std::fs::write(path, "ready\n");
        }
    }

    unsafe fn state(hwnd: HWND) -> Option<&'static FixtureState> {
        let ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA)
            as *const FixtureState;
        ptr.as_ref()
    }

    unsafe fn state_mut(hwnd: HWND) -> Option<&'static mut FixtureState> {
        let ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA)
            as *mut FixtureState;
        ptr.as_mut()
    }

    unsafe fn set_window_user_data(hwnd: HWND, state: *mut FixtureState) {
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            state as isize,
        );
    }

    unsafe fn take_window_user_data(hwnd: HWND) -> Option<*mut FixtureState> {
        let ptr = windows::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd, GWLP_USERDATA)
            as *mut FixtureState;
        windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        if ptr.is_null() {
            None
        } else {
            Some(ptr)
        }
    }

    fn timer_ms(duration: Duration) -> u32 {
        u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
    }

    fn low_word(value: usize) -> usize {
        value & 0xffff
    }

    fn control_style(bits: i32) -> WINDOW_STYLE {
        WINDOW_STYLE(u32::try_from(bits).unwrap_or_default())
    }

    fn control_id_menu(id: i32) -> HMENU {
        HMENU(isize::try_from(id).map_or(std::ptr::null_mut(), |value| value as *mut c_void))
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}
