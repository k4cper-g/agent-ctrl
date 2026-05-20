//! Opt-in end-to-end Linux AT-SPI fixture coverage.
//!
//! Exercises the snapshot-read path of the `atspi` surface against the
//! deterministic GTK4 fixture (`crates/atspi-fixture/main.py`). Gated behind
//! `RUN_ATSPI_TESTS=1` and a Linux host, and expects the headless a11y stack
//! from `docker/linux-dev/` (Xvfb + a private session bus + the AT-SPI
//! registry). The action vocabulary is a follow-up, so this test only covers
//! `snapshot`, `find`, `get`, `is`, and `window-list`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn linux_atspi_fixture_snapshot_flow() {
    if std::env::var_os("RUN_ATSPI_TESTS").is_none() {
        eprintln!("skipping Linux AT-SPI fixture test; set RUN_ATSPI_TESTS=1 to run it");
        return;
    }
    if !cfg!(target_os = "linux") {
        eprintln!("skipping Linux AT-SPI fixture test on non-Linux host");
        return;
    }

    run_fixture_flow();
}

fn run_fixture_flow() {
    let cli = PathBuf::from(env!("CARGO_BIN_EXE_agent-ctrl"));
    let fixture = fixture_script_path();
    assert!(
        fixture.exists(),
        "missing AT-SPI fixture script at {}",
        fixture.display()
    );

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let home = std::env::temp_dir().join(format!("agent-ctrl-atspi-test-{stamp}"));
    let ready = std::env::temp_dir().join(format!("agent-ctrl-atspi-test-{stamp}.ready"));
    std::fs::create_dir_all(&home).unwrap();

    // Open the daemon *before* launching the fixture: opening the `atspi`
    // surface flips `org.a11y.Status.IsEnabled`, so the GTK app builds and
    // registers its accessibility tree from the moment it starts.
    let run = FixtureRun {
        cli: &cli,
        home: &home,
    };
    run.open();

    let mut fixture_child = Command::new("python3")
        .arg(&fixture)
        .arg("--ready-file")
        .arg(&ready)
        .arg("--auto-close-ms")
        .arg("120000")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("launching the AT-SPI GTK fixture (is python3 + GTK4 installed?)");
    let _guard = Cleanup {
        cli: cli.clone(),
        home: home.clone(),
        ready: ready.clone(),
        fixture: &mut fixture_child,
    };

    wait_for_ready(&ready);

    run.exercise_snapshot();
    run.exercise_find_and_inspect();
    run.exercise_window_list();
}

struct FixtureRun<'a> {
    cli: &'a Path,
    home: &'a Path,
}

impl FixtureRun<'_> {
    fn open(&self) {
        run_cli_no_capture(
            self.cli,
            self.home,
            ["open", "atspi", "--session", "fixture"],
        );
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
                "agent-ctrl-atspi-fixture",
            ],
        );
    }

    fn exercise_snapshot(&self) {
        self.snapshot();
        let json = run_cli(
            self.cli,
            self.home,
            [
                "snapshot",
                "--session",
                "fixture",
                "--target-process",
                "agent-ctrl-atspi-fixture",
                "--json",
            ],
        );
        let snap: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(snap["surface_kind"], "atspi");
        assert_eq!(snap["app"]["name"], "agent-ctrl-atspi-fixture");

        let mut refs = Vec::new();
        collect_refs(&snap["root"], &mut refs);

        // The fixture exposes a stable set of widgets; confirm the role/name
        // mapping landed each one.
        for (role, name) in [
            ("text-field", "Message"),
            ("checkbox", "Enable advanced mode"),
            ("button", "Increment"),
            ("button", "Open dialog"),
            ("list-item", "First"),
        ] {
            assert!(
                refs.iter().any(|(r, n)| r == role && n == name),
                "snapshot is missing the {role} {name:?}; captured refs: {refs:?}"
            );
        }
    }

    fn exercise_find_and_inspect(&self) {
        self.snapshot();
        // `find --first` prints a bare ref the rest of the flow composes on.
        let button = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "Increment",
                "--role",
                "button",
                "--first",
                "--session",
                "fixture",
            ],
        );
        let button = button.trim();
        assert!(
            button.starts_with("@e"),
            "unexpected find output {button:?}"
        );

        let enabled = run_cli(
            self.cli,
            self.home,
            ["is", "enabled", button, "--json", "--session", "fixture"],
        );
        let enabled: serde_json::Value = serde_json::from_str(&enabled).unwrap();
        assert_eq!(enabled["value"], true);

        // The fixture entry's accessible name is its label ("Message"); its
        // value is the text content ("fixture text").
        let entry = run_cli(
            self.cli,
            self.home,
            [
                "find",
                "Message",
                "--role",
                "text-field",
                "--first",
                "--session",
                "fixture",
            ],
        );
        let text = run_cli(
            self.cli,
            self.home,
            ["get", "text", entry.trim(), "--session", "fixture"],
        );
        assert!(
            text.contains("fixture text"),
            "entry text was {text:?}, expected it to contain \"fixture text\""
        );
    }

    fn exercise_window_list(&self) {
        self.snapshot();
        let windows = run_cli(
            self.cli,
            self.home,
            ["window-list", "--json", "--session", "fixture"],
        );
        let windows: serde_json::Value = serde_json::from_str(&windows).unwrap();
        assert!(
            windows["windows"]
                .as_array()
                .unwrap()
                .iter()
                .any(|w| { w["process"] == "agent-ctrl-atspi-fixture" && w["pinned"] == true }),
            "window-list did not report the pinned fixture window: {windows}"
        );
    }
}

fn collect_refs(node: &serde_json::Value, out: &mut Vec<(String, String)>) {
    if node
        .get("ref_id")
        .and_then(serde_json::Value::as_str)
        .is_some()
    {
        let role = node["role"].as_str().unwrap_or_default().to_owned();
        let name = node["name"].as_str().unwrap_or_default().to_owned();
        out.push((role, name));
    }
    if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
        for child in children {
            collect_refs(child, out);
        }
    }
}

fn fixture_script_path() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../crates/cli; the fixture lives alongside it.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .join("atspi-fixture")
        .join("main.py")
}

fn run_cli<const N: usize>(cli: &Path, home: &Path, args: [&str; N]) -> String {
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
            "agent-ctrl failed with status {:?}\nargs: {args:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            output.status.code(),
        );
    }
    stdout
}

fn run_cli_no_capture<const N: usize>(cli: &Path, home: &Path, args: [&str; N]) {
    eprintln!("running agent-ctrl {args:?}");
    let status = Command::new(cli)
        .args(args)
        .env("AGENT_CTRL_HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("running agent-ctrl");
    assert!(status.success(), "agent-ctrl {args:?} failed: {status:?}");
}

fn run_cli_allow_failure<const N: usize>(cli: &Path, home: &Path, args: [&str; N]) {
    let _ = Command::new(cli)
        .args(args)
        .env("AGENT_CTRL_HOME", home)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn wait_for_ready(path: &Path) {
    for _ in 0..100 {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "AT-SPI fixture did not signal readiness at {}",
        path.display()
    );
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
