//! AT-SPI surface implementation (Linux only).
//!
//! Connects to the freedesktop accessibility bus through the `atspi` crate,
//! walks the `org.a11y.atspi.Registry` tree, and maps AT-SPI roles, states,
//! and geometry into the shared schema. The mapping contract is
//! `docs/atspi-mapping.md`.
//!
//! Every AT-SPI accessible is addressed by a `(bus_name, object_path)` pair
//! and exposes its data over several D-Bus interfaces (`Accessible`,
//! `Component`, `Text`, `Value`, ...). There is no batch-cache call like
//! UIA's `BuildUpdatedCache`, so a snapshot is many small D-Bus round trips;
//! that cost is bounded by [`DEFAULT_DEPTH`] and [`MAX_NODES`].

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use agent_ctrl_core::{
    assign_scope_refs, AppContext, Bounds, Checked, Error, NativeHandle, Node, RefMap, Result,
    Role, Snapshot, SnapshotOptions, State, SurfaceKind, WindowContext, WindowInfo, WindowTarget,
};
use atspi::connection::AccessibilityConnection;
use atspi::proxy::accessible::AccessibleProxy;
use atspi::proxy::bus::StatusProxy;
use atspi::proxy::proxy_ext::{Proxies, ProxyExt};
use atspi::zbus::fdo::DBusProxy;
use atspi::zbus::names::BusName;
use atspi::zbus::proxy::CacheProperties;
use atspi::zbus::zvariant::ObjectPath;
use atspi::zbus::Connection;
use atspi::{CoordType, Role as AtRole, State as AtState, StateSet};

/// Default maximum tree depth walked when [`SnapshotOptions::depth`] is unset.
/// GTK/Qt trees nest deeper than a typical Win32 window, so this is generous.
const DEFAULT_DEPTH: usize = 25;

/// Hard ceiling on the number of nodes captured in one snapshot. A runaway or
/// pathological tree is truncated rather than wedging the daemon on D-Bus.
const MAX_NODES: usize = 4000;

/// The AT-SPI registry's well-known destination and its root object path.
const REGISTRY_DEST: &str = "org.a11y.atspi.Registry";
const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";

/// How long to keep retrying the app enumeration while a toolkit that has just
/// been told accessibility is enabled finishes registering on the bus.
const APP_REGISTER_TIMEOUT: Duration = Duration::from_secs(2);
/// Poll interval for the app-registration retry.
const APP_REGISTER_POLL: Duration = Duration::from_millis(125);

/// The top-level window the session is bound to, threaded across snapshots.
#[derive(Debug, Clone)]
struct Pinned {
    /// Unique D-Bus name of the owning application (e.g. `":1.42"`).
    bus: String,
    /// Object path of the owning `application` accessible.
    app_path: String,
    /// Human-readable application name (the `--target-process` match key).
    app_name: String,
    /// Object path of the pinned top-level `frame` accessible.
    frame_path: String,
}

/// The window a snapshot resolved its target to.
struct ResolvedFrame {
    bus: String,
    app_path: String,
    app_name: String,
    frame_path: String,
}

/// An `application` accessible under the registry root.
struct AppEntry {
    bus: String,
    path: String,
    name: String,
    pid: u32,
}

/// A top-level `frame` window owned by an application.
struct FrameEntry {
    path: String,
    name: String,
    active: bool,
}

/// Owns the AT-SPI bus connection and the session's pinned-window state.
pub(crate) struct AtSpiInner {
    conn: AccessibilityConnection,
    pinned: Mutex<Option<Pinned>>,
}

impl AtSpiInner {
    /// Connect to the accessibility bus and enable the accessibility stack.
    pub(crate) async fn new() -> Result<Self> {
        let conn = AccessibilityConnection::new().await.map_err(|e| {
            Error::PermissionDenied(format!(
                "could not connect to the AT-SPI accessibility bus ({e}); \
                 install at-spi2-core and ensure the a11y bus is running"
            ))
        })?;
        enable_accessibility().await;
        Ok(Self {
            conn,
            pinned: Mutex::new(None),
        })
    }

