//! macOS Accessibility (AX) surface.
//!
//! Walks the system-wide AX tree via `AXUIElementCopy*` calls, maps AX roles
//! to canonical [`Role`](agent_ctrl_core::Role) variants, and performs actions
//! through `AXUIElementPerformAction`.
//!
//! Status: scaffold. The real implementation will use the `accessibility-sys`
//! / `core-foundation` crates and require the user to grant Accessibility
//! permission in System Settings → Privacy → Accessibility for the binary.
//! For consumers on other platforms this crate compiles to an empty rlib.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]
#![forbid(unsafe_code)]

use agent_ctrl_core::{
    Action, ActionResult, CapabilitySet, Error, Result, Snapshot, SnapshotOptions, Surface,
    SurfaceKind,
};
use async_trait::async_trait;

/// Surface backed by macOS Accessibility.
pub struct AxSurface {
    capabilities: CapabilitySet,
}

impl AxSurface {
    /// Initialize an AX session.
    ///
    /// Stubbed: the real implementation will check `AXIsProcessTrusted()`
    /// and prompt the user to grant Accessibility permission if not.
    #[allow(clippy::unused_async)] // body is sync today; will await once AX walking is real
    pub async fn open() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                capabilities: CapabilitySet::new()
                    .with("snapshot")
                    .with("screenshot")
                    .with("keyboard")
                    .with("mouse")
                    .with("multi_app"),
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::PermissionDenied(
                "AX surface is only available on macOS".into(),
            ))
        }
    }
}

#[async_trait]
impl Surface for AxSurface {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::Ax
    }

    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    async fn snapshot(&self, _opts: &SnapshotOptions) -> Result<Snapshot> {
        Err(Error::Unsupported {
            surface: SurfaceKind::Ax.as_str().into(),
            action: "snapshot".into(),
        })
    }

    async fn act(&self, _action: &Action) -> Result<ActionResult> {
        Err(Error::Unsupported {
            surface: SurfaceKind::Ax.as_str().into(),
            action: "act".into(),
        })
    }

    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}
