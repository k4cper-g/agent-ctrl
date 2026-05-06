//! Transport-agnostic request / response dispatch.

use std::time::{Duration, Instant};

use agent_ctrl_core::{
    tree_signature, Action, ActionResult, Checked, FindMatch, FindQuery, GetField, GetResult,
    IsResult, Node, RefId, Snapshot, SnapshotOptions, StateField, SurfaceKind, WaitOptions,
    WaitOutcome, WaitPredicate, WindowInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::factory;
use crate::state::{DaemonState, SessionId};

/// Floor on `wait-for` poll cadence. Anything faster burns CPU on UIA tree
/// walks without buying reliability - a heavy app's snapshot already takes
/// 100-300ms.
const MIN_POLL_MS: u64 = 50;

/// Cap on `wait-for` total timeout. Prevents a buggy or malicious client
/// from pinning the daemon in a polling loop for years. One hour is well
/// past anything an interactive agent should ever need.
const MAX_TIMEOUT_MS: u64 = 60 * 60 * 1000;

/// Current JSON wire protocol version.
pub const PROTOCOL_VERSION: u32 = 2;

/// A request from a client to the daemon.
///
/// JSON shape:
/// ```json
/// {"id": "abc", "op": "snapshot", "session": "...", "opts": {...}}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    /// Caller-supplied correlation id; echoed back in the matching [`Response`].
    pub id: String,
    /// Bearer token required for TCP transport. Stdio transport ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<String>,
    /// The actual operation to perform.
    #[serde(flatten)]
    pub op: RequestOp,
}

/// The operation portion of a [`Request`], internally tagged by `"op"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum RequestOp {
    /// Open a new surface session.
    OpenSession {
        /// Surface implementation to instantiate.
        surface: SurfaceKind,
    },
    /// Capture a snapshot from an open session.
    Snapshot {
        /// Session to snapshot.
        session: SessionId,
        /// Capture options.
        #[serde(default)]
        opts: SnapshotOptions,
    },
    /// Execute an action against an open session.
    Act {
        /// Session to act on.
        session: SessionId,
        /// Action to perform.
        action: Action,
    },
    /// Search the most recent snapshot's tree for refs matching `query`.
    ///
    /// Pure read against the daemon's cached snapshot - does not re-walk the
    /// OS accessibility tree. Errors when no snapshot has been captured yet
    /// on this session.
    Find {
        /// Session to query.
        session: SessionId,
        /// Filters describing what to match.
        query: FindQuery,
    },
    /// Read one field from the most recent cached snapshot.
    Get {
        /// Session to query.
        session: SessionId,
        /// Ref to read. Required except for `field = "window"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        ref_id: Option<RefId>,
        /// Field to return.
        field: GetField,
    },
    /// Check one boolean state on a ref in the most recent cached snapshot.
    Is {
        /// Session to query.
        session: SessionId,
        /// Ref to check.
        ref_id: RefId,
        /// State to return.
        field: StateField,
    },
    /// Block until `opts.predicate` is satisfied or `opts.timeout_ms` fires.
    ///
    /// Polls the surface at `opts.poll_ms` (floored at 50). Each successful
    /// poll's snapshot is cached as `last_snapshot` so a follow-up `Act` or
    /// `Find` sees fresh refs without an extra round-trip.
    Wait {
        /// Session to wait on.
        session: SessionId,
        /// Predicate, timeout, and poll cadence.
        opts: WaitOptions,
    },
    /// Enumerate top-level windows the session can target.
    ///
    /// Mirrors agent-browser's `tab_list`: the agent uses the result to
    /// decide which window to switch to via `FocusWindow`. Useful when a
    /// dialog or popup spawned outside the currently pinned window.
    ListWindows {
        /// Session to enumerate windows for.
        session: SessionId,
    },
    /// Close an open session.
    CloseSession {
        /// Session to close.
        session: SessionId,
    },
    /// Shut the daemon down. Used by the `agent-ctrl close` CLI command to
    /// terminate a long-running TCP daemon. The daemon answers `Stopped`
    /// and then drops its accept loop; the transport layer is responsible
    /// for actually triggering the shutdown after seeing this op.
    Shutdown,
    /// Execute multiple operations in order.
    Batch {
        /// Session used for every step.
        session: SessionId,
        /// Ordered steps.
        steps: Vec<BatchStep>,
        /// Stop after the first failed step.
        #[serde(default)]
        bail: bool,
    },
}

