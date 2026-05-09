//! macOS Accessibility (AX) surface.
//!
//! The current AX implementation is partial: it checks macOS
//! Accessibility trust, captures the focused window's AX tree, maps common
//! roles/states/bounds into the shared schema, lists AX windows for the pinned
//! app, supports `focus-window` through `AXRaise`, and implements first
//! element actions, checkable controls, keyboard actions, and screenshot
//! capture through AX and CoreGraphics calls. It is still a partial surface,
//! not a Windows UIA parity implementation.

#![cfg_attr(target_os = "macos", allow(unsafe_code))]

use agent_ctrl_core::{
    Action, ActionResult, CapabilitySet, Error, Result, Snapshot, SnapshotOptions, Surface,
    SurfaceKind, WindowInfo,
};
use async_trait::async_trait;
#[cfg(target_os = "macos")]
use std::sync::Mutex;

#[cfg(target_os = "macos")]
mod macos;

/// macOS Accessibility permission state for the current process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxTrustStatus {
    /// The process is trusted and can query other apps through AX.
    Trusted,
    /// The process needs Accessibility permission in System Settings.
    NotTrusted,
    /// The current OS cannot host AX.
    Unavailable,
}

/// Return the current process's macOS Accessibility trust state.
#[must_use]
pub fn accessibility_trust_status() -> AxTrustStatus {
    #[cfg(target_os = "macos")]
    {
        if macos::is_process_trusted() {
            AxTrustStatus::Trusted
        } else {
            AxTrustStatus::NotTrusted
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        AxTrustStatus::Unavailable
    }
}

/// Surface backed by macOS Accessibility.
pub struct AxSurface {
    capabilities: CapabilitySet,
    #[cfg(target_os = "macos")]
    pinned: Mutex<Option<macos::AxPinnedWindow>>,
    #[cfg(target_os = "macos")]
    last_snapshot: Mutex<Option<Snapshot>>,
}

impl AxSurface {
    /// Initialize an AX session.
    ///
    /// On macOS this requires the user to grant Accessibility permission to
    /// the `agent-ctrl` binary in System Settings.
    #[allow(clippy::unused_async)] // non-macOS body is sync; macOS will grow async work later
    pub async fn open() -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            if accessibility_trust_status() != AxTrustStatus::Trusted {
                return Err(Error::PermissionDenied(
                    "macOS Accessibility permission is required. Grant access in System Settings > Privacy & Security > Accessibility, then retry.".into(),
                ));
            }
            Ok(Self {
                capabilities: CapabilitySet::new()
                    .with("snapshot")
                    .with("windows")
                    .with("keyboard")
                    .with("screenshot"),
                pinned: Mutex::new(None),
                last_snapshot: Mutex::new(None),
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

#[cfg(target_os = "macos")]
const PINNED_LOCK_ERR: &str = "AX pinned-window mutex poisoned";
#[cfg(target_os = "macos")]
const SNAPSHOT_LOCK_ERR: &str = "AX last-snapshot mutex poisoned";

#[async_trait]
impl Surface for AxSurface {
    fn kind(&self) -> SurfaceKind {
        SurfaceKind::Ax
    }

    fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }

    async fn snapshot(&self, opts: &SnapshotOptions) -> Result<Snapshot> {
        #[cfg(target_os = "macos")]
        {
            let pinned = *self
                .pinned
                .lock()
                .map_err(|_| Error::Surface(PINNED_LOCK_ERR.into()))?;
            let capture = macos::snapshot(opts, pinned)?;
            *self
                .pinned
                .lock()
                .map_err(|_| Error::Surface(PINNED_LOCK_ERR.into()))? = Some(capture.pinned);
            *self
                .last_snapshot
                .lock()
                .map_err(|_| Error::Surface(SNAPSHOT_LOCK_ERR.into()))? =
                Some(capture.snapshot.clone());
            Ok(capture.snapshot)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = opts;
            Err(Error::Unsupported {
                surface: SurfaceKind::Ax.as_str().into(),
                action: "snapshot".into(),
            })
        }
    }

    async fn act(&self, action: &Action) -> Result<ActionResult> {
        #[cfg(target_os = "macos")]
        {
            if let Action::FocusWindow { window_id } = action {
                let pinned = macos::focus_window(window_id)?;
                *self
                    .pinned
                    .lock()
                    .map_err(|_| Error::Surface(PINNED_LOCK_ERR.into()))? = Some(pinned);
                *self
                    .last_snapshot
                    .lock()
                    .map_err(|_| Error::Surface(SNAPSHOT_LOCK_ERR.into()))? = None;
                return Ok(ActionResult::ok());
            }
            let pinned = *self
                .pinned
                .lock()
                .map_err(|_| Error::Surface(PINNED_LOCK_ERR.into()))?;
            let snapshot = self
                .last_snapshot
                .lock()
                .map_err(|_| Error::Surface(SNAPSHOT_LOCK_ERR.into()))?
                .clone();
            macos::act(action, pinned, snapshot.as_ref())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = action;
            Err(Error::Unsupported {
                surface: SurfaceKind::Ax.as_str().into(),
                action: "act".into(),
            })
        }
    }

    async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        #[cfg(target_os = "macos")]
        {
            let pinned = *self
                .pinned
                .lock()
                .map_err(|_| Error::Surface(PINNED_LOCK_ERR.into()))?;
            let windows = macos::list_windows(pinned)?;
            if let Some(next_pinned) = windows.pinned {
                *self
                    .pinned
                    .lock()
                    .map_err(|_| Error::Surface(PINNED_LOCK_ERR.into()))? = Some(next_pinned);
            }
            Ok(windows.windows)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::Unsupported {
                surface: SurfaceKind::Ax.as_str().into(),
                action: "list_windows".into(),
            })
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }
}
