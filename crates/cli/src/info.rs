//! `agent-ctrl info` — cheap, side-effect-free "what does this binary
//! know about itself?" probe. Designed for an agent to run as the first
//! command in a fresh session: it answers OS, build version, which
//! surfaces are usable, where the home directory is, and whether any
//! daemons are already running.
//!
//! No network. No process spawns. No file I/O beyond reading the
//! discovery directory. Live probes belong in `doctor`.
//!
//! `--json` emits a stable JSON shape parseable without regex.

use agent_ctrl_core::SurfaceKind;
use agent_ctrl_daemon::{session_file, surface_status, SurfaceStatus};
use anyhow::Result;
use serde::Serialize;

const ALL_SURFACES: &[SurfaceKind] = &[
    SurfaceKind::Mock,
    SurfaceKind::Uia,
    SurfaceKind::Ax,
    SurfaceKind::Cdp,
    SurfaceKind::Android,
    SurfaceKind::Ios,
];

#[derive(Debug, Serialize)]
struct SurfaceInfo {
    kind: &'static str,
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct InfoReport {
    version: &'static str,
    os: &'static str,
    arch: &'static str,
    recommended_surface: &'static str,
    surfaces: Vec<SurfaceInfo>,
    agent_ctrl_home: String,
    active_sessions: usize,
}

/// Run the `info` command. `json = true` emits the [`InfoReport`] as
/// pretty-printed JSON; otherwise human-readable text.
pub(crate) fn run_info(json: bool) -> Result<()> {
    let report = build_report();
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_text(&report);
    }
    Ok(())
}

fn build_report() -> InfoReport {
    let surfaces: Vec<SurfaceInfo> = ALL_SURFACES
        .iter()
        .map(|k| SurfaceInfo {
            kind: k.as_str(),
            status: surface_status(*k).as_str(),
        })
        .collect();

    InfoReport {
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        recommended_surface: recommended_surface().as_str(),
        surfaces,
        agent_ctrl_home: session_file::discovery_dir().display().to_string(),
        active_sessions: session_file::list_alive().len(),
    }
}

/// Per-OS default surface for "I just want to drive this machine".
/// Returned by `info` so an agent can `agent-ctrl open <recommended>`
/// without a per-OS lookup table on its own side.
fn recommended_surface() -> SurfaceKind {
    // Pick the first surface whose status is `Ready` on this OS, in a
    // priority order that prefers the native a11y stack over the browser
    // stack. Falls back to mock so callers always get something working.
    let priority = [
        SurfaceKind::Uia,
        SurfaceKind::Ax,
        SurfaceKind::Cdp,
        SurfaceKind::Mock,
    ];
    priority
        .into_iter()
        .find(|k| surface_status(*k) == SurfaceStatus::Ready)
        .unwrap_or(SurfaceKind::Mock)
}

fn print_text(r: &InfoReport) {
    println!("agent-ctrl {} on {}/{}", r.version, r.os, r.arch);
    println!("recommended surface: {}", r.recommended_surface);
    println!("home: {}", r.agent_ctrl_home);
    println!("active sessions: {}", r.active_sessions);
    println!();
    println!("surfaces:");
    for s in &r.surfaces {
        let marker = match s.status {
            "ready" => "✓",
            "wrong-os" => "—",
            // stub / not-implemented / any future neutral status: same dim marker.
            _ => "·",
        };
        println!("  {marker}  {:<10}  {}", s.kind, s.status);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn report_includes_every_surface_kind() {
        let r = build_report();
        let kinds: Vec<&str> = r.surfaces.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&"mock"));
        assert!(kinds.contains(&"uia"));
        assert!(kinds.contains(&"ax"));
        assert!(kinds.contains(&"cdp"));
        assert!(kinds.contains(&"android"));
        assert!(kinds.contains(&"ios"));
    }

    #[test]
    fn report_serializes_to_stable_json_shape() {
        let r = build_report();
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("version").is_some());
        assert!(v.get("os").is_some());
        assert!(v.get("recommended_surface").is_some());
        assert!(v
            .get("surfaces")
            .and_then(serde_json::Value::as_array)
            .is_some());
        assert!(v
            .get("active_sessions")
            .and_then(serde_json::Value::as_u64)
            .is_some());
    }

    #[test]
    fn recommended_surface_picks_first_ready() {
        let kind = recommended_surface();
        // On every supported build, mock is the floor.
        assert_eq!(surface_status(kind), SurfaceStatus::Ready);
    }
}
