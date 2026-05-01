// Public entrypoint for `@agent-ctrl/client`.

export { AgentCtrl, type AgentCtrlOptions } from "./client.js";
export { systemTimeToMs } from "./types.js";
export type {
  Action,
  ActionResult,
  AppContext,
  Bounds,
  FindMatch,
  FindQuery,
  NativeHandle,
  Node,
  NodeState,
  RefEntry,
  RefId,
  RefMap,
  Request,
  RequestOp,
  Response,
  Role,
  RustSystemTime,
  SessionId,
  Snapshot,
  SnapshotOptions,
  SurfaceKind,
  WaitOptions,
  WaitOutcome,
  WaitPredicate,
  WindowContext,
  WindowInfo,
  WindowTarget,
} from "./types.js";