    /// Capture the pinned window's accessibility tree.
    pub(crate) async fn snapshot(&self, opts: &SnapshotOptions) -> Result<Snapshot> {
        self.capture(opts, true).await
    }

    /// Capture the pinned tree without replacing session targeting state.
    pub(crate) async fn snapshot_for_observation(
        &self,
        opts: &SnapshotOptions,
    ) -> Result<Snapshot> {
        self.capture(opts, false).await
    }

    async fn capture(&self, opts: &SnapshotOptions, commit: bool) -> Result<Snapshot> {
        let hint = self.pinned.lock().map_err(|_| lock_err())?.clone();
        if !commit && hint.is_none() {
            return Err(Error::Snapshot(
                "no snapshot committed for this session - run `agent-ctrl snapshot` first".into(),
            ));
        }
        let target = if commit {
            opts.target.clone()
        } else {
            WindowTarget::Foreground
        };
        let frame = self.resolve_target(&target, hint.as_ref()).await?;
        let conn = self.conn.connection().clone();

        let mut walk = Walk {
            conn: conn.clone(),
            refs: RefMap::new(),
            nth: HashMap::new(),
            max_depth: opts.depth.unwrap_or(DEFAULT_DEPTH),
            compact: opts.compact,
            budget: MAX_NODES,
        };
        let mut root = build_subtree(&mut walk, frame.bus.clone(), frame.frame_path.clone(), 0)
            .await
            .ok_or_else(|| {
                Error::Snapshot("AT-SPI returned no accessible for the target window".into())
            })?;
        assign_scope_refs(&mut root);

        let pid = pid_for_bus(&conn, &frame.bus).await.unwrap_or(0);
        let window_id = format!("{}@{}", frame.bus, frame.frame_path);
        let window_title = (!root.name.is_empty()).then(|| root.name.clone());

        if commit {
            *self.pinned.lock().map_err(|_| lock_err())? = Some(Pinned {
                bus: frame.bus.clone(),
                app_path: frame.app_path.clone(),
                app_name: frame.app_name.clone(),
                frame_path: frame.frame_path.clone(),
            });
        }

        Ok(Snapshot {
            captured_at: SystemTime::now(),
            surface_kind: SurfaceKind::AtSpi,
            app: AppContext {
                id: if pid == 0 {
                    frame.bus.clone()
                } else {
                    format!("pid:{pid}")
                },
                name: frame.app_name.clone(),
            },
            window: Some(WindowContext {
                id: window_id,
                title: window_title,
            }),
            root,
            refs: walk.refs,
        })
    }

    /// Enumerate the top-level windows owned by the pinned application.
    pub(crate) async fn list_windows(&self) -> Result<Vec<WindowInfo>> {
        let pinned = self
            .pinned
            .lock()
            .map_err(|_| lock_err())?
            .clone()
            .ok_or_else(|| {
                Error::Surface(
                    "no snapshot cached for this session - run `agent-ctrl snapshot` first".into(),
                )
            })?;

        let conn = self.conn.connection().clone();
        let pid = pid_for_bus(&conn, &pinned.bus).await.unwrap_or(0);
        let frames = frames_of_app(&conn, &pinned.bus, &pinned.app_path).await?;

        Ok(frames
            .into_iter()
            .map(|f| WindowInfo {
                id: format!("{}@{}", pinned.bus, f.path),
                title: (!f.name.is_empty()).then_some(f.name),
                process: pinned.app_name.clone(),
                pid,
                focused: f.active,
                pinned: f.path == pinned.frame_path,
            })
            .collect())
    }

