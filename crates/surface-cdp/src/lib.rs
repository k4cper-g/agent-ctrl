//! Chromium surface over Chrome DevTools Protocol.
//!
//! Connects to a running Chrome instance via CDP and builds snapshots from
//! `Accessibility.getFullAXTree` - the same approach as agent-browser. Works
//! on every platform Chrome runs on, so this is the surface to lean on for
//! cross-platform browser automation.
//!
//! Status: scaffold. The real implementation will mirror the structure of
//! `agent-browser/cli/src/native/cdp/` - a CDP client, a tree walker, and an
//! action layer.

#![forbid(unsafe_code)]

use agent_ctrl_core::{
    Action, ActionResult, CapabilitySet, Error, Result, Snapshot, SnapshotOptions, Surface,
    SurfaceKind,
};
use async_trait::async_trait;

/// Surface backed by a CDP WebSocket connection to a Chrome instance.
pub struct CdpSurface {
    capabilities: CapabilitySet,
    ws_url: String,
}

impl CdpSurface {
    /// Connect to Chrome at the given CDP WebSocket URL.
    ///
    /// Stubbed: the real implementation will open the WebSocket, attach to a
    /// page target, and prepare the accessibility tree subscription.
    #[allow(clippy::unused_async)] // body is sync today; will await once the WebSocket is real
    pub async fn connect(ws_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            capabilities: CapabilitySet::new()
                .with("snapshot")
                .with("screenshot")
                .with("keyboard")
                .with("mouse")
                .with("drag")
                .with("network_intercept"),
            ws_url: ws_url.into(),
        })
    }

    /// CDP endpoint this surface is bound to.
    #[must_use]
    pub fn ws_url(&self) -> &str {
        &self.ws_url
    }
}

#[async_trait]
impl Surface for CdpSurface {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::Cdp
    }

    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    async fn snapshot(&self, _opts: &SnapshotOptions) -> Result<Snapshot> {
        Err(Error::Unsupported {
            surface: SurfaceKind::Cdp.as_str().into(),
            action: "snapshot".into(),
        })
    }

    async fn act(&self, action: &Action) -> Result<ActionResult> {
        Err(Error::Unsupported {
            surface: SurfaceKind::Cdp.as_str().into(),
            action: action_name(action).into(),
        })
    }

    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}

fn action_name(action: &Action) -> &'static str {
    match action {
        Action::Click { .. } => "click",
        Action::DoubleClick { .. } => "double_click",
        Action::RightClick { .. } => "right_click",
        Action::Hover { .. } => "hover",
        Action::Focus { .. } => "focus",
        Action::Type { .. } => "type",
        Action::Fill { .. } => "fill",
        Action::Press { .. } => "press",
        Action::KeyDown { .. } => "key_down",
        Action::KeyUp { .. } => "key_up",
        Action::Scroll { .. } => "scroll",
        Action::Drag { .. } => "drag",
        Action::Select { .. } => "select",
        Action::SelectAll { .. } => "select_all",
        Action::ScrollIntoView { .. } => "scroll_into_view",
        Action::Wait { .. } => "wait",
        Action::SwitchApp { .. } => "switch_app",
        Action::FocusWindow { .. } => "focus_window",
        Action::Screenshot { .. } => "screenshot",
    }
}
