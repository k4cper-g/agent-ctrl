//! Stdio JSON-RPC transport.
//!
//! Reads one [`Request`] per newline-delimited line on stdin, writes one
//! [`Response`] per line to stdout. Suitable for spawning the daemon as a
//! subprocess from any language with a JSON parser.

use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::dispatcher::{self, Request, Response};
use crate::state::DaemonState;

/// Run the stdio loop until stdin closes.
pub async fn run_stdio(state: &DaemonState) -> std::io::Result<()> {
    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => dispatcher::dispatch(state, req).await,
            Err(e) => {
                // Best-effort: try to extract the request id so the client can
                // match this error response to its pending entry. Falls back to
                // empty id if the line isn't even partially parseable as JSON.
                let id = serde_json::from_str::<IdProbe>(&line)
                    .map(|p| p.id)
                    .unwrap_or_default();
                Response::error(&id, format!("invalid request: {e}"))
            }
        };
        let bytes = match serde_json::to_vec(&response) {
            Ok(b) => b,
            Err(e) => {
                // Build the fallback through serde_json::json! so the error
                // message gets properly escaped — `format!()` would emit
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
        };
        stdout.write_all(&bytes).await?;
        stdout.write_all(b"\n").await?;
        stdout.flush().await?;
    }

    Ok(())
}

/// Tiny shape used to recover a request id from a partially-parseable line.
#[derive(serde::Deserialize)]
struct IdProbe {
    #[serde(default)]
    id: String,
}