    /// Resolve a [`WindowTarget`] to a concrete top-level frame.
    async fn resolve_target(
        &self,
        target: &WindowTarget,
        hint: Option<&Pinned>,
    ) -> Result<ResolvedFrame> {
        let conn = self.conn.connection().clone();

        // A bare `snapshot` (Foreground) on an already-pinned session must
        // re-capture the *same* window, not re-resolve whatever is focused -
        // the same discipline the UIA surface follows.
        if matches!(target, WindowTarget::Foreground) {
            if let Some(p) = hint {
                return Ok(ResolvedFrame {
                    bus: p.bus.clone(),
                    app_path: p.app_path.clone(),
                    app_name: p.app_name.clone(),
                    frame_path: p.frame_path.clone(),
                });
            }
        }

        let started = Instant::now();
        let mut last_err: Error;

        // A toolkit told "accessibility on" only moments ago may still be
        // registering. Retry the full target resolution, not just the
        // "registry is empty" case: another app may already be registered
        // while the requested app or its first frame has not appeared yet.
        loop {
            let apps = enumerate_apps(&conn).await?;
            if apps.is_empty() {
                last_err = Error::Snapshot(
                    "no accessible applications are registered on the AT-SPI bus".into(),
                );
            } else {
                match resolve_target_once(&conn, target, &apps).await {
                    Ok(frame) => return Ok(frame),
                    Err(e) => last_err = e,
                }
            }

            if started.elapsed() >= APP_REGISTER_TIMEOUT {
                return Err(last_err);
            }
            tokio::time::sleep(APP_REGISTER_POLL).await;
        }
    }
}

/// Try one pass of target resolution against the currently registered apps.
async fn resolve_target_once(
    conn: &Connection,
    target: &WindowTarget,
    apps: &[AppEntry],
) -> Result<ResolvedFrame> {
    match target {
        WindowTarget::Pid { pid } => {
            let app = apps
                .iter()
                .find(|a| a.pid == *pid)
                .ok_or_else(|| Error::Snapshot(format!("no AT-SPI application with pid {pid}")))?;
            first_frame(conn, app).await
        }
        WindowTarget::ProcessName { name } => {
            let needle = name.to_lowercase();
            let app = apps
                .iter()
                .find(|a| {
                    a.name.to_lowercase().contains(&needle)
                        || proc_exe_stem(a.pid).is_some_and(|s| s.to_lowercase() == needle)
                })
                .ok_or_else(|| {
                    Error::Snapshot(format!("no AT-SPI application matching process {name:?}"))
                })?;
            first_frame(conn, app).await
        }
        WindowTarget::Title { title } => {
            let needle = title.to_lowercase();
            for app in apps {
                let frames = frames_of_app(conn, &app.bus, &app.path).await?;
                if let Some(f) = frames
                    .into_iter()
                    .find(|f| f.name.to_lowercase().contains(&needle))
                {
                    return Ok(resolved(app, f));
                }
            }
            Err(Error::Snapshot(format!(
                "no AT-SPI window with a title containing {title:?}"
            )))
        }
        WindowTarget::Foreground => {
            // Prefer the app whose frame reports the `Active` state; fall
            // back to the first app that exposes any top-level window.
            let mut fallback: Option<ResolvedFrame> = None;
            for app in apps {
                let frames = frames_of_app(conn, &app.bus, &app.path).await?;
                for f in frames {
                    if f.active {
                        return Ok(resolved(app, f));
                    }
                    if fallback.is_none() {
                        fallback = Some(resolved(app, f));
                    }
                }
            }
            fallback.ok_or_else(|| {
                Error::Snapshot("no AT-SPI application exposes a top-level window".into())
            })
        }
    }
}

/// Build a [`ResolvedFrame`] from an app and one of its frames.
fn resolved(app: &AppEntry, frame: FrameEntry) -> ResolvedFrame {
    ResolvedFrame {
        bus: app.bus.clone(),
        app_path: app.path.clone(),
        app_name: app.name.clone(),
        frame_path: frame.path,
    }
}

/// Pick an application's first top-level frame, preferring an active one.
async fn first_frame(conn: &Connection, app: &AppEntry) -> Result<ResolvedFrame> {
    let mut frames = frames_of_app(conn, &app.bus, &app.path).await?;
    if frames.is_empty() {
        return Err(Error::Snapshot(format!(
            "AT-SPI application {:?} has no top-level window",
            app.name
        )));
    }
    let idx = frames.iter().position(|f| f.active).unwrap_or(0);
    Ok(resolved(app, frames.swap_remove(idx)))
}

