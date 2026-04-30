//! The [`Surface`] trait — the cross-platform contract every device implements.
//!
//! Inspired by agent-browser's `BrowserBackend` trait, but data-oriented:
//! `snapshot()` returns the unified [`Snapshot`] schema instead of letting
//! each backend build its tree ad-hoc. This locks in the cross-runtime
//! shape from day one.

use std::collections::HashSet;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionResult};
use crate::error::Result;
use crate::snapshot::{Snapshot, SnapshotOptions};

/// Identifier for a concrete [`Surface`] implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceKind {
    /// Chromium via Chrome DevTools Protocol.
    Cdp,
    /// Windows UI Automation.
    Uia,
    /// macOS Accessibility (AX).
    Ax,
    /// Android AccessibilityService.
    Android,
    /// iOS via XCUITest / WebDriverAgent.
    Ios,
    /// In-memory mock surface (for tests and protocol demos). Available when
    /// the `mock` feature is enabled on `agent-ctrl-core`.
    Mock,
}

impl SurfaceKind {
    /// Stable string label used in errors and serialization.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cdp => "cdp",
            Self::Uia => "uia",
            Self::Ax => "ax",
            Self::Android => "android",
            Self::Ios => "ios",
            Self::Mock => "mock",
        }
    }
}

/// Capability flags advertised by a surface.
///
/// Callers must check `supports(...)` before issuing optional actions.
/// Standard feature names:
///
/// - `"snapshot"` — basic tree capture (every surface)
/// - `"screenshot"` — pixel capture
/// - `"keyboard"` — synthetic keyboard input
/// - `"mouse"` — synthetic pointer input
/// - `"drag"` — pointer drag gestures
/// - `"multi_app"` — can list and switch among multiple apps
/// - `"network_intercept"` — CDP-only
/// - `"trace"` — CDP-only profiling
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitySet {
    features: HashSet<String>,
}

impl CapabilitySet {
    /// Build an empty capability set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a feature flag (builder-style).
    #[must_use]
    pub fn with(mut self, feature: impl Into<String>) -> Self {
        self.features.insert(feature.into());
        self
    }

    /// Check whether the surface supports `feature`.
    #[must_use]
    pub fn supports(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }

    /// Iterate over advertised features.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.features.iter().map(String::as_str)
    }
}

/// The cross-platform contract every device implements.
///
/// Implementations live in the per-platform `surface-*` crates. The daemon
/// owns boxed `dyn Surface` values and dispatches snapshot / action requests
/// to them.
#[async_trait]
pub trait Surface: Send + Sync {
    /// Identifier of this surface type.
    fn kind(&self) -> SurfaceKind;

    /// Capabilities advertised by this surface for the current session.
    fn capabilities(&self) -> &CapabilitySet;

    /// Capture a snapshot of the current accessibility tree.
    async fn snapshot(&self, opts: &SnapshotOptions) -> Result<Snapshot>;

    /// Execute an action against the most recent snapshot's [`RefMap`].
    ///
    /// The surface is responsible for re-resolving any [`RefId`](crate::node::RefId)
    /// in the action to a real native element. Stale refs (from snapshots that
    /// have since been discarded) must return [`Error::RefNotFound`](crate::error::Error::RefNotFound).
    async fn act(&self, action: &Action) -> Result<ActionResult>;

    /// Tear down the session. After this returns the surface must not be used.
    async fn shutdown(&mut self) -> Result<()>;
}
