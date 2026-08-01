//! Opt-in end-to-end macOS AX fixture coverage.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn macos_ax_fixture_core_flow() {
    if std::env::var_os("RUN_AX_TESTS").is_none() {
        eprintln!("skipping macOS AX fixture test; set RUN_AX_TESTS=1 to run it");
        return;
    }
    if !cfg!(target_os = "macos") {
        eprintln!("skipping macOS AX fixture test on non-macOS host");
        return;
    }

    run_fixture_flow();
}

fn run_fixture_flow() {
    let cli = PathBuf::from(env!("CARGO_BIN_EXE_agent-ctrl"));
    let fixture = fixture_exe_path();
    assert!(
        fixture.exists(),
        "missing fixture binary at {}; run `cargo build -p agent-ctrl-ax-fixture` before RUN_AX_TESTS=1",
        fixture.display()
    );

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let home = std::env::temp_dir().join(format!("agent-ctrl-ax-test-{stamp}"));
    let ready = std::env::temp_dir().join(format!("agent-ctrl-ax-test-{stamp}.ready"));
    std::fs::create_dir_all(&home).unwrap();

    let mut fixture_child = Command::new(&fixture)
        .arg("--ready-file")
        .arg(&ready)
        .arg("--auto-close-ms")
        .arg("60000")
        .spawn()
        .expect("launching AX fixture");
    let fixture_pid = fixture_child.id();
    let _guard = Cleanup {
        cli: cli.clone(),
        home: home.clone(),
        ready: ready.clone(),
        fixture: &mut fixture_child,
    };

    wait_for_ready(&ready);

    let run = FixtureRun {
        cli: &cli,
        home: &home,
        pid: fixture_pid,
    };
    run.open();
    run.snapshot();
    run.exercise_native_handles_stay_private();
    run.exercise_button_click();
    run.exercise_double_click();
    run.exercise_non_checkable_actions_are_rejected();
    run.exercise_duplicate_identifiers();
    run.exercise_hover();
    run.exercise_fill();
    run.exercise_clear();
    run.exercise_unicode_keyboard_input();
    run.exercise_clipboard();
    run.exercise_select();
    run.exercise_checkbox();
    run.exercise_window_list();
    run.exercise_attached_sheet_scope();
    run.exercise_popover_scope();
    run.exercise_screenshot();
    run.exercise_switch_app();
}

struct FixtureRun<'a> {
    cli: &'a Path,
    home: &'a Path,
    pid: u32,
}