/// Enumerate every `application` accessible registered under the registry root.
async fn enumerate_apps(conn: &Connection) -> Result<Vec<AppEntry>> {
    let root = accessible(conn, REGISTRY_DEST, ROOT_PATH).await?;
    let children = root
        .get_children()
        .await
        .map_err(|e| Error::Snapshot(format!("AT-SPI registry GetChildren failed: {e}")))?;

    let mut apps = Vec::new();
    for child in children {
        let Some(bus) = child.name_as_str() else {
            continue;
        };
        let bus = bus.to_owned();
        let path = child.path_as_str().to_owned();
        let Ok(proxy) = accessible(conn, &bus, &path).await else {
            continue;
        };
        let name = proxy.name().await.unwrap_or_default().trim().to_owned();
        let pid = pid_for_bus(conn, &bus).await.unwrap_or(0);
        apps.push(AppEntry {
            bus,
            path,
            name,
            pid,
        });
    }
    Ok(apps)
}

/// List the top-level `frame`/`window` accessibles of one application.
async fn frames_of_app(
    conn: &Connection,
    app_bus: &str,
    app_path: &str,
) -> Result<Vec<FrameEntry>> {
    let app = accessible(conn, app_bus, app_path).await?;
    let children = app
        .get_children()
        .await
        .map_err(|e| Error::Snapshot(format!("AT-SPI application GetChildren failed: {e}")))?;

    let mut frames = Vec::new();
    for child in children {
        if child.is_null() {
            continue;
        }
        let path = child.path_as_str().to_owned();
        let Ok(proxy) = accessible(conn, app_bus, &path).await else {
            continue;
        };
        let role = proxy.get_role().await.unwrap_or(AtRole::Invalid);
        if !is_window_role(role) {
            continue;
        }
        let name = proxy.name().await.unwrap_or_default().trim().to_owned();
        let active = proxy
            .get_state()
            .await
            .is_ok_and(|s| s.contains(AtState::Active));
        frames.push(FrameEntry { path, name, active });
    }
    Ok(frames)
}

/// State carried through the recursive tree walk.
struct Walk {
    conn: Connection,
    refs: RefMap,
    nth: HashMap<(Role, String), usize>,
    max_depth: usize,
    compact: bool,
    budget: usize,
}

/// Recursively build a [`Node`] subtree rooted at `(bus, path)`.
///
/// Returns `None` when the accessible is gone (defunct) or the node budget is
/// exhausted. The recursion is `Box::pin`ned because `async fn`s cannot recurse
/// directly.
fn build_subtree(
    walk: &mut Walk,
    bus: String,
    path: String,
    depth: usize,
) -> Pin<Box<dyn Future<Output = Option<Node>> + Send + '_>> {
    Box::pin(async move {
        if walk.budget == 0 {
            return None;
        }
        walk.budget -= 1;

        let proxy = accessible(&walk.conn, &bus, &path).await.ok()?;
        let at_role = proxy.get_role().await.unwrap_or(AtRole::Invalid);
        let states = proxy
            .get_state()
            .await
            .unwrap_or_else(|_| StateSet::empty());
        let role = map_role(at_role, states);
        let state = map_state(states);
        let name = proxy.name().await.unwrap_or_default().trim().to_owned();
        let description = proxy
            .description()
            .await
            .ok()
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty() && *d != name);

        let (bounds, value) = match proxy.proxies().await {
            Ok(px) => (node_bounds(&px).await, node_value(&px, &name).await),
            Err(_) => (None, None),
        };

        let native = Some(NativeHandle::AtSpi {
            bus_name: bus.clone(),
            path: path.clone(),
        });

        // Emit a ref for anything an agent can target (interactive controls
        // and text-bearing content); `nth` is the global pre-order count of
        // earlier nodes with the same `(role, name)`, mirrored at action time.
        let ref_id = if role.is_interactive() || role.is_content() {
            let key = (role.clone(), name.clone());
            let counter = walk.nth.entry(key).or_insert(0);
            let nth = *counter;
            *counter += 1;
            Some(
                walk.refs
                    .insert(role.clone(), name.clone(), nth, native.clone()),
            )
        } else {
            None
        };

        let mut children = Vec::new();
        if depth < walk.max_depth {
            if let Ok(kids) = proxy.get_children().await {
                for kid in kids {
                    if kid.is_null() {
                        continue;
                    }
                    let (Some(kid_bus), kid_path) = (
                        kid.name_as_str().map(str::to_owned),
                        kid.path_as_str().to_owned(),
                    ) else {
                        continue;
                    };
                    if let Some(child) = build_subtree(walk, kid_bus, kid_path, depth + 1).await {
                        if walk.compact && is_compactable(&child) {
                            // Drop the empty wrapper, keep its descendants.
                            children.extend(child.children);
                        } else {
                            children.push(child);
                        }
                    }
                }
            }
        }

        Some(Node {
            ref_id,
            role,
            name,
            description,
            value,
            state,
            bounds,
            level: None,
            children,
            opaque: false,
            native,
        })
    })
}

