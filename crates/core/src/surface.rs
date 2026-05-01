//! The [`Surface`] trait - the cross-platform contract every device implements.
//!
//! Inspired by agent-browser's `BrowserBackend` trait, but data-oriented:
//! `snapshot()` returns the unified [`Snapshot`] schema instead of letting
//! each backend build its tree ad-hoc. This locks in the cross-runtime
//! shape from day one.

use std::collections::HashSet;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionResult};
use crate::error::{Error, Result};
use crate::snapshot::{Snapshot, SnapshotOptions};

/// One row of [`Surface::list_windows`] output.
///
/// Mirrors agent-browser's `tab_list` shape (one entry per open tab) but
/// generalized for native UI: the same app process can own multiple
/// top-level windows simultaneously (main window plus dialogs and popup
/// menus on Windows; multiple `AXWindow`s per `AXApplication` on macOS).
///
/// Like agent-browser's tab list, the agent is expected to inspect this
/// and use [`Action::FocusWindow`](crate::action::Action::FocusWindow) to
/// switch the session's pinned target before the next snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Stable per-platform window id, formatted for human consumption.
    /// On Windows this is the HWND in lowercase hex (e.g. `"0x1717ca"`).
    pub id: String,

    /// Window title text. May be `None` for unnamed system windows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Owning process executable name (file stem, no extension on Windows).
    pub process: String,

    /// Owning process id.
    pub pid: u32,

    /// Whether this window currently has user focus on the host.
    pub focused: bool,

    /// Whether this window is the one the session's last `snapshot` was
    /// pinned to. Subsequent `Action`s and `wait-for` polls target this
    /// window until a `FocusWindow` action re-pins.
    pub pinned: bool,
}

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
/// - `"snapshot"` - basic tree capture (every surface)
/// - `"screenshot"` - pixel capture
/// - `"keyboard"` - synthetic keyboard input
/// - `"mouse"` - synthetic pointer input
/// - `"drag"` - pointer drag gestures
/// - `"multi_app"` - can list and switch among multiple apps
/// - `"network_intercept"` - CDP-only
/// - `"trace"` - CDP-only profiling
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

    /// Enumerate the top-level windows the session can target.
    ///
    /// Implementations should return windows that share an "app" with the
    /// session's currently pinned target - typically all windows owned by
    /// the same OS process (UIA), all `AXWindow`s of the same
    /// `AXApplication` (macOS), or all tabs of the same browser context
    /// (CDP). The pinned window is included with `pinned: true`.
    ///
    /// The default implementation returns [`Error::Unsupported`] so
    /// scaffold surfaces (CDP, AX, Android, iOS) compile without having
    /// to provide a stub.
    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        Err(Error::Unsupported {
            surface: self.kind().as_str().into(),
            action: "list_windows".into(),
        })
    }

    /// Tear down the session. After this returns the surface must not be used.
    async fn shutdown(&mut self) -> Result<()>;
}
