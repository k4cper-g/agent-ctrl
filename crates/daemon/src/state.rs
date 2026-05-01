//! Daemon state: the registry of active [`Surface`] sessions.

use std::collections::HashMap;
use std::sync::Arc;

use agent_ctrl_core::{Error, Result, Snapshot, SnapshotOptions, Surface};
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

/// Per-session state held under the daemon's session lock.
///
/// `surface` is `Option` so we can take ownership during `shutdown` and drop
/// it exactly once. `last_snapshot` caches the most recent successful
/// `Surface::snapshot` result so query verbs (`find`, future `wait-for`) can
/// run against it without forcing a fresh round-trip into the OS.
pub struct SessionCell {
    /// The platform surface for this session. `None` after shutdown.
    pub surface: Option<Box<dyn Surface>>,
    /// Most recent snapshot taken on this session. Replaced wholesale on each
    /// successful `Surface::snapshot` so query verbs never see a mix of refs
    /// from different captures.
    pub last_snapshot: Option<Snapshot>,
    /// Options that produced [`Self::last_snapshot`].
    ///
    /// The `wait-for` polling loop reuses these so it keeps targeting the
    /// pinned window - passing `SnapshotOptions::default()` would re-resolve
    /// `target: Foreground` to whichever window has focus *now*, which on a
    /// terminal-driven flow is almost never the app the user pinned.
    pub last_snapshot_options: Option<SnapshotOptions>,
}

impl SessionCell {
    /// Build a fresh cell wrapping a newly-opened surface.
    #[must_use]
    pub fn new(surface: Box<dyn Surface>) -> Self {
        Self {
            surface: Some(surface),
            last_snapshot: None,
            last_snapshot_options: None,
        }
    }
}

/// A session paired with the lock that serializes access to it.
///
/// Surfaces are guarded by a `Mutex` because most platform a11y APIs are
/// single-threaded by design (Windows UIA prefers STA, macOS AX must run on
/// the main thread).
pub type SurfaceCell = Arc<Mutex<SessionCell>>;

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
        let cell = Arc::new(Mutex::new(SessionCell::new(surface)));
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
        if let Some(mut surface) = guard.surface.take() {
            surface.shutdown().await?;
        }
        guard.last_snapshot = None;
        guard.last_snapshot_options = None;
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
