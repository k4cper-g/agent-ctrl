//! Session discovery: per-named-session state files at
//! `<home>/.agent-ctrl/<session>.json`.
//!
//! Each long-running daemon writes one of these on startup announcing the
//! TCP endpoint it's listening on, and removes it on shutdown. Short-lived
//! CLI commands read the file to find the daemon they should talk to,
//! using a `TcpStream` connect as the liveness probe. Read-only discovery
//! never removes files; `agent-ctrl doctor --fix` owns stale cleanup.
//!
//! This is the same shape agent-browser uses (see its
//! `cli/src/connection.rs`). We get away with TCP localhost on every
//! platform because UIA / AX are both driven from the same machine the
//! agent runs on, and localhost firewall rules block everything else.

use std::io::{self, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(unix)]
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Default session name used when the agent doesn't pass `--session`.
pub const DEFAULT_SESSION: &str = "default";

/// Healthcheck timeout for the connect probe. Short - a live daemon on
/// localhost answers in well under a millisecond; anything past 200ms is
/// almost certainly a dead file pointing at a recycled port.
const HEALTH_TIMEOUT: Duration = Duration::from_millis(200);

/// On-disk shape of `<home>/.agent-ctrl/<session>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFile {
    /// Session name. Matches the file stem.
    pub name: String,
    /// Daemon process id. Useful for diagnostic listings; we don't rely on
    /// it for liveness - the TCP connect probe is the source of truth.
    pub pid: u32,
    /// `host:port` the daemon is listening on. Always localhost for now.
    pub endpoint: String,
    /// Version of the `agent-ctrl-cli` crate that started the daemon.
    pub version: String,
    /// Wire protocol version spoken by the daemon.
    #[serde(default)]
    pub protocol_version: u32,
    /// Surface kind the session was opened against (`"uia"`, `"ax"`, etc.).
    pub surface: String,
    /// Capabilities advertised by the opened surface.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Random bearer token required on every TCP request.
    #[serde(default)]
    pub auth_token: String,
    /// Wall-clock time the daemon started, seconds since the Unix epoch.
    pub started_at_unix: u64,
    /// UUID of the open Surface session inside the daemon - written by the
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

/// Validate a session name before using it as a state-file stem.
pub fn validate_session_name(session: &str) -> io::Result<()> {
    let valid = !session.is_empty()
        && session.len() <= 128
        && session != "."
        && session != ".."
        && session
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'));
    if valid {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "session name must be 1-128 ASCII letters, digits, dots, hyphens, or underscores",
        ))
    }
}

/// Path of the state file for a validated session name.
pub fn path_for(session: &str) -> io::Result<PathBuf> {
    validate_session_name(session)?;
    Ok(discovery_dir().join(format!("{session}.json")))
}

/// Write `info` to its session file, creating the discovery directory if
/// needed. Caller is responsible for cleanup via [`remove`].
pub fn write(info: &SessionFile) -> io::Result<()> {
    validate_session_name(&info.name)?;
    let dir = discovery_dir();
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    secure_discovery_dir(&dir)?;
    let path = dir.join(format!("{}.json", info.name));
    let body = serde_json::to_vec_pretty(info).map_err(io::Error::other)?;
    let temp_path = dir.join(format!(
        ".{}.{}.{}.tmp",
        info.name,
        std::process::id(),
        uuid::Uuid::new_v4()
    ));

    let result = (|| {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temp_path)?;
        file.write_all(&body)?;
        file.flush()?;
        file.sync_all()?;

        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        std::fs::rename(&temp_path, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(unix)]
fn secure_discovery_dir(dir: &Path) -> io::Result<()> {
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Remove the session file for `session`. Missing-file is not an error -
/// callers are typically running this on shutdown and don't want noise.
pub fn remove(session: &str) -> io::Result<()> {
    let path = path_for(session)?;
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
    let path = path_for(session).ok()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Read a session file and verify the daemon at its endpoint responds.
/// A failed probe returns `None` without modifying the discovery directory.
#[must_use]
pub fn read_alive(session: &str) -> Option<SessionFile> {
    let info = read(session)?;
    let addr: SocketAddr = info.endpoint.parse().ok()?;
    if TcpStream::connect_timeout(&addr, HEALTH_TIMEOUT).is_ok() {
        Some(info)
    } else {
        None
    }
}

/// List every session file in the discovery directory whose endpoint
/// answers a connect probe. This operation is read-only.
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
/// Used by `agent-ctrl open` after spawning the daemon child - the spawn
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

#[cfg(test)]
mod tests {
    use super::validate_session_name;

    #[test]
    fn accepts_safe_session_names() {
        for name in ["default", "desktop-1", "qa_run", "v1.2"] {
            assert!(validate_session_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn rejects_path_like_session_names() {
        for name in ["", ".", "..", "../escape", "nested/name", "back\\slash"] {
            assert!(validate_session_name(name).is_err(), "{name}");
        }
    }
}
