//! Error type shared across the workspace.

use thiserror::Error;

/// Result alias used throughout `agent-ctrl-core` and downstream crates.
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Top-level error type for any operation against a [`Surface`](crate::surface::Surface).
#[derive(Debug, Error)]
pub enum Error {
    /// A snapshot operation failed at the platform layer.
    #[error("snapshot failed: {0}")]
    Snapshot(String),

    /// An action could not be executed.
    #[error("action '{action}' failed: {reason}")]
    Action {
        /// Name of the action that failed (e.g. `"click"`).
        action: String,
        /// Human-readable description of the failure.
        reason: String,
    },

    /// A [`RefId`](crate::node::RefId) was not present in the current snapshot.
    #[error("element with ref '{0}' not found in current snapshot")]
    RefNotFound(String),

    /// The surface does not support the requested action.
    #[error("action '{action}' is not supported by the {surface} surface")]
    Unsupported {
        /// Surface kind, e.g. `"uia"` or `"cdp"`.
        surface: String,
        /// Action name.
        action: String,
    },

    /// A platform permission grant is required (macOS Accessibility, Android
    /// AccessibilityService binding, etc.).
    #[error("permission required: {0}")]
    PermissionDenied(String),

    /// Generic surface failure that does not fit another variant.
    #[error("surface failed: {0}")]
    Surface(String),

    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON (de)serialization failure.
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
}