/// One operation inside a batch request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BatchStep {
    /// Execute an action.
    Act {
        /// Action to execute.
        action: Action,
    },
    /// Find refs in the cached snapshot.
    Find {
        /// Query to run.
        query: FindQuery,
    },
    /// Read a field from the cached snapshot.
    Get {
        /// Ref to read. Required except for `field = "window"`.
        #[serde(skip_serializing_if = "Option::is_none")]
        ref_id: Option<RefId>,
        /// Field to return.
        field: GetField,
    },
    /// Check a boolean state.
    Is {
        /// Ref to check.
        ref_id: RefId,
        /// State to return.
        field: StateField,
    },
    /// Wait for a predicate.
    Wait {
        /// Wait options.
        opts: WaitOptions,
    },
    /// List windows.
    ListWindows,
}

/// One result inside a batch response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchStepOutcome {
    /// Zero-based step index.
    pub index: usize,
    /// Whether the step succeeded.
    pub ok: bool,
    /// Result payload when successful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error message when failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// A response from the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// Echoes the [`Request::id`] that produced this response.
    /// Empty when the request couldn't be parsed at all.
    pub id: String,
    /// The actual result.
    #[serde(flatten)]
    pub body: ResponseBody,
}

/// The result portion of a [`Response`], internally tagged by `"result"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ResponseBody {
    /// `OpenSession` succeeded.
    SessionOpened {
        /// Identifier of the new session.
        session: SessionId,
        /// Wire protocol version.
        protocol_version: u32,
        /// Surface kind.
        surface: SurfaceKind,
        /// Advertised capability names.
        capabilities: Vec<String>,
    },
    /// `Snapshot` succeeded.
    Snapshot {
        /// Captured snapshot.
        snapshot: Box<Snapshot>,
    },
    /// `Act` succeeded.
    ActionDone {
        /// Action result.
        outcome: ActionResult,
    },
    /// `Find` succeeded. May be empty if no node matched.
    FindResults {
        /// Matched refs in tree order.
        matches: Vec<FindMatch>,
    },
    /// `Get` succeeded.
    GetDone {
        /// Field result.
        output: GetResult,
    },
    /// `Is` succeeded.
    IsDone {
        /// State result.
        output: IsResult,
    },
    /// `Wait` finished, either by satisfying its predicate or by timing out.
    WaitDone {
        /// What happened. `Timeout` is *not* a wire-level error - it's a
        /// distinguishable normal outcome the CLI maps to exit code 2.
        outcome: WaitOutcome,
    },
    /// `ListWindows` succeeded.
    Windows {
        /// Top-level windows the session can target. The pinned window is
        /// flagged via `WindowInfo::pinned`.
        windows: Vec<WindowInfo>,
    },
    /// `CloseSession` succeeded.
    Closed,
    /// `Shutdown` succeeded - the daemon will exit shortly after this
    /// response is delivered.
    Stopped,
    /// `Batch` finished.
    BatchDone {
        /// Ordered per-step outcomes.
        outcomes: Vec<BatchStepOutcome>,
    },
    /// Any error path.
    Error {
        /// Human-readable error message.
        message: String,
    },
}

impl Response {
    /// Build an error response with the given id.
    #[must_use]
    pub fn error(id: &str, message: impl Into<String>) -> Self {
        Self {
            id: id.to_owned(),
            body: ResponseBody::Error {
                message: message.into(),
            },
        }
    }
}

