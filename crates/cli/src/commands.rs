//! Top-level CLI subcommands.

use agent_ctrl_core::{SnapshotOptions, SurfaceKind, WindowTarget};
use agent_ctrl_daemon::{dispatch, ipc, DaemonState, Request, RequestOp, ResponseBody};
use anyhow::{anyhow, Context, Result};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run the long-running daemon, talking JSON-RPC over stdin/stdout.
    Daemon,
    /// One-shot: open a surface session, snapshot it, print JSON, exit.
    Snapshot {
        /// Surface to use (`mock`, `cdp`, `uia`, `ax`, `android`, `ios`).
        #[arg(long)]
        surface: String,

        /// Target a specific top-level window by title substring (case-insensitive).
        #[arg(long, conflicts_with_all = ["target_pid", "target_process"])]
        target_title: Option<String>,

        /// Target the first visible top-level window owned by this process id.
        #[arg(long, conflicts_with = "target_process")]
        target_pid: Option<u32>,

        /// Target the first visible top-level window owned by a process whose
        /// executable file stem matches this name (case-insensitive, locale-independent).
        #[arg(long)]
        target_process: Option<String>,
    },
}

impl Command {
    pub(crate) async fn run(self) -> Result<()> {
        match self {
            Self::Daemon => run_daemon().await,
            Self::Snapshot {
                surface,
                target_title,
                target_pid,
                target_process,
            } => run_snapshot(&surface, target_title, target_pid, target_process).await,
        }
    }
}

async fn run_daemon() -> Result<()> {
    tracing::info!("starting agent-ctrl daemon on stdio");
    let state = DaemonState::new();
    ipc::run_stdio(&state)
        .await
        .context("daemon stdio loop failed")
}

async fn run_snapshot(
    surface: &str,
    target_title: Option<String>,
    target_pid: Option<u32>,
    target_process: Option<String>,
) -> Result<()> {
    let kind = parse_surface(surface)?;
    let target = match (target_title, target_pid, target_process) {
        (Some(title), _, _) => WindowTarget::Title { title },
        (None, Some(pid), _) => WindowTarget::Pid { pid },
        (None, None, Some(name)) => WindowTarget::ProcessName { name },
        (None, None, None) => WindowTarget::Foreground,
    };
    let opts = SnapshotOptions {
        target,
        ..SnapshotOptions::default()
    };
    let state = DaemonState::new();

    // 1. Open a session.
    let opened = dispatch(
        &state,
        Request {
            id: "open".into(),
            op: RequestOp::OpenSession { surface: kind },
        },
    )
    .await;
    let session = match opened.body {
        ResponseBody::SessionOpened { session } => session,
        ResponseBody::Error { message } => return Err(anyhow!("open_session failed: {message}")),
        other => return Err(anyhow!("unexpected response to open_session: {other:?}")),
    };

    // 2. Capture snapshot and print it as pretty JSON to stdout.
    let snapshot = dispatch(
        &state,
        Request {
            id: "snap".into(),
            op: RequestOp::Snapshot { session, opts },
        },
    )
    .await;
    match snapshot.body {
        ResponseBody::Snapshot { snapshot } => {
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        ResponseBody::Error { message } => return Err(anyhow!("snapshot failed: {message}")),
        other => return Err(anyhow!("unexpected response to snapshot: {other:?}")),
    }

    // 3. Best-effort close — ignore errors since we're exiting anyway.
    let _ = dispatch(
        &state,
        Request {
            id: "close".into(),
            op: RequestOp::CloseSession { session },
        },
    )
    .await;

    Ok(())
}

fn parse_surface(s: &str) -> Result<SurfaceKind> {
    match s {
        "mock" => Ok(SurfaceKind::Mock),
        "cdp" => Ok(SurfaceKind::Cdp),
        "uia" => Ok(SurfaceKind::Uia),
        "ax" => Ok(SurfaceKind::Ax),
        "android" => Ok(SurfaceKind::Android),
        "ios" => Ok(SurfaceKind::Ios),
        other => Err(anyhow!(
            "unknown surface `{other}` (expected one of: mock, cdp, uia, ax, android, ios)"
        )),
    }
}
