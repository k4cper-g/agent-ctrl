//! Long-running daemon for `agent-ctrl`.
//!
//! Owns a registry of [`Surface`](agent_ctrl_core::Surface) sessions and
//! dispatches snapshot / action requests against them. The default transport
//! is stdio JSON-RPC (one [`Request`] per line in, one [`Response`] per line
//! out), but the [`dispatcher`] layer is transport-agnostic — an HTTP server,
//! a Unix socket, or a Tauri command bridge could plug in directly.

#![forbid(unsafe_code)]

pub mod client;
pub mod dispatcher;
pub mod factory;
pub mod ipc;
pub mod session_file;
pub mod state;

pub use dispatcher::{dispatch, Request, RequestOp, Response, ResponseBody};
pub use factory::{open_surface, surface_status, SurfaceStatus};
pub use session_file::{
    discovery_dir, list_alive, path_for, read_alive, remove, wait_for_alive, write, SessionFile,
    DEFAULT_SESSION,
};
pub use state::{DaemonState, SessionId};