impl FixtureRun<'_> {
    fn open(&self) {
        run_cli_no_capture(self.cli, self.home, ["open", "ax", "--session", "fixture"]);
    }

    fn snapshot(&self) {
        run_cli(
            self.cli,
            self.home,
            [
                "snapshot",
                "--session",
                "fixture",
                "--target-process",
                "agent-ctrl-ax-fixture",
            ],
        );
    }

    fn exercise_native_handles_stay_private(&self) {
        // The following action tests exercise internal AXIdentifier-based
        // rediscovery. This assertion guards the public half of the contract:
        // platform handles must never appear in serialized snapshots.
        let snap = run_cli(
            self.cli,
            self.home,
            [
                "snapshot",
                "--session",
                "fixture",
                "--target-process",
                "agent-ctrl-ax-fixture",
                "--json",
            ],
        );
        let snap: serde_json::Value = serde_json::from_str(&snap).unwrap();
        assert_no_native_fields(&snap);
    }

    fn exercise_button_click(&self) {
        let button = self.find("Increment", "button");
        let out = run_cli(
            self.cli,
            self.home,
            ["click", button.trim(), "--session", "fixture"],
        );
        // NSButton accepts AXPress, so the click should take the AX fast path
        // rather than the CGEvent fallback. Method tag proves which path ran.
        assert!(
            out.contains("method=ax-press"),
            "expected method=ax-press, got {out:?}"
        );
        self.snapshot();
        run_cli(
            self.cli,
            self.home,
            ["find", "Status: count 1", "--first", "--session", "fixture"],
        );
    }

    fn exercise_double_click(&self) {
        // exercise_button_click left the count at 1. A double-click on the
        // NSButton fires its action twice (count 1 -> 3), proving the CGEvent
        // path actually drives the target window.
        let button = self.find("Increment", "button");
        let out = run_cli(
            self.cli,
            self.home,
            ["double-click", button.trim(), "--session", "fixture"],
        );
        assert!(
            out.contains("method=cg-double-click"),
            "expected method=cg-double-click, got {out:?}"
        );
        self.snapshot();
        run_cli(
            self.cli,
            self.home,
            ["find", "Status: count 3", "--first", "--session", "fixture"],
        );
    }

    fn exercise_hover(&self) {
        let button = self.find("Increment", "button");
        run_cli(
            self.cli,
            self.home,
            ["hover", button.trim(), "--session", "fixture"],
        );
    }

    fn exercise_non_checkable_actions_are_rejected(&self) {
        let button = self.find("Increment", "button");
        let check_error = run_cli_expect_failure(
            self.cli,
            self.home,
            ["check", button.trim(), "--session", "fixture"],
        );
        assert!(
            check_error.contains("does not expose a readable check state"),
            "unexpected check error: {check_error:?}"
        );
        self.snapshot();
        let _ = self.find("Status: count 3", "region");

        let button = self.find("Increment", "button");
        let toggle_error = run_cli_expect_failure(
            self.cli,
            self.home,
            ["toggle", button.trim(), "--session", "fixture"],
        );
        assert!(
            toggle_error.contains("does not expose a readable check state"),
            "unexpected toggle error: {toggle_error:?}"
        );
        self.snapshot();
        let _ = self.find("Status: count 3", "region");
    }

    fn exercise_duplicate_identifiers(&self) {
        self.snapshot();
        let second = self.find("Duplicate Second", "button");
        run_cli(
            self.cli,
            self.home,
            ["click", second.trim(), "--session", "fixture"],
        );
        self.snapshot();
        let _ = self.find("Status: duplicate second", "region");
    }

    fn exercise_fill(&self) {
        let field = self.find("", "text-field");
        run_cli(
            self.cli,
            self.home,
            [
                "fill",
                field.trim(),
                "fixture edited",
                "--session",
                "fixture",
            ],
        );
        self.snapshot();
        run_cli(
            self.cli,
            self.home,
            [
                "find",
                "fixture edited",
                "--role",
                "text-field",
                "--first",
                "--session",
                "fixture",
            ],
        );
    }

    fn exercise_clear(&self) {
        // exercise_fill leaves the field with "fixture edited"; clear should
        // empty AXValue, and the next snapshot's value-derived name should be
        // empty (the field has no AXTitle/AXDescription).
        let field = self.find("fixture edited", "text-field");
        run_cli(
            self.cli,
            self.home,
            ["clear", field.trim(), "--session", "fixture"],
        );
        self.snapshot();
        let field = self.find("", "text-field");
        let value = run_cli(
            self.cli,
            self.home,
            [
                "get",
                "value",
                field.trim(),
                "--json",
                "--session",
                "fixture",
            ],
        );
        let value: serde_json::Value = serde_json::from_str(&value).unwrap();
        // After clear, AXValue is empty. The snapshot omits an empty value
        // when it matches the (now also empty) name, so `value` may be null
        // or "". Either is a successful clear.
        let cleared = value["value"].as_str().is_none_or(str::is_empty);
        assert!(
            cleared,
            "text-field value after clear should be null or empty, got {:?}",
            value["value"]
        );
        // Restore the original value so downstream assertions stay stable.
        run_cli(
            self.cli,
            self.home,
            [
                "fill",
                field.trim(),
                "fixture edited",
                "--session",
                "fixture",
            ],
        );
        self.snapshot();
    }

    fn exercise_unicode_keyboard_input(&self) {
        let field = self.find("fixture edited", "text-field");
        run_cli(
            self.cli,
            self.home,
            ["fill", field.trim(), "", "--session", "fixture"],
        );
        run_cli(
            self.cli,
            self.home,
            ["type", "AX unicode 🙂", "--session", "fixture"],
        );
        std::thread::sleep(Duration::from_millis(100));
        self.snapshot();
        let field = self.find("AX unicode 🙂", "text-field");
        run_cli(
            self.cli,
            self.home,
            [
                "fill",
                field.trim(),
                "fixture edited",
                "--session",
                "fixture",
            ],
        );
        self.snapshot();
    }

    fn exercise_clipboard(&self) {
        let needle = "ax-clip-roundtrip-marker";
        run_cli(
            self.cli,
            self.home,
            ["clipboard", "write", needle, "--session", "fixture"],
        );
        let out = run_cli(
            self.cli,
            self.home,
            ["clipboard", "read", "--session", "fixture"],
        );
        assert!(
            out.trim() == needle,
            "clipboard round trip mismatched, got {:?}",
            out.trim()
        );
    }

    fn exercise_select(&self) {
        // The fixture's NSPopUpButton starts with "Apple" selected. Picking
        // "Banana" fires `selectionChanged:` on the target which updates the
        // status field.
        let popup = self.find("Apple", "button");
        run_cli(
            self.cli,
            self.home,
            ["select", popup.trim(), "Banana", "--session", "fixture"],
        );
        self.snapshot();
        run_cli(
            self.cli,
            self.home,
            [
                "find",
                "Status: chose Banana",
                "--first",
                "--session",
                "fixture",
            ],
        );
    }

    fn exercise_checkbox(&self) {
        let checkbox = self.find("Enable advanced mode", "checkbox");
        run_cli(
            self.cli,
            self.home,
            ["check", checkbox.trim(), "--session", "fixture"],
        );
        self.assert_checked(true);

        let checkbox = self.find("Enable advanced mode", "checkbox");
        run_cli(
            self.cli,
            self.home,
            ["uncheck", checkbox.trim(), "--session", "fixture"],
        );
        self.assert_checked(false);

        let checkbox = self.find("Enable advanced mode", "checkbox");
        run_cli(
            self.cli,
            self.home,
            ["toggle", checkbox.trim(), "--session", "fixture"],
        );
        self.assert_checked(true);
    }

    fn assert_checked(&self, expected: bool) {
        self.snapshot();
        let checkbox = self.find("Enable advanced mode", "checkbox");
        let checked = run_cli(
            self.cli,
            self.home,
            [
                "is",
                "checked",
                checkbox.trim(),
                "--json",
                "--session",
                "fixture",
            ],
        );
        let checked: serde_json::Value = serde_json::from_str(&checked).unwrap();
        assert_eq!(checked["value"], expected);
    }

    fn exercise_screenshot(&self) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let path = std::env::temp_dir().join(format!("agent-ctrl-ax-test-{stamp}.png"));
        let out = run_cli(
            self.cli,
            self.home,
            [
                "screenshot",
                path.to_str().unwrap(),
                "--annotated",
                "--json",
                "--session",
                "fixture",
            ],
        );
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["format"], "png");
        assert_eq!(value["annotated"], true);
        let width = value["width"].as_u64().unwrap();
        let height = value["height"].as_u64().unwrap();
        let bytes = value["bytes"].as_u64().unwrap();
        assert!(width > 0, "width should be > 0 (got {width})");
        assert!(height > 0, "height should be > 0 (got {height})");
        assert!(bytes > 0, "PNG should have non-zero size (got {bytes})");
        assert!(path.exists(), "PNG was not written to {}", path.display());
        let on_disk = std::fs::metadata(&path).unwrap().len();
        assert_eq!(on_disk, bytes);
        let _ = std::fs::remove_file(&path);
    }

    fn exercise_switch_app(&self) {
        // Switch to Finder via bundle id (proves NSWorkspace path), then
        // back to the fixture via executable name (proves the file-stem
        // fallback). After the round trip, snapshot must still re-pin the
        // fixture and a follow-up find must succeed.
        run_cli(
            self.cli,
            self.home,
            ["switch-app", "com.apple.finder", "--session", "fixture"],
        );
        std::thread::sleep(Duration::from_millis(400));
        run_cli(
            self.cli,
            self.home,
            [
                "switch-app",
                "agent-ctrl-ax-fixture",
                "--session",
                "fixture",
            ],
        );
        std::thread::sleep(Duration::from_millis(400));
        self.snapshot();
        let _ = self.find("Increment", "button");
    }

    fn exercise_window_list(&self) {
        let windows = run_cli(
            self.cli,
            self.home,
            ["window-list", "--json", "--session", "fixture"],
        );
        let windows: serde_json::Value = serde_json::from_str(&windows).unwrap();
        assert!(windows["windows"].as_array().unwrap().iter().any(|window| {
            window["process"] == "agent-ctrl-ax-fixture" && window["pinned"] == true
        }));
    }

    fn exercise_attached_sheet_scope(&self) {
        self.snapshot();
        let opener = self.find("Open attached sheet", "button");
        run_cli(
            self.cli,
            self.home,
            ["click", opener.trim(), "--session", "fixture"],
        );
        run_cli(
            self.cli,
            self.home,
            [
                "wait-for",
                "Attached sheet content",
                "--role",
                "region",
                "--timeout",
                "5000",
                "--session",
                "fixture",
            ],
        );

        let pinned = self.snapshot_json(&[]);
        assert_eq!(pinned["root"]["role"], "window");
        let dialog = find_node_by_role(&pinned["root"], "dialog")
            .expect("attached sheet should be nested under the pinned window");
        assert!(dialog["ref_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("scope_")));

        let scope = self.find("", "dialog");
        assert!(scope.trim().starts_with("@s"));
        let _confirm = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "Sheet Confirm",
                "--role",
                "button",
                "--in",
                scope.trim(),
                "--first",
                "--session",
                "fixture",
            ],
        );

        let windows = run_cli(
            self.cli,
            self.home,
            ["window-list", "--json", "--session", "fixture"],
        );
        let windows: serde_json::Value = serde_json::from_str(&windows).unwrap();
        assert_eq!(windows["windows"].as_array().unwrap().len(), 1);
        assert_eq!(windows["windows"][0]["pinned"], true);

        // Re-targeting by PID while the sheet owns focus must normalize back
        // to its listed parent window. Otherwise the next action reopens
        // window index 0 and cannot rediscover refs captured from the sheet.
        let pid = self.pid.to_string();
        let retargeted = self.snapshot_json(&["--target-pid", &pid]);
        assert_eq!(retargeted["root"]["role"], "window");
        assert!(find_node_by_role(&retargeted["root"], "dialog").is_some());

        let scope = self.find("", "dialog");
        let confirm = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "Sheet Confirm",
                "--role",
                "button",
                "--in",
                scope.trim(),
                "--first",
                "--session",
                "fixture",
            ],
        );
        run_cli(
            self.cli,
            self.home,
            ["click", confirm.trim(), "--session", "fixture"],
        );
        self.snapshot();
        let _ = self.find("Status: sheet confirmed", "region");
    }

    fn exercise_popover_scope(&self) {
        self.snapshot();
        let opener = self.find("Show popover", "button");
        run_cli(
            self.cli,
            self.home,
            ["click", opener.trim(), "--session", "fixture"],
        );
        run_cli(
            self.cli,
            self.home,
            [
                "wait-for",
                "Fixture Popover",
                "--role",
                "region",
                "--timeout",
                "5000",
                "--session",
                "fixture",
            ],
        );

        let snapshot = self.snapshot_json(&[]);
        assert_eq!(snapshot["root"]["role"], "window");
        let dialog = find_node_by_role(&snapshot["root"], "dialog")
            .expect("popover should be nested under the pinned window");
        assert!(dialog["ref_id"]
            .as_str()
            .is_some_and(|id| id.starts_with("scope_")));

        let scope = self.find("", "dialog");
        let close = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "Popover Close",
                "--role",
                "button",
                "--in",
                scope.trim(),
                "--first",
                "--session",
                "fixture",
            ],
        );
        run_cli(
            self.cli,
            self.home,
            ["click", close.trim(), "--session", "fixture"],
        );
        self.snapshot();
        let _ = self.find("Status: popover closed", "region");
    }

    fn snapshot_json(&self, target_args: &[&str]) -> serde_json::Value {
        let mut args = vec!["snapshot", "--json", "--session", "fixture"];
        args.extend_from_slice(target_args);
        let output = run_cli_vec(self.cli, self.home, &args);
        serde_json::from_str(&output).unwrap()
    }

    fn find(&self, name: &str, role: &str) -> String {
        if name.is_empty() {
            run_cli(
                self.cli,
                self.home,
                ["find", "--role", role, "--first", "--session", "fixture"],
            )
        } else {
            run_cli(
                self.cli,
                self.home,
                [
                    "find",
                    name,
                    "--role",
                    role,
                    "--first",
                    "--session",
                    "fixture",
                ],
            )
        }
    }
}

