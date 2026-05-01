//! Transport-agnostic request / response dispatch.

use std::time::{Duration, Instant};

use agent_ctrl_core::{
    tree_signature, Action, ActionResult, FindMatch, FindQuery, Snapshot, SnapshotOptions,
    SurfaceKind, WaitOptions, WaitOutcome, WaitPredicate, WindowInfo,
};
use serde::{Deserialize, Serialize};

use crate::factory;
use crate::state::{DaemonState, SessionId};

/// Floor on `wait-for` poll cadence. Anything faster burns CPU on UIA tree
/// walks without buying reliability - a heavy app's snapshot already takes
/// 100-300ms.
const MIN_POLL_MS: u64 = 50;

/// Cap on `wait-for` total timeout. Prevents a buggy or malicious client
/// from pinning the daemon in a polling loop for years. One hour is well
/// past anything an interactive agent should ever need.
const MAX_TIMEOUT_MS: u64 = 60 * 60 * 1000;

/// A request from a client to the daemon.
///
/// JSON shape:
/// ```json
/// {"id": "abc", "op": "snapshot", "session": "...", "opts": {...}}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Caller-supplied correlation id; echoed back in the matching [`Response`].
    pub id: String,
    /// The actual operation to perform.
    #[serde(flatten)]
    pub op: RequestOp,
}

/// The operation portion of a [`Request`], internally tagged by `"op"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RequestOp {
    /// Open a new surface session.
    OpenSession {
        /// Surface implementation to instantiate.
        surface: SurfaceKind,
    },
    /// Capture a snapshot from an open session.
    Snapshot {
        /// Session to snapshot.
        session: SessionId,
        /// Capture options.
        #[serde(default)]
        opts: SnapshotOptions,
    },
    /// Execute an action against an open session.
    Act {
        /// Session to act on.
        session: SessionId,
        /// Action to perform.
        action: Action,
    },
    /// Search the most recent snapshot's tree for refs matching `query`.
    ///
    /// Pure read against the daemon's cached snapshot - does not re-walk the
    /// OS accessibility tree. Errors when no snapshot has been captured yet
    /// on this session.
    Find {
        /// Session to query.
        session: SessionId,
        /// Filters describing what to match.
        query: FindQuery,
    },
    /// Block until `opts.predicate` is satisfied or `opts.timeout_ms` fires.
    ///
    /// Polls the surface at `opts.poll_ms` (floored at 50). Each successful
    /// poll's snapshot is cached as `last_snapshot` so a follow-up `Act` or
    /// `Find` sees fresh refs without an extra round-trip.
    Wait {
        /// Session to wait on.
        session: SessionId,
        /// Predicate, timeout, and poll cadence.
        opts: WaitOptions,
    },
    /// Enumerate top-level windows the session can target.
    ///
    /// Mirrors agent-browser's `tab_list`: the agent uses the result to
    /// decide which window to switch to via `FocusWindow`. Useful when a
    /// dialog or popup spawned outside the currently pinned window.
    ListWindows {
        /// Session to enumerate windows for.
        session: SessionId,
    },
    /// Close an open session.
    CloseSession {
        /// Session to close.
        session: SessionId,
    },
    /// Shut the daemon down. Used by the `agent-ctrl close` CLI command to
    /// terminate a long-running TCP daemon. The daemon answers `Stopped`
    /// and then drops its accept loop; the transport layer is responsible
    /// for actually triggering the shutdown after seeing this op.
    Shutdown,
}

/// A response from the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Echoes the [`Request::id`] that produced this response.
    /// Empty when the request couldn't be parsed at all.
    pub id: String,
    /// The actual result.
    #[serde(flatten)]
    pub body: ResponseBody,
}

/// The result portion of a [`Response`], internally tagged by `"result"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ResponseBody {
    /// `OpenSession` succeeded.
    SessionOpened {
        /// Identifier of the new session.
        session: SessionId,
    },
    /// `Snapshot` succeeded.
    Snapshot {
        /// Captured snapshot.
        snapshot: Box<Snapshot>,
    },
    /// `Act` succeeded.
    ActionDone {
        /// Action result.
        outcome: ActionResult,
    },
    /// `Find` succeeded. May be empty if no node matched.
    FindResults {
        /// Matched refs in tree order.
        matches: Vec<FindMatch>,
    },
    /// `Wait` finished, either by satisfying its predicate or by timing out.
    WaitDone {
        /// What happened. `Timeout` is *not* a wire-level error - it's a
        /// distinguishable normal outcome the CLI maps to exit code 2.
        outcome: WaitOutcome,
    },
    /// `ListWindows` succeeded.
    Windows {
        /// Top-level windows the session can target. The pinned window is
        /// flagged via `WindowInfo::pinned`.
        windows: Vec<WindowInfo>,
    },
    /// `CloseSession` succeeded.
    Closed,
    /// `Shutdown` succeeded - the daemon will exit shortly after this
    /// response is delivered.
    Stopped,
    /// Any error path.
    Error {
        /// Human-readable error message.
        message: String,
    },
}

