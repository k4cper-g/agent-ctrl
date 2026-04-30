//! Windows UI Automation (UIA) surface.
//!
//! Walks the UIA element tree starting from the foreground window, mapping
//! UIA `ControlType` values to canonical [`Role`](agent_ctrl_core::Role)
//! variants and using the UIA `Invoke`, `Value`, and `Toggle` patterns to
//! perform actions.
//!
//! On non-Windows hosts this crate compiles to a stub that returns
//! [`Error::PermissionDenied`] from [`UiaSurface::open`], so the workspace
//! still builds everywhere.
//!
//! UIA is a COM API and this crate intentionally uses `unsafe` FFI.
//! Safety arguments live at each `unsafe { ... }` block in [`windows_impl`].

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]
#![allow(unsafe_code)]

#[cfg(not(target_os = "windows"))]
use agent_ctrl_core::Error;
use agent_ctrl_core::{
    Action, ActionResult, CapabilitySet, Result, Snapshot, SnapshotOptions, Surface, SurfaceKind,
};
use async_trait::async_trait;

#[cfg(target_os = "windows")]
mod windows_impl;

/// Surface backed by Windows UI Automation.
pub struct UiaSurface {
    capabilities: CapabilitySet,
    #[cfg(target_os = "windows")]
    inner: windows_impl::UiaInner,
}

impl UiaSurface {
    /// Initialize a UIA session.
    ///
    /// On Windows: spawns a worker thread, initializes COM in the
    /// multi-threaded apartment, creates the `CUIAutomation` singleton,
    /// and returns a `UiaSurface` whose lifetime owns those resources.
    ///
    /// On other platforms: returns [`Error::PermissionDenied`].
    #[allow(clippy::unused_async)] // async on non-Windows where the body is sync
    pub async fn open() -> Result<Self> {
        #[cfg(target_os = "windows")]
        {
            let inner = windows_impl::UiaInner::new()?;
            Ok(Self {
                capabilities: CapabilitySet::new()
                    .with("snapshot")
                    .with("keyboard")
                    .with("mouse")
                    .with("multi_app"),
                inner,
            })
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(Error::PermissionDenied(
                "UIA surface is only available on Windows".into(),
            ))
        }
    }
}

#[async_trait]
impl Surface for UiaSurface {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::Uia
    }

    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    async fn snapshot(&self, opts: &SnapshotOptions) -> Result<Snapshot> {
        #[cfg(target_os = "windows")]
        {
            self.inner.snapshot(opts).await
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = opts;
            Err(Error::Unsupported {
                surface: SurfaceKind::Uia.as_str().into(),
                action: "snapshot".into(),
            })
        }
    }

    async fn act(&self, action: &Action) -> Result<ActionResult> {
        #[cfg(target_os = "windows")]
        {
            self.inner.act(action.clone()).await
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = action;
            Err(Error::Unsupported {
                surface: SurfaceKind::Uia.as_str().into(),
                action: "act".into(),
            })
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        // The worker thread is torn down by `UiaInner::drop`; nothing to do here.
        Ok(())
    }
}
