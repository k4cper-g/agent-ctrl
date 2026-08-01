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

/// Stable capability names advertised by surfaces and enforced by the daemon.
pub mod capability {
    /// Basic accessibility-tree capture.
    pub const SNAPSHOT: &str = "snapshot";
    /// General semantic actions such as click, focus, fill, and select.
    pub const ACTIONS: &str = "actions";
    /// Pixel capture.
    pub const SCREENSHOT: &str = "screenshot";
    /// Synthetic keyboard input.
    pub const KEYBOARD: &str = "keyboard";
    /// Synthetic pointer input.
    pub const MOUSE: &str = "mouse";
    /// Pointer drag gestures.
    pub const DRAG: &str = "drag";
    /// Top-level window enumeration and focus.
    pub const WINDOWS: &str = "windows";
    /// Cross-application switching.
    pub const MULTI_APP: &str = "multi_app";
}

/// Capabilities required to dispatch an action.
///
/// Every action requires [`capability::ACTIONS`]. Actions that depend on an
/// optional input or output subsystem require its capability as well.
#[must_use]
pub fn required_capabilities(action: &Action) -> &'static [&'static str] {
    use capability::{ACTIONS, DRAG, KEYBOARD, MOUSE, MULTI_APP, SCREENSHOT, WINDOWS};

    match action {
        Action::Type { .. }
        | Action::Press { .. }
        | Action::KeyDown { .. }
        | Action::KeyUp { .. }
        | Action::SelectAll { .. }
        | Action::Clipboard {
            op: crate::action::ClipboardOp::Copy | crate::action::ClipboardOp::Paste,
        } => &[ACTIONS, KEYBOARD],
        Action::DoubleClick { .. }
        | Action::RightClick { .. }
        | Action::Hover { .. }
        | Action::Scroll { .. }
        | Action::Mouse { .. }
        | Action::Highlight { .. } => &[ACTIONS, MOUSE],
        Action::Drag { .. } => &[ACTIONS, MOUSE, DRAG],
        Action::SwitchApp { .. } => &[ACTIONS, MULTI_APP],
        Action::FocusWindow { .. } => &[ACTIONS, WINDOWS],
        Action::Screenshot { .. } => &[ACTIONS, SCREENSHOT],
        _ => &[ACTIONS],
    }
}

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
///
/// agent-ctrl is intentionally scoped to native UI surfaces. Browsers are
/// covered by the sibling agent-browser project, so there is no CDP variant
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceKind {
    /// Windows UI Automation.
    Uia,
    /// macOS Accessibility (AX).
    Ax,
    /// Linux AT-SPI (the freedesktop desktop accessibility bus).
    // `kebab-case` would render this `"at-spi"`; keep it `"atspi"` to match
    // the no-hyphen convention of the other surface names (`uia`, `ax`).
    #[serde(rename = "atspi")]
    AtSpi,
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
            Self::Uia => "uia",
            Self::Ax => "ax",
            Self::AtSpi => "atspi",
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
/// - `"actions"` - semantic action dispatch
/// - `"screenshot"` - pixel capture
/// - `"keyboard"` - synthetic keyboard input
/// - `"mouse"` - synthetic pointer input
/// - `"drag"` - pointer drag gestures
/// - `"windows"` - can list top-level windows for the pinned app
/// - `"multi_app"` - can list and switch among multiple apps
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

    /// Capture a snapshot without replacing the refs used by [`Self::act`].
    ///
    /// Wait loops use this while polling so an action interleaved from another
    /// client keeps resolving against the last committed user snapshot. The
    /// returned capture may later be installed with
    /// [`Self::commit_observation`].
    async fn snapshot_for_observation(&self, opts: &SnapshotOptions) -> Result<Snapshot>;

    /// Install a capture returned by [`Self::snapshot_for_observation`].
    ///
    /// Implementations must replace their action-time ref state with the
    /// exact refs and native handles in `snapshot`, without recapturing the OS
    /// tree. The daemon calls this only when a wait reaches a terminal outcome.
    async fn commit_observation(&self, snapshot: &Snapshot) -> Result<()>;

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
    /// the same OS process (UIA) or all `AXWindow`s of the same
    /// `AXApplication` (macOS). The pinned window is included with
    /// `pinned: true`.
    ///
    /// The default implementation returns [`Error::Unsupported`] so
    /// scaffold surfaces (AX, Android, iOS) compile without having to
    /// provide a stub.
    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        Err(Error::Unsupported {
            surface: self.kind().as_str().into(),
            action: "list_windows".into(),
        })
    }

    /// Tear down the session. After this returns the surface must not be used.
    async fn shutdown(&mut self) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::{capability, required_capabilities};
    use crate::{Action, ClipboardOp, RefId};

    #[test]
    fn action_capabilities_include_optional_subsystems() {
        let target = RefId("ref_0".into());
        assert_eq!(
            required_capabilities(&Action::Click {
                ref_id: target.clone()
            }),
            &[capability::ACTIONS]
        );
        assert_eq!(
            required_capabilities(&Action::Press {
                keys: "Enter".into()
            }),
            &[capability::ACTIONS, capability::KEYBOARD]
        );
        assert_eq!(
            required_capabilities(&Action::Drag {
                from: target.clone(),
                to: target
            }),
            &[capability::ACTIONS, capability::MOUSE, capability::DRAG]
        );
        assert_eq!(
            required_capabilities(&Action::Clipboard {
                op: ClipboardOp::Read
            }),
            &[capability::ACTIONS]
        );
    }
}