/// Dispatch one request against the daemon state.
// Central wire dispatcher intentionally keeps protocol branches in one match.
#[allow(clippy::too_many_lines)]
pub async fn dispatch(state: &DaemonState, request: Request) -> Response {
    let id = request.id;
    let body = match request.op {
        RequestOp::OpenSession { surface } => match factory::open_surface(surface).await {
            Ok(s) => {
                let surface = s.kind();
                let capabilities = s
                    .capabilities()
                    .iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                ResponseBody::SessionOpened {
                    session: state.open(s).await,
                    protocol_version: PROTOCOL_VERSION,
                    surface,
                    capabilities,
                }
            }
            Err(e) => ResponseBody::Error {
                message: e.to_string(),
            },
        },
        RequestOp::Snapshot { session, opts } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let mut guard = cell.lock().await;
            let Some(surface) = guard.surface.as_ref() else {
                return Response::error(&id, format!("session {session} is closed"));
            };
            match surface.snapshot(&opts).await {
                Ok(s) => {
                    // Cache both the snapshot and the options that produced
                    // it, so the wait-for polling loop can keep targeting
                    // the same window instead of re-resolving Foreground.
                    guard.last_snapshot = Some(s.clone());
                    guard.last_snapshot_options = Some(opts);
                    ResponseBody::Snapshot {
                        snapshot: Box::new(s),
                    }
                }
                Err(e) => ResponseBody::Error {
                    message: e.to_string(),
                },
            }
        }
        RequestOp::Act { session, action } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let guard = cell.lock().await;
            let Some(surface) = guard.surface.as_ref() else {
                return Response::error(&id, format!("session {session} is closed"));
            };
            match surface.act(&action).await {
                Ok(o) => ResponseBody::ActionDone { outcome: o },
                Err(e) => ResponseBody::Error {
                    message: e.to_string(),
                },
            }
        }
        RequestOp::Find { session, query } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let guard = cell.lock().await;
            let Some(snap) = guard.last_snapshot.as_ref() else {
                // Don't leak the internal session UUID here - the caller
                // already knows which session they targeted, and a UUID
                // distracts from the actionable hint.
                return Response::error(
                    &id,
                    "no snapshot cached for this session - run `agent-ctrl snapshot` first",
                );
            };
            ResponseBody::FindResults {
                matches: snap.find(&query),
            }
        }
        RequestOp::Get {
            session,
            ref_id,
            field,
        } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            match read_get(&cell, field, ref_id).await {
                Ok(output) => ResponseBody::GetDone { output },
                Err(message) => ResponseBody::Error { message },
            }
        }
        RequestOp::Is {
            session,
            ref_id,
            field,
        } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            match read_is(&cell, field, ref_id).await {
                Ok(output) => ResponseBody::IsDone { output },
                Err(message) => ResponseBody::Error { message },
            }
        }
        RequestOp::Wait { session, opts } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let outcome = run_wait(&cell, &opts).await;
            match outcome {
                Ok(o) => ResponseBody::WaitDone { outcome: o },
                Err(e) => ResponseBody::Error { message: e },
            }
        }
        RequestOp::ListWindows { session } => {
            let Some(cell) = state.get(session).await else {
                return Response::error(&id, format!("unknown session: {session}"));
            };
            let guard = cell.lock().await;
            let Some(surface) = guard.surface.as_ref() else {
                return Response::error(&id, format!("session {session} is closed"));
            };
            match surface.list_windows().await {
                Ok(windows) => ResponseBody::Windows { windows },
                Err(e) => ResponseBody::Error {
                    message: e.to_string(),
                },
            }
        }
        RequestOp::CloseSession { session } => match state.close(session).await {
            Ok(()) => ResponseBody::Closed,
            Err(e) => ResponseBody::Error {
                message: e.to_string(),
            },
        },
        RequestOp::Shutdown => ResponseBody::Stopped,
        RequestOp::Batch {
            session,
            steps,
            bail,
        } => ResponseBody::BatchDone {
            outcomes: run_batch(state, session, steps, bail).await,
        },
    };
    Response { id, body }
}

async fn cached_snapshot(
    cell: &crate::state::SurfaceCell,
) -> Result<agent_ctrl_core::Snapshot, String> {
    let guard = cell.lock().await;
    guard.last_snapshot.clone().ok_or_else(|| {
        "no snapshot cached for this session - run `agent-ctrl snapshot` first".to_string()
    })
}

async fn read_get(
    cell: &crate::state::SurfaceCell,
    field: GetField,
    ref_id: Option<RefId>,
) -> Result<GetResult, String> {
    let snap = cached_snapshot(cell).await?;
    if matches!(field, GetField::Window) {
        return Ok(GetResult {
            field,
            ref_id: None,
            value: json!(snap.window),
        });
    }
    let Some(ref_id) = ref_id else {
        return Err(format!("get {field:?} requires a ref"));
    };
    let node = snap
        .node_by_ref(&ref_id)
        .ok_or_else(|| format!("ref not found in cached snapshot: {ref_id}"))?;
    Ok(GetResult {
        field,
        ref_id: Some(ref_id),
        value: get_value(node, field),
    })
}

