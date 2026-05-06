//! CLI JSON contract tests against the mock surface.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[test]
fn json_errors_cover_parse_and_runtime_failures() {
    let cli = cli_path();
    let parse = run_cli(&cli, None, &["click", "--json"]);
    assert_eq!(parse.status.code(), Some(2));
    let body = parse_json(&parse);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "invalid_request");
    assert!(body["error"]["hint"].as_str().unwrap().contains("--help"));

    let home = unique_home("agent-ctrl-json-contract-error");
    let runtime = run_cli(
        &cli,
        Some(&home),
        &["snapshot", "--json", "--session", "missing"],
    );
    assert_eq!(runtime.status.code(), Some(1));
    let body = parse_json(&runtime);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "not_ready");
    assert!(body["error"]["hint"]
        .as_str()
        .unwrap()
        .contains("agent-ctrl open"));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn mock_session_json_success_shapes_are_stable() {
    let cli = cli_path();
    let home = unique_home("agent-ctrl-json-contract-success");
    let _guard = SessionCleanup {
        cli: cli.clone(),
        home: home.clone(),
        session: "contract".to_owned(),
    };

    let opened = expect_success(run_cli(
        &cli,
        Some(&home),
        &["open", "--json", "mock", "--session", "contract"],
    ));
    assert_eq!(opened["ok"], true);
    assert_eq!(opened["session"]["name"], "contract");
    assert_token_redacted(&opened);

    let listed = expect_success(run_cli(&cli, Some(&home), &["list", "--json"]));
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(listed["sessions"][0]["name"], "contract");
    assert_token_redacted(&listed);

    let snapshot = expect_success(run_cli(
        &cli,
        Some(&home),
        &["snapshot", "--json", "--session", "contract"],
    ));
    assert_eq!(snapshot["surface_kind"], "mock");
    assert_eq!(snapshot["root"]["name"], "Mock Window");

    let found = expect_success(run_cli(
        &cli,
        Some(&home),
        &[
            "find",
            "OK",
            "--role",
            "button",
            "--json",
            "--session",
            "contract",
        ],
    ));
    assert_eq!(found["first"]["ref_id"], "ref_0");
    assert_eq!(found["matches"].as_array().unwrap().len(), 1);

    let name = expect_success(run_cli(
        &cli,
        Some(&home),
        &["get", "name", "ref_0", "--json", "--session", "contract"],
    ));
    assert_eq!(name["field"], "name");
    assert_eq!(name["value"], "OK");

    let enabled = expect_success(run_cli(
        &cli,
        Some(&home),
        &["is", "enabled", "ref_0", "--json", "--session", "contract"],
    ));
    assert_eq!(enabled["field"], "enabled");
    assert_eq!(enabled["value"], true);

    let clicked = expect_success(run_cli(
        &cli,
        Some(&home),
        &["click", "ref_0", "--json", "--session", "contract"],
    ));
    assert_eq!(clicked["ok"], true);

    let waited = expect_success(run_cli(
        &cli,
        Some(&home),
        &["wait", "1", "--json", "--session", "contract"],
    ));
    assert_eq!(waited["ok"], true);

    let windows = expect_success(run_cli(
        &cli,
        Some(&home),
        &["window-list", "--json", "--session", "contract"],
    ));
    assert_eq!(windows["windows"][0]["id"], "mock-window");
    assert_eq!(windows["windows"][0]["pinned"], true);

    let closed = expect_success(run_cli(
        &cli,
        Some(&home),
        &["close", "--json", "--session", "contract"],
    ));
    assert_eq!(closed["ok"], true);
}

#[test]
fn json_query_misses_preserve_exit_codes_and_structured_stdout() {
    let cli = cli_path();
    let home = unique_home("agent-ctrl-json-contract-miss");
    let _guard = SessionCleanup {
        cli: cli.clone(),
        home: home.clone(),
        session: "miss".to_owned(),
    };

    let opened = expect_success(run_cli(
        &cli,
        Some(&home),
        &["open", "--json", "mock", "--session", "miss"],
    ));
    assert_eq!(opened["session"]["name"], "miss");
    expect_success(run_cli(
        &cli,
        Some(&home),
        &["snapshot", "--json", "--session", "miss"],
    ));

    let missing = run_cli(
        &cli,
        Some(&home),
        &["find", "Definitely Missing", "--json", "--session", "miss"],
    );
    assert_eq!(missing.status.code(), Some(1));
    let missing = parse_json(&missing);
    assert!(missing["matches"].as_array().unwrap().is_empty());
    assert!(missing["first"].is_null());

    let timeout = run_cli(
        &cli,
        Some(&home),
        &[
            "wait-for",
            "Definitely Missing",
            "--json",
            "--timeout",
            "1",
            "--poll",
            "50",
            "--session",
            "miss",
        ],
    );
    assert_eq!(timeout.status.code(), Some(2));
    let timeout = parse_json(&timeout);
    assert_eq!(timeout["outcome"], "timeout");
    assert!(timeout["elapsed_ms"].is_number());
}

fn cli_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_agent-ctrl"))
}

fn unique_home(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn expect_success(output: Output) -> Value {
    let Output {
        status,
        stdout,
        stderr,
    } = output;
    if !status.success() {
        panic_bytes("agent-ctrl command failed", status.code(), &stdout, &stderr);
    }
    serde_json::from_slice(&stdout).unwrap_or_else(|e| {
        let stdout = String::from_utf8_lossy(&stdout);
        let stderr = String::from_utf8_lossy(&stderr);
        panic!("stdout was not JSON: {e}\nstdout:\n{stdout}\nstderr:\n{stderr}");
    })
}

fn parse_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("stdout was not JSON: {e}\nstdout:\n{stdout}\nstderr:\n{stderr}");
    })
}

fn assert_token_redacted(value: &Value) {
    let text = serde_json::to_string(value).unwrap();
    assert!(
        !text.contains("auth_token"),
        "JSON output leaked auth_token: {text}"
    );
}

fn run_cli(cli: &Path, home: Option<&Path>, args: &[&str]) -> Output {
    eprintln!("running agent-ctrl {args:?}");
    let mut cmd = Command::new(cli);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(home) = home {
        cmd.env("AGENT_CTRL_HOME", home);
    }
    let child = cmd.spawn().expect("running agent-ctrl");
    wait_for_output(child, args, Duration::from_secs(30))
}

fn wait_for_output(mut child: Child, args: &[&str], timeout: Duration) -> Output {
    let started = Instant::now();
    loop {
        if child.try_wait().expect("polling agent-ctrl").is_some() {
            return child
                .wait_with_output()
                .expect("collecting agent-ctrl output");
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .expect("collecting timed-out agent-ctrl output");
            panic_output(&format!("agent-ctrl timed out, args: {args:?}"), &output);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn panic_output(message: &str, output: &Output) -> ! {
    panic_bytes(
        message,
        output.status.code(),
        &output.stdout,
        &output.stderr,
    );
}

fn panic_bytes(message: &str, code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> ! {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    panic!("{message}\nstatus: {code:?}\nstdout:\n{stdout}\nstderr:\n{stderr}");
}

struct SessionCleanup {
    cli: PathBuf,
    home: PathBuf,
    session: String,
}

impl Drop for SessionCleanup {
    fn drop(&mut self) {
        let _ = run_cli(
            &self.cli,
            Some(&self.home),
            &["close", "--json", "--session", &self.session],
        );
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
