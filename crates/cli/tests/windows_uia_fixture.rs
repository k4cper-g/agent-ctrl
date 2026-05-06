//! Opt-in end-to-end Windows UIA fixture coverage.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn windows_uia_fixture_core_flow() {
    if std::env::var_os("RUN_UIA_TESTS").is_none() {
        eprintln!("skipping Windows UIA fixture test; set RUN_UIA_TESTS=1 to run it");
        return;
    }
    if !cfg!(target_os = "windows") {
        eprintln!("skipping Windows UIA fixture test on non-Windows host");
        return;
    }

    run_fixture_flow();
}

fn run_fixture_flow() {
    let cli = PathBuf::from(env!("CARGO_BIN_EXE_agent-ctrl"));
    let fixture = fixture_exe_path();
    assert!(
        fixture.exists(),
        "missing fixture binary at {}; run `cargo build -p agent-ctrl-uia-fixture` before RUN_UIA_TESTS=1",
        fixture.display()
    );

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let home = std::env::temp_dir().join(format!("agent-ctrl-uia-test-{stamp}"));
    let ready = std::env::temp_dir().join(format!("agent-ctrl-uia-test-{stamp}.ready"));
    std::fs::create_dir_all(&home).unwrap();

    let mut fixture_child = Command::new(&fixture)
        .arg("--ready-file")
        .arg(&ready)
        .arg("--auto-close-ms")
        .arg("60000")
        .spawn()
        .expect("launching UIA fixture");
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
    run.exercise_json_outputs();
    run.exercise_button_click();
    run.exercise_text_field();
    run.exercise_selection();
    run.exercise_checkbox();
    run.exercise_screenshots();
    run.exercise_dialog_window();
}

struct FixtureRun<'a> {
    cli: &'a Path,
    home: &'a Path,
}

