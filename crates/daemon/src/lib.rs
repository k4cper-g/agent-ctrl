//! Long-running daemon for `agent-ctrl`.
//!
//! Owns a registry of [`Surface`](agent_ctrl_core::Surface) sessions and
//! dispatches snapshot / action requests against them. The default transport
//! is stdio JSON-RPC (one [`Request`] per line in, one [`Response`] per line
//! out), but the [`dispatcher`] layer is transport-agnostic — an HTTP server,
//! a Unix socket, or a Tauri command bridge could plug in directly.

#![forbid(unsafe_code)]

pub mod dispatcher;
pub mod factory;
pub mod ipc;
pub mod state;

pub use dispatcher::{dispatch, Request, RequestOp, Response, ResponseBody};
pub use factory::open_surface;
pub use state::{DaemonState, SessionId};
