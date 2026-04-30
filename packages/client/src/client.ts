// AgentCtrl: TypeScript client for the agent-ctrl daemon.
//
// Spawns the daemon as a subprocess and talks newline-delimited JSON-RPC
// over stdio. Requests carry a correlation id; concurrent calls are matched
// to their responses via a pending-id table.

import { spawn, type ChildProcess } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createInterface, type Interface as ReadlineInterface } from "node:readline";

import type {
  Action,
  ActionResult,
  Request,
  RequestOp,
  Response,
  SessionId,
  Snapshot,
  SnapshotOptions,
  SurfaceKind,
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
}

interface PendingRequest {
  resolve: (response: Response) => void;
  reject: (err: Error) => void;
}

/**
 * Client for one running agent-ctrl daemon.
 *
 * Construction spawns the daemon eagerly; call `close()` to terminate it.
 * Every public method is safe to call concurrently — requests are correlated
 * by id under the hood.
 */
export class AgentCtrl {
  private readonly proc: ChildProcess;
  private readonly reader: ReadlineInterface;
  private readonly pending = new Map<string, PendingRequest>();
  private closed = false;
  private exitError: Error | null = null;

  constructor(options: AgentCtrlOptions = {}) {
    const command = options.command ?? ["agent-ctrl", "daemon"];
    if (command.length === 0) {
      throw new Error("AgentCtrl: `command` cannot be empty");
    }
    const [binary, ...args] = command as [string, ...string[]];

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
        p.reject(this.exitError);
      }
      this.pending.clear();
    });

    this.proc.on("error", (err) => {
      // Spawn failure (e.g., binary not found) — flag the client as closed
      // so subsequent `send()` short-circuits and `close()` doesn't hang
      // waiting for an `exit` event that will never fire.
      this.closed = true;
      this.exitError = err;
      for (const p of this.pending.values()) {
        p.reject(err);
      }
      this.pending.clear();
    });
  }

  /** Open a new session against the requested surface. */
  async openSession(surface: SurfaceKind): Promise<SessionId> {
    const r = await this.send({ op: "open_session", surface });
    if (r.result === "session_opened") return r.session;
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
    const r = await this.send({ op: "act", session, action });
    if (r.result === "action_done") return r.outcome;
    throw asError("act", r);
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
    if (this.closed && this.proc.exitCode !== null) return;
    if (!this.closed) {
      this.proc.stdin?.end();
    }

    // Reject any pending requests up front — we don't want their promises
    // hanging on the daemon's eventual exit.
    if (this.pending.size > 0) {
      const closeError = new Error("AgentCtrl: client closed before response arrived");
      for (const p of this.pending.values()) {
        p.reject(closeError);
      }
      this.pending.clear();
    }

    if (this.proc.exitCode !== null) return;

    await new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        // Daemon didn't exit gracefully — escalate.
        this.proc.kill("SIGKILL");
      }, gracePeriodMs);
      this.proc.once("exit", () => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  private send(op: RequestOp): Promise<Response> {
    if (this.closed) {
      return Promise.reject(this.exitError ?? new Error("daemon is closed"));
    }
    const id = randomUUID();
    const request: Request = { id, ...op };
    return new Promise<Response>((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      const stdin = this.proc.stdin;
      if (!stdin) {
        this.pending.delete(id);
        reject(new Error("daemon stdin is not writable"));
        return;
      }
      stdin.write(`${JSON.stringify(request)}\n`, (err) => {
        if (err) {
          this.pending.delete(id);
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
    pending.resolve(response);
  }
}

function asError(operation: string, response: Response): Error {
  if (response.result === "error") {
    return new Error(`${operation} failed: ${response.message}`);
  }
  return new Error(`${operation}: unexpected response result \`${response.result}\``);
}
