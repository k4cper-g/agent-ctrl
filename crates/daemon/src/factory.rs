//! Surface factory.
//!
//! Maps a [`SurfaceKind`] to a freshly-opened `Box<dyn Surface>`. This is the
//! one place that knows which concrete surface implementation goes with which
//! kind, and what platform constraints apply.

use agent_ctrl_core::{Error, MockSurface, Result, Surface, SurfaceKind};

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
