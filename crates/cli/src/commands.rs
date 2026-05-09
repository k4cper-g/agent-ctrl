//! Top-level CLI subcommands.
//!
//! The CLI is a thin client around `agent-ctrl-daemon`. Every action verb
//! is one shell command per invocation; the persistent state (the
//! `Surface` session, the pinned window, the most recent snapshot's refs)
//! lives in a long-running daemon discovered through `~/.agent-ctrl/<session>.json`.
//!
//! `agent-ctrl open <surface>` spawns the daemon as a detached child and
//! waits for the state file to appear. Subsequent `snapshot` / `click` /
//! etc. commands read the file, dial the TCP endpoint, send one JSON-RPC
//! request, print the response, and exit.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command as ProcessCommand};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agent_ctrl_core::{
    Action, ClipboardOp, FindQuery, GetField, MouseButton, MouseOp, RefId, Region, Role,
    ScreenshotTarget, SnapshotOptions, StateField, SurfaceKind, WaitOptions, WaitOutcome,
    WaitPredicate, WindowTarget,
};
// `WindowInfo` is only used in the helper that renders/parses the list,
// kept in a separate import so the wide `Action, FindQuery, ...` line stays
// stable as more verbs land.
use agent_ctrl_core::WindowInfo;
use agent_ctrl_daemon::{
    client, ipc, session_file, BatchStep, DaemonState, RequestOp, ResponseBody, SessionFile,
    SessionId, DEFAULT_SESSION,
};
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use clap::{Args, Subcommand};
use serde::Serialize;
use serde_json::json;

const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Top-level subcommand enumeration.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run the daemon. Without `--bind`, talks JSON-RPC over stdio (used
    /// by the TS client). With `--bind`, listens on TCP and writes a
    /// session file under `~/.agent-ctrl/` so CLI commands can find it.
    Daemon(DaemonArgs),

    /// Spawn a daemon for `<surface>` as a detached child process and wait
    /// for it to be ready. Subsequent `snapshot`/`click`/etc. commands
    /// will find this daemon via its session file.
    Open(OpenArgs),

    /// Close the named session: tells the daemon to close its surface
    /// session and exit. Removes the state file.
    Close(SessionArg),

    /// List active daemon sessions discovered under the agent-ctrl home
    /// directory. Stale state files (whose endpoint no longer answers) are
    /// pruned as a side effect.
    List(ListArgs),

    /// Print static facts about this binary: OS, build version, which
    /// surfaces are usable, recommended surface, home directory, and
    /// active session count. Cheap and side-effect-free; designed to be
    /// the first command an agent runs in a fresh session.
    Info(InfoArgs),

    /// Diagnose the install: environment, surface compile status, daemon
    /// state, and a live mock-daemon round-trip probe. Reports each
    /// check as pass/warn/fail/info with optional fix hints. `--fix`
    /// applies the safe automatic repairs (e.g. removing stale session
    /// files).
    Doctor(DoctorArgs),

    /// Launch a process detached from this shell. Prints `ok pid=<n>` so
    /// the agent's next command can `snapshot --target-pid <n>`. Use
    /// `--wait MS` to sleep before returning so the spawned app has time
    /// to create its window before the agent acts on it.
    Launch(LaunchArgs),

    /// Capture the current snapshot of the named session's surface and
    /// print it as a tree of refs (or as raw JSON with `--json`). Window
    /// targeting flags pin which window subsequent actions will operate on.
    Snapshot(SnapshotArgs),

    /// Look up refs in the most recent snapshot without re-walking the OS
    /// accessibility tree. Filters by name (substring, case-insensitive by
    /// default) and optionally by role or subtree. Run `agent-ctrl
    /// snapshot` first to populate the cache.
    Find(FindCliArgs),
    /// Read one field from a ref in the most recent cached snapshot.
    Get(GetArgs),
    /// Check one boolean state on a ref in the most recent cached snapshot.
    Is(IsArgs),

    /// Enumerate the top-level windows the session can target. Mirrors
    /// agent-browser's `tab_list` for native UI: the agent uses this to
    /// discover dialogs and popups that opened outside the currently
    /// pinned window, then switches to one with `focus-window <id>`.
    #[command(name = "window-list")]
    WindowList(WindowListArgs),

    // --- Pointer / focus actions ---
    /// Click an element by ref (e.g. `@e3` or `ref_3`).
    Click(RefArg),
    /// Double-click an element by ref.
    #[command(name = "double-click")]
    DoubleClick(RefArg),
    /// Right-click an element by ref.
    #[command(name = "right-click")]
    RightClick(RefArg),
    /// Move the cursor over an element (no buttons).
    Hover(RefArg),
    /// Move keyboard focus to an element (UIA SetFocus).
    Focus(RefArg),
    /// Set a checkable control to checked.
    Check(RefArg),
    /// Set a checkable control to unchecked.
    Uncheck(RefArg),
    /// Toggle a checkable control.
    Toggle(RefArg),
    /// Clear an editable field.
    Clear(RefArg),

    // --- Text input ---
    /// Replace the value of an editable element via UIA ValuePattern.
    Fill(FillArgs),
    /// Type a literal string at the current focus via SendInput.
    #[command(name = "type")]
    TypeText(TypeArgs),
    /// Press a key chord (e.g. `Enter`, `Ctrl+A`, `Ctrl+Shift+T`).
    Press(PressArgs),
    /// Press a single key down without releasing.
    #[command(name = "key-down")]
    KeyDown(KeyArgs),
    /// Release a previously-pressed key.
    #[command(name = "key-up")]
    KeyUp(KeyArgs),

    // --- Selection / scroll ---
    /// Select the named option inside (or under) the referenced element.
    Select(SelectArgs),
    /// Select all content. `--ref` focuses the field first; without it,
    /// just sends `Ctrl+A` to whatever has focus.
    #[command(name = "select-all")]
    SelectAll(OptRefArg),
    /// Read, write, copy, or paste via the host clipboard.
    Clipboard(ClipboardArgs),
    /// Send a raw mouse event.
    Mouse(MouseArgs),
    /// Scroll the referenced element into view.
    #[command(name = "scroll-into-view")]
    ScrollIntoView(RefArg),
    /// Scroll by `(dx, dy)` logical pixels. Positive `dy` scrolls content
    /// downward. `--ref` positions the cursor over the element first so
    /// the wheel events route to its scroll container.
    Scroll(ScrollArgs),
    /// Drag from one element to another.
    Drag(DragArgs),

    // --- Window / app targeting ---
    /// Bring the named app to the foreground and re-pin the session on it.
    #[command(name = "switch-app")]
    SwitchApp(AppIdArg),
    /// Bring the window with the given hex `window_id` to the foreground.
    #[command(name = "focus-window")]
    FocusWindow(WindowIdArg),
    /// Briefly mark an element for human debugging.
    Highlight(HighlightArgs),

    // --- Output / waits ---
    /// Capture a screenshot. With no path, writes a PNG to a temp file and
    /// prints its absolute path. With a path, writes there.
    Screenshot(ScreenshotArgs),
    /// Sleep on the daemon worker for `ms` milliseconds.
    Wait(WaitArgs),

    /// Block until a UI predicate is satisfied. Three modes:
    ///   `wait-for "Save"`           - appearance (race-window caveat: chain
    ///                                 with `--stable` for racy follow-up)
    ///   `wait-for "Save" --gone`    - disappearance (very reliable)
    ///   `wait-for --stable`         - tree signature unchanged for `--idle-ms`
    ///                                 (very reliable, dodges naming a node)
    /// On match prints `ok matched ...`, on disappearance `ok gone ...`,
    /// on stable `ok stable ...`. On timeout prints to stderr and exits 2.
    #[command(name = "wait-for")]
    WaitFor(WaitForArgs),
    /// Execute ordered JSON batch steps.
    Batch(BatchArgs),
}

// ---------- Arg structs ----------

#[derive(Debug, Args)]
pub(crate) struct InfoArgs {
    /// Emit a parseable JSON payload instead of the human-readable form.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct LaunchArgs {
    /// Emit JSON instead of `ok pid=<n>`.
    #[arg(long)]
    json: bool,
    /// Absolute path to an executable, or a name resolvable on PATH.
    path: String,
    /// Arguments forwarded to the launched process. Anything after the
    /// path is collected verbatim, including hyphenated flags.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
    /// Sleep this many milliseconds after spawn before returning. Useful
    /// so a snapshot run immediately afterwards finds the spawned app's
    /// window already drawn. Defaults to 0 (fire-and-forget).
    #[arg(long, default_value_t = 0)]
    wait: u64,
}

