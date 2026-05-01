//! Session discovery: per-named-session state files at
//! `<home>/.agent-ctrl/<session>.json`.
//!
//! Each long-running daemon writes one of these on startup announcing the
//! TCP endpoint it's listening on, and removes it on shutdown. Short-lived
//! CLI commands read the file to find the daemon they should talk to,
//! using a `TcpStream` connect as the liveness probe — if the connect
//! fails the file is treated as stale and removed.
//!
//! This is the same shape agent-browser uses (see its
//! `cli/src/connection.rs`). We get away with TCP localhost on every
//! platform because UIA / AX / CDP are all driven from the same machine
//! the agent runs on, and localhost firewall rules block everything else.

use std::io;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Default session name used when the agent doesn't pass `--session`.
pub const DEFAULT_SESSION: &str = "default";

/// Healthcheck timeout for the connect probe. Short — a live daemon on
/// localhost answers in well under a millisecond; anything past 200ms is
/// almost certainly a dead file pointing at a recycled port.
const HEALTH_TIMEOUT: Duration = Duration::from_millis(200);

/// On-disk shape of `<home>/.agent-ctrl/<session>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    /// Session name. Matches the file stem.
    pub name: String,
    /// Daemon process id. Useful for diagnostic listings; we don't rely on
    /// it for liveness — the TCP connect probe is the source of truth.
    pub pid: u32,
    /// `host:port` the daemon is listening on. Always localhost for now.
    pub endpoint: String,
    /// Version of the `agent-ctrl-cli` crate that started the daemon.
    pub version: String,
    /// Surface kind the session was opened against (`"uia"`, `"cdp"`, etc.).
    pub surface: String,
    /// Wall-clock time the daemon started, seconds since the Unix epoch.
    pub started_at_unix: u64,
    /// UUID of the open Surface session inside the daemon — written by the
    /// daemon after it auto-opens its single session. The CLI uses this as
    /// the `session` field on every Snapshot / Act / CloseSession request,
    /// so agents never have to track session ids themselves.
    pub daemon_session_id: String,
}

/// Directory where session files live. Created on demand by [`write`].
#[must_use]
pub fn discovery_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("AGENT_CTRL_HOME") {
        return PathBuf::from(dir);
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".agent-ctrl");
    }
    // Last-resort fallback: temp dir. A daemon started with no usable home
    // is unusual enough that the agent will see the file in /tmp and figure
    // out where it came from.
    std::env::temp_dir().join("agent-ctrl")
}

/// Path of the state file for a given session name.
#[must_use]
pub fn path_for(session: &str) -> PathBuf {
    discovery_dir().join(format!("{session}.json"))
}

/// Write `info` to its session file, creating the discovery directory if
/// needed. Caller is responsible for cleanup via [`remove`].
pub fn write(info: &SessionFile) -> io::Result<()> {
    let dir = discovery_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", info.name));
    let body = serde_json::to_vec_pretty(info).map_err(io::Error::other)?;
    std::fs::write(&path, body)
}

/// Remove the session file for `session`. Missing-file is not an error —
/// callers are typically running this on shutdown and don't want noise.
pub fn remove(session: &str) -> io::Result<()> {
    let path = path_for(session);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Read a session file without any liveness check. Returns `None` when the
/// file is missing or malformed.
#[must_use]
pub fn read(session: &str) -> Option<SessionFile> {
    let path = path_for(session);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read a session file AND verify the daemon at its endpoint actually
/// responds. If the connect probe fails the file is removed and `None`
/// returned — most CLI flows want this rather than the raw [`read`].
#[must_use]
pub fn read_alive(session: &str) -> Option<SessionFile> {
    let info = read(session)?;
    let addr: SocketAddr = info.endpoint.parse().ok()?;
    if TcpStream::connect_timeout(&addr, HEALTH_TIMEOUT).is_ok() {
        Some(info)
    } else {
        let _ = remove(&info.name);
        None
    }
}

/// List every session file in the discovery directory whose endpoint
/// answers a connect probe. Stale files are pruned as a side effect, so a
/// `list` command also keeps the directory tidy.
#[must_use]
pub fn list_alive() -> Vec<SessionFile> {
    let dir = discovery_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut alive = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(info) = read_alive(stem) {
            alive.push(info);
        }
    }
    alive
}

/// Wait until the named session's file appears AND its endpoint responds.
/// Used by `agent-ctrl open` after spawning the daemon child — the spawn
/// returns immediately but the bind is async, so we poll briefly.
#[must_use]
pub fn wait_for_alive(session: &str, timeout: Duration) -> Option<SessionFile> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Some(info) = read_alive(session) {
            return Some(info);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}
