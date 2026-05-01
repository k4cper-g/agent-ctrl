//! Surface factory.
//!
//! Maps a [`SurfaceKind`] to a freshly-opened `Box<dyn Surface>`. This is the
//! one place that knows which concrete surface implementation goes with which
//! kind, and what platform constraints apply.

use agent_ctrl_core::{Error, MockSurface, Result, Surface, SurfaceKind};

/// Static answer to "can I open this surface on this build?" — without
/// actually instantiating it. Used by `agent-ctrl info` and `agent-ctrl
/// doctor` so an agent can disambiguate "feature missing" / "wrong OS" /
/// "scaffold only" before it tries an action that would fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceStatus {
    /// Fully implemented and verified end-to-end on this OS.
    Ready,
    /// Compiled in on this OS but only a stub — `open_surface` will return
    /// an error or the surface will reject every action.
    Stub,
    /// The surface is meaningful in principle but not on this OS (e.g.
    /// `Uia` on macOS, `Ax` on Windows). The crate is gated to an empty
    /// shell here.
    WrongOs,
    /// No scaffolding for this kind yet — `open_surface` returns
    /// `Error::Surface` immediately.
    NotImplemented,
}

impl SurfaceStatus {
    /// Stable string label for JSON output and logs.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Stub => "stub",
            Self::WrongOs => "wrong-os",
            Self::NotImplemented => "not-implemented",
        }
    }
}

/// Report what each `SurfaceKind` would do if asked to open right now.
///
/// This is intentionally a `match` over every variant rather than a probe:
/// `info` / `doctor` should be cheap to run and side-effect-free. Probes
/// belong in the doctor's optional probe checks, not in this enumeration.
#[must_use]
pub fn surface_status(kind: SurfaceKind) -> SurfaceStatus {
    match kind {
        SurfaceKind::Mock => SurfaceStatus::Ready,

        SurfaceKind::Uia => {
            #[cfg(target_os = "windows")]
            {
                SurfaceStatus::Ready
            }
            #[cfg(not(target_os = "windows"))]
            {
                SurfaceStatus::WrongOs
            }
        }

        SurfaceKind::Ax => {
            #[cfg(target_os = "macos")]
            {
                // The AX crate compiles on macOS but its body is a stub
                // until someone fills it in. Mark it Stub rather than Ready
                // so doctor warns instead of green-checking it.
                SurfaceStatus::Stub
            }
            #[cfg(not(target_os = "macos"))]
            {
                SurfaceStatus::WrongOs
            }
        }

        // CDP is cross-platform but currently a stub on every OS.
        SurfaceKind::Cdp => SurfaceStatus::Stub,

        SurfaceKind::Android | SurfaceKind::Ios => SurfaceStatus::NotImplemented,
    }
}

/// Open a new surface of the requested kind.
///
/// Returns [`Error::PermissionDenied`] when the requested surface exists but
/// is not available on this platform (e.g. asking for `Uia` on macOS), and
/// [`Error::Surface`] for kinds that aren't implemented yet.
pub async fn open_surface(kind: SurfaceKind) -> Result<Box<dyn Surface>> {
    match kind {
        SurfaceKind::Mock => Ok(Box::new(MockSurface::new())),

        SurfaceKind::Cdp => Err(Error::Surface(
            "CDP surface requires a WebSocket URL; not yet wired into open_session".into(),
        )),

        #[cfg(target_os = "windows")]
        SurfaceKind::Uia => {
            let surface = agent_ctrl_surface_uia::UiaSurface::open().await?;
            Ok(Box::new(surface))
        }
        #[cfg(not(target_os = "windows"))]
        SurfaceKind::Uia => Err(Error::PermissionDenied(
            "UIA surface is only available on Windows".into(),
        )),

        #[cfg(target_os = "macos")]
        SurfaceKind::Ax => {
            let surface = agent_ctrl_surface_ax::AxSurface::open().await?;
            Ok(Box::new(surface))
        }
        #[cfg(not(target_os = "macos"))]
        SurfaceKind::Ax => Err(Error::PermissionDenied(
            "AX surface is only available on macOS".into(),
        )),

        SurfaceKind::Android => Err(Error::Surface("Android surface not yet implemented".into())),
        SurfaceKind::Ios => Err(Error::Surface("iOS surface not yet implemented".into())),
    }
}
