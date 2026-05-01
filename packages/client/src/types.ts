// Wire types for the agent-ctrl daemon protocol.
//
// These mirror the Rust types in `crates/core` and `crates/daemon/src/dispatcher.rs`.
// They are hand-maintained for now; a future commit will generate them from the
// Rust source via `ts-rs` to eliminate drift.

// ---------- Identifiers ----------

/** Opaque session id allocated by `OpenSession`. */
export type SessionId = string;

/** Stable per-snapshot reference to a node. Only valid for the snapshot that produced it. */
export type RefId = string;

// ---------- Surface ----------

export type SurfaceKind = "cdp" | "uia" | "ax" | "android" | "ios" | "mock";

// ---------- Roles ----------

/**
 * ARIA-derived role taxonomy. Modeled loosely as a string so unknown roles
 * (sent as `{ unknown: "..." }` over the wire) and future-added roles can
 * round-trip without TS changes.
 */
export type Role = string | { unknown: string };

// ---------- Geometry ----------

export interface Bounds {
  x: number;
  y: number;
  w: number;
  h: number;
}

// ---------- Node ----------

export interface NodeState {
  visible: boolean;
  enabled: boolean;
  focused: boolean;
  selected?: boolean;
  checked?: "true" | "false" | "mixed";
  expanded?: boolean;
  required?: boolean;
}

/** Platform-specific element handle, kept opaque to client code. */
export type NativeHandle =
  | { platform: "cdp"; backend_node_id: number }
  | { platform: "uia"; runtime_id: number[]; automation_id?: string }
  | { platform: "ax"; element_ref: number }
  | { platform: "android"; window_id: number; virtual_view_id: number; resource_id?: string }
  | { platform: "ios"; element_id: string };

export interface Node {
  ref_id?: RefId;
  role: Role;
  name: string;
  description?: string;
  value?: string;
  state: NodeState;
  bounds?: Bounds;
  level?: number;
  children?: Node[];
  opaque?: boolean;
  native?: NativeHandle;
}

// ---------- Snapshot ----------

export interface SnapshotOptions {
  selector?: string;
  interactive?: boolean;
  compact?: boolean;
  depth?: number;
  /** Which window/process to capture. Defaults to the foreground window. */
  target?: WindowTarget;
}

/**
 * How to pick the window the surface should bind to. Surfaces translate this
 * into a platform handle (HWND on Windows, etc.) at snapshot time, then keep
 * the handle pinned for actions that follow.
 */
export type WindowTarget =
  | { by: "foreground" }
  | { by: "pid"; pid: number }
  | { by: "title"; title: string }
  | { by: "process-name"; name: string };

export interface AppContext {
  id: string;
  name: string;
}

export interface WindowContext {
  id: string;
  title?: string;
}

/** Rust's `SystemTime` serializes as a two-field object. Use `systemTimeToMs` to convert. */
export interface RustSystemTime {
  secs_since_epoch: number;
  nanos_since_epoch: number;
}

export interface RefEntry {
  role: Role;
  name: string;
  nth: number;
  native?: NativeHandle;
}

export interface RefMap {
  entries: Record<RefId, RefEntry>;
  next: number;
}

export interface Snapshot {
  captured_at: RustSystemTime;
  surface_kind: SurfaceKind;
  app: AppContext;
  window?: WindowContext;
  root: Node;
  refs: RefMap;
}

// ---------- Actions ----------

export type Action =
  | { kind: "click"; ref_id: RefId }
  | { kind: "double_click"; ref_id: RefId }
  | { kind: "right_click"; ref_id: RefId }
  | { kind: "hover"; ref_id: RefId }
  | { kind: "focus"; ref_id: RefId }
  | { kind: "type"; text: string }
  | { kind: "fill"; ref_id: RefId; value: string }
  | { kind: "press"; keys: string }
  | { kind: "key_down"; key: string }
  | { kind: "key_up"; key: string }
  | { kind: "scroll"; ref_id?: RefId; dx: number; dy: number }
  | { kind: "drag"; from: RefId; to: RefId }
  | { kind: "select"; ref_id: RefId; value: string }
  | { kind: "select_all"; ref_id?: RefId }
  | { kind: "scroll_into_view"; ref_id: RefId }
  | { kind: "wait"; ms: number }
  | { kind: "switch_app"; app_id: string }
  | { kind: "focus_window"; window_id: string }
  | { kind: "screenshot"; region?: { x: number; y: number; w: number; h: number } };

