// Opt-in end-to-end test: drives the UIA surface through the TypeScript client
// against the deterministic Win32 fixture app.
//
// Skipped unless:
//   - `process.platform === "win32"` (UIA is Windows-only), AND
//   - `RUN_UIA_TESTS=1` is set in the environment.
//
// Build the fixture and run locally with:
//
//   cargo build -p agent-ctrl-cli -p agent-ctrl-uia-fixture
//   RUN_UIA_TESTS=1 npm run test --workspace=@agent-ctrl/client

import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { spawn, type ChildProcess } from "node:child_process";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { AgentCtrl, type RefId, type SessionId, type Snapshot } from "../src/index.js";

const DAEMON_COMMAND = [
  "cargo",
  "run",
  "-q",
  "--manifest-path",
  "../../Cargo.toml",
  "-p",
  "agent-ctrl-cli",
  "--",
  "daemon",
];

const isWindows = process.platform === "win32";
const optedIn = process.env.RUN_UIA_TESTS === "1";
const runSuite = isWindows && optedIn;

describe.skipIf(!runSuite)("AgentCtrl driving the UIA fixture", () => {
  let client: AgentCtrl | null = null;
  let session: SessionId | null = null;
  let fixture: ChildProcess | null = null;
  let tempDir: string | null = null;

  beforeEach(async () => {
    tempDir = mkdtempSync(resolve(tmpdir(), "agent-ctrl-ts-uia-"));
    const readyFile = resolve(tempDir, "fixture.ready");
    const fixtureExe = fixturePath();
    if (!existsSync(fixtureExe)) {
      throw new Error(
        `missing UIA fixture at ${fixtureExe}; run cargo build -p agent-ctrl-uia-fixture`,
      );
    }

    fixture = spawn(
      fixtureExe,
      ["--ready-file", readyFile, "--auto-close-ms", "60000"],
      { stdio: "ignore" },
    );
    await waitFor(async () => (existsSync(readyFile) ? true : null));

    client = new AgentCtrl({ command: DAEMON_COMMAND, stderr: "ignore" });
    session = await client.openSession("uia");
  });

  afterEach(async () => {
    if (client) {
      if (session) {
        await client.closeSession(session).catch(() => {});
      }
      await client.close().catch(() => {});
      client = null;
      session = null;
    }
    if (fixture) {
      fixture.kill();
      fixture = null;
    }
    if (tempDir) {
      rmSync(tempDir, { recursive: true, force: true });
      tempDir = null;
    }
  });

  it("snapshots the fixture and exposes inspect helpers", async () => {
    const snap = await snapshotFixture();
    expect(snap.surface_kind).toBe("uia");
    expect(snap.app.name).toBe("agent-ctrl-uia-fixture");

    const button = await firstRef({ name: "Increment", role: "button" });
    const name = await client!.get(session!, "name", button);
    expect(name.value).toBe("Increment");

    const enabled = await client!.is(session!, button, "enabled");
    expect(enabled.value).toBe(true);
  }, 120_000);

  it("acts, waits, and reads updated values", async () => {
    await snapshotFixture();
    const field = await firstRef({ role: "text-field" });
    const button = await firstRef({ name: "Increment", role: "button" });
    const clicked = await client!.act(session!, { kind: "click", ref_id: button });
    expect(clicked.ok).toBe(true);

    const waited = await client!.waitFor(session!, {
      predicate: {
        kind: "appears",
        query: { name: "Status: count 1", role: "text-field", limit: 1 },
      },
      timeout_ms: 5000,
      poll_ms: 100,
    });
    expect(waited.outcome).toBe("matched");

    const filled = await client!.act(session!, {
      kind: "fill",
      ref_id: field,
      value: "fixture edited from ts",
    });
    expect(filled.ok).toBe(true);

    const valueWait = await client!.waitFor(session!, {
      predicate: {
        kind: "value-contains",
        query: { role: "text-field", limit: 1 },
        value: "fixture edited from ts",
      },
      timeout_ms: 5000,
      poll_ms: 100,
    });
    expect(valueWait.outcome).toBe("matched");
  }, 120_000);

  it("handles check state and selection state", async () => {
    await snapshotFixture();

    const checkbox = await firstRef({ name: "Enable advanced mode", role: "checkbox" });
    const checked = await client!.act(session!, { kind: "check", ref_id: checkbox });
    expect(checked.ok).toBe(true);

    const checkedState = await client!.waitFor(session!, {
      predicate: {
        kind: "state",
        query: { name: "Enable advanced mode", role: "checkbox", limit: 1 },
        field: "checked",
        value: true,
      },
      timeout_ms: 5000,
      poll_ms: 100,
    });
    expect(checkedState.outcome).toBe("matched");

    await snapshotFixture();
    const option = await firstRef({ name: "Second", role: "option" });
    const selected = await client!.act(session!, {
      kind: "select",
      ref_id: option,
      value: "Second",
    });
    expect(selected.ok).toBe(true);

    const selectedState = await client!.waitFor(session!, {
      predicate: {
        kind: "state",
        query: { name: "Second", role: "option", limit: 1 },
        field: "selected",
        value: true,
      },
      timeout_ms: 5000,
      poll_ms: 100,
    });
    expect(selectedState.outcome).toBe("matched");
  }, 120_000);

  it("captures window, ref, region, and annotated screenshots", async () => {
    await snapshotFixture();
    const checkbox = await firstRef({ name: "Enable advanced mode", role: "checkbox" });

    const windowShot = await client!.act(session!, {
      kind: "screenshot",
      target: { kind: "window" },
    });
    const refShot = await client!.act(session!, {
      kind: "screenshot",
      target: { kind: "ref", ref_id: checkbox },
    });
    const regionShot = await client!.act(session!, {
      kind: "screenshot",
      target: { kind: "region", region: { x: 0, y: 0, w: 64, h: 64 } },
    });
    const annotated = await client!.act(session!, {
      kind: "screenshot",
      target: { kind: "window" },
      annotated: true,
    });

    assertPngPayload(windowShot.data);
    assertPngPayload(refShot.data);
    assertPngPayload(regionShot.data);
    assertPngPayload(annotated.data);
    expect((annotated.data as { annotated?: boolean }).annotated).toBe(true);
  }, 120_000);

  it("executes batch steps in order", async () => {
    await snapshotFixture();
    const outcomes = await client!.batch(
      session!,
      [
        { op: "find", query: { name: "Increment", role: "button", limit: 1 } },
        { op: "get", field: "window" },
        { op: "list_windows" },
      ],
      { bail: true },
    );

    expect(outcomes).toHaveLength(3);
    expect(outcomes.every((outcome) => outcome.ok)).toBe(true);
  }, 120_000);

  it("tracks sibling dialog windows", async () => {
    await snapshotFixture();
    const opener = await firstRef({ name: "Open dialog", role: "button" });
    const opened = await client!.act(session!, { kind: "click", ref_id: opener });
    expect(opened.ok).toBe(true);

    const appeared = await client!.waitFor(session!, {
      predicate: { kind: "window-appears", title: "Fixture Secondary Dialog" },
      timeout_ms: 5000,
      poll_ms: 100,
    });
    expect(appeared.outcome).toBe("matched");

    const windows = await client!.listWindows(session!);
    const dialog = windows.find((w) => w.title?.includes("Fixture Secondary Dialog"));
    expect(dialog).toBeDefined();

    const focused = await client!.act(session!, {
      kind: "focus_window",
      window_id: dialog!.id,
    });
    expect(focused.ok).toBe(true);

    await client!.snapshot(session!);
    const ok = await firstRef({ name: "Dialog OK", role: "button" });
    const closed = await client!.act(session!, { kind: "click", ref_id: ok });
    expect(closed.ok).toBe(true);
  }, 120_000);

  async function snapshotFixture(): Promise<Snapshot> {
    return client!.snapshot(session!, {
      target: { by: "process-name", name: "agent-ctrl-uia-fixture" },
    });
  }

  async function firstRef(query: { name?: string; role?: string }): Promise<RefId> {
    const matches = await waitFor(async () => {
      const results = await client!.find(session!, { ...query, limit: 1 });
      return results[0]?.ref_id;
    });
    return matches;
  }
});

