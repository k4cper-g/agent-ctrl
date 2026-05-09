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
    };
    run.open();
    run.snapshot();
    run.exercise_identifier_capture();
    run.exercise_button_click();
    run.exercise_double_click();
    run.exercise_hover();
    run.exercise_fill();
    run.exercise_clear();
    run.exercise_clipboard();
    run.exercise_select();
    run.exercise_checkbox();
    run.exercise_window_list();
    run.exercise_screenshot();
    run.exercise_switch_app();
}

struct FixtureRun<'a> {
    cli: &'a Path,
    home: &'a Path,
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

    fn exercise_identifier_capture(&self) {
        // Confirms that AXIdentifier is captured into NativeHandle::Ax during
        // snapshot. The fast path in resolve_element uses this identifier
        // to rediscover elements when (role, name, nth) would drift.
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
        let identifiers = collect_identifiers(&snap["root"]);
        for required in [
            "fixture-status",
            "fixture-text-field",
            "fixture-increment-button",
            "fixture-advanced-checkbox",
            "fixture-fruit-popup",
        ] {
            assert!(
                identifiers.contains(&required.to_string()),
                "snapshot did not capture identifier {required:?}; got {identifiers:?}"
            );
        }
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

fn collect_identifiers(node: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    walk_identifiers(node, &mut out);
    out
}

fn walk_identifiers(node: &serde_json::Value, out: &mut Vec<String>) {
    if let Some(id) = node
        .get("native")
        .and_then(|n| n.get("identifier"))
        .and_then(|v| v.as_str())
    {
        out.push(id.to_owned());
    }
    if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
        for child in children {
            walk_identifiers(child, out);
        }
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
