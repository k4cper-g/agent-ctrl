//! In-memory mock [`Surface`] for tests and protocol demos.
//!
//! Returns a small hand-built accessibility tree, accepts every action, and
//! never touches the OS. Available behind the `mock` Cargo feature.

#![allow(clippy::unused_async)] // every method is sync today; trait demands async signatures

use std::sync::Mutex;
use std::time::SystemTime;

use async_trait::async_trait;

use crate::action::{Action, ActionResult};
use crate::error::{Error, Result};
use crate::node::{AppContext, Bounds, Node, State, WindowContext};
use crate::role::Role;
use crate::snapshot::{RefMap, Snapshot, SnapshotOptions};
use crate::surface::{CapabilitySet, Surface, SurfaceKind};

/// Surface that returns a fixed fake tree and records actions in memory.
pub struct MockSurface {
    capabilities: CapabilitySet,
    actions: Mutex<Vec<Action>>,
}

impl Default for MockSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl MockSurface {
    /// Build a new mock surface.
    #[must_use]
    pub fn new() -> Self {
        Self {
            capabilities: CapabilitySet::new()
                .with("snapshot")
                .with("keyboard")
                .with("mouse"),
            actions: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot of every action received so far (most recent last).
    ///
    /// Recovers from a poisoned mutex by returning the inner data — losing
    /// poisoning is fine here because the data is only ever appended to.
    #[must_use]
    pub fn actions(&self) -> Vec<Action> {
        match self.actions.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }
}

#[async_trait]
impl Surface for MockSurface {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::Mock
    }

    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    async fn snapshot(&self, _opts: &SnapshotOptions) -> Result<Snapshot> {
        let mut refs = RefMap::new();
        let ok_id = refs.insert(Role::Button, "OK".into(), 0, None);
        let cancel_id = refs.insert(Role::Button, "Cancel".into(), 0, None);

        let bounds = |x: f64, y: f64, w: f64, h: f64| Bounds { x, y, w, h };
        let active = State {
            visible: true,
            enabled: true,
            ..State::default()
        };

        let root = Node {
            ref_id: None,
            role: Role::Window,
            name: "Mock Window".into(),
            description: None,
            value: None,
            state: active.clone(),
            bounds: Some(bounds(0.0, 0.0, 400.0, 300.0)),
            level: None,
            children: vec![
                Node {
                    ref_id: Some(ok_id),
                    role: Role::Button,
                    name: "OK".into(),
                    description: None,
                    value: None,
                    state: active.clone(),
                    bounds: Some(bounds(60.0, 240.0, 80.0, 30.0)),
                    level: None,
                    children: Vec::new(),
                    opaque: false,
                    native: None,
                },
                Node {
                    ref_id: Some(cancel_id),
                    role: Role::Button,
                    name: "Cancel".into(),
                    description: None,
                    value: None,
                    state: active,
                    bounds: Some(bounds(180.0, 240.0, 80.0, 30.0)),
                    level: None,
                    children: Vec::new(),
                    opaque: false,
                    native: None,
                },
            ],
            opaque: false,
            native: None,
        };

        Ok(Snapshot {
            captured_at: SystemTime::now(),
            surface_kind: SurfaceKind::Mock,
            app: AppContext {
                id: "agent.ctrl.mock".into(),
                name: "Mock".into(),
            },
            window: Some(WindowContext {
                id: "mock-window".into(),
                title: Some("Mock Window".into()),
            }),
            root,
            refs,
        })
    }

    async fn act(&self, action: &Action) -> Result<ActionResult> {
        self.actions
            .lock()
            .map_err(|_| Error::Surface("mock action log mutex poisoned".into()))?
            .push(action.clone());
        Ok(ActionResult::ok())
    }

    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::node::RefId;

    #[tokio::test]
    async fn mock_returns_two_buttons() {
        let surface = MockSurface::new();
        let snap = surface.snapshot(&SnapshotOptions::default()).await.unwrap();
        assert_eq!(snap.refs.len(), 2);
        assert_eq!(snap.root.children.len(), 2);
    }

    #[tokio::test]
    async fn mock_records_actions() {
        let surface = MockSurface::new();
        surface
            .act(&Action::Click {
                ref_id: RefId::new(0),
            })
            .await
            .unwrap();
        assert_eq!(surface.actions().len(), 1);
    }
}