async function waitFor<T>(
  predicate: () => Promise<T | null | undefined>,
  { timeoutMs = 10_000, intervalMs = 100 } = {},
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const out = await predicate();
    if (out) return out;
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  throw new Error(`waitFor timed out after ${timeoutMs}ms`);
}

function fixturePath(): string {
  if (process.env.AGENT_CTRL_UIA_FIXTURE) return process.env.AGENT_CTRL_UIA_FIXTURE;
  const exe = process.platform === "win32" ? "agent-ctrl-uia-fixture.exe" : "agent-ctrl-uia-fixture";
  return resolve(process.cwd(), "../../target/debug", exe);
}

function assertPngPayload(data: unknown): void {
  const payload = data as
    | { format?: string; encoding?: string; width?: number; height?: number; data?: string }
    | undefined;
  expect(payload?.format).toBe("png");
  expect(payload?.encoding).toBe("base64");
  expect(payload?.width).toBeGreaterThan(0);
  expect(payload?.height).toBeGreaterThan(0);
  expect(typeof payload?.data).toBe("string");

  const bytes = Buffer.from(payload!.data!, "base64");
  expect(bytes.length).toBeGreaterThan(8);
  expect(bytes[0]).toBe(0x89);
  expect(bytes[1]).toBe(0x50);
  expect(bytes[2]).toBe(0x4e);
  expect(bytes[3]).toBe(0x47);
}
