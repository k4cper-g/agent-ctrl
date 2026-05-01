//! Tiny synchronous client for talking to a running daemon over TCP.
//!
//! Used by the `agent-ctrl` CLI to issue one request per invocation:
//! discover the session, dial the endpoint, write a JSON-RPC line, read a
//! JSON-RPC line, hand the response back. Synchronous because each CLI
//! command is its own short-lived process - there's nothing else for us
//! to be doing while we wait.

use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::dispatcher::{Request, RequestOp, Response};
use crate::session_file::SessionFile;

/// Default timeout for the request/response round trip. Local snapshots
/// of large UIA trees can take a couple of seconds in the worst case
/// (Office, Outlook); 30s is plenty for everything we care about and
/// short enough that hung daemons surface quickly.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Send one request to the daemon at `info.endpoint` and read one response.
///
/// `op` is the body of the request; the wire `id` is filled with a fresh
/// UUID so the daemon can echo it back. The response is parsed and
/// returned as-is - callers do their own destructuring of
/// `Response::body`.
pub fn send(info: &SessionFile, op: RequestOp) -> io::Result<Response> {
    send_with_timeout(info, op, DEFAULT_TIMEOUT)
}

/// Variant of [`send`] that lets callers override the read/write timeouts.
/// Used by long-running ops like `screenshot` of a huge desktop region or
/// `wait` for a user-supplied duration.
pub fn send_with_timeout(
    info: &SessionFile,
    op: RequestOp,
    timeout: Duration,
) -> io::Result<Response> {
    let request = Request {
        id: uuid::Uuid::new_v4().to_string(),
        op,
    };
    let mut payload = serde_json::to_vec(&request).map_err(io::Error::other)?;
    payload.push(b'\n');

    let addr = info.endpoint.parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid endpoint {}: {e}", info.endpoint),
        )
    })?;
    let stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let mut writer = stream.try_clone()?;
    writer.write_all(&payload)?;
    writer.flush()?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "daemon closed the connection without answering",
        ));
    }

    serde_json::from_str(line.trim()).map_err(io::Error::other)
}