fn get_value(node: &Node, field: GetField) -> serde_json::Value {
    match field {
        GetField::Text => json!(node.value.as_ref().unwrap_or(&node.name)),
        GetField::Value => json!(node.value),
        GetField::Name => json!(node.name),
        GetField::Role => json!(node.role),
        GetField::State => json!(node.state),
        GetField::Bounds => json!(node.bounds),
        GetField::Window => serde_json::Value::Null,
    }
}

async fn read_is(
    cell: &crate::state::SurfaceCell,
    field: StateField,
    ref_id: RefId,
) -> Result<IsResult, String> {
    let snap = cached_snapshot(cell).await?;
    let node = snap
        .node_by_ref(&ref_id)
        .ok_or_else(|| format!("ref not found in cached snapshot: {ref_id}"))?;
    Ok(IsResult {
        field,
        ref_id,
        value: state_value(node, field),
    })
}

fn state_value(node: &Node, field: StateField) -> bool {
    match field {
        StateField::Visible => node.state.visible,
        StateField::Enabled => node.state.enabled,
        StateField::Focused => node.state.focused,
        StateField::Selected => node.state.selected.unwrap_or(false),
        StateField::Checked => node.state.checked == Some(Checked::True),
        StateField::Expanded => node.state.expanded.unwrap_or(false),
    }
}

async fn run_batch(
    state: &DaemonState,
    session: SessionId,
    steps: Vec<BatchStep>,
    bail: bool,
) -> Vec<BatchStepOutcome> {
    let mut outcomes = Vec::with_capacity(steps.len());
    for (index, step) in steps.into_iter().enumerate() {
        let outcome = run_batch_step(state, session, step, index).await;
        let failed = !outcome.ok;
        outcomes.push(outcome);
        if bail && failed {
            break;
        }
    }
    outcomes
}

async fn run_batch_step(
    state: &DaemonState,
    session: SessionId,
    step: BatchStep,
    index: usize,
) -> BatchStepOutcome {
    let result = match step {
        BatchStep::Act { action } => batch_act(state, session, action).await,
        BatchStep::Find { query } => batch_find(state, session, query).await,
        BatchStep::Get { ref_id, field } => batch_get(state, session, field, ref_id).await,
        BatchStep::Is { ref_id, field } => batch_is(state, session, field, ref_id).await,
        BatchStep::Wait { opts } => batch_wait(state, session, opts).await,
        BatchStep::ListWindows => batch_list_windows(state, session).await,
    };
    match result {
        Ok(value) => BatchStepOutcome {
            index,
            ok: true,
            result: Some(value),
            error: None,
        },
        Err(message) => BatchStepOutcome {
            index,
            ok: false,
            result: None,
            error: Some(message),
        },
    }
}

async fn batch_act(
    state: &DaemonState,
    session: SessionId,
    action: Action,
) -> Result<serde_json::Value, String> {
    let Some(cell) = state.get(session).await else {
        return Err(format!("unknown session: {session}"));
    };
    let guard = cell.lock().await;
    let Some(surface) = guard.surface.as_ref() else {
        return Err(format!("session {session} is closed"));
    };
    surface
        .act(&action)
        .await
        .map(|outcome| json!({ "result": "action_done", "outcome": outcome }))
        .map_err(|e| e.to_string())
}

async fn batch_find(
    state: &DaemonState,
    session: SessionId,
    query: FindQuery,
) -> Result<serde_json::Value, String> {
    let Some(cell) = state.get(session).await else {
        return Err(format!("unknown session: {session}"));
    };
    let snap = cached_snapshot(&cell).await?;
    Ok(json!({ "result": "find_results", "matches": snap.find(&query) }))
}

async fn batch_get(
    state: &DaemonState,
    session: SessionId,
    field: GetField,
    ref_id: Option<RefId>,
) -> Result<serde_json::Value, String> {
    let Some(cell) = state.get(session).await else {
        return Err(format!("unknown session: {session}"));
    };
    read_get(&cell, field, ref_id)
        .await
        .map(|output| json!({ "result": "get_done", "output": output }))
}

async fn batch_is(
    state: &DaemonState,
    session: SessionId,
    field: StateField,
    ref_id: RefId,
) -> Result<serde_json::Value, String> {
    let Some(cell) = state.get(session).await else {
        return Err(format!("unknown session: {session}"));
    };
    read_is(&cell, field, ref_id)
        .await
        .map(|output| json!({ "result": "is_done", "output": output }))
}