export interface ActionResult {
  ok: boolean;
  message?: string;
  data?: unknown;
}

// ---------- Find ----------

/**
 * Filter set for `find` / `wait-for` queries against a snapshot's tree.
 *
 * All fields are filters; an unset filter matches anything. Multiple filters
 * AND together. Matching always requires the node to carry a `RefId` -
 * non-interactive structural nodes are never returned because they cannot
 * be acted on.
 */
export interface FindQuery {
  /**
   * Match against `node.name`. Case-insensitive substring by default;
   * becomes case-sensitive equality when `exact` is set.
   */
  name?: string;
  /** When `true`, `name` must equal `node.name` exactly. */
  exact?: boolean;
  /** Restrict matches to a single role. */
  role?: Role;
  /**
   * Restrict the search to the subtree rooted at this ref. The root node
   * itself is included in the search.
   */
  in_ref?: RefId;
  /** Cap on the number of matches returned. Omit for unlimited. */
  limit?: number;
}

/** One row of `AgentCtrl.find` output. */
export interface FindMatch {
  /** Ref the agent uses to target this node. */
  ref_id: RefId;
  /** Role at the time the snapshot was taken. */
  role: Role;
  /** Name at the time the snapshot was taken. */
  name: string;
}

// ---------- Wait-for ----------

/**
 * Predicate evaluated against a fresh snapshot on every poll iteration.
 *
 * - `appears`: at least one node matching `query` is present. Has a real
 *   race window - a node can appear before its children/state are fully
 *   populated. For racy follow-up actions, chain a `stable` wait afterward.
 * - `gone`: no node matches `query`. More reliable than `appears`.
 * - `stable`: the tree's structural signature has been unchanged for
 *   `idle_ms` consecutive milliseconds. The honest "let the UI settle"
 *   primitive.
 */
export type WaitPredicate =
  | { kind: "appears"; query: FindQuery }
  | { kind: "gone"; query: FindQuery }
  | { kind: "stable"; idle_ms: number };

/** Options for one `wait-for` invocation. */
export interface WaitOptions {
  predicate: WaitPredicate;
  /** Maximum total wait, in milliseconds. Daemon-side cap is one hour. */
  timeout_ms: number;
  /** Poll interval in milliseconds. Floored at 50 by the daemon. */
  poll_ms: number;
}

/** Outcome of a `wait-for` invocation. */
export type WaitOutcome =
  | { outcome: "matched"; found?: FindMatch; elapsed_ms: number }
  | { outcome: "gone"; elapsed_ms: number }
  | { outcome: "stable"; elapsed_ms: number }
  | { outcome: "timeout"; elapsed_ms: number };

// ---------- Window list ----------

/** One row of `AgentCtrl.listWindows` output. */
export interface WindowInfo {
  /** Stable per-platform window id. On Windows this is the HWND in lowercase hex (e.g. `"0x1717ca"`). */
  id: string;
  /** Window title text. May be missing for unnamed system windows. */
  title?: string;
  /** Owning process executable name (file stem, no extension on Windows). */
  process: string;
  /** Owning process id. */
  pid: number;
  /** Whether this window currently has user focus on the host. */
  focused: boolean;
  /** Whether this window is the session's currently pinned target. */
  pinned: boolean;
}

// ---------- Wire envelope ----------

export type RequestOp =
  | { op: "open_session"; surface: SurfaceKind }
  | { op: "snapshot"; session: SessionId; opts?: SnapshotOptions }
  | { op: "act"; session: SessionId; action: Action }
  | { op: "find"; session: SessionId; query: FindQuery }
  | { op: "wait"; session: SessionId; opts: WaitOptions }
  | { op: "list_windows"; session: SessionId }
  | { op: "close_session"; session: SessionId };

export type Request = { id: string } & RequestOp;

export type Response = { id: string } & (
  | { result: "session_opened"; session: SessionId }
  | { result: "snapshot"; snapshot: Snapshot }
  | { result: "action_done"; outcome: ActionResult }
  | { result: "find_results"; matches: FindMatch[] }
  | { result: "wait_done"; outcome: WaitOutcome }
  | { result: "windows"; windows: WindowInfo[] }
  | { result: "closed" }
  | { result: "error"; message: string }
);

// ---------- Helpers ----------

/** Convert a Rust `SystemTime` value to milliseconds since the Unix epoch. */
export function systemTimeToMs(t: RustSystemTime): number {
  return t.secs_since_epoch * 1000 + Math.floor(t.nanos_since_epoch / 1_000_000);
}
