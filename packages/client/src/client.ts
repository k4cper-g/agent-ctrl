// AgentCtrl: TypeScript client for the agent-ctrl daemon.
//
// Spawns the daemon as a subprocess and talks newline-delimited JSON-RPC
// over stdio. Requests carry a correlation id; concurrent calls are matched
// to their responses via a pending-id table.

import { spawn, type ChildProcess } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createInterface, type Interface as ReadlineInterface } from "node:readline";

import { PROTOCOL_VERSION } from "./types.js";
import type {
  Action,
  ActionResult,
  BatchStep,
  BatchStepOutcome,
  FindMatch,
  FindQuery,
  GetField,
  GetResult,
  IsResult,
  Request,
  RequestOp,
  Response,
  RefId,
  SessionId,
  Snapshot,
  SnapshotOptions,
  StateField,
  SurfaceKind,
  WaitOptions,
  WaitOutcome,
  WindowInfo,
} from "./types.js";

/** Configuration for an [`AgentCtrl`] instance. */
export interface AgentCtrlOptions {
  /**
   * Full spawn command. Defaults to `["agent-ctrl", "daemon"]`. Override for
   * tests, custom binaries, or to wrap with `cargo run` during development.
   */
  command?: string[];
  /** What to do with the daemon's stderr. Defaults to `"inherit"`. */
  stderr?: "inherit" | "ignore";
  /** Working directory for the daemon process. */
  cwd?: string;
  /** Default deadline for one request. Defaults to 30 seconds. */
  requestTimeoutMs?: number;
}