impl FixtureRun<'_> {
    fn open(&self) {
        run_cli_no_capture(self.cli, self.home, ["open", "uia", "--session", "fixture"]);
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
                "agent-ctrl-uia-fixture",
            ],
        );
    }

    fn exercise_button_click(&self) {
        let button = self.find("Increment", "button");
        let clicked = run_cli(
            self.cli,
            self.home,
            ["click", button.trim(), "--session", "fixture"],
        );
        assert!(
            clicked.contains("method="),
            "expected button click method diagnostic, got {clicked:?}"
        );
        run_cli(
            self.cli,
            self.home,
            [
                "wait-for",
                "Status: count 1",
                "--role",
                "text-field",
                "--timeout",
                "5000",
                "--session",
                "fixture",
            ],
        );
    }

    fn exercise_json_outputs(&self) {
        let snapshot = run_cli(
            self.cli,
            self.home,
            [
                "snapshot",
                "--session",
                "fixture",
                "--target-process",
                "agent-ctrl-uia-fixture",
                "--json",
            ],
        );
        let snapshot: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        assert_eq!(snapshot["surface_kind"], "uia");

        let matches = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "Increment",
                "--role",
                "button",
                "--json",
                "--session",
                "fixture",
            ],
        );
        let matches: serde_json::Value = serde_json::from_str(&matches).unwrap();
        let ref_id = matches["first"]["ref_id"].as_str().unwrap();

        let name = run_cli(
            self.cli,
            self.home,
            ["get", "name", ref_id, "--json", "--session", "fixture"],
        );
        let name: serde_json::Value = serde_json::from_str(&name).unwrap();
        assert_eq!(name["value"], "Increment");

        let enabled = run_cli(
            self.cli,
            self.home,
            ["is", "enabled", ref_id, "--json", "--session", "fixture"],
        );
        let enabled: serde_json::Value = serde_json::from_str(&enabled).unwrap();
        assert_eq!(enabled["value"], true);

        let windows = run_cli(
            self.cli,
            self.home,
            ["window-list", "--json", "--session", "fixture"],
        );
        let windows: serde_json::Value = serde_json::from_str(&windows).unwrap();
        assert!(windows["windows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| { w["process"] == "agent-ctrl-uia-fixture" && w["pinned"] == true }));

        let waited = run_cli(
            self.cli,
            self.home,
            ["wait", "1", "--json", "--session", "fixture"],
        );
        let waited: serde_json::Value = serde_json::from_str(&waited).unwrap();
        assert_eq!(waited["ok"], true);
    }

    fn exercise_text_field(&self) {
        let field = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "--role",
                "text-field",
                "--first",
                "--session",
                "fixture",
            ],
        );
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
        run_cli(
            self.cli,
            self.home,
            [
                "wait-for",
                "--role",
                "text-field",
                "--value-contains",
                "fixture edited",
                "--timeout",
                "5000",
                "--session",
                "fixture",
            ],
        );
        run_cli(
            self.cli,
            self.home,
            ["clear", field.trim(), "--session", "fixture"],
        );
        self.snapshot();
        let field = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "--role",
                "text-field",
                "--first",
                "--session",
                "fixture",
            ],
        );
        let value = run_cli(
            self.cli,
            self.home,
            ["get", "value", field.trim(), "--session", "fixture"],
        );
        assert!(
            matches!(value.trim(), "\"\"" | "null"),
            "expected cleared text field, got {value:?}"
        );
    }

    fn exercise_selection(&self) {
        let option = self.find("Second", "option");
        let selected = run_cli(
            self.cli,
            self.home,
            ["select", option.trim(), "Second", "--session", "fixture"],
        );
        assert!(
            selected.contains("method=selection-item-pattern"),
            "expected select diagnostic, got {selected:?}"
        );
        run_cli(
            self.cli,
            self.home,
            [
                "wait-for",
                "Second",
                "--role",
                "option",
                "--state",
                "selected",
                "--timeout",
                "5000",
                "--session",
                "fixture",
            ],
        );
    }

    fn exercise_checkbox(&self) {
        let checkbox = self.find("Enable advanced mode", "checkbox");
        let checked = run_cli(
            self.cli,
            self.home,
            ["check", checkbox.trim(), "--session", "fixture"],
        );
        assert!(
            checked.contains("method=toggle-pattern"),
            "expected check diagnostic, got {checked:?}"
        );
        run_cli(
            self.cli,
            self.home,
            [
                "wait-for",
                "Enable advanced mode",
                "--role",
                "checkbox",
                "--state",
                "checked",
                "--timeout",
                "5000",
                "--session",
                "fixture",
            ],
        );
    }

    fn exercise_screenshots(&self) {
        self.snapshot();
        let checkbox = self.find("Enable advanced mode", "checkbox");
        let window_png = self.home.join("fixture-window.png");
        let ref_png = self.home.join("fixture-ref.png");
        let region_png = self.home.join("fixture-region.png");
        let annotated_png = self.home.join("fixture-annotated.png");

        let window_meta = self.screenshot(
            &window_png,
            &[
                "screenshot",
                "--target",
                "window",
                "--json",
                "--session",
                "fixture",
            ],
        );
        let window_meta: serde_json::Value = serde_json::from_str(&window_meta).unwrap();
        assert_eq!(window_meta["format"], "png");
        assert_eq!(window_meta["encoding"], "file");
        assert_eq!(window_meta["path"], window_png.display().to_string());
        self.screenshot(
            &ref_png,
            &[
                "screenshot",
                "--target",
                "ref",
                "--ref",
                checkbox.trim(),
                "--session",
                "fixture",
            ],
        );
        self.screenshot(
            &region_png,
            &[
                "screenshot",
                "--target",
                "region",
                "--region",
                "0,0,64,64",
                "--session",
                "fixture",
            ],
        );
        self.screenshot(
            &annotated_png,
            &[
                "screenshot",
                "--target",
                "window",
                "--annotated",
                "--session",
                "fixture",
            ],
        );

        assert_png(&window_png, 100, 100);
        assert_png(&ref_png, 10, 10);
        assert_png(&region_png, 64, 64);
        assert_png(&annotated_png, 100, 100);
        let plain = std::fs::read(&window_png).unwrap();
        let annotated = std::fs::read(&annotated_png).unwrap();
        assert_ne!(
            plain, annotated,
            "annotated screenshot should alter PNG bytes"
        );
    }

    fn exercise_dialog_window(&self) {
        self.snapshot();
        let opener = self.find("Open dialog", "button");
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
                "--window-appears",
                "Fixture Secondary Dialog",
                "--timeout",
                "5000",
                "--session",
                "fixture",
            ],
        );
        let dialog_id = run_cli(
            self.cli,
            self.home,
            ["window-list", "--first-other", "--session", "fixture"],
        );
        run_cli(
            self.cli,
            self.home,
            ["focus-window", dialog_id.trim(), "--session", "fixture"],
        );
        run_cli(self.cli, self.home, ["snapshot", "--session", "fixture"]);

        let ok = self.find("Dialog OK", "button");
        run_cli(
            self.cli,
            self.home,
            ["click", ok.trim(), "--session", "fixture"],
        );
        self.snapshot();
        run_cli(
            self.cli,
            self.home,
            [
                "wait-for",
                "--window-gone",
                "Fixture Secondary Dialog",
                "--timeout",
                "5000",
                "--session",
                "fixture",
            ],
        );
    }

    fn find(&self, name: &str, role: &str) -> String {
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

    fn screenshot(&self, path: &Path, args: &[&str]) -> String {
        let mut full_args = Vec::with_capacity(args.len() + 1);
        full_args.push("screenshot");
        full_args.push(path.to_str().expect("screenshot path must be UTF-8"));
        full_args.extend(args.iter().copied().skip(1));
        run_cli_vec(self.cli, self.home, &full_args)
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

fn assert_png(path: &Path, min_width: u32, min_height: u32) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    assert!(
        bytes.len() >= 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
        "{} is not a PNG",
        path.display()
    );
    assert_eq!(&bytes[12..16], b"IHDR", "{} missing IHDR", path.display());
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    assert!(
        width >= min_width && height >= min_height,
        "{} dimensions {width}x{height} below expected minimum {min_width}x{min_height}",
        path.display()
    );
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
    panic!("UIA fixture did not signal readiness at {}", path.display());
}

fn fixture_exe_path() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.file_name().is_some_and(|name| name == "deps") {
        path.pop();
    }
    path.push("agent-ctrl-uia-fixture.exe");
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