impl Response {
    /// Build an error response with the given id.
    #[must_use]
    pub fn error(id: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            body: ResponseBody::Error {
                message: message.into(),
            },
        }
    }
}

/// Dispatch one request against the daemon state.
pub async fn dispatch(state: &DaemonState, request: Request) -> Response {
    let id = request.id;
    let body = match request.op {
        RequestOp::OpenSession { surface } => match factory::open_surface(surface).await {
            Ok(s) => ResponseBody::SessionOpened {
                session: state.open(s).await,
            },
            Err(e) => ResponseBody::Error {
                message: e.to_string(),
            },
        },
        RequestOp::Snapshot { session, opts } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let mut guard = cell.lock().await;
            let Some(surface) = guard.surface.as_ref() else {
                return Response::error(&id, format!("session {session} is closed"));
            };
            match surface.snapshot(&opts).await {
                Ok(s) => {
                    // Cache both the snapshot and the options that produced
                    // it, so the wait-for polling loop can keep targeting
                    // the same window instead of re-resolving Foreground.
                    guard.last_snapshot = Some(s.clone());
                    guard.last_snapshot_options = Some(opts);
                    ResponseBody::Snapshot {
                        snapshot: Box::new(s),
                    }
                }
                Err(e) => ResponseBody::Error {
                    message: e.to_string(),
                },
            }
        }
        RequestOp::Act { session, action } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let guard = cell.lock().await;
            let Some(surface) = guard.surface.as_ref() else {
                return Response::error(&id, format!("session {session} is closed"));
            };
            match surface.act(&action).await {
                Ok(o) => ResponseBody::ActionDone { outcome: o },
                Err(e) => ResponseBody::Error {
                    message: e.to_string(),
                },
            }
        }
        RequestOp::Find { session, query } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let guard = cell.lock().await;
            let Some(snap) = guard.last_snapshot.as_ref() else {
                // Don't leak the internal session UUID here - the caller
                // already knows which session they targeted, and a UUID
                // distracts from the actionable hint.
                return Response::error(
                    &id,
                    "no snapshot cached for this session - run `agent-ctrl snapshot` first",
                );
            };
            ResponseBody::FindResults {
                matches: snap.find(&query),
            }
        }
        RequestOp::Wait { session, opts } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let outcome = run_wait(&cell, &opts).await;
            match outcome {
                Ok(o) => ResponseBody::WaitDone { outcome: o },
                Err(e) => ResponseBody::Error { message: e },
            }
        }
        RequestOp::ListWindows { session } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let guard = cell.lock().await;
            let Some(surface) = guard.surface.as_ref() else {
                return Response::error(&id, format!("session {session} is closed"));
            };
            match surface.list_windows().await {
                Ok(windows) => ResponseBody::Windows { windows },
                Err(e) => ResponseBody::Error {
                    message: e.to_string(),
                },
            }
        }
        RequestOp::CloseSession { session } => match state.close(session).await {
            Ok(()) => ResponseBody::Closed,
            Err(e) => ResponseBody::Error {
                message: e.to_string(),
            },
        },
        RequestOp::Shutdown => ResponseBody::Stopped,
    };
    Response { id, body }
}