async fn batch_wait(
    state: &DaemonState,
    session: SessionId,
    opts: WaitOptions,
) -> Result<serde_json::Value, String> {
    let Some(cell) = state.get(session).await else {
        return Err(format!("unknown session: {session}"));
    };
    run_wait(&cell, &opts)
        .await
        .map(|outcome| json!({ "result": "wait_done", "outcome": outcome }))
}

async fn batch_list_windows(
    state: &DaemonState,
    session: SessionId,
) -> Result<serde_json::Value, String> {
    let Some(cell) = state.get(session).await else {
        return Err(format!("unknown session: {session}"));
    };
    let guard = cell.lock().await;
    let Some(surface) = guard.surface.as_ref() else {
        return Err(format!("session {session} is closed"));
    };
    surface
        .list_windows()
        .await
        .map(|windows| json!({ "result": "windows", "windows": windows }))
        .map_err(|e| e.to_string())
}

/// Run a wait-for polling loop against `cell`.
///
/// Each iteration acquires the session lock briefly to take a snapshot and
/// cache it, then releases the lock before sleeping - so other CLI
/// invocations (a `find`, a `close`, an `act`) on the same session can
/// interleave between polls. The minimum poll interval is clamped at
/// [`MIN_POLL_MS`] to prevent CPU thrash on heavy a11y trees.
///
/// Each poll uses the same `SnapshotOptions` that produced the session's
/// most recent snapshot - without that, `target: Foreground` would
/// re-resolve to whatever window has focus right now (the terminal,
/// typically) and the wait would target the wrong app entirely. A prior
/// `Snapshot` is therefore required; if none exists this returns an error.
///
/// Returns `Err` only for fatal session-state problems (closed surface,
/// snapshot error, no prior snapshot). Timeout is a normal
/// `Ok(WaitOutcome::Timeout)` so the CLI can map it to its own exit code.
// The wait loop is kept together so timeout, polling, and cache updates stay coupled.
#[allow(clippy::too_many_lines)]
async fn run_wait(
    cell: &crate::state::SurfaceCell,
    opts: &WaitOptions,
) -> Result<WaitOutcome, String> {
    // Both ends are clamped: poll has a floor (CPU thrash protection) and
    // timeout has a ceiling (don't let a runaway request occupy the worker
    // for hours). Hitting MAX_TIMEOUT_MS still produces a normal `Timeout`
    // outcome, not an error - the agent sees "timeout after 3600000ms".
    let timeout = Duration::from_millis(opts.timeout_ms.min(MAX_TIMEOUT_MS));
    let poll = Duration::from_millis(opts.poll_ms.max(MIN_POLL_MS));
    let started = Instant::now();

    if matches!(
        opts.predicate,
        WaitPredicate::WindowAppears { .. } | WaitPredicate::WindowGone { .. }
    ) {
        return run_window_wait(cell, opts, timeout, poll, started).await;
    }

    // Snapshot options to reuse on every poll. Captured up front so we
    // hold the lock briefly.
    let snap_opts: SnapshotOptions = {
        let guard = cell.lock().await;
        guard.last_snapshot_options.clone().ok_or_else(|| {
            "no snapshot cached for this session - run `agent-ctrl snapshot` first so wait-for knows which window to target".to_string()
        })?
    };

    // Stable-only state: signature seen on the previous successful poll, and
    // when the current run of equal signatures began. Reset to None whenever
    // the signature changes.
    let mut last_signature: Option<u64> = None;
    let mut stable_since: Option<Instant> = None;

    loop {
        // Take a snapshot inside a short lock window. The lock is released
        // before we evaluate or sleep so other ops can interleave.
        let snap = {
            let mut guard = cell.lock().await;
            let Some(surface) = guard.surface.as_ref() else {
                return Err("session is closed".into());
            };
            let snap = surface
                .snapshot(&snap_opts)
                .await
                .map_err(|e| format!("wait: snapshot failed: {e}"))?;
            // Cache so a follow-up action sees fresh refs without an extra
            // OS round-trip.
            guard.last_snapshot = Some(snap.clone());
            snap
        };

        // Evaluate the predicate against the snapshot we just took.
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &opts.predicate {
            WaitPredicate::Appears { query } => {
                if let Some(found) = snap.find(query).into_iter().next() {
                    return Ok(WaitOutcome::Matched {
                        found: Some(found),
                        elapsed_ms,
                    });
                }
            }
            WaitPredicate::Gone { query } => {
                if snap.find(query).is_empty() {
                    return Ok(WaitOutcome::Gone { elapsed_ms });
                }
            }
            WaitPredicate::Stable { idle_ms } => {
                let sig = tree_signature(&snap);
                if last_signature == Some(sig) {
                    let since = stable_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_millis(*idle_ms) {
                        return Ok(WaitOutcome::Stable { elapsed_ms });
                    }
                } else {
                    last_signature = Some(sig);
                    stable_since = None;
                }
            }
            WaitPredicate::State {
                query,
                field,
                value,
            } => {
                if let Some(found) =
                    first_match_where(&snap, query, |node| state_value(node, *field) == *value)
                {
                    return Ok(WaitOutcome::Matched {
                        found: Some(found),
                        elapsed_ms,
                    });
                }
            }
            WaitPredicate::TextContains { query, text } => {
                let needle = text.to_lowercase();
                if let Some(found) = first_match_where(&snap, query, |node| {
                    node.value
                        .as_deref()
                        .unwrap_or(&node.name)
                        .to_lowercase()
                        .contains(&needle)
                }) {
                    return Ok(WaitOutcome::Matched {
                        found: Some(found),
                        elapsed_ms,
                    });
                }
            }
            WaitPredicate::ValueContains { query, value } => {
                let needle = value.to_lowercase();
                if let Some(found) = first_match_where(&snap, query, |node| {
                    node.value
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .contains(&needle)
                }) {
                    return Ok(WaitOutcome::Matched {
                        found: Some(found),
                        elapsed_ms,
                    });
                }
            }
            WaitPredicate::WindowAppears { title } => {
                if window_title_present(cell, title).await? {
                    return Ok(WaitOutcome::Matched {
                        found: None,
                        elapsed_ms,
                    });
                }
            }
            WaitPredicate::WindowGone { title } => {
                if !window_title_present(cell, title).await? {
                    return Ok(WaitOutcome::Gone { elapsed_ms });
                }
            }
        }

        // Check the timeout *before* sleeping - overruns of one poll
        // duration are tolerable, but two polls past the deadline isn't.
        if started.elapsed() >= timeout {
            return Ok(WaitOutcome::Timeout { elapsed_ms });
        }

        tokio::time::sleep(poll).await;
    }
}

