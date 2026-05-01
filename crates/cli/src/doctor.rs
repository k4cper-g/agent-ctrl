//! `agent-ctrl doctor` - diagnose an agent-ctrl install.
//!
//! Modeled on agent-browser's [`cli/src/doctor`](../../../agent-browser/cli/src/doctor/).
//! Each check returns `pass` / `warn` / `fail` / `info` plus a message
//! and an optional fix hint. `--json` emits the same structure as a
//! parseable payload; `--fix` attempts the safe automatic repairs
//! (currently: prune stale session files).
//!
//! Categories:
//!
//! - `environment` - OS, build, surface compile status, home dir
//! - `daemon` - discovery dir, stale session files, active sessions
//! - `probe` - round-trip a request through a freshly-spawned mock daemon
//!   child; the strongest signal that the local install actually works
//!   end-to-end. Skip with `--quick`.
//!
//! New categories will land as new surfaces arrive (CDP wants a Chrome
//! check; AX wants a permission check; Android wants a binding check).

use std::path::PathBuf;
use std::time::Duration;

use agent_ctrl_core::SurfaceKind;
use agent_ctrl_daemon::{session_file, surface_status, SurfaceStatus};
use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DoctorOptions {
    pub(crate) json: bool,
    pub(crate) fix: bool,
    /// Skip the live mock-daemon probe. Speeds up `doctor` from ~1s to
    /// instant, at the cost of not verifying the spawn / TCP / dispatch
    /// path actually works.
    pub(crate) quick: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Status {
    Pass,
    Warn,
    Fail,
    Info,
}

impl Status {
    fn marker(self) -> &'static str {
        match self {
            Self::Pass => "✓",
            Self::Warn => "!",
            Self::Fail => "✗",
            Self::Info => "·",
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Fail => "fail",
            Self::Info => "info",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Check {
    pub(crate) id: String,
    pub(crate) category: &'static str,
    pub(crate) status: Status,
    pub(crate) message: String,
    pub(crate) fix: Option<String>,
}

impl Check {
    fn new(id: &str, category: &'static str, status: Status, message: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            category,
            status,
            message: message.into(),
            fix: None,
        }
    }
    fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

/// Run the doctor and print results. Failed checks are reported through
/// the printout itself, not through the function's `Result`.
//
// Returns `Result` to compose cleanly with the rest of the CLI dispatch
// (every other command returns `Result`); flagged by clippy as
// unnecessary in isolation.
#[allow(clippy::unnecessary_wraps)]
pub(crate) fn run_doctor(opts: DoctorOptions) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();
    let mut fixed: Vec<String> = Vec::new();

    check_environment(&mut checks);
    check_daemon(&mut checks);
    if !opts.quick {
        check_probe(&mut checks);
    }

    if opts.fix {
        run_fixes(&mut checks, &mut fixed);
    }

    let summary = summarize(&checks);

    if opts.json {
        print_json(&checks, &summary, &fixed);
    } else {
        print_text(&checks, &summary, &fixed, opts.fix);
    }
    Ok(())
}

// ---------- environment ----------

fn check_environment(out: &mut Vec<Check>) {
    out.push(Check::new(
        "env.version",
        "environment",
        Status::Info,
        format!(
            "agent-ctrl {} on {}/{}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
        ),
    ));

    for kind in [
        SurfaceKind::Mock,
        SurfaceKind::Uia,
        SurfaceKind::Ax,
        SurfaceKind::Cdp,
        SurfaceKind::Android,
        SurfaceKind::Ios,
    ] {
        let (status, msg) = match surface_status(kind) {
            SurfaceStatus::Ready => (Status::Pass, format!("surface {}: ready", kind.as_str())),
            SurfaceStatus::Stub => (
                Status::Warn,
                format!(
                    "surface {}: scaffold only - open will succeed but actions return Unsupported",
                    kind.as_str()
                ),
            ),
            SurfaceStatus::WrongOs => (
                Status::Info,
                format!(
                    "surface {}: not available on {} (different OS)",
                    kind.as_str(),
                    std::env::consts::OS
                ),
            ),
            // NotImplemented and (the future case of) any other "neutral"
            // status both want the same `Info` line - distinct from Stub
            // (warn, partial) and WrongOs (info, OS-gated).
            SurfaceStatus::NotImplemented => (
                Status::Info,
                format!("surface {}: not implemented yet", kind.as_str()),
            ),
        };
        out.push(Check::new(
            &format!("env.surface.{}", kind.as_str()),
            "environment",
            status,
            msg,
        ));
    }
}

// ---------- daemon / sessions ----------

fn check_daemon(out: &mut Vec<Check>) {
    let dir = session_file::discovery_dir();
    match ensure_dir_writable(&dir) {
        Ok(()) => out.push(Check::new(
            "daemon.home",
            "daemon",
            Status::Pass,
            format!("home dir is writable: {}", dir.display()),
        )),
        Err(reason) => out.push(
            Check::new(
                "daemon.home",
                "daemon",
                Status::Fail,
                format!("home dir not writable: {} ({reason})", dir.display()),
            )
            .with_fix(format!(
                "ensure {} exists and is writable, or set AGENT_CTRL_HOME to an alternative",
                dir.display()
            )),
        ),
    }

    // Discover stale files BEFORE counting active sessions: `list_alive`
    // prunes stale files as a side effect of its TCP probe, so checking
    // it first would silently remove the very files we want to surface.
    let stale = list_stale(&dir);

    let alive = session_file::list_alive();
    out.push(Check::new(
        "daemon.active",
        "daemon",
        Status::Info,
        format!("{} active session(s)", alive.len()),
    ));
    if stale.is_empty() {
        out.push(Check::new(
            "daemon.stale",
            "daemon",
            Status::Pass,
            "no stale session files",
        ));
    } else {
        let names: Vec<String> = stale
            .iter()
            .filter_map(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(ToOwned::to_owned)
            })
            .collect();
        out.push(
            Check::new(
                "daemon.stale",
                "daemon",
                Status::Warn,
                format!(
                    "{} stale session file(s): {}",
                    stale.len(),
                    names.join(", ")
                ),
            )
            .with_fix("re-run with --fix to remove them".to_owned()),
        );
    }
}

/// Walk the discovery dir for `.json` files whose endpoint no longer
/// responds. `read_alive` already prunes them as a side effect, so this
/// just collects the paths it removed and any remaining duds.
fn list_stale(dir: &PathBuf) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut stale = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // read_alive removes the file and returns None when the endpoint
        // doesn't answer; catching the case before that requires reading
        // the file ourselves.
        if let Some(info) = session_file::read(stem) {
            // Try to parse the endpoint and probe it directly. We don't
            // call `read_alive` because we want to collect the path of
            // the stale file before it's deleted (read_alive deletes on
            // probe failure).
            let alive = info
                .endpoint
                .parse::<std::net::SocketAddr>()
                .ok()
                .and_then(|addr| {
                    std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok()
                })
                .is_some();
            if !alive {
                stale.push(path);
            }
        } else {
            // Unparseable file - also a candidate for cleanup.
            stale.push(path);
        }
    }
    stale
}