/// Run a wait-for polling loop against `cell`.
///
/// Each iteration acquires the session lock briefly to take a snapshot and
/// cache it, then releases the lock before sleeping - so other CLI
/// invocations (a `find`, a `close`, an `act`) on the same session can
/// interleave between polls. The minimum poll interval is clamped at
/// [`MIN_POLL_MS`] to prevent CPU thrash on heavy a11y trees.
///
/// Each poll uses the same `SnapshotOptions` that produced the session's
/// most recent snapshot - without that, `target: Foreground` would
/// re-resolve to whatever window has focus right now (the terminal,
/// typically) and the wait would target the wrong app entirely. A prior
/// `Snapshot` is therefore required; if none exists this returns an error.
///
/// Returns `Err` only for fatal session-state problems (closed surface,
/// snapshot error, no prior snapshot). Timeout is a normal
/// `Ok(WaitOutcome::Timeout)` so the CLI can map it to its own exit code.
async fn run_wait(
    cell: &crate::state::SurfaceCell,
    opts: &WaitOptions,
) -> Result<WaitOutcome, String> {
    // Both ends are clamped: poll has a floor (CPU thrash protection) and
    // timeout has a ceiling (don't let a runaway request occupy the worker
    // for hours). Hitting MAX_TIMEOUT_MS still produces a normal `Timeout`
    // outcome, not an error - the agent sees "timeout after 3600000ms".
    let timeout = Duration::from_millis(opts.timeout_ms.min(MAX_TIMEOUT_MS));
    let poll = Duration::from_millis(opts.poll_ms.max(MIN_POLL_MS));
    let started = Instant::now();

    // Snapshot options to reuse on every poll. Captured up front so we
    // hold the lock briefly.
    let snap_opts: SnapshotOptions = {
        let guard = cell.lock().await;
        guard.last_snapshot_options.clone().ok_or_else(|| {
            "no snapshot cached for this session - run `agent-ctrl snapshot` first so wait-for knows which window to target".to_string()
        })?
    };

    // Stable-only state: signature seen on the previous successful poll, and
    // when the current run of equal signatures began. Reset to None whenever
    // the signature changes.
    let mut last_signature: Option<u64> = None;
    let mut stable_since: Option<Instant> = None;

    loop {
        // Take a snapshot inside a short lock window. The lock is released
        // before we evaluate or sleep so other ops can interleave.
        let snap = {
            let mut guard = cell.lock().await;
            let Some(surface) = guard.surface.as_ref() else {
                return Err("session is closed".into());
            };
            let snap = surface
                .snapshot(&snap_opts)
                .await
                .map_err(|e| format!("wait: snapshot failed: {e}"))?;
            // Cache so a follow-up action sees fresh refs without an extra
            // OS round-trip.
            guard.last_snapshot = Some(snap.clone());
            snap
        };

        // Evaluate the predicate against the snapshot we just took.
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &opts.predicate {
            WaitPredicate::Appears { query } => {
                if let Some(found) = snap.find(query).into_iter().next() {
                    return Ok(WaitOutcome::Matched {
                        found: Some(found),
                        elapsed_ms,
                    });
                }
            }
            WaitPredicate::Gone { query } => {
                if snap.find(query).is_empty() {
                    return Ok(WaitOutcome::Gone { elapsed_ms });
                }
            }
            WaitPredicate::Stable { idle_ms } => {
                let sig = tree_signature(&snap);
                if last_signature == Some(sig) {
                    let since = stable_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_millis(*idle_ms) {
                        return Ok(WaitOutcome::Stable { elapsed_ms });
                    }
                } else {
                    last_signature = Some(sig);
                    stable_since = None;
                }
            }
        }

        // Check the timeout *before* sleeping - overruns of one poll
        // duration are tolerable, but two polls past the deadline isn't.
        if started.elapsed() >= timeout {
            return Ok(WaitOutcome::Timeout { elapsed_ms });
        }

        tokio::time::sleep(poll).await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn request_roundtrips_with_correlation_id() {
        let req = Request {
            id: "abc-123".into(),
            op: RequestOp::OpenSession {
                surface: SurfaceKind::Mock,
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""id":"abc-123""#));
        assert!(json.contains(r#""op":"open_session""#));
        assert!(json.contains(r#""surface":"mock""#));

        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "abc-123");
        assert!(matches!(
            back.op,
            RequestOp::OpenSession {
                surface: SurfaceKind::Mock
            }
        ));
    }

    #[tokio::test]
    async fn dispatch_open_then_snapshot_then_close() {
        let state = DaemonState::new();
        let opened = dispatch(
            &state,
            Request {
                id: "1".into(),
                op: RequestOp::OpenSession {
                    surface: SurfaceKind::Mock,
                },
            },
        )
        .await;
        assert_eq!(opened.id, "1");
        let session = match opened.body {
            ResponseBody::SessionOpened { session } => session,
            other => panic!("expected SessionOpened, got {other:?}"),
        };

        let snap = dispatch(
            &state,
            Request {
                id: "2".into(),
                op: RequestOp::Snapshot {
                    session,
                    opts: SnapshotOptions::default(),
                },
            },
        )
        .await;
        assert!(matches!(snap.body, ResponseBody::Snapshot { .. }));

        let closed = dispatch(
            &state,
            Request {
                id: "3".into(),
                op: RequestOp::CloseSession { session },
            },
        )
        .await;
        assert!(matches!(closed.body, ResponseBody::Closed));
    }
}