fn find_node_by_role<'a>(node: &'a serde_json::Value, role: &str) -> Option<&'a serde_json::Value> {
    if node["role"] == role {
        return Some(node);
    }
    node["children"]
        .as_array()?
        .iter()
        .find_map(|child| find_node_by_role(child, role))
}

fn assert_no_native_fields(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            assert!(
                !object.contains_key("native"),
                "native handle leaked: {value}"
            );
            for child in object.values() {
                assert_no_native_fields(child);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                assert_no_native_fields(item);
            }
        }
        _ => {}
    }
}

fn run_cli<const N: usize>(cli: &Path, home: &Path, args: [&str; N]) -> String {
    run_cli_vec(cli, home, &args)
}

fn run_cli_vec(cli: &Path, home: &Path, args: &[&str]) -> String {
    eprintln!("running agent-ctrl {args:?}");
    let mut child = Command::new(cli)
        .args(args)
        .env("AGENT_CTRL_HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running agent-ctrl");
    let started = Instant::now();
    let output = loop {
        if child.try_wait().expect("polling agent-ctrl").is_some() {
            break child
                .wait_with_output()
                .expect("collecting agent-ctrl output");
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .expect("collecting timed-out agent-ctrl output");
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!(
                "agent-ctrl command timed out after 30s\nargs: {args:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "agent-ctrl failed with status {:?}\nargs: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status.code(),
            args,
        );
    }
    stdout
}

fn run_cli_expect_failure<const N: usize>(cli: &Path, home: &Path, args: [&str; N]) -> String {
    eprintln!("running agent-ctrl {args:?} (expecting failure)");
    let output = Command::new(cli)
        .args(args)
        .env("AGENT_CTRL_HOME", home)
        .output()
        .expect("running agent-ctrl");
    assert!(
        !output.status.success(),
        "agent-ctrl unexpectedly succeeded"
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn run_cli_no_capture<const N: usize>(cli: &Path, home: &Path, args: [&str; N]) {
    eprintln!("running agent-ctrl {args:?}");
    let mut child = Command::new(cli)
        .args(args)
        .env("AGENT_CTRL_HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("running agent-ctrl");
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("polling agent-ctrl") {
            assert!(
                status.success(),
                "agent-ctrl failed with status {status:?}, args: {args:?}"
            );
            return;
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("agent-ctrl command timed out after 30s, args: {args:?}");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn run_cli_allow_failure<const N: usize>(cli: &Path, home: &Path, args: [&str; N]) {
    let Ok(mut child) = Command::new(cli)
        .args(args)
        .env("AGENT_CTRL_HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    let started = Instant::now();
    loop {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        if started.elapsed() > Duration::from_secs(5) {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_ready(path: &Path) {
    for _ in 0..50 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("AX fixture did not signal readiness at {}", path.display());
}

fn fixture_exe_path() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.file_name().is_some_and(|name| name == "deps") {
        path.pop();
    }
    path.push("agent-ctrl-ax-fixture");
    path
}

struct Cleanup<'a> {
    cli: PathBuf,
    home: PathBuf,
    ready: PathBuf,
    fixture: &'a mut Child,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        run_cli_allow_failure(&self.cli, &self.home, ["close", "--session", "fixture"]);
        let _ = self.fixture.kill();
        let _ = self.fixture.wait();
        let _ = std::fs::remove_dir_all(&self.home);
        let _ = std::fs::remove_file(&self.ready);
    }
}