async fn run_window_wait(
    cell: &crate::state::SurfaceCell,
    opts: &WaitOptions,
    timeout: Duration,
    poll: Duration,
    started: Instant,
) -> Result<WaitOutcome, String> {
    loop {
        let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match &opts.predicate {
            WaitPredicate::WindowAppears { title } => {
                if window_title_present(cell, title).await? {
                    return Ok(WaitOutcome::Matched {
                        found: None,
                        elapsed_ms,
                    });
                }
            }
            WaitPredicate::WindowGone { title } => {
                if !window_title_present(cell, title).await? {
                    return Ok(WaitOutcome::Gone { elapsed_ms });
                }
            }
            _ => unreachable!("run_window_wait only handles window predicates"),
        }

        if started.elapsed() >= timeout {
            return Ok(WaitOutcome::Timeout { elapsed_ms });
        }
        tokio::time::sleep(poll).await;
    }
}

fn first_match_where<F>(snap: &Snapshot, query: &FindQuery, pred: F) -> Option<FindMatch>
where
    F: Fn(&Node) -> bool,
{
    let mut query = query.clone();
    query.limit = None;
    snap.find(&query)
        .into_iter()
        .find(|m| snap.node_by_ref(&m.ref_id).is_some_and(&pred))
}

async fn window_title_present(
    cell: &crate::state::SurfaceCell,
    title: &str,
) -> Result<bool, String> {
    let needle = title.to_lowercase();
    let guard = cell.lock().await;
    let Some(surface) = guard.surface.as_ref() else {
        return Err("session is closed".into());
    };
    let windows = surface
        .list_windows()
        .await
        .map_err(|e| format!("wait: list_windows failed: {e}"))?;
    Ok(windows
        .iter()
        .filter_map(|w| w.title.as_deref())
        .any(|candidate| candidate.to_lowercase().contains(&needle)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use agent_ctrl_core::{AppContext, RefMap, Role, State};

    #[test]
    fn request_roundtrips_with_correlation_id() {
        let req = Request {
            id: "abc-123".into(),
            auth: None,
            op: RequestOp::OpenSession {
                surface: SurfaceKind::Mock,
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""id":"abc-123""#));
        assert!(json.contains(r#""op":"open_session""#));
        assert!(json.contains(r#""surface":"mock""#));

        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "abc-123");
        assert!(matches!(
            back.op,
            RequestOp::OpenSession {
                surface: SurfaceKind::Mock
            }
        ));
    }

    #[test]
    fn inspect_and_batch_roundtrip_json() {
        let req = Request {
            id: "batch".into(),
            auth: Some("token".into()),
            op: RequestOp::Batch {
                session: serde_json::from_str("\"00000000-0000-0000-0000-000000000000\"").unwrap(),
                bail: true,
                steps: vec![
                    BatchStep::Get {
                        ref_id: Some(RefId("ref_0".into())),
                        field: GetField::Name,
                    },
                    BatchStep::Is {
                        ref_id: RefId("ref_0".into()),
                        field: StateField::Enabled,
                    },
                ],
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""auth":"token""#));
        assert!(json.contains(r#""op":"batch""#));
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(back.op, RequestOp::Batch { bail: true, .. }));
    }

    #[test]
    fn first_match_where_ignores_query_limit_and_returns_satisfying_node() {
        let mut refs = RefMap::new();
        let first_ref = refs.insert(Role::Button, "Save".into(), 0, None);
        let second_ref = refs.insert(Role::Button, "Save".into(), 1, None);
        let snap = Snapshot {
            captured_at: std::time::SystemTime::UNIX_EPOCH,
            surface_kind: SurfaceKind::Mock,
            app: AppContext {
                id: "fixture".into(),
                name: "Fixture".into(),
            },
            window: None,
            root: Node {
                ref_id: None,
                role: Role::Window,
                name: "Window".into(),
                description: None,
                value: None,
                state: State::default(),
                bounds: None,
                level: None,
                children: vec![
                    Node {
                        ref_id: Some(first_ref),
                        role: Role::Button,
                        name: "Save".into(),
                        description: None,
                        value: None,
                        state: State {
                            enabled: false,
                            ..State::default()
                        },
                        bounds: None,
                        level: None,
                        children: Vec::new(),
                        opaque: false,
                        native: None,
                    },
                    Node {
                        ref_id: Some(second_ref.clone()),
                        role: Role::Button,
                        name: "Save".into(),
                        description: None,
                        value: None,
                        state: State {
                            enabled: true,
                            ..State::default()
                        },
                        bounds: None,
                        level: None,
                        children: Vec::new(),
                        opaque: false,
                        native: None,
                    },
                ],
                opaque: false,
                native: None,
            },
            refs,
        };
        let query = FindQuery {
            name: Some("Save".into()),
            limit: Some(1),
            ..FindQuery::default()
        };
        let found = first_match_where(&snap, &query, |node| node.state.enabled).unwrap();
        assert_eq!(found.ref_id, second_ref);
    }

    #[tokio::test]
    async fn dispatch_open_then_snapshot_then_close() {
        let state = DaemonState::new();
        let opened = dispatch(
            &state,
            Request {
                id: "1".into(),
                auth: None,
                op: RequestOp::OpenSession {
                    surface: SurfaceKind::Mock,
                },
            },
        )
        .await;
        assert_eq!(opened.id, "1");
        let session = match opened.body {
            ResponseBody::SessionOpened { session, .. } => session,
            other => panic!("expected SessionOpened, got {other:?}"),
        };

        let snap = dispatch(
            &state,
            Request {
                id: "2".into(),
                auth: None,
                op: RequestOp::Snapshot {
                    session,
                    opts: SnapshotOptions::default(),
                },
            },
        )
        .await;
        assert!(matches!(snap.body, ResponseBody::Snapshot { .. }));

        let closed = dispatch(
            &state,
            Request {
                id: "3".into(),
                auth: None,
                op: RequestOp::CloseSession { session },
            },
        )
        .await;
        assert!(matches!(closed.body, ResponseBody::Closed));
    }
}
