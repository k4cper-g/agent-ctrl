//! Transport-agnostic request / response dispatch.

use agent_ctrl_core::{Action, ActionResult, Snapshot, SnapshotOptions, SurfaceKind};
use serde::{Deserialize, Serialize};

use crate::factory;
use crate::state::{DaemonState, SessionId};

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
    /// Close an open session.
    CloseSession {
        /// Session to close.
        session: SessionId,
    },
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
    /// `CloseSession` succeeded.
    Closed,
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
            let guard = cell.lock().await;
            let Some(surface) = guard.as_ref() else {
                return Response::error(&id, format!("session {session} is closed"));
            };
            match surface.snapshot(&opts).await {
                Ok(s) => ResponseBody::Snapshot {
                    snapshot: Box::new(s),
                },
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
            let Some(surface) = guard.as_ref() else {
                return Response::error(&id, format!("session {session} is closed"));
            };
            match surface.act(&action).await {
                Ok(o) => ResponseBody::ActionDone { outcome: o },
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
    };
    Response { id, body }
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