/// Read an element's screen-space bounds via its `Component` interface.
async fn node_bounds(px: &Proxies<'_>) -> Option<Bounds> {
    let component = px.component().await.ok()?;
    let (x, y, w, h) = component.get_extents(CoordType::Screen).await.ok()?;
    (w > 0 && h > 0).then(|| Bounds {
        x: f64::from(x),
        y: f64::from(y),
        w: f64::from(w),
        h: f64::from(h),
    })
}

/// Read an element's value: editable/visible text, else a numeric value.
async fn node_value(px: &Proxies<'_>, name: &str) -> Option<String> {
    if let Ok(text) = px.text().await {
        if let Ok(count) = text.character_count().await {
            if count > 0 {
                if let Ok(content) = text.get_text(0, count).await {
                    if !content.is_empty() && content != name {
                        return Some(content);
                    }
                }
            }
        }
    }
    if let Ok(value) = px.value().await {
        if let Ok(current) = value.current_value().await {
            return Some(format_number(current));
        }
    }
    None
}

/// Format an AT-SPI `f64` value, dropping a trailing `.0` for whole numbers.
fn format_number(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v}")
    }
}

/// Build an `Accessible` proxy for an explicit `(bus, path)`.
async fn accessible(conn: &Connection, bus: &str, path: &str) -> Result<AccessibleProxy<'static>> {
    let bus = BusName::try_from(bus.to_owned())
        .map_err(|e| Error::Surface(format!("invalid AT-SPI bus name {bus:?}: {e}")))?;
    let path = ObjectPath::try_from(path.to_owned())
        .map_err(|e| Error::Surface(format!("invalid AT-SPI object path: {e}")))?;
    AccessibleProxy::builder(conn)
        .destination(bus)
        .map_err(zbus_err)?
        .path(path)
        .map_err(zbus_err)?
        .cache_properties(CacheProperties::No)
        .build()
        .await
        .map_err(zbus_err)
}

/// Tell the desktop accessibility stack that an assistive tech is present.
///
/// GTK and Qt only build and register their accessibility trees when
/// `org.a11y.Status.IsEnabled` is true - the Linux analogue of macOS only
/// exposing AX to a trusted process. agent-ctrl is that assistive tech, so it
/// flips the flag on connect. Best-effort: a failure is logged, not fatal,
/// since some toolkits expose their tree unconditionally.
async fn enable_accessibility() {
    let result: std::result::Result<(), atspi::zbus::Error> = async {
        let session = Connection::session().await?;
        let status = StatusProxy::new(&session).await?;
        status.set_is_enabled(true).await?;
        status.set_screen_reader_enabled(true).await?;
        Ok(())
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(
            "could not set org.a11y.Status.IsEnabled ({e}); apps that gate \
             their accessibility tree on it may snapshot empty"
        );
    }
}

/// Look up the Unix pid behind a D-Bus unique name on the a11y bus.
async fn pid_for_bus(conn: &Connection, bus: &str) -> Option<u32> {
    let dbus = DBusProxy::new(conn).await.ok()?;
    let name = BusName::try_from(bus.to_owned()).ok()?;
    dbus.get_connection_unix_process_id(name).await.ok()
}

