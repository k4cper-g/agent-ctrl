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

    // Terminate any fixture left running by an earlier, improperly torn-down
    // run. `snapshot --target-process` resolves the target window by its
    // executable name, so a second live instance makes targeting ambiguous -
    // actions could drive one process while reads observe the other, which
    // surfaces as a "cleared" field still showing stale characters.
    kill_stale_fixtures();

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
    run.exercise_ref_round_trip();
    run.exercise_json_outputs();
    run.exercise_button_click();
    run.exercise_text_field();
    run.exercise_selection();
    run.exercise_checkbox();
    run.exercise_slider();
    run.exercise_password();
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

    /// Every ref the snapshot emitted must round-trip through the action path.
    ///
    /// `screenshot --target ref` resolves the ref via `resolve_element` (the
    /// same AutomationId -> RuntimeId -> (role, name, nth) ladder every action
    /// uses) and is otherwise a pure read. Running it for every ref in one
    /// batch is a regression guard for the most delicate invariant in the UIA
    /// surface: the predicate deciding which elements get a ref at snapshot
    /// time must exactly match the one the action-time walk uses to count
    /// `nth`; if they drift, some refs stop resolving and this fails, naming
    /// the offending `(ref_id, role, name)`.
    fn exercise_ref_round_trip(&self) {
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
        let mut refs: Vec<(String, String, String)> = Vec::new();
        collect_refs(&snapshot["root"], &mut refs);
        assert!(
            refs.len() >= 5,
            "expected the fixture snapshot to emit several refs, got {}",
            refs.len()
        );

        let steps: Vec<serde_json::Value> = refs
            .iter()
            .map(|(ref_id, _, _)| {
                serde_json::json!({
                    "op": "act",
                    "action": { "kind": "screenshot", "target": { "kind": "ref", "ref_id": ref_id } }
                })
            })
            .collect();
        let steps_json = serde_json::to_string(&steps).unwrap();
        let out = run_cli_vec(
            self.cli,
            self.home,
            &["batch", &steps_json, "--session", "fixture"],
        );
        let outcomes: serde_json::Value = serde_json::from_str(&out).unwrap();
        let outcomes = outcomes.as_array().expect("batch outcomes array");
        assert_eq!(
            outcomes.len(),
            refs.len(),
            "batch returned {} outcomes for {} refs",
            outcomes.len(),
            refs.len()
        );
        for (outcome, (ref_id, role, name)) in outcomes.iter().zip(&refs) {
            assert_eq!(
                outcome["ok"], true,
                "ref {ref_id} (role={role} name={name:?}) failed to re-resolve: {}",
                outcome["error"]
            );
        }
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
        let snap = run_cli(
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
            "expected cleared text field, got {value:?}\nfield ref: {field:?}\npost-clear snapshot:\n{snap}"
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

    /// The fixture trackbar exposes its position through `RangeValuePattern`,
    /// not `ValuePattern`. A snapshot of it must surface `value` via the
    /// range-value fallback, and the slider role must earn a ref.
    fn exercise_slider(&self) {
        self.snapshot();
        let slider = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "--role",
                "slider",
                "--first",
                "--session",
                "fixture",
            ],
        );
        assert!(
            slider.trim().starts_with("@e") || slider.trim().starts_with("ref_"),
            "expected a ref for the slider, got {slider:?}"
        );
        let value = run_cli(
            self.cli,
            self.home,
            ["get", "value", slider.trim(), "--session", "fixture"],
        );
        assert!(
            value.contains("40"),
            "expected the fixture trackbar to report RangeValue 40, got {value:?}"
        );
    }

    /// The fixture's password edit (`ES_PASSWORD`) must keep its ref - it is
    /// still a text field an agent can focus and fill - while its content is
    /// withheld from the snapshot entirely.
    fn exercise_password(&self) {
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
        assert!(
            !snapshot.contains("hunter2secret"),
            "password edit content leaked into the snapshot:\n{snapshot}"
        );
        let snapshot: serde_json::Value = serde_json::from_str(&snapshot).unwrap();
        let mut text_fields = Vec::new();
        collect_role_nodes(&snapshot["root"], "text-field", &mut text_fields);
        assert!(
            text_fields.len() >= 2,
            "expected the plain and password edits as text-field nodes, got {}",
            text_fields.len()
        );
        assert!(
            text_fields.iter().all(|node| node.get("ref_id").is_some()),
            "every text field, the password edit included, must keep a ref"
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

/// Depth-first collect every node whose `role` equals `role`.
fn collect_role_nodes<'a>(
    node: &'a serde_json::Value,
    role: &str,
    out: &mut Vec<&'a serde_json::Value>,
) {
    if node.get("role").and_then(serde_json::Value::as_str) == Some(role) {
        out.push(node);
    }
    if let Some(children) = node.get("children").and_then(serde_json::Value::as_array) {
        for child in children {
            collect_role_nodes(child, role, out);
        }
    }
}

/// Depth-first collect `(ref_id, role, name)` for every node carrying a ref.
fn collect_refs(node: &serde_json::Value, out: &mut Vec<(String, String, String)>) {
    if let Some(ref_id) = node.get("ref_id").and_then(serde_json::Value::as_str) {
        let role = node
            .get("role")
            .map(ToString::to_string)
            .unwrap_or_default();
        let name = node
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        out.push((ref_id.to_string(), role, name));
    }
    if let Some(children) = node.get("children").and_then(serde_json::Value::as_array) {
        for child in children {
            collect_refs(child, out);
        }
    }
}

fn run_cli<const N: usize>(cli: &Path, home: &Path, args: [&str; N]) -> String {
    run_cli_vec(cli, home, &args)
}

fn run_cli_vec(cli: &Path, home: &Path, args: &[&str]) -> String {
    use std::io::Read;

    eprintln!("running agent-ctrl {args:?}");
    let mut child = Command::new(cli)
        .args(args)
        .env("AGENT_CTRL_HOME", home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("running agent-ctrl");

    // Drain stdout and stderr on dedicated threads. A command like `batch`
    // with many `screenshot` steps emits more than the OS pipe buffer holds;
    // if we waited for the child to exit before reading, it would block
    // writing into a full pipe and never exit (a classic deadlock). Reading
    // concurrently keeps the pipe moving regardless of output size.
    let mut stdout_pipe = child.stdout.take().expect("child stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("child stderr is piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });
    let collect = |reader: std::thread::JoinHandle<Vec<u8>>| {
        String::from_utf8_lossy(&reader.join().expect("output reader thread")).into_owned()
    };

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().expect("polling agent-ctrl") {
            break status;
        }
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = collect(stdout_reader);
            let stderr = collect(stderr_reader);
            panic!(
                "agent-ctrl command timed out after 30s\nargs: {args:?}\nstdout:\n{stdout}\nstderr:\n{stderr}"
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let stdout = collect(stdout_reader);
    if !status.success() {
        let stderr = collect(stderr_reader);
        panic!(
            "agent-ctrl failed with status {:?}\nargs: {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            status.code(),
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

/// Best-effort termination of any leftover fixture process so this run's
/// `--target-process` resolution is unambiguous. A missing process (nothing
/// to kill) just makes `taskkill` exit non-zero, which we ignore.
fn kill_stale_fixtures() {
    let _ = Command::new("taskkill")
        .args(["/F", "/IM", "agent-ctrl-uia-fixture.exe"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
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