#[derive(Debug, Args)]
pub(crate) struct DoctorArgs {
    /// Emit JSON with the same shape as the human form.
    #[arg(long)]
    json: bool,
    /// Apply safe automatic repairs (currently: prune stale session files).
    #[arg(long)]
    fix: bool,
    /// Skip the live mock-daemon probe. Faster but less coverage.
    #[arg(long)]
    quick: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DaemonArgs {
    /// `host:port` to bind a TCP listener on. When set, the daemon also
    /// auto-opens a surface session and writes a state file under
    /// `~/.agent-ctrl/<session>.json`. Without this, the daemon stays in
    /// stdio mode (used by the TS client).
    #[arg(long)]
    bind: Option<String>,

    /// Session name for the state file. Defaults to `default`. Only
    /// meaningful with `--bind`.
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,

    /// Surface kind to auto-open in TCP mode (`mock`, `uia`, `ax`,
    /// `android`, `ios`). Required when `--bind` is given.
    #[arg(long)]
    surface: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct OpenArgs {
    /// Emit JSON instead of the human-readable status line.
    #[arg(long)]
    json: bool,
    /// Surface to open: `mock` / `uia` / `ax` / `android` / `ios`.
    surface: String,
    /// Session name. Defaults to `default`.
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct SessionArg {
    /// Emit JSON instead of the human-readable status line.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct ListArgs {
    /// Emit JSON instead of the human-readable table.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SnapshotArgs {
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,

    /// Pin to the first window owned by a process whose executable file
    /// stem matches this name (case-insensitive). Locale-independent.
    #[arg(long, conflicts_with_all = ["target_pid", "target_title", "target_foreground"])]
    target_process: Option<String>,

    /// Pin to the first visible top-level window owned by this PID.
    #[arg(long, conflicts_with_all = ["target_title", "target_foreground"])]
    target_pid: Option<u32>,

    /// Pin to the first window whose title contains this substring
    /// (case-insensitive). Title text is locale-dependent - prefer
    /// `--target-process` for portable scripts.
    #[arg(long, conflicts_with = "target_foreground")]
    target_title: Option<String>,

    /// Pin to whatever window currently has user focus.
    #[arg(long)]
    target_foreground: bool,

    /// Print the raw JSON response instead of the pretty tree.
    #[arg(long)]
    json: bool,

    /// Drop redundant intermediate nodes from the printed tree.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    compact: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RefArg {
    /// Element ref like `@e3` or `ref_3`.
    #[arg(value_name = "REF")]
    target: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct HighlightArgs {
    /// Element ref like `@e3` or `ref_3`.
    #[arg(value_name = "REF")]
    target: String,
    /// Highlight duration in milliseconds.
    #[arg(long)]
    duration_ms: Option<u64>,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct OptRefArg {
    /// Optional element ref like `@e3` or `ref_3`. When omitted, the
    /// action targets whatever has focus.
    #[arg(value_name = "REF")]
    target: Option<String>,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct ClipboardArgs {
    /// Operation: `read`, `write`, `copy`, or `paste`.
    op: String,
    /// Text for `write`.
    text: Option<String>,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct MouseArgs {
    /// Operation: `move`, `down`, `up`, or `wheel`.
    op: String,
    /// X coordinate in screen pixels.
    x: i32,
    /// Y coordinate in screen pixels.
    y: i32,
    /// Button for `down` or `up`: `left`, `right`, or `middle`.
    #[arg(long, default_value = "left")]
    button: String,
    /// Horizontal wheel delta for `wheel`.
    #[arg(long, default_value_t = 0)]
    dx: i32,
    /// Vertical wheel delta for `wheel`.
    #[arg(long, default_value_t = 0)]
    dy: i32,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct FillArgs {
    /// Element ref like `@e3` or `ref_3`.
    #[arg(value_name = "REF")]
    target: String,
    /// New value for the element.
    value: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct TypeArgs {
    /// Text to type at the current focus.
    text: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct PressArgs {
    /// Key chord, e.g. `Enter`, `Ctrl+A`, `Ctrl+Shift+T`.
    keys: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct KeyArgs {
    /// Key name, e.g. `Shift`, `A`, `F12`, `ArrowUp`.
    key: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct SelectArgs {
    /// Container ref OR option ref.
    #[arg(value_name = "REF")]
    target: String,
    /// Option name to select. When the ref is itself the option, this is
    /// used as a name match (or `--exact`-equivalent sanity check).
    value: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct ScrollArgs {
    /// Horizontal delta in logical pixels (positive scrolls right).
    dx: f64,
    /// Vertical delta in logical pixels (positive scrolls content down).
    dy: f64,
    /// Optional ref to position the cursor over before scrolling.
    #[arg(long, value_name = "REF")]
    r#ref: Option<String>,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct DragArgs {
    /// Source element ref.
    from: String,
    /// Destination element ref.
    to: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct AppIdArg {
    /// Application id - full path (`C:\...\Notepad.exe`) or bare name
    /// (`Notepad`).
    app_id: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct WindowIdArg {
    /// Window id from a prior snapshot's `window.id` (hex, e.g. `0x10edc`).
    window_id: String,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct ScreenshotArgs {
    /// Where to write the PNG. When omitted, the file is written to the
    /// system temp dir and its absolute path is printed.
    path: Option<PathBuf>,
    /// Optional region in physical screen pixels. Format: `X,Y,W,H`. When
    /// omitted, captures the snapshot's pinned window.
    #[arg(long, value_name = "X,Y,W,H")]
    region: Option<String>,
    /// Target to capture: `window`, `desktop`, `region`, or `ref`.
    #[arg(long)]
    target: Option<String>,
    /// Element ref used when `--target ref`.
    #[arg(long, value_name = "REF")]
    r#ref: Option<String>,
    /// Draw visible ref labels when supported.
    #[arg(long)]
    annotated: bool,
    /// Emit JSON metadata for the written PNG.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct FindCliArgs {
    /// Substring to match against `name`. Case-insensitive by default;
    /// pass `--exact` for case-sensitive equality. Omit to match any name.
    #[arg(value_name = "NAME")]
    name: Option<String>,

    /// Restrict to a single role (kebab-case, e.g. `button`, `menu-item`,
    /// `text-field`). Run `agent-ctrl snapshot` to see what roles are in
    /// the tree.
    #[arg(long)]
    role: Option<String>,

    /// Treat `NAME` as a case-sensitive exact match instead of a
    /// case-insensitive substring.
    #[arg(long)]
    exact: bool,

    /// Restrict the search to the subtree under this ref (e.g. `@e2`).
    /// Useful for "find OK button inside the Save dialog".
    #[arg(long, value_name = "REF")]
    r#in: Option<String>,

    /// Print only the first match and as a bare ref (`@e5`), no role/name
    /// suffix. Designed for shell substitution: `agent-ctrl click
    /// "$(agent-ctrl find Save --first)"`. Exits 1 if there is no match.
    #[arg(long)]
    first: bool,

    /// Cap the number of results. Ignored when `--first` is set.
    #[arg(long, default_value_t = 50)]
    limit: usize,

    /// Emit JSON instead of the human-readable rows. With `--first`, the
    /// first match is included as `first` instead of printing a bare ref.
    #[arg(long)]
    json: bool,

    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct GetArgs {
    /// Field: `text`, `value`, `name`, `role`, `state`, `bounds`, or `window`.
    field: String,
    /// Ref to read. Omit for `window`.
    #[arg(value_name = "REF")]
    target: Option<String>,
    /// Emit the full get result as JSON instead of only the value.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct IsArgs {
    /// State: `visible`, `enabled`, `focused`, `selected`, `checked`, or `expanded`.
    field: String,
    /// Ref to check.
    #[arg(value_name = "REF")]
    target: String,
    /// Emit the full state check result as JSON instead of only the bool.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct BatchArgs {
    /// Stop after the first failed step.
    #[arg(long)]
    bail: bool,
    /// JSON array of batch steps. Convenient on Unix shells; prefer
    /// `--file` or `--stdin` on PowerShell.
    steps_json: Option<String>,
    /// Read the JSON array of batch steps from a file.
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// Read the JSON array of batch steps from stdin.
    #[arg(long)]
    stdin: bool,
    /// Compatibility no-op: batch output is always JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct WindowListArgs {
    /// Print only the bare hex id of the first window that is *not* the
    /// session's currently pinned window. Designed for shell substitution
    /// into `focus-window` when a dialog just spawned:
    ///
    ///     agent-ctrl focus-window "$(agent-ctrl window-list --first-other)"
    ///
    /// Exits 1 with `no other windows` on stderr when only the pinned
    /// window exists.
    #[arg(long)]
    first_other: bool,

    /// Emit JSON instead of the human-readable table. With `--first-other`,
    /// the first sibling window is included as `window`.
    #[arg(long)]
    json: bool,

    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
// Clap mirrors user-facing flags directly; bools are clearer than enum wrappers here.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct WaitForArgs {
    /// Name to match (substring, case-insensitive by default). Required
    /// unless `--stable` is set.
    #[arg(value_name = "NAME")]
    name: Option<String>,

    /// Wait for the tree's structural signature to be unchanged for
    /// `--idle-ms` consecutive milliseconds. Mutually exclusive with NAME
    /// and the per-node filters.
    #[arg(long)]
    stable: bool,

    /// Quiet period (ms) that counts as "stable". Only meaningful with
    /// `--stable`.
    #[arg(long, default_value_t = 500)]
    idle_ms: u64,

    /// Wait for the match to disappear instead of appear.
    #[arg(long)]
    gone: bool,

    /// Wait until the first match has this state.
    #[arg(long)]
    state: Option<String>,

    /// Desired value for `--state`.
    #[arg(long, default_value_t = true)]
    state_value: bool,

    /// Wait until matched text contains this substring.
    #[arg(long)]
    text_contains: Option<String>,

    /// Wait until matched value contains this substring.
    #[arg(long)]
    value_contains: Option<String>,

    /// Wait for a window whose title contains this substring.
    #[arg(long)]
    window_appears: Option<String>,

    /// Wait until no window title contains this substring.
    #[arg(long)]
    window_gone: Option<String>,

    /// Restrict to a single role (kebab-case, e.g. `button`, `dialog`).
    #[arg(long)]
    role: Option<String>,

    /// Treat NAME as a case-sensitive exact match.
    #[arg(long)]
    exact: bool,

    /// Restrict the search to the subtree under this ref (e.g. `@e2`).
    #[arg(long, value_name = "REF")]
    r#in: Option<String>,

    /// Maximum total wait, milliseconds.
    #[arg(long, default_value_t = 10_000)]
    timeout: u64,

    /// Polling interval, milliseconds. Floored at 50 by the daemon - finer
    /// polling burns CPU on UIA tree walks without buying reliability.
    #[arg(long, default_value_t = 250)]
    poll: u64,

    /// Emit the structured wait outcome as JSON.
    #[arg(long)]
    json: bool,

    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

#[derive(Debug, Args)]
pub(crate) struct WaitArgs {
    /// Milliseconds to sleep on the daemon worker thread.
    ms: u64,
    /// Emit the structured action result as JSON.
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = DEFAULT_SESSION)]
    session: String,
}

// ---------- Dispatch ----------

impl Command {
    pub(crate) fn wants_json_output(&self) -> bool {
        match self {
            Self::Daemon(_) => false,
            Self::Open(args) => args.json,
            Self::Close(args) => args.json,
            Self::List(args) => args.json,
            Self::Info(args) => args.json,
            Self::Doctor(args) => args.json,
            Self::Launch(args) => args.json,
            Self::Snapshot(args) => args.json,
            Self::Find(args) => args.json,
            Self::Get(args) => args.json,
            Self::Is(args) => args.json,
            Self::WindowList(args) => args.json,
            Self::Click(args)
            | Self::DoubleClick(args)
            | Self::RightClick(args)
            | Self::Hover(args)
            | Self::Focus(args)
            | Self::Check(args)
            | Self::Uncheck(args)
            | Self::Toggle(args)
            | Self::Clear(args)
            | Self::ScrollIntoView(args) => args.json,
            Self::Fill(args) => args.json,
            Self::TypeText(args) => args.json,
            Self::Press(args) => args.json,
            Self::KeyDown(args) | Self::KeyUp(args) => args.json,
            Self::Select(args) => args.json,
            Self::SelectAll(args) => args.json,
            Self::Clipboard(args) => args.json,
            Self::Mouse(args) => args.json,
            Self::Scroll(args) => args.json,
            Self::Drag(args) => args.json,
            Self::SwitchApp(args) => args.json,
            Self::FocusWindow(args) => args.json,
            Self::Highlight(args) => args.json,
            Self::Screenshot(args) => args.json,
            Self::Wait(args) => args.json,
            Self::WaitFor(args) => args.json,
            Self::Batch(_) => true,
        }
    }

    pub(crate) async fn run(self) -> Result<()> {
        match self {
            Self::Daemon(args) => run_daemon(args).await,
            Self::Open(args) => run_open(&args),
            Self::Close(args) => run_close(&args.session, args.json),
            Self::List(args) => run_list(args.json),
            Self::Info(a) => crate::info::run_info(a.json),
            Self::Doctor(a) => crate::doctor::run_doctor(crate::doctor::DoctorOptions {
                json: a.json,
                fix: a.fix,
                quick: a.quick,
            }),
            Self::Launch(a) => run_launch(&a),
            Self::Snapshot(args) => run_snapshot(args),
            Self::Find(args) => run_find(args),
            Self::Get(args) => run_get(&args),
            Self::Is(args) => run_is(&args),
            Self::WindowList(args) => run_window_list(&args),
            Self::Click(a) => run_simple_ref_action(&a, |r| Action::Click { ref_id: r }),
            Self::DoubleClick(a) => {
                run_simple_ref_action(&a, |r| Action::DoubleClick { ref_id: r })
            }
            Self::RightClick(a) => run_simple_ref_action(&a, |r| Action::RightClick { ref_id: r }),
            Self::Hover(a) => run_simple_ref_action(&a, |r| Action::Hover { ref_id: r }),
            Self::Focus(a) => run_simple_ref_action(&a, |r| Action::Focus { ref_id: r }),
            Self::Check(a) => run_simple_ref_action(&a, |r| Action::Check { ref_id: r }),
            Self::Uncheck(a) => run_simple_ref_action(&a, |r| Action::Uncheck { ref_id: r }),
            Self::Toggle(a) => run_simple_ref_action(&a, |r| Action::Toggle { ref_id: r }),
            Self::Clear(a) => run_simple_ref_action(&a, |r| Action::Clear { ref_id: r }),
            Self::Fill(a) => run_fill(a),
            Self::TypeText(a) => run_act(&a.session, Action::Type { text: a.text }, a.json),
            Self::Press(a) => run_act(&a.session, Action::Press { keys: a.keys }, a.json),
            Self::KeyDown(a) => run_act(&a.session, Action::KeyDown { key: a.key }, a.json),
            Self::KeyUp(a) => run_act(&a.session, Action::KeyUp { key: a.key }, a.json),
            Self::Select(a) => run_select(a),
            Self::SelectAll(a) => run_select_all(a),
            Self::Clipboard(a) => run_clipboard(a),
            Self::Mouse(a) => run_mouse(&a),
            Self::ScrollIntoView(a) => {
                run_simple_ref_action(&a, |r| Action::ScrollIntoView { ref_id: r })
            }
            Self::Scroll(a) => run_scroll(a),
            Self::Drag(a) => run_drag(&a),
            Self::SwitchApp(a) => {
                run_act(&a.session, Action::SwitchApp { app_id: a.app_id }, a.json)
            }
            Self::FocusWindow(a) => run_focus_window(a),
            Self::Highlight(a) => run_highlight(&a),
            Self::Screenshot(a) => run_screenshot(a),
            Self::Wait(a) => run_act(&a.session, Action::Wait { ms: a.ms }, a.json),
            Self::WaitFor(a) => run_wait_for(&a),
            Self::Batch(a) => run_batch(&a),
        }
    }
}

// ---------- Daemon mode ----------

async fn run_daemon(args: DaemonArgs) -> Result<()> {
    if let Some(bind) = args.bind {
        let surface = args
            .surface
            .ok_or_else(|| anyhow!("--surface is required when --bind is given"))?;
        let kind = parse_surface(&surface)?;
        run_tcp_daemon(&bind, &args.session, kind).await
    } else {
        tracing::info!("starting agent-ctrl daemon on stdio");
        let state = DaemonState::new();
        ipc::run_stdio(&state)
            .await
            .context("daemon stdio loop failed")
    }
}

async fn run_tcp_daemon(bind: &str, session_name: &str, surface: SurfaceKind) -> Result<()> {
    let state = std::sync::Arc::new(DaemonState::new());

    // Auto-open a single surface session up front; the session id goes in
    // the state file so CLI commands can use it implicitly.
    let surface_box = agent_ctrl_daemon::open_surface(surface)
        .await
        .with_context(|| format!("opening surface {}", surface.as_str()))?;
    let session_id = state.open(surface_box).await;

    let session_name = session_name.to_owned();
    let session_for_cleanup = session_name.clone();
    let surface_label = surface.as_str().to_owned();
    let session_id_str = session_id.to_string();
    let auth_token = uuid::Uuid::new_v4().to_string();
    let auth_for_listener = auth_token.clone();

    // Catch SIGINT / Ctrl-C so the state file gets removed even when the
    // user kills the daemon manually.
    {
        let cleanup_name = session_for_cleanup.clone();
        tokio::spawn(async move {
            if let Ok(()) = tokio::signal::ctrl_c().await {
                tracing::info!("ctrl-c received; cleaning up session file");
                let _ = session_file::remove(&cleanup_name);
                std::process::exit(0);
            }
        });
    }

    let result = ipc::run_tcp(state, bind, auth_for_listener, move |addr| {
        let info = SessionFile {
            name: session_name.clone(),
            pid: std::process::id(),
            endpoint: addr.to_string(),
            version: CRATE_VERSION.to_owned(),
            protocol_version: agent_ctrl_daemon::PROTOCOL_VERSION,
            surface: surface_label.clone(),
            auth_token: auth_token.clone(),
            started_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            daemon_session_id: session_id_str.clone(),
        };
        async move {
            if let Err(e) = session_file::write(&info) {
                tracing::warn!("failed to write session file: {e}");
            }
        }
    })
    .await;

    // Clean up the state file on graceful exit too.
    let _ = session_file::remove(&session_for_cleanup);
    result.map_err(Into::into)
}

// ---------- open / close / list ----------

fn run_open(args: &OpenArgs) -> Result<()> {
    let kind = parse_surface(&args.surface)?;

    if let Some(existing) = session_file::read_alive(&args.session) {
        bail!(
            "session {:?} is already running (pid {}, surface {}). Run `agent-ctrl close --session {}` first.",
            args.session,
            existing.pid,
            existing.surface,
            args.session
        );
    }

    // Spawn the daemon as a detached child. `bind 127.0.0.1:0` lets the OS
    // pick an ephemeral port; the daemon writes the actual port to the
    // state file when it binds.
    let exe = std::env::current_exe().context("locating agent-ctrl binary")?;
    let mut cmd = ProcessCommand::new(&exe);
    cmd.arg("daemon")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--session")
        .arg(&args.session)
        .arg("--surface")
        .arg(kind.as_str())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP - the daemon survives
        // when the spawning shell exits, and Ctrl-C in the shell doesn't
        // kill it.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }

    let child = spawn_detached_child(&mut cmd).context("spawning agent-ctrl daemon child")?;
    let child_pid = child.id();

    let info = session_file::wait_for_alive(&args.session, Duration::from_secs(10)).ok_or_else(
        || {
            anyhow!(
                "daemon child (pid {child_pid}) did not become ready within 10s; check `~/.agent-ctrl/{}.json` and stderr",
                args.session
            )
        },
    )?;

    if args.json {
        print_json(&json!({
            "ok": true,
            "session": session_public_json(&info),
        }))?;
    } else {
        println!(
            "ok session={} surface={} pid={} endpoint={}",
            info.name, info.surface, info.pid, info.endpoint
        );
    }
    Ok(())
}

fn run_close(session: &str, json_output: bool) -> Result<()> {
    let info = session_file::read_alive(session)
        .ok_or_else(|| anyhow!("no live daemon for session {session:?}"))?;

    // Ask the daemon to close its surface session, then to shut down. We
    // ignore the CloseSession response on error - the surface might fail
    // to clean up, but we still want the daemon to exit and the state
    // file to be removed.
    let session_id = parse_session_id(&info.daemon_session_id)?;
    let _ = client::send(
        &info,
        RequestOp::CloseSession {
            session: session_id,
        },
    );

    let resp = client::send(&info, RequestOp::Shutdown)
        .with_context(|| format!("sending Shutdown to {}", info.endpoint))?;
    match resp.body {
        ResponseBody::Stopped => {}
        ResponseBody::Error { message } => bail!("daemon refused shutdown: {message}"),
        other => bail!("unexpected response to Shutdown: {other:?}"),
    }

    // Wait briefly for the daemon to remove its own state file. Force-clean
    // if it didn't get the chance (e.g., it was killed mid-cleanup).
    for _ in 0..20 {
        if session_file::read(session).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = session_file::remove(session);

    if json_output {
        print_json(&json!({
            "ok": true,
            "session": session,
        }))?;
    } else {
        println!("ok session={session} stopped");
    }
    Ok(())
}

/// Spawn a process detached from this shell. Doesn't touch any daemon -
/// it's an orthogonal primitive an agent can compose with `snapshot
/// --target-pid <n>` next, since `agent-ctrl` itself didn't have a way
/// to start an app from inside its own verb vocabulary before this.
///
/// Detachment matters: agents typically run `agent-ctrl launch ...`
/// from a short-lived shell and expect the spawned app to outlive that
/// shell. On Windows we set `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`
/// so the child's stdout/stderr aren't tied to ours and Ctrl-C in the
/// parent shell doesn't take it down. On Unix we put the child in its
/// own process group via `process_group(0)` (the std equivalent of
/// `setsid`), so closing the spawning terminal doesn't SIGHUP the child.
///
/// On macOS, paths ending in `.app` are routed through `/usr/bin/open
/// -na`, which delegates to LaunchServices the same way Finder does.
/// Trying to exec `Foo.app` directly would fail because `.app` bundles
/// are directories, not Mach-O binaries.
fn run_launch(args: &LaunchArgs) -> Result<()> {
    #[cfg(target_os = "macos")]
    if is_macos_app_bundle(&args.path) {
        return launch_macos_app_bundle(args);
    }

    let mut cmd = ProcessCommand::new(&args.path);
    cmd.args(&args.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Same flags `open` uses for the daemon child: outlive the
        // spawning shell and ignore its Ctrl-C.
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }

    let child =
        spawn_detached_child(&mut cmd).with_context(|| format!("launching {:?}", args.path))?;
    let pid = child.id();

    if args.wait > 0 {
        std::thread::sleep(Duration::from_millis(args.wait));
    }

    if args.json {
        print_json(&json!({
            "ok": true,
            "pid": pid,
        }))?;
    } else {
        println!("ok pid={pid}");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn is_macos_app_bundle(path: &str) -> bool {
    let path = std::path::Path::new(path);
    if path.extension().and_then(|e| e.to_str()) == Some("app") {
        return true;
    }
    // Catch the case where someone passed the bundle dir without the
    // `.app` extension (rare but legal): if `Contents/MacOS` exists
    // beneath, it's a bundle.
    path.is_dir() && path.join("Contents").join("MacOS").is_dir()
}

#[cfg(target_os = "macos")]
fn launch_macos_app_bundle(args: &LaunchArgs) -> Result<()> {
    // `open -n` asks for a new instance even if one is already running;
    // `-a` treats the argument as an app bundle. `--args` passes the
    // remaining argv to the launched app. The `open` helper exits
    // immediately after the app is on its way, so its child pid is
    // useless to the caller; we omit it from the JSON.
    let mut cmd = ProcessCommand::new("/usr/bin/open");
    cmd.arg("-na")
        .arg(&args.path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    if !args.args.is_empty() {
        cmd.arg("--args").args(&args.args);
    }
    let output = cmd
        .output()
        .with_context(|| format!("running /usr/bin/open -na {:?}", args.path))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "/usr/bin/open exited with status {:?}: {}",
            output.status.code(),
            stderr.trim()
        );
    }
    if args.wait > 0 {
        std::thread::sleep(Duration::from_millis(args.wait));
    }
    if args.json {
        print_json(&json!({
            "ok": true,
            "path": &args.path,
            "via": "open",
        }))?;
    } else {
        println!("ok via=open path={}", args.path);
    }
    Ok(())
}

fn run_list(json_output: bool) -> Result<()> {
    let sessions = session_file::list_alive();
    if json_output {
        let public_sessions = sessions.iter().map(session_public_json).collect::<Vec<_>>();
        print_json(&json!({ "sessions": public_sessions }))?;
        return Ok(());
    }
    if sessions.is_empty() {
        println!("(no active sessions)");
        return Ok(());
    }
    println!(
        "{:<16}  {:<8}  {:<12}  ENDPOINT",
        "SESSION", "SURFACE", "PID"
    );
    for s in sessions {
        println!(
            "{:<16}  {:<8}  {:<12}  {}",
            s.name, s.surface, s.pid, s.endpoint
        );
    }
    Ok(())
}

// ---------- Snapshot ----------

fn run_snapshot(args: SnapshotArgs) -> Result<()> {
    let info = require_session(&args.session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;

    let target = match (args.target_process, args.target_pid, args.target_title) {
        (Some(name), _, _) => WindowTarget::ProcessName { name },
        (_, Some(pid), _) => WindowTarget::Pid { pid },
        (_, _, Some(title)) => WindowTarget::Title { title },
        // Foreground covers both `--target-foreground` and the no-flag default.
        _ => WindowTarget::Foreground,
    };
    let _ = args.target_foreground;
    let opts = SnapshotOptions {
        target,
        compact: args.compact,
        ..SnapshotOptions::default()
    };

    let resp = client::send(
        &info,
        RequestOp::Snapshot {
            session: session_id,
            opts,
        },
    )
    .context("sending snapshot request")?;

    let snapshot = match resp.body {
        ResponseBody::Snapshot { snapshot } => snapshot,
        ResponseBody::Error { message } => bail!("snapshot failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    };

    if args.json {
        print_json(&snapshot)?;
    } else {
        print_snapshot_tree(&snapshot);
    }
    Ok(())
}

// ---------- Find ----------

fn run_find(args: FindCliArgs) -> Result<()> {
    let info = require_session(&args.session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;

    let role = match args.role.as_deref() {
        Some(s) => Some(parse_role(s)?),
        None => None,
    };
    let in_ref = match args.r#in.as_deref() {
        Some(s) => Some(parse_ref(s)?),
        None => None,
    };
    // `--first` always wins: cap at 1 result so the daemon stops walking early.
    let limit = if args.first {
        Some(1)
    } else {
        Some(args.limit)
    };

    let query = FindQuery {
        name: args.name,
        exact: args.exact,
        role,
        in_ref,
        limit,
    };

    let resp = client::send(
        &info,
        RequestOp::Find {
            session: session_id,
            query,
        },
    )
    .context("sending find request")?;

    let matches = match resp.body {
        ResponseBody::FindResults { matches } => matches,
        ResponseBody::Error { message } => bail!("find failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    };

    if args.json {
        let first = matches.first().cloned();
        let no_match = first.is_none();
        print_json(&json!({
            "matches": matches,
            "first": first,
        }))?;
        if no_match {
            std::process::exit(1);
        }
        return Ok(());
    }

    if matches.is_empty() {
        // "no match" is a query result, not a runtime error - print it
        // plainly without anyhow's `Error:` prefix, but still exit non-zero
        // so shell pipelines can branch on it.
        eprintln!("no match");
        std::process::exit(1);
    }

    if args.first {
        // Bare ref so `agent-ctrl click "$(agent-ctrl find Save --first)"`
        // works without trimming.
        println!("{}", display_ref(&matches[0].ref_id.0));
    } else {
        for m in &matches {
            let ref_label = display_ref(&m.ref_id.0);
            let role = role_label(&m.role);
            // Human-first: aligned columns, quoted name. Roles longer than
            // 12 chars (e.g. `menu-item-checkbox`) just push the name over
            // - readability over rigid alignment.
            println!("{ref_label:<5} {role:<12} {:?}", m.name);
        }
    }
    Ok(())
}

fn run_get(args: &GetArgs) -> Result<()> {
    let info = require_session(&args.session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;
    let field = parse_get_field(&args.field)?;
    let ref_id = match args.target.as_deref() {
        Some(target) => Some(parse_ref(target)?),
        None => None,
    };
    let resp = client::send(
        &info,
        RequestOp::Get {
            session: session_id,
            ref_id,
            field,
        },
    )
    .context("sending get request")?;
    match resp.body {
        ResponseBody::GetDone { output } => {
            if args.json {
                print_json(&output)?;
            } else {
                print_json(&output.value)?;
            }
            Ok(())
        }
        ResponseBody::Error { message } => bail!("get failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

fn run_is(args: &IsArgs) -> Result<()> {
    let info = require_session(&args.session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;
    let field = parse_state_field(&args.field)?;
    let ref_id = parse_ref(&args.target)?;
    let resp = client::send(
        &info,
        RequestOp::Is {
            session: session_id,
            ref_id,
            field,
        },
    )
    .context("sending is request")?;
    match resp.body {
        ResponseBody::IsDone { output } => {
            if args.json {
                print_json(&output)?;
            } else {
                println!("{}", output.value);
            }
            Ok(())
        }
        ResponseBody::Error { message } => bail!("is failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

fn parse_role(s: &str) -> Result<Role> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).with_context(|| {
        format!(
            "unknown role {s:?} (expected kebab-case, e.g. `button`, `menu-item`, `text-field`)"
        )
    })
}

fn parse_get_field(s: &str) -> Result<GetField> {
    match s {
        "text" => Ok(GetField::Text),
        "value" => Ok(GetField::Value),
        "name" => Ok(GetField::Name),
        "role" => Ok(GetField::Role),
        "state" => Ok(GetField::State),
        "bounds" => Ok(GetField::Bounds),
        "window" => Ok(GetField::Window),
        other => bail!("unknown get field {other:?}"),
    }
}

fn parse_state_field(s: &str) -> Result<StateField> {
    match s {
        "visible" => Ok(StateField::Visible),
        "enabled" => Ok(StateField::Enabled),
        "focused" => Ok(StateField::Focused),
        "selected" => Ok(StateField::Selected),
        "checked" => Ok(StateField::Checked),
        "expanded" => Ok(StateField::Expanded),
        other => bail!("unknown state field {other:?}"),
    }
}

// ---------- Window list ----------

fn run_window_list(args: &WindowListArgs) -> Result<()> {
    let info = require_session(&args.session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;

    let resp = client::send(
        &info,
        RequestOp::ListWindows {
            session: session_id,
        },
    )
    .context("sending window-list request")?;

    let windows = match resp.body {
        ResponseBody::Windows { windows } => windows,
        ResponseBody::Error { message } => bail!("window-list failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    };

    if args.json {
        if args.first_other {
            let other = windows.iter().find(|w| !w.pinned);
            print_json(&json!({ "window": other }))?;
            if other.is_none() {
                std::process::exit(1);
            }
        } else {
            print_json(&json!({ "windows": windows }))?;
        }
        return Ok(());
    }

    if args.first_other {
        let other = windows.iter().find(|w| !w.pinned);
        if let Some(w) = other {
            // Bare id for shell substitution into `focus-window`.
            println!("{}", w.id);
            Ok(())
        } else {
            eprintln!("no other windows");
            std::process::exit(1);
        }
    } else {
        print_window_table(&windows);
        Ok(())
    }
}

fn print_window_table(windows: &[WindowInfo]) {
    if windows.is_empty() {
        println!("(no windows)");
        return;
    }
    // Match the column style used by the existing `list` verb: uppercase
    // headers, padded columns, two-space separators.
    println!("{:<12}  {:<8}  {:<14}  TITLE", "ID", "PID", "PROCESS");
    for w in windows {
        let title = w.title.as_deref().unwrap_or("");
        let mut tags = Vec::new();
        if w.pinned {
            tags.push("pinned");
        }
        if w.focused {
            tags.push("focused");
        }
        let tag_suffix = if tags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", tags.join(" "))
        };
        println!(
            "{:<12}  {:<8}  {:<14}  {title}{tag_suffix}",
            w.id, w.pid, w.process,
        );
    }
}

// ---------- Wait-for ----------

// Predicate construction shares validation with transport handling in one CLI verb.
#[allow(clippy::too_many_lines)]
fn run_wait_for(args: &WaitForArgs) -> Result<()> {
    // Mode validation up front. Could be done with clap's `conflicts_with`
    // but post-parse checks read more clearly than the attribute soup.
    let explicit_modes = [
        args.stable,
        args.gone,
        args.state.is_some(),
        args.text_contains.is_some(),
        args.value_contains.is_some(),
        args.window_appears.is_some(),
        args.window_gone.is_some(),
    ]
    .into_iter()
    .filter(|b| *b)
    .count();
    if explicit_modes > 1 {
        bail!("wait-for accepts only one predicate mode at a time");
    }

    let predicate = if args.stable {
        if args.name.is_some() || args.role.is_some() || args.exact || args.r#in.is_some() {
            bail!("--stable cannot be combined with NAME, --role, --exact, or --in");
        }
        WaitPredicate::Stable {
            idle_ms: args.idle_ms,
        }
    } else if let Some(title) = args.window_appears.clone() {
        reject_window_wait_selectors(args)?;
        WaitPredicate::WindowAppears { title }
    } else if let Some(title) = args.window_gone.clone() {
        reject_window_wait_selectors(args)?;
        WaitPredicate::WindowGone { title }
    } else {
        // At least one *selector* (name or role) must be present, otherwise
        // we're either matching nothing (impossible) or every node (which
        // includes the window root itself, so `--gone` would never fire and
        // `Appears` would match on snapshot 1 - useless either way).
        // `--exact` and `--in` are modifiers, not selectors.
        if args.name.is_none() && args.role.is_none() {
            bail!(
                "provide NAME, `--role`, or `--stable` - wait-for needs something to match against"
            );
        }
        let role = match args.role.as_deref() {
            Some(s) => Some(parse_role(s)?),
            None => None,
        };
        let in_ref = match args.r#in.as_deref() {
            Some(s) => Some(parse_ref(s)?),
            None => None,
        };
        let needs_all_matches =
            args.state.is_some() || args.text_contains.is_some() || args.value_contains.is_some();
        let query = FindQuery {
            name: args.name.clone(),
            exact: args.exact,
            role,
            in_ref,
            // The wait predicate fires on the *first* match, so no point
            // walking the whole tree once one is found.
            limit: if needs_all_matches { None } else { Some(1) },
        };
        if let Some(field) = args.state.as_deref() {
            WaitPredicate::State {
                query,
                field: parse_state_field(field)?,
                value: args.state_value,
            }
        } else if let Some(text) = args.text_contains.clone() {
            WaitPredicate::TextContains { query, text }
        } else if let Some(value) = args.value_contains.clone() {
            WaitPredicate::ValueContains { query, value }
        } else if args.gone {
            WaitPredicate::Gone { query }
        } else {
            WaitPredicate::Appears { query }
        }
    };

    let info = require_session(&args.session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;
    let opts = WaitOptions {
        predicate,
        timeout_ms: args.timeout,
        poll_ms: args.poll,
    };

    // Allow the wait to run up to its full timeout plus a little headroom
    // for the final snapshot. Without this, the daemon read timeout (30s)
    // would race a long user-supplied --timeout.
    //
    // Saturating add: a user passing `--timeout u64::MAX` (or anything
    // close) shouldn't wrap to a tiny millis value and produce a confusing
    // TCP timeout instead of the daemon-side timeout outcome.
    let transport_timeout = Duration::from_millis(args.timeout.saturating_add(5_000));
    let resp = client::send_with_timeout(
        &info,
        RequestOp::Wait {
            session: session_id,
            opts,
        },
        transport_timeout,
    )
    .context("sending wait-for request")?;

    let outcome = match resp.body {
        ResponseBody::WaitDone { outcome } => outcome,
        ResponseBody::Error { message } => bail!("wait-for failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    };

    if args.json {
        let timed_out = matches!(outcome, WaitOutcome::Timeout { .. });
        print_json(&outcome)?;
        if timed_out {
            std::process::exit(2);
        }
        return Ok(());
    }

    match outcome {
        WaitOutcome::Matched { found, elapsed_ms } => {
            if let Some(m) = found {
                println!(
                    "ok matched {} {} {:?} after {}ms",
                    display_ref(&m.ref_id.0),
                    role_label(&m.role),
                    m.name,
                    elapsed_ms
                );
            } else {
                println!("ok matched after {elapsed_ms}ms");
            }
            Ok(())
        }
        WaitOutcome::Gone { elapsed_ms } => {
            println!("ok gone after {elapsed_ms}ms");
            Ok(())
        }
        WaitOutcome::Stable { elapsed_ms } => {
            println!("ok stable after {elapsed_ms}ms");
            Ok(())
        }
        WaitOutcome::Timeout { elapsed_ms } => {
            // Timeout is its own thing - not a runtime error in the anyhow
            // sense. Use exit 2 so shell pipelines can branch on
            // "satisfied / timeout / other-error" without parsing strings.
            eprintln!("timeout after {elapsed_ms}ms");
            std::process::exit(2);
        }
    }
}

fn reject_window_wait_selectors(args: &WaitForArgs) -> Result<()> {
    if args.name.is_some() || args.role.is_some() || args.exact || args.r#in.is_some() {
        bail!("window wait predicates cannot be combined with NAME, --role, --exact, or --in");
    }
    Ok(())
}

fn print_snapshot_tree(snap: &agent_ctrl_core::Snapshot) {
    println!(
        "# {} ({}) - {} refs",
        snap.app.name,
        snap.surface_kind.as_str(),
        snap.refs.len(),
    );
    if let Some(w) = &snap.window {
        println!(
            "# window: {}{}",
            w.id,
            w.title
                .as_deref()
                .map(|t| format!(" - {t}"))
                .unwrap_or_default()
        );
    }
    print_node(&snap.root, 0);
}

fn print_node(node: &agent_ctrl_core::Node, depth: usize) {
    let indent = "  ".repeat(depth);
    let role = role_label(&node.role);
    let ref_label = node
        .ref_id
        .as_ref()
        .map(|r| format!("{} ", display_ref(&r.0)))
        .unwrap_or_default();
    let name = if node.name.is_empty() {
        String::new()
    } else {
        format!(" {:?}", node.name)
    };
    let value = node
        .value
        .as_ref()
        .filter(|v| !v.is_empty())
        .map(|v| {
            let trimmed: String = v.chars().take(60).collect();
            let suffix = if v.chars().count() > 60 { "…" } else { "" };
            format!(" = {trimmed:?}{suffix}")
        })
        .unwrap_or_default();
    let state = state_summary(&node.state);
    println!("{indent}{ref_label}{role}{name}{value}{state}");
    for child in &node.children {
        print_node(child, depth + 1);
    }
}

fn role_label(role: &agent_ctrl_core::Role) -> String {
    match serde_json::to_value(role) {
        Ok(serde_json::Value::String(s)) => s,
        Ok(serde_json::Value::Object(m)) => m
            .get("unknown")
            .and_then(|v| v.as_str())
            .map_or_else(|| "?".to_owned(), |s| format!("?{s}")),
        _ => "?".to_owned(),
    }
}

fn state_summary(state: &agent_ctrl_core::State) -> String {
    let mut bits = Vec::new();
    if !state.enabled {
        bits.push("disabled".to_owned());
    }
    if state.focused {
        bits.push("focused".to_owned());
    }
    if let Some(true) = state.selected {
        bits.push("selected".to_owned());
    }
    if let Some(c) = state.checked {
        bits.push(format!(
            "checked={}",
            match c {
                agent_ctrl_core::Checked::True => "true",
                agent_ctrl_core::Checked::False => "false",
                agent_ctrl_core::Checked::Mixed => "mixed",
            }
        ));
    }
    if let Some(true) = state.expanded {
        bits.push("expanded".to_owned());
    }
    if let Some(true) = state.required {
        bits.push("required".to_owned());
    }
    if bits.is_empty() {
        String::new()
    } else {
        format!(" [{}]", bits.join(","))
    }
}

/// Render an internal `ref_N` id as the agent-friendly `@eN` form.
fn display_ref(internal: &str) -> String {
    internal
        .strip_prefix("ref_")
        .map_or_else(|| internal.to_owned(), |n| format!("@e{n}"))
}

// ---------- Action helpers ----------

fn run_simple_ref_action<F>(args: &RefArg, build: F) -> Result<()>
where
    F: FnOnce(RefId) -> Action,
{
    let r = parse_ref(&args.target)?;
    run_act(&args.session, build(r), args.json)
}

fn run_fill(args: FillArgs) -> Result<()> {
    run_act(
        &args.session,
        Action::Fill {
            ref_id: parse_ref(&args.target)?,
            value: args.value,
        },
        args.json,
    )
}

fn run_select(args: SelectArgs) -> Result<()> {
    run_act(
        &args.session,
        Action::Select {
            ref_id: parse_ref(&args.target)?,
            value: args.value,
        },
        args.json,
    )
}

fn run_select_all(args: OptRefArg) -> Result<()> {
    let ref_id = match args.target {
        Some(t) => Some(parse_ref(&t)?),
        None => None,
    };
    run_act(&args.session, Action::SelectAll { ref_id }, args.json)
}

fn run_clipboard(args: ClipboardArgs) -> Result<()> {
    let op = match args.op.as_str() {
        "read" => ClipboardOp::Read,
        "write" => ClipboardOp::Write {
            text: args
                .text
                .ok_or_else(|| anyhow!("clipboard write requires TEXT"))?,
        },
        "copy" => ClipboardOp::Copy,
        "paste" => ClipboardOp::Paste,
        other => bail!("unknown clipboard op {other:?}"),
    };
    if matches!(op, ClipboardOp::Read) {
        return run_clipboard_read(&args.session, args.json);
    }
    run_act(&args.session, Action::Clipboard { op }, args.json)
}

fn run_clipboard_read(session: &str, json_output: bool) -> Result<()> {
    let info = require_session(session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;
    let resp = client::send(
        &info,
        RequestOp::Act {
            session: session_id,
            action: Action::Clipboard {
                op: ClipboardOp::Read,
            },
        },
    )
    .context("sending clipboard read request")?;
    let outcome = match resp.body {
        ResponseBody::ActionDone { outcome } => outcome,
        ResponseBody::Error { message } => bail!("clipboard read failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    };
    if !outcome.ok {
        bail!(outcome
            .message
            .unwrap_or_else(|| "clipboard read failed".to_owned()));
    }
    if json_output {
        print_json(&outcome)?;
        return Ok(());
    }
    let text = outcome
        .data
        .and_then(|data| data.get("text").and_then(|v| v.as_str()).map(str::to_owned))
        .unwrap_or_default();
    println!("{text}");
    Ok(())
}

fn run_mouse(args: &MouseArgs) -> Result<()> {
    let button = parse_mouse_button(&args.button)?;
    let op = match args.op.as_str() {
        "move" => MouseOp::Move {
            x: args.x,
            y: args.y,
        },
        "down" => MouseOp::Down {
            x: args.x,
            y: args.y,
            button,
        },
        "up" => MouseOp::Up {
            x: args.x,
            y: args.y,
            button,
        },
        "wheel" => MouseOp::Wheel {
            x: args.x,
            y: args.y,
            dx: args.dx,
            dy: args.dy,
        },
        other => bail!("unknown mouse op {other:?}"),
    };
    run_act(&args.session, Action::Mouse { op }, args.json)
}

fn run_scroll(args: ScrollArgs) -> Result<()> {
    let ref_id = match args.r#ref {
        Some(t) => Some(parse_ref(&t)?),
        None => None,
    };
    run_act(
        &args.session,
        Action::Scroll {
            ref_id,
            dx: args.dx,
            dy: args.dy,
        },
        args.json,
    )
}

fn run_drag(args: &DragArgs) -> Result<()> {
    run_act(
        &args.session,
        Action::Drag {
            from: parse_ref(&args.from)?,
            to: parse_ref(&args.to)?,
        },
        args.json,
    )
}

fn run_focus_window(args: WindowIdArg) -> Result<()> {
    run_act(
        &args.session,
        Action::FocusWindow {
            window_id: args.window_id,
        },
        args.json,
    )
}

fn run_highlight(args: &HighlightArgs) -> Result<()> {
    run_act(
        &args.session,
        Action::Highlight {
            ref_id: parse_ref(&args.target)?,
            duration_ms: args.duration_ms,
        },
        args.json,
    )
}

fn parse_mouse_button(s: &str) -> Result<MouseButton> {
    match s {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        other => bail!("unknown mouse button {other:?}"),
    }
}

fn run_act(session: &str, action: Action, json_output: bool) -> Result<()> {
    let info = require_session(session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;
    let resp = client::send(
        &info,
        RequestOp::Act {
            session: session_id,
            action,
        },
    )
    .context("sending act request")?;
    match resp.body {
        ResponseBody::ActionDone { outcome } => {
            if json_output {
                if outcome.ok {
                    print_json(&outcome)?;
                    return Ok(());
                }
                bail!(outcome
                    .message
                    .unwrap_or_else(|| "action failed".to_owned()));
            }
            if outcome.ok {
                if let Some(msg) = outcome.message {
                    println!("ok {msg}");
                } else if let Some(method) = outcome
                    .data
                    .as_ref()
                    .and_then(|data| data.get("method"))
                    .and_then(serde_json::Value::as_str)
                {
                    println!("ok method={method}");
                } else {
                    println!("ok");
                }
                Ok(())
            } else {
                bail!(outcome
                    .message
                    .unwrap_or_else(|| "action failed".to_owned()))
            }
        }
        ResponseBody::Error { message } => bail!("act failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

fn run_screenshot(args: ScreenshotArgs) -> Result<()> {
    let info = require_session(&args.session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;

    let region = match args.region.as_deref() {
        Some(s) => Some(parse_region(s)?),
        None => None,
    };
    let target = match args.target.as_deref() {
        Some("window") => Some(ScreenshotTarget::Window),
        Some("desktop") => Some(ScreenshotTarget::Desktop),
        Some("region") => Some(ScreenshotTarget::Region {
            region: region.ok_or_else(|| anyhow!("--target region requires --region X,Y,W,H"))?,
        }),
        Some("ref") => Some(ScreenshotTarget::Ref {
            ref_id: parse_ref(
                args.r#ref
                    .as_deref()
                    .ok_or_else(|| anyhow!("--target ref requires --ref REF"))?,
            )?,
        }),
        Some(other) => bail!("unknown screenshot target {other:?}"),
        None => None,
    };

    let resp = client::send_with_timeout(
        &info,
        RequestOp::Act {
            session: session_id,
            action: Action::Screenshot {
                region,
                target,
                annotated: args.annotated,
            },
        },
        Duration::from_secs(60),
    )
    .context("sending screenshot request")?;

    let outcome = match resp.body {
        ResponseBody::ActionDone { outcome } => outcome,
        ResponseBody::Error { message } => bail!("screenshot failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    };
    if !outcome.ok {
        bail!(outcome
            .message
            .unwrap_or_else(|| "screenshot failed".to_owned()));
    }

    let data = outcome
        .data
        .ok_or_else(|| anyhow!("screenshot returned no data"))?;
    let b64 = data
        .get("data")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("screenshot data field missing or not a string"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("decoding screenshot base64")?;
    let byte_len = bytes.len();

    let path = args.path.unwrap_or_else(|| {
        let mut p = std::env::temp_dir();
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        p.push(format!("agent-ctrl-screenshot-{stamp}.png"));
        p
    });
    std::fs::write(&path, &bytes).with_context(|| format!("writing PNG to {}", path.display()))?;
    if args.json {
        print_json(&screenshot_public_json(&path, &data, byte_len))?;
    } else {
        println!("ok {}", path.display());
    }
    Ok(())
}

fn run_batch(args: &BatchArgs) -> Result<()> {
    let _ = args.json;
    let info = require_session(&args.session)?;
    let session_id = parse_session_id(&info.daemon_session_id)?;
    let steps_json = read_batch_steps_json(args)?;
    let steps: Vec<BatchStep> =
        serde_json::from_str(&steps_json).context("parsing batch steps JSON")?;
    let resp = client::send_with_timeout(
        &info,
        RequestOp::Batch {
            session: session_id,
            steps,
            bail: args.bail,
        },
        Duration::from_secs(60),
    )
    .context("sending batch request")?;
    match resp.body {
        ResponseBody::BatchDone { outcomes } => {
            println!("{}", serde_json::to_string_pretty(&outcomes)?);
            Ok(())
        }
        ResponseBody::Error { message } => bail!("batch failed: {message}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

fn read_batch_steps_json(args: &BatchArgs) -> Result<String> {
    let sources = usize::from(args.steps_json.is_some())
        + usize::from(args.file.is_some())
        + usize::from(args.stdin);
    if sources != 1 {
        bail!("provide exactly one batch input: STEPS_JSON, --file PATH, or --stdin");
    }
    if let Some(json) = &args.steps_json {
        return Ok(strip_leading_json_bom(json.clone()));
    }
    if let Some(path) = &args.file {
        let json = std::fs::read_to_string(path)
            .with_context(|| format!("reading batch steps from {}", path.display()))?;
        return Ok(strip_leading_json_bom(json));
    }
    let mut json = String::new();
    std::io::stdin()
        .read_to_string(&mut json)
        .context("reading batch steps from stdin")?;
    Ok(strip_leading_json_bom(json))
}

fn strip_leading_json_bom(json: String) -> String {
    if let Some(stripped) = json.strip_prefix('\u{feff}') {
        stripped.to_owned()
    } else {
        json
    }
}

#[cfg(windows)]
fn spawn_detached_child(cmd: &mut ProcessCommand) -> std::io::Result<Child> {
    let _guard = StdHandleInheritanceGuard::new();
    cmd.spawn()
}

#[cfg(not(windows))]
fn spawn_detached_child(cmd: &mut ProcessCommand) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    // process_group(0) puts the child in a fresh process group, so a
    // SIGHUP delivered to the spawning terminal's group doesn't take
    // the child down with it. This is the std-stable equivalent of
    // calling `setsid` from a `pre_exec` closure.
    cmd.process_group(0);
    cmd.spawn()
}

#[cfg(windows)]
struct StdHandleInheritanceGuard {
    saved: Vec<(isize, u32)>,
}

#[cfg(windows)]
impl StdHandleInheritanceGuard {
    #[allow(unsafe_code)] // Windows has no safe std API for clearing std-handle inheritance.
    fn new() -> Self {
        use windows_sys::Win32::Foundation::{
            GetHandleInformation, SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        };
        use windows_sys::Win32::System::Console::{
            GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
        };

        let mut saved = Vec::new();
        for std_handle in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: `GetStdHandle` reads the process std-handle table.
            let handle = unsafe { GetStdHandle(std_handle) };
            if handle == 0 || handle == INVALID_HANDLE_VALUE {
                continue;
            }

            let mut flags = 0;
            // SAFETY: `handle` came from `GetStdHandle`; invalid handles are ignored above.
            if unsafe { GetHandleInformation(handle, &raw mut flags) } == 0 {
                continue;
            }
            if flags & HANDLE_FLAG_INHERIT == 0 {
                continue;
            }

            // SAFETY: clearing only HANDLE_FLAG_INHERIT on a valid process std handle.
            if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } != 0 {
                saved.push((handle, flags & HANDLE_FLAG_INHERIT));
            }
        }
        Self { saved }
    }
}

#[cfg(windows)]
impl Drop for StdHandleInheritanceGuard {
    #[allow(unsafe_code)] // Restore the same std-handle inheritance bit saved above.
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{
            SetHandleInformation, HANDLE_FLAGS, HANDLE_FLAG_INHERIT,
        };

        for (handle, inherit_flag) in self.saved.drain(..) {
            // SAFETY: handles were captured from `GetStdHandle` and remain process-owned.
            let _ = unsafe {
                SetHandleInformation(
                    handle,
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAGS::from(inherit_flag),
                )
            };
        }
    }
}

// ---------- Helpers ----------

fn print_json<T>(value: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub(crate) fn print_error_json(error: &anyhow::Error) {
    let message = error.to_string();
    print_error_payload(
        classify_error_code(&message),
        &message,
        error_hint(&message),
        {
            let details = error
                .chain()
                .skip(1)
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if details.is_empty() {
                None
            } else {
                Some(details)
            }
        },
    );
}

pub(crate) fn print_parse_error_json(error: &clap::Error) {
    let message = error.to_string();
    print_error_payload(
        "invalid_request",
        message.trim(),
        Some("Run the same command with `--help` to inspect the expected arguments."),
        None,
    );
}

fn print_error_payload(
    code: &str,
    message: &str,
    hint: Option<&str>,
    details: Option<Vec<String>>,
) {
    let mut body = json!({
        "ok": false,
        "error": {
            "code": code,
            "message": message,
        },
    });
    if let Some(chain) = body
        .get_mut("error")
        .and_then(serde_json::Value::as_object_mut)
    {
        if let Some(details) = details {
            chain.insert("details".to_owned(), json!(details));
        }
        if let Some(hint) = hint {
            chain.insert("hint".to_owned(), json!(hint));
        }
    }
    match serde_json::to_string_pretty(&body) {
        Ok(text) => println!("{text}"),
        Err(serialize_error) => println!(
            "{{\"ok\":false,\"error\":{{\"code\":\"internal\",\"message\":\"failed to serialize error: {serialize_error}\"}}}}"
        ),
    }
}

fn classify_error_code(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("no live daemon") || lower.contains("no prior snapshot") {
        "not_ready"
    } else if lower.contains("not found")
        || lower.contains("no match")
        || lower.contains("does not refer")
    {
        "not_found"
    } else if lower.contains("timeout") || lower.contains("did not become ready") {
        "timeout"
    } else if lower.contains("unsupported") {
        "unsupported"
    } else if lower.contains("authentication failed") {
        "auth"
    } else if lower.contains("invalid")
        || lower.contains("unknown")
        || lower.contains("requires")
        || lower.contains("provide")
    {
        "invalid_request"
    } else if lower.contains("stale")
        || lower.contains("no longer valid")
        || lower.contains("foreground")
        || lower.contains("uipi")
        || lower.contains("elevation")
    {
        "surface"
    } else {
        "error"
    }
}

fn error_hint(message: &str) -> Option<&'static str> {
    let lower = message.to_ascii_lowercase();
    if lower.contains("no live daemon") {
        Some("Run `agent-ctrl open <surface>` first, or pass the intended `--session`.")
    } else if lower.contains("no prior snapshot") || lower.contains("no cached snapshot") {
        Some("Run `agent-ctrl snapshot` before using cached refs, find, inspect, or window-list.")
    } else if lower.contains("ref not found") || lower.contains("stale") {
        Some("Refs are snapshot-scoped. Re-run `agent-ctrl snapshot`, then find the element again.")
    } else if lower.contains("foreground") {
        Some("Bring the target window forward with `focus-window`, `switch-app`, or a fresh targeted snapshot.")
    } else if lower.contains("uipi") || lower.contains("elevation") {
        Some("Run agent-ctrl at the same integrity level as the target app.")
    } else if lower.contains("unsupported") {
        Some("Check `agent-ctrl info --json` for supported surfaces and capabilities.")
    } else {
        None
    }
}

fn session_public_json(info: &SessionFile) -> serde_json::Value {
    json!({
        "name": &info.name,
        "pid": info.pid,
        "endpoint": &info.endpoint,
        "version": &info.version,
        "protocol_version": info.protocol_version,
        "surface": &info.surface,
        "started_at_unix": info.started_at_unix,
        "daemon_session_id": &info.daemon_session_id,
    })
}

fn screenshot_public_json(
    path: &std::path::Path,
    data: &serde_json::Value,
    byte_len: usize,
) -> serde_json::Value {
    json!({
        "ok": true,
        "path": path.display().to_string(),
        "format": data.get("format").and_then(serde_json::Value::as_str).unwrap_or("png"),
        "encoding": "file",
        "width": data.get("width"),
        "height": data.get("height"),
        "annotated": data.get("annotated").and_then(serde_json::Value::as_bool).unwrap_or(false),
        "bytes": byte_len,
    })
}

fn parse_surface(s: &str) -> Result<SurfaceKind> {
    match s {
        "mock" => Ok(SurfaceKind::Mock),
        "uia" => Ok(SurfaceKind::Uia),
        "ax" => Ok(SurfaceKind::Ax),
        "android" => Ok(SurfaceKind::Android),
        "ios" => Ok(SurfaceKind::Ios),
        other => bail!("unknown surface {other:?} (expected: mock, uia, ax, android, ios)"),
    }
}

/// Parse the agent-friendly `@eN` form (and accept the raw `ref_N` form
/// that the wire protocol carries). The internal representation stays
/// `ref_N` so the JSON-RPC envelope is unchanged.
fn parse_ref(s: &str) -> Result<RefId> {
    let internal = if let Some(n) = s.strip_prefix("@e") {
        if !n.chars().all(|c| c.is_ascii_digit()) {
            bail!("invalid ref {s:?}; expected `@eN` or `ref_N` with N a non-negative integer");
        }
        format!("ref_{n}")
    } else if s.starts_with("ref_") {
        s.to_owned()
    } else {
        bail!("invalid ref {s:?}; expected `@eN` or `ref_N`")
    };
    Ok(RefId(internal))
}

fn parse_region(s: &str) -> Result<Region> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        bail!("region must be `X,Y,W,H`, got {s:?}");
    }
    let left: i32 = parts[0].trim().parse().context("region X")?;
    let top: i32 = parts[1].trim().parse().context("region Y")?;
    let width: u32 = parts[2].trim().parse().context("region W")?;
    let height: u32 = parts[3].trim().parse().context("region H")?;
    Ok(Region {
        x: left,
        y: top,
        w: width,
        h: height,
    })
}

fn require_session(name: &str) -> Result<SessionFile> {
    session_file::read_alive(name).ok_or_else(|| {
        anyhow!(
            "no live daemon for session {name:?}. Run `agent-ctrl open <surface> --session {name}` first."
        )
    })
}

fn parse_session_id(id: &str) -> Result<SessionId> {
    serde_json::from_str(&format!("\"{id}\""))
        .with_context(|| format!("invalid daemon_session_id {id:?} in session file"))
}

#[cfg(test)]
mod tests {
    use super::{classify_error_code, error_hint, strip_leading_json_bom};

    #[test]
    fn batch_json_allows_leading_utf8_bom() {
        let json = strip_leading_json_bom("\u{feff}[{\"op\":\"find\"}]".into());
        assert_eq!(json, "[{\"op\":\"find\"}]");
    }

    #[test]
    fn batch_json_without_bom_is_unchanged() {
        let json = strip_leading_json_bom("[{\"op\":\"find\"}]".into());
        assert_eq!(json, "[{\"op\":\"find\"}]");
    }

    #[test]
    fn error_codes_classify_common_agent_failures() {
        assert_eq!(
            classify_error_code("no live daemon for session \"default\""),
            "not_ready"
        );
        assert_eq!(
            classify_error_code("ref not found in cached snapshot: ref_99"),
            "not_found"
        );
        assert_eq!(
            classify_error_code("window_id \"0x1\" does not refer to an existing window"),
            "not_found"
        );
        assert_eq!(
            classify_error_code("--target ref requires --ref REF"),
            "invalid_request"
        );
    }

    #[test]
    fn stale_ref_errors_get_recovery_hint() {
        assert_eq!(
            error_hint("ref not found in cached snapshot: ref_99"),
            Some("Refs are snapshot-scoped. Re-run `agent-ctrl snapshot`, then find the element again.")
        );
    }
}