fn ensure_dir_writable(dir: &PathBuf) -> std::result::Result<(), String> {
    if let Err(e) = std::fs::create_dir_all(dir) {
        return Err(e.to_string());
    }
    let probe = dir.join(".agent-ctrl-doctor-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

// ---------- probe ----------

fn check_probe(out: &mut Vec<Check>) {
    match probe_mock_roundtrip() {
        Ok(refs) => out.push(Check::new(
            "probe.mock",
            "probe",
            Status::Pass,
            format!("spawned mock daemon and round-tripped a snapshot ({refs} refs)"),
        )),
        Err(reason) => out.push(
            Check::new(
                "probe.mock",
                "probe",
                Status::Fail,
                format!("mock daemon round-trip failed: {reason}"),
            )
            .with_fix(
                "re-run `cargo build -p agent-ctrl-cli` and check the daemon's stderr".to_owned(),
            ),
        ),
    }
}

/// Spawn an `agent-ctrl daemon --bind 127.0.0.1:0 --surface mock` child,
/// wait for the state file, send one snapshot, send Shutdown, return
/// the ref count. Stops on first error.
fn probe_mock_roundtrip() -> std::result::Result<usize, String> {
    use agent_ctrl_daemon::{client, RequestOp, ResponseBody};

    let session_name = format!("doctor-probe-{}", std::process::id());
    let exe = std::env::current_exe().map_err(|e| format!("locating self: {e}"))?;

    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("daemon")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--session")
        .arg(&session_name)
        .arg("--surface")
        .arg("mock")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }

    let mut child = cmd.spawn().map_err(|e| format!("spawn: {e}"))?;
    let info = session_file::wait_for_alive(&session_name, Duration::from_secs(5))
        .ok_or_else(|| "daemon did not become ready within 5s".to_owned())?;

    // Always send Shutdown, even on snapshot failure, so we don't leak
    // a daemon process.
    let session_id_str = info.daemon_session_id.clone();
    let session_id: agent_ctrl_daemon::SessionId =
        serde_json::from_str(&format!("\"{session_id_str}\""))
            .map_err(|e| format!("parse session id: {e}"))?;

    let snap_result = client::send(
        &info,
        RequestOp::Snapshot {
            session: session_id,
            opts: agent_ctrl_core::SnapshotOptions::default(),
        },
    );

    let _ = client::send(&info, RequestOp::Shutdown);
    let _ = child.wait();
    let _ = session_file::remove(&session_name);

    let resp = snap_result.map_err(|e| format!("snapshot: {e}"))?;
    match resp.body {
        ResponseBody::Snapshot { snapshot } => Ok(snapshot.refs.len()),
        ResponseBody::Error { message } => Err(message),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

// ---------- fixes ----------

fn run_fixes(checks: &mut [Check], fixed: &mut Vec<String>) {
    for c in checks.iter_mut() {
        if c.id == "daemon.stale" && c.status == Status::Warn {
            let dir = session_file::discovery_dir();
            for path in list_stale(&dir) {
                if std::fs::remove_file(&path).is_ok() {
                    fixed.push(format!("removed stale session file: {}", path.display()));
                }
            }
            c.status = Status::Pass;
            "no stale session files".clone_into(&mut c.message);
            c.fix = None;
        }
    }
}

// ---------- summary / output ----------

#[derive(Debug, Default)]
struct Summary {
    pass: usize,
    warn: usize,
    fail: usize,
}

fn summarize(checks: &[Check]) -> Summary {
    let mut s = Summary::default();
    for c in checks {
        match c.status {
            Status::Pass => s.pass += 1,
            Status::Warn => s.warn += 1,
            Status::Fail => s.fail += 1,
            Status::Info => {}
        }
    }
    s
}

fn print_text(checks: &[Check], summary: &Summary, fixed: &[String], fix_ran: bool) {
    println!("agent-ctrl doctor");

    let mut current = "";
    for c in checks {
        if c.category != current {
            current = c.category;
            println!();
            println!("{current}");
        }
        println!("  {}  {}", c.status.marker(), c.message);
        if let Some(f) = &c.fix {
            println!("        fix: {f}");
        }
    }

    if !fixed.is_empty() {
        println!();
        println!("fixed");
        for line in fixed {
            println!("  ✓  {line}");
        }
    }

    println!();
    println!(
        "summary: {} pass, {} warn, {} fail",
        summary.pass, summary.warn, summary.fail
    );

    if !fix_ran && checks.iter().any(|c| c.fix.is_some()) {
        println!("tip: re-run with --fix to attempt the suggested repairs");
    }
}

fn print_json(checks: &[Check], summary: &Summary, fixed: &[String]) {
    let checks_json: Vec<Value> = checks
        .iter()
        .map(|c| {
            let mut obj = json!({
                "id": c.id,
                "category": c.category,
                "status": c.status.as_str(),
                "message": c.message,
            });
            if let Some(f) = &c.fix {
                obj["fix"] = json!(f);
            }
            obj
        })
        .collect();
    let payload = json!({
        "success": summary.fail == 0,
        "summary": {
            "pass": summary.pass,
            "warn": summary.warn,
            "fail": summary.fail,
        },
        "checks": checks_json,
        "fixed": fixed,
    });
    println!("{payload}");
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn summary_counts_each_status() {
        let checks = vec![
            Check::new("a", "x", Status::Pass, ""),
            Check::new("b", "x", Status::Pass, ""),
            Check::new("c", "x", Status::Warn, ""),
            Check::new("d", "x", Status::Fail, ""),
            Check::new("e", "x", Status::Info, ""),
        ];
        let s = summarize(&checks);
        assert_eq!(s.pass, 2);
        assert_eq!(s.warn, 1);
        assert_eq!(s.fail, 1);
    }

    #[test]
    fn check_with_fix_records_hint() {
        let c = Check::new("id", "cat", Status::Warn, "msg").with_fix("do thing");
        assert_eq!(c.fix.as_deref(), Some("do thing"));
    }

    #[test]
    fn environment_check_emits_one_entry_per_surface() {
        let mut out = Vec::new();
        check_environment(&mut out);
        // Six surfaces + the version info line.
        let surface_checks = out
            .iter()
            .filter(|c| c.id.starts_with("env.surface."))
            .count();
        assert_eq!(surface_checks, 6);
        assert!(out.iter().any(|c| c.id == "env.version"));
    }
}
