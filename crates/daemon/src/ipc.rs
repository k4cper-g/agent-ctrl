//! JSON-RPC transports.
//!
//! Two flavors share the same line-protocol payload:
//!
//! - [`run_stdio`] - one daemon per child process. Used by the TS client
//!   (`@agent-ctrl/client`) which spawns and owns the daemon directly.
//! - [`run_tcp`] - long-running daemon with a TCP listener on localhost.
//!   Used by the `agent-ctrl` CLI: a single daemon stays alive across many
//!   short-lived CLI invocations and is discovered through a state file.
//!
//! Both encode the same `{ id, ... }` request envelope and the same
//! response shape; only the transport differs.

use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;

use crate::dispatcher::{self, Request, RequestOp, Response};
use crate::state::DaemonState;

/// Run the stdio loop until stdin closes.
pub async fn run_stdio(state: &DaemonState) -> std::io::Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    // Stdio mode never observes a `Shutdown` request specially - stdin
    // closing is the existing exit signal, and the TS client uses that.
    // Pass a Notify that nobody waits on.
    let dummy_shutdown = Arc::new(Notify::new());

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let bytes = handle_line(state, &line, &dummy_shutdown).await;
        stdout.write_all(&bytes).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    Ok(())
}

/// Bind a TCP listener and serve JSON-RPC over it until `shutdown` fires.
///
/// Returns the bound `SocketAddr` to the caller via the `bound` callback so
/// the caller can write a state file announcing the actual port - typically
/// what `--bind 127.0.0.1:0` (ephemeral port) needs.
///
/// Each connection is handled on its own task so multiple CLI invocations
/// can talk to the daemon concurrently. Errors on individual connections
/// are logged but don't tear the listener down.
pub async fn run_tcp<F, Fut>(state: Arc<DaemonState>, bind: &str, bound: F) -> std::io::Result<()>
where
    F: FnOnce(std::net::SocketAddr) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let listener = TcpListener::bind(bind).await?;
    let local = listener.local_addr()?;
    bound(local).await;
    tracing::info!("agent-ctrl daemon listening on tcp://{local}");

    // Single shared shutdown signal: any connection that processes a
    // `Shutdown` request notifies this, the accept loop wakes and exits.
    let shutdown = Arc::new(Notify::new());

    loop {
        tokio::select! {
            biased;
            () = shutdown.notified() => {
                tracing::info!("daemon received shutdown wire op; exiting");
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, peer) = match accept {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("accept failed: {e}");
                        continue;
                    }
                };
                let state = Arc::clone(&state);
                let shutdown = Arc::clone(&shutdown);
                tokio::spawn(async move {
                    if let Err(e) = serve_connection(state, stream, shutdown).await {
                        tracing::warn!("connection from {peer} ended: {e}");
                    }
                });
            }
        }
    }
}

/// Process one TCP connection until the peer closes it or sends `Shutdown`.
/// The framing matches `run_stdio`: newline-delimited JSON in, newline-
/// delimited JSON out.
async fn serve_connection(
    state: Arc<DaemonState>,
    stream: TcpStream,
    shutdown: Arc<Notify>,
) -> std::io::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let bytes = handle_line(&state, &line, &shutdown).await;
        write_half.write_all(&bytes).await?;
        write_half.write_all(b"\n").await?;
        write_half.flush().await?;
    }
    Ok(())
}

/// Parse one JSON-RPC line, dispatch it, and serialize the response into
/// the bytes the transport will frame. Pulled out so stdio and TCP share
/// exactly the same shape - no chance of one drifting from the other.
///
/// `shutdown` fires when the request was a `Shutdown` op; the TCP accept
/// loop wakes on this and exits. Stdio passes a Notify nobody is waiting
/// on, since stdin closing is its existing exit signal.
async fn handle_line(state: &DaemonState, line: &str, shutdown: &Notify) -> Vec<u8> {
    let response = match serde_json::from_str::<Request>(line) {
        Ok(req) => {
            let is_shutdown = matches!(req.op, RequestOp::Shutdown);
            let resp = dispatcher::dispatch(state, req).await;
            if is_shutdown {
                shutdown.notify_one();
            }
            resp
        }
        Err(e) => {
            // Best-effort: try to extract the request id so the client can
            // match this error response to its pending entry. Falls back to
            // empty id if the line isn't even partially parseable as JSON.
            let id = serde_json::from_str::<IdProbe>(line)
                .map(|p| p.id)
                .unwrap_or_default();
            Response::error(&id, format!("invalid request: {e}"))
        }
    };
    match serde_json::to_vec(&response) {
        Ok(b) => b,
        Err(e) => {
            // Build the fallback through serde_json::json! so the error
            // message gets properly escaped - `format!()` would emit
            // invalid JSON if `e` contains quotes, backslashes, or
            // control characters and desync the line-based reader on the
            // other end.
            let fallback = json!({
                "id": &response.id,
                "result": "error",
                "message": format!("serialization failed: {e}"),
            });
            serde_json::to_vec(&fallback).unwrap_or_else(|_| {
                br#"{"id":"","result":"error","message":"serialization failed"}"#.to_vec()
            })
        }
    }
}

/// Tiny shape used to recover a request id from a partially-parseable line.
#[derive(serde::Deserialize)]
struct IdProbe {
    #[serde(default)]
    id: String,
}