/// Resolve `/proc/<pid>/exe` to its file stem (the executable name).
fn proc_exe_stem(pid: u32) -> Option<String> {
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    exe.file_stem().and_then(|s| s.to_str()).map(str::to_owned)
}

/// A node compaction can drop: an unnamed structural wrapper without a ref.
fn is_compactable(node: &Node) -> bool {
    matches!(node.role, Role::Generic) && node.name.is_empty() && !node.state.focused
}

/// `true` for AT-SPI roles that represent a top-level window.
fn is_window_role(role: AtRole) -> bool {
    matches!(
        role,
        AtRole::Frame
            | AtRole::Window
            | AtRole::Dialog
            | AtRole::Alert
            | AtRole::FileChooser
            | AtRole::ColorChooser
            | AtRole::FontChooser
    )
}

/// Map an AT-SPI [`AtRole`] to a canonical [`Role`]. See `docs/atspi-mapping.md` §1.
///
/// `states` disambiguates the AT-SPI `text` role, which toolkits reuse for
/// both editable single-line entries and static/document text.
fn map_role(role: AtRole, states: StateSet) -> Role {
    match role {
        AtRole::Button | AtRole::ToggleButton | AtRole::PushButtonMenu => Role::Button,
        AtRole::CheckBox => Role::Checkbox,
        AtRole::CheckMenuItem => Role::MenuItemCheckbox,
        AtRole::RadioButton => Role::Radio,
        AtRole::RadioMenuItem => Role::MenuItemRadio,
        AtRole::ComboBox => Role::ComboBox,
        AtRole::Entry | AtRole::PasswordText | AtRole::Autocomplete | AtRole::Editbar => {
            Role::TextField
        }
        AtRole::Link => Role::Link,
        AtRole::Heading => Role::Heading,
        AtRole::Image | AtRole::Icon | AtRole::ImageMap => Role::Image,
        AtRole::List | AtRole::ListBox => Role::List,
        AtRole::ListItem => Role::ListItem,
        AtRole::Menu | AtRole::PopupMenu => Role::Menu,
        AtRole::MenuBar => Role::MenuBar,
        AtRole::MenuItem | AtRole::TearoffMenuItem => Role::MenuItem,
        AtRole::PageTab => Role::Tab,
        AtRole::PageTabList => Role::TabList,
        AtRole::Frame | AtRole::Window | AtRole::InternalFrame | AtRole::DesktopFrame => {
            Role::Window
        }
        AtRole::Dialog
        | AtRole::Alert
        | AtRole::FileChooser
        | AtRole::ColorChooser
        | AtRole::FontChooser => Role::Dialog,
        AtRole::Slider | AtRole::Dial => Role::Slider,
        AtRole::SpinButton => Role::SpinButton,
        AtRole::Table => Role::Table,
        AtRole::TreeTable => Role::Grid,
        AtRole::TableCell => Role::Cell,
        AtRole::TableColumnHeader | AtRole::ColumnHeader => Role::ColumnHeader,
        AtRole::TableRowHeader | AtRole::RowHeader => Role::RowHeader,
        AtRole::TableRow => Role::Row,
        AtRole::Tree => Role::Tree,
        AtRole::TreeItem => Role::TreeItem,
        AtRole::ToolBar => Role::Toolbar,
        AtRole::StatusBar => Role::Region,
        AtRole::Article => Role::Article,
        AtRole::Form => Role::Group,
        AtRole::Application => Role::Application,
        AtRole::DocumentFrame
        | AtRole::DocumentText
        | AtRole::DocumentWeb
        | AtRole::DocumentEmail
        | AtRole::DocumentSpreadsheet
        | AtRole::DocumentPresentation
        | AtRole::HTMLContainer => Role::Document,
        // AT-SPI `text` is overloaded: an editable one is an input field, a
        // read-only one is document/body text.
        AtRole::Text => {
            if states.contains(AtState::Editable) {
                Role::TextField
            } else {
                Role::Document
            }
        }
        AtRole::Label | AtRole::AcceleratorLabel | AtRole::Caption | AtRole::Static => {
            Role::Generic
        }
        AtRole::Panel
        | AtRole::Filler
        | AtRole::Separator
        | AtRole::ScrollBar
        | AtRole::ScrollPane
        | AtRole::Viewport
        | AtRole::Section
        | AtRole::RedundantObject
        | AtRole::Grouping
        | AtRole::GlassPane
        | AtRole::LayeredPane
        | AtRole::RootPane
        | AtRole::SplitPane
        | AtRole::OptionPane
        | AtRole::ProgressBar
        | AtRole::LevelBar
        | AtRole::Unknown
        | AtRole::Invalid => Role::Generic,
        other => Role::Unknown(format!("atspi_{}", other.name().replace(' ', "_"))),
    }
}

