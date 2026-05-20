//! Linux AT-SPI surface.
//!
//! AT-SPI2 is the freedesktop desktop accessibility bus - a D-Bus protocol
//! every GTK and Qt app speaks. This surface connects to the a11y bus, walks
//! the `org.a11y.atspi.Registry` tree, and maps AT-SPI roles/states/geometry
//! into the shared schema. The mapping contract is documented in
//! [`docs/atspi-mapping.md`](https://github.com/k4cper-g/agent-ctrl/blob/main/docs/atspi-mapping.md).
//!
//! Scope of this surface today: `snapshot` and `list_windows`. The action
//! vocabulary (`click`, `focus`, `fill`, ...) is a follow-up - `act` returns
//! [`Error::Unsupported`] for now.
//!
//! On non-Linux hosts this crate compiles to a stub that returns
//! [`Error::PermissionDenied`] from [`AtSpiSurface::open`], so the workspace
//! still builds everywhere. Unlike `surface-uia` (COM is `!Send`, needs a
//! worker thread) the `atspi` crate is async, so the `Surface` trait's
//! `async fn`s drive the D-Bus proxies directly.

#![forbid(unsafe_code)]
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use agent_ctrl_core::{
    Action, ActionResult, CapabilitySet, Error, Result, Snapshot, SnapshotOptions, Surface,
    SurfaceKind, WindowInfo,
};
use async_trait::async_trait;

#[cfg(target_os = "linux")]
mod linux;

/// Surface backed by the Linux AT-SPI accessibility bus.
pub struct AtSpiSurface {
    capabilities: CapabilitySet,
    #[cfg(target_os = "linux")]
    inner: linux::AtSpiInner,
}

impl AtSpiSurface {
    /// Initialize an AT-SPI session.
    ///
    /// On Linux: connects to the session bus, resolves the a11y bus address
    /// via `org.a11y.Bus.GetAddress`, and connects to the AT-SPI registry.
    /// Requires a running AT-SPI stack (`at-spi2-core`); a headless setup is
    /// in [`docker/linux-dev/`](https://github.com/k4cper-g/agent-ctrl/tree/main/docker/linux-dev).
    ///
    /// On other platforms: returns [`Error::PermissionDenied`].
    #[allow(clippy::unused_async)] // the body only awaits on Linux; the stub path is sync
    pub async fn open() -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            let inner = linux::AtSpiInner::new().await?;
            Ok(Self {
                // snapshot-only for now; the action vocabulary is a follow-up.
                capabilities: CapabilitySet::new().with("snapshot").with("windows"),
                inner,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(Error::PermissionDenied(
                "AT-SPI surface is only available on Linux".into(),
            ))
        }
    }
}

#[async_trait]
impl Surface for AtSpiSurface {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::AtSpi
    }

    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    async fn snapshot(&self, opts: &SnapshotOptions) -> Result<Snapshot> {
        #[cfg(target_os = "linux")]
        {
            self.inner.snapshot(opts).await
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = opts;
            Err(Error::Unsupported {
                surface: SurfaceKind::AtSpi.as_str().into(),
                action: "snapshot".into(),
            })
        }
    }

    async fn act(&self, action: &Action) -> Result<ActionResult> {
        // The AT-SPI action vocabulary (Action.DoAction, Component.GrabFocus,
        // EditableText.SetTextContents) is a follow-up PR; this surface ships
        // the snapshot-read path first. Every action reports Unsupported so
        // the daemon and agents get a clear, capability-consistent answer.
        Err(Error::Unsupported {
            surface: SurfaceKind::AtSpi.as_str().into(),
            action: action_name(action).into(),
        })
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        #[cfg(target_os = "linux")]
        {
            self.inner.list_windows().await
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(Error::Unsupported {
                surface: SurfaceKind::AtSpi.as_str().into(),
                action: "list_windows".into(),
            })
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        // The zbus connection is closed when `AtSpiInner` is dropped.
        Ok(())
    }
}

/// Snake-case label for an [`Action`], used in [`Error::Unsupported`] messages.
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
        Action::Check { .. } => "check",
        Action::Uncheck { .. } => "uncheck",
        Action::Toggle { .. } => "toggle",
        Action::Clear { .. } => "clear",
        Action::Clipboard { .. } => "clipboard",
        Action::Mouse { .. } => "mouse",
        Action::Highlight { .. } => "highlight",
        Action::ScrollIntoView { .. } => "scroll_into_view",
        Action::Wait { .. } => "wait",
        Action::SwitchApp { .. } => "switch_app",
        Action::FocusWindow { .. } => "focus_window",
        Action::Screenshot { .. } => "screenshot",
    }
}