interface PendingRequest {
  resolve: (response: Response) => void;
  reject: (err: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/** Metadata negotiated when a surface session opens. */
export interface OpenedSession {
  session: SessionId;
  protocolVersion: number;
  surface: SurfaceKind;
  capabilities: readonly string[];
}

/**
 * Client for one running agent-ctrl daemon.
 *
 * Construction spawns the daemon eagerly; call `close()` to terminate it.
 * Every public method is safe to call concurrently - requests are correlated
 * by id under the hood.
 */
export class AgentCtrl {
  private readonly proc: ChildProcess;
  private readonly reader: ReadlineInterface;
  private readonly requestTimeoutMs: number;
  private readonly pending = new Map<string, PendingRequest>();
  private closed = false;
  private exitError: Error | null = null;

  constructor(options: AgentCtrlOptions = {}) {
    const command = options.command ?? ["agent-ctrl", "daemon"];
    if (command.length === 0) {
      throw new Error("AgentCtrl: `command` cannot be empty");
    }
    const [binary, ...args] = command as [string, ...string[]];
    this.requestTimeoutMs = options.requestTimeoutMs ?? 30_000;
    if (!Number.isFinite(this.requestTimeoutMs) || this.requestTimeoutMs <= 0) {
      throw new Error("AgentCtrl: `requestTimeoutMs` must be a positive number");
    }

    this.proc = spawn(binary, args, {
      stdio: ["pipe", "pipe", options.stderr ?? "inherit"],
      cwd: options.cwd,
    });

    if (!this.proc.stdout || !this.proc.stdin) {
      throw new Error("AgentCtrl: daemon did not provide stdio pipes");
    }

    this.reader = createInterface({ input: this.proc.stdout });
    this.reader.on("line", (line) => this.handleLine(line));

    this.proc.on("exit", (code, signal) => {
      this.closed = true;
      // Record the exit reason regardless of pending count, so a `send()`
      // arriving after exit can surface a meaningful error rather than a
      // generic "daemon is closed".
      const reason = signal
        ? `daemon exited with signal ${signal}`
        : `daemon exited with code ${code ?? "unknown"}`;
      this.exitError ??= new Error(reason);
      for (const p of this.pending.values()) {
        clearTimeout(p.timer);
        p.reject(this.exitError);
      }
      this.pending.clear();
    });

    this.proc.on("error", (err) => {
      // Spawn failure (e.g., binary not found) - flag the client as closed
      // so subsequent `send()` short-circuits and `close()` doesn't hang
      // waiting for an `exit` event that will never fire.
      this.closed = true;
      this.exitError = err;
      for (const p of this.pending.values()) {
        clearTimeout(p.timer);
        p.reject(err);
      }
      this.pending.clear();
    });
  }

  /** Open a new session against the requested surface. */
  async openSession(surface: SurfaceKind): Promise<SessionId> {
    return (await this.openSessionInfo(surface)).session;
  }

  /** Open a session and return its negotiated protocol metadata. */
  async openSessionInfo(surface: SurfaceKind): Promise<OpenedSession> {
    const r = await this.send({ op: "open_session", surface });
    if (r.result === "session_opened") {
      if (r.protocol_version !== PROTOCOL_VERSION) {
        throw new Error(
          `open_session failed: daemon protocol ${r.protocol_version} is incompatible with client protocol ${PROTOCOL_VERSION}`,
        );
      }
      return {
        session: r.session,
        protocolVersion: r.protocol_version,
        surface: r.surface,
        capabilities: r.capabilities,
      };
    }
    throw asError("open_session", r);
  }

  /** Capture a snapshot of the surface tree. */
  async snapshot(session: SessionId, opts: SnapshotOptions = {}): Promise<Snapshot> {
    const r = await this.send({ op: "snapshot", session, opts });
    if (r.result === "snapshot") return r.snapshot;
    throw asError("snapshot", r);
  }

  /** Execute an action against the session. */
  async act(session: SessionId, action: Action): Promise<ActionResult> {
    const timeoutMs =
      action.kind === "wait"
        ? Math.max(this.requestTimeoutMs, action.ms + 5_000)
        : this.requestTimeoutMs;
    const r = await this.send({ op: "act", session, action }, timeoutMs);
    if (r.result === "action_done") return r.outcome;
    throw asError("act", r);
  }

  /**
   * Look up refs in the session's most recent snapshot.
   *
   * Pure read against the daemon's cached snapshot - does not re-walk the
   * OS accessibility tree. Throws if no snapshot has been captured yet on
   * this session; call `snapshot` first.
   */
  async find(session: SessionId, query: FindQuery = {}): Promise<FindMatch[]> {
    const r = await this.send({ op: "find", session, query });
    if (r.result === "find_results") return r.matches;
    throw asError("find", r);
  }

  /** Read one field from a ref in the session's most recent snapshot. */
  async get(session: SessionId, field: GetField, refId?: RefId): Promise<GetResult> {
    const r = await this.send({ op: "get", session, field, ref_id: refId });
    if (r.result === "get_done") return r.output;
    throw asError("get", r);
  }

  /** Check one boolean state on a ref in the session's most recent snapshot. */
  async is(session: SessionId, refId: RefId, field: StateField): Promise<IsResult> {
    const r = await this.send({ op: "is", session, ref_id: refId, field });
    if (r.result === "is_done") return r.output;
    throw asError("is", r);
  }

  /**
   * Block until a UI predicate is satisfied or the timeout fires.
   *
   * The daemon polls the surface at `opts.poll_ms` (floored at 50ms) and
   * caches each successful snapshot, so a follow-up `find` or `act` after
   * a successful wait sees fresh refs without an extra round-trip.
   *
   * Requires a prior `snapshot` so the polling loop knows which window to
   * target. The returned outcome distinguishes `matched` / `gone` /
   * `stable` / `timeout` - `timeout` is *not* an exception, callers
   * branch on `outcome.outcome` directly.
   */
  async waitFor(session: SessionId, opts: WaitOptions): Promise<WaitOutcome> {
    const r = await this.send(
      { op: "wait", session, opts },
      Math.max(this.requestTimeoutMs, opts.timeout_ms + 5_000),
    );
    if (r.result === "wait_done") return r.outcome;
    throw asError("wait", r);
  }

  /**
   * Enumerate the top-level windows the session can target.
   *
   * Mirrors agent-browser's `tab_list`: when a dialog or popup spawns
   * outside the currently pinned window, this returns it as a sibling.
   * Switch to a different window with `act(session, { kind: "focus_window",
   * window_id })` and then re-snapshot.
   */
  async listWindows(session: SessionId): Promise<WindowInfo[]> {
    const r = await this.send({ op: "list_windows", session });
    if (r.result === "windows") return r.windows;
    throw asError("list_windows", r);
  }

  /** Execute multiple operations in order on the daemon. */
  async batch(
    session: SessionId,
    steps: BatchStep[],
    { bail = false }: { bail?: boolean } = {},
  ): Promise<BatchStepOutcome[]> {
    const declaredWaitMs = steps.reduce((total, step) => {
      if (step.op === "wait") return total + step.opts.timeout_ms;
      if (step.op === "act" && step.action.kind === "wait") {
        return total + step.action.ms;
      }
      return total;
    }, 0);
    const r = await this.send(
      { op: "batch", session, steps, bail },
      Math.max(this.requestTimeoutMs, declaredWaitMs + 5_000),
    );
    if (r.result === "batch_done") return r.outcomes;
    throw asError("batch", r);
  }

  /** Close one session without shutting down the daemon. */
  async closeSession(session: SessionId): Promise<void> {
    const r = await this.send({ op: "close_session", session });
    if (r.result === "closed") return;
    throw asError("close_session", r);
  }

  /**
   * Shut down the daemon process. Idempotent.
   *
   * After the stdin pipe is closed, waits up to `gracePeriodMs` for the
   * daemon to exit on its own. Any in-flight requests are rejected
   * immediately rather than being left to hang on the daemon's exit.
   */
  async close({ gracePeriodMs = 5_000 } = {}): Promise<void> {
    if (this.closed || this.proc.exitCode !== null || this.proc.signalCode !== null) return;
    if (!this.closed) {
      this.proc.stdin?.end();
    }

    // Reject any pending requests up front - we don't want their promises
    // hanging on the daemon's eventual exit.
    if (this.pending.size > 0) {
      const closeError = new Error("AgentCtrl: client closed before response arrived");
      for (const p of this.pending.values()) {
        clearTimeout(p.timer);
        p.reject(closeError);
      }
      this.pending.clear();
    }

    if (this.proc.exitCode !== null || this.proc.signalCode !== null) return;

    await new Promise<void>((resolve) => {
      let forceTimer: ReturnType<typeof setTimeout> | undefined;
      const finish = () => {
        clearTimeout(timer);
        if (forceTimer) clearTimeout(forceTimer);
        this.proc.off("exit", finish);
        this.proc.off("close", finish);
        this.proc.off("error", finish);
        resolve();
      };
      const timer = setTimeout(() => {
        // Daemon didn't exit gracefully - escalate.
        this.proc.kill("SIGKILL");
        // Some failed or already-reaped child processes never emit another
        // event after kill. Bound close latency even in that state.
        forceTimer = setTimeout(finish, 1_000);
      }, gracePeriodMs);
      this.proc.once("exit", finish);
      this.proc.once("close", finish);
      this.proc.once("error", finish);
    });
  }

  private send(op: RequestOp, timeoutMs = this.requestTimeoutMs): Promise<Response> {
    if (this.closed) {
      return Promise.reject(this.exitError ?? new Error("daemon is closed"));
    }
    const deadlineMs =
      Number.isFinite(timeoutMs) && timeoutMs > 0
        ? Math.min(timeoutMs, 2_147_483_647)
        : this.requestTimeoutMs;
    const id = randomUUID();
    const request: Request = { id, ...op };
    return new Promise<Response>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${op.op} timed out after ${deadlineMs}ms`));
      }, deadlineMs);
      this.pending.set(id, { resolve, reject, timer });
      const stdin = this.proc.stdin;
      if (!stdin) {
        this.pending.delete(id);
        clearTimeout(timer);
        reject(new Error("daemon stdin is not writable"));
        return;
      }
      stdin.write(`${JSON.stringify(request)}\n`, (err) => {
        if (err) {
          this.pending.delete(id);
          clearTimeout(timer);
          reject(err);
        }
      });
    });
  }

  private handleLine(line: string): void {
    const trimmed = line.trim();
    if (trimmed.length === 0) return;
    let response: Response;
    try {
      response = JSON.parse(trimmed) as Response;
    } catch (e) {
      console.error("[agent-ctrl] unparseable response:", trimmed, e);
      return;
    }
    const pending = this.pending.get(response.id);
    if (!pending) {
      // Server sent an empty-id parse-failure response, or a response for an
      // already-rejected request. Either way, surface it and move on.
      console.error("[agent-ctrl] response with unknown id:", response);
      return;
    }
    this.pending.delete(response.id);
    clearTimeout(pending.timer);
    pending.resolve(response);
  }
}

function asError(operation: string, response: Response): Error {
  if (response.result === "error") {
    return new Error(`${operation} failed: ${response.message}`);
  }
  return new Error(`${operation}: unexpected response result \`${response.result}\``);
}