/// Map an AT-SPI [`StateSet`] to the schema [`State`]. See `docs/atspi-mapping.md` §2.
fn map_state(states: StateSet) -> State {
    let checked = if states.contains(AtState::Checkable) {
        Some(if states.contains(AtState::Indeterminate) {
            Checked::Mixed
        } else if states.contains(AtState::Checked) {
            Checked::True
        } else {
            Checked::False
        })
    } else {
        None
    };
    State {
        visible: states.contains(AtState::Showing) && states.contains(AtState::Visible),
        // GTK marks interactable widgets `Sensitive` but does not always also
        // set `Enabled`; treat either as enabled, and a control disabled only
        // when it has lost both.
        enabled: states.contains(AtState::Sensitive) || states.contains(AtState::Enabled),
        focused: states.contains(AtState::Focused),
        selected: states
            .contains(AtState::Selectable)
            .then(|| states.contains(AtState::Selected)),
        checked,
        expanded: states
            .contains(AtState::Expandable)
            .then(|| states.contains(AtState::Expanded)),
        required: states.contains(AtState::Required).then_some(true),
    }
}

/// Wrap a [`zbus::Error`](atspi::zbus::Error) as a surface error.
// Takes the error by value so it can be passed directly to `Result::map_err`,
// which hands ownership of the error to its callback.
#[allow(clippy::needless_pass_by_value)]
fn zbus_err(e: atspi::zbus::Error) -> Error {
    Error::Surface(format!("AT-SPI D-Bus call failed: {e}"))
}

/// The pinned-window mutex was poisoned by a panic on another thread.
fn lock_err() -> Error {
    Error::Surface("AT-SPI pinned-window mutex poisoned".into())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn role_mapping_covers_common_widgets() {
        let none = StateSet::empty();
        assert_eq!(map_role(AtRole::Button, none), Role::Button);
        assert_eq!(map_role(AtRole::Entry, none), Role::TextField);
        assert_eq!(map_role(AtRole::CheckBox, none), Role::Checkbox);
        assert_eq!(map_role(AtRole::Frame, none), Role::Window);
        assert_eq!(map_role(AtRole::PageTab, none), Role::Tab);
        assert_eq!(map_role(AtRole::Panel, none), Role::Generic);
    }

    #[test]
    fn text_role_splits_on_the_editable_state() {
        let editable = StateSet::new(AtState::Editable);
        assert_eq!(map_role(AtRole::Text, editable), Role::TextField);
        assert_eq!(map_role(AtRole::Text, StateSet::empty()), Role::Document);
    }

    #[test]
    fn unknown_role_keeps_the_raw_atspi_name() {
        // `Terminal` has no schema equivalent; it falls through to Unknown.
        match map_role(AtRole::Terminal, StateSet::empty()) {
            Role::Unknown(name) => assert_eq!(name, "atspi_terminal"),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn state_mapping_reads_checkable_controls() {
        let on = StateSet::new(AtState::Checkable | AtState::Checked | AtState::Enabled);
        assert_eq!(map_state(on).checked, Some(Checked::True));

        let off = StateSet::new(AtState::Checkable);
        assert_eq!(map_state(off).checked, Some(Checked::False));

        let plain = StateSet::new(AtState::Enabled);
        assert!(!plain.contains(AtState::Checkable));
        assert_eq!(map_state(plain).checked, None);
    }

    #[test]
    fn whole_numbers_format_without_a_trailing_zero() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(2.5), "2.5");
    }
}
