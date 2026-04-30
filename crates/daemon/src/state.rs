//! Daemon state: the registry of active [`Surface`] sessions.

use std::collections::HashMap;
use std::sync::Arc;

use agent_ctrl_core::{Error, Result, Surface};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

/// Identifier for one open session inside the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub Uuid);

impl SessionId {
    /// Allocate a new random session id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// A surface paired with the lock that serializes access to it.
///
/// Surfaces are guarded by a `Mutex` because most platform a11y APIs are
/// single-threaded by design (Windows UIA prefers STA, macOS AX must run on
/// the main thread). The inner `Option` lets us take ownership during
/// `shutdown` so the surface is dropped exactly once.
pub type SurfaceCell = Arc<Mutex<Option<Box<dyn Surface>>>>;

/// In-memory registry of active sessions.
#[derive(Default)]
pub struct DaemonState {
    sessions: Mutex<HashMap<SessionId, SurfaceCell>>,
}

impl DaemonState {
    /// Build an empty state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a surface and return its new session id.
    pub async fn open(&self, surface: Box<dyn Surface>) -> SessionId {
        let id = SessionId::new();
        let cell = Arc::new(Mutex::new(Some(surface)));
        self.sessions.lock().await.insert(id, cell);
        id
    }

    /// Look up a session by id.
    pub async fn get(&self, id: SessionId) -> Option<SurfaceCell> {
        self.sessions.lock().await.get(&id).cloned()
    }

    /// Tear down a session, calling [`Surface::shutdown`] before dropping it.
    pub async fn close(&self, id: SessionId) -> Result<()> {
        let Some(cell) = self.sessions.lock().await.remove(&id) else {
            return Err(Error::Surface(format!("unknown session: {id}")));
        };
        let mut guard = cell.lock().await;
        if let Some(mut surface) = guard.take() {
            surface.shutdown().await?;
        }
        Ok(())
    }

    /// Number of currently open sessions.
    pub async fn len(&self) -> usize {
        self.sessions.lock().await.len()
    }

    /// Whether no sessions are open.
    pub async fn is_empty(&self) -> bool {
        self.sessions.lock().await.is_empty()
    }
}
