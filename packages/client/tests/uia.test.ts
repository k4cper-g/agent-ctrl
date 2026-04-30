// End-to-end test: drives the UIA surface against a real Notepad window.
//
// This file is a *contract* for `surface-uia` — it captures what v0.1 of
// the implementation must deliver. As we implement the surface against
// the spec in `docs/uia-mapping.md`, these tests turn green.
//
// Skipped unless:
//   - `process.platform === "win32"` (UIA is Windows-only), AND
//   - `RUN_UIA_TESTS=1` is set in the environment.
//
// The env-var gate keeps CI green and avoids spawning Notepad windows on
// every contributor's machine. Run locally with:
//
//   RUN_UIA_TESTS=1 npm run test --workspace=@agent-ctrl/client

import { execFileSync, spawn, type ChildProcess } from "node:child_process";

import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { AgentCtrl, type Snapshot } from "../src/index.js";

/**
 * Bring a window matching `titleSubstring` to the foreground via `WScript.Shell.AppActivate`.
 *
 * Notepad is foregrounded by Windows when it spawns, but adjacent apps (the
 * IDE / terminal hosting the test runner) often steal focus back before our
 * snapshot fires. Without this helper the snapshot captures whatever app
 * happens to be foreground, not Notepad.
 */
function bringToForeground(titleSubstring: string): void {
  try {
    execFileSync(
      "powershell",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        `$null = (New-Object -ComObject WScript.Shell).AppActivate('${titleSubstring}')`,
      ],
      { stdio: "ignore" },
    );
  } catch {
    // Best-effort. If PowerShell or WScript is unavailable the test will fail
    // later with a clearer assertion error.
  }
}

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

/** Wait until `predicate` returns truthy or the deadline passes. */
async function waitFor<T>(
  predicate: () => Promise<T | null | undefined>,
  { timeoutMs = 10_000, intervalMs = 200 } = {},
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const out = await predicate();
    if (out) return out;
    await new Promise((r) => setTimeout(r, intervalMs));
  }
  throw new Error(`waitFor timed out after ${timeoutMs}ms`);
}

describe.skipIf(!runSuite)("AgentCtrl driving the UIA surface against Notepad", () => {
  let client: AgentCtrl | null = null;
  let notepad: ChildProcess | null = null;

  beforeEach(async () => {
    notepad = spawn("notepad.exe", [], { detached: false, stdio: "ignore" });
    // Give the window time to appear. We no longer need it foreground because
    // the snapshot is targeted by title, not by foreground.
    await new Promise((r) => setTimeout(r, 750));
    client = new AgentCtrl({ command: DAEMON_COMMAND, stderr: "ignore" });
  });

  afterEach(async () => {
    if (client) {
      await client.close();
      client = null;
    }
    if (notepad) {
      notepad.kill();
      notepad = null;
    }
  });

  // Use process-name targeting so the test is locale-independent — the window
  // title is "Untitled - Notepad" in English but localized in other languages.
  const NOTEPAD_TARGET = { target: { by: "process-name" as const, name: "Notepad" } };

  it("captures Notepad and exposes its edit area", async () => {
    const session = await client!.openSession("uia");
    const snap: Snapshot = await client!.snapshot(session, NOTEPAD_TARGET);

    expect(snap.surface_kind).toBe("uia");
    expect(snap.app.name.toLowerCase()).toMatch(/notepad/);

    // Notepad's edit surface is a TextField on classic Notepad (UIA `Edit`)
    // or a Document on Win11 Notepad (UIA `Document` + ValuePattern). Either
    // is a legitimate "user-editable text region" and should produce a ref.
    const editableRefs = findEditableRefs(snap);
    expect(editableRefs.length).toBeGreaterThan(0);

    await client!.closeSession(session);
  }, 120_000);

  it("fills the edit area and reads the value back", async () => {
    const session = await client!.openSession("uia");
    const snap = await client!.snapshot(session, NOTEPAD_TARGET);

    const editableRefs = findEditableRefs(snap);
    const editRef = editableRefs[0];
    expect(editRef).toBeDefined();

    const text = "hello from agent-ctrl";
    const result = await client!.act(session, {
      kind: "fill",
      ref_id: editRef!,
      value: text,
    });
    expect(result.ok).toBe(true);

    // Re-snapshot the same window. The edit's value should reflect the typed text.
    const snap2 = await waitFor(async () => {
      const s = await client!.snapshot(session, NOTEPAD_TARGET);
      const node = findNodeByRef(s, editRef!);
      return node?.value && node.value.includes(text) ? s : null;
    });

    const node = findNodeByRef(snap2, editRef!);
    expect(node?.value).toContain(text);

    await client!.closeSession(session);
  }, 120_000);

  it("clicks an invoke-able menu item", async () => {
    const session = await client!.openSession("uia");
    const snap = await client!.snapshot(session, NOTEPAD_TARGET);

    // Pick any menu-item ref. We can't hardcode "File" because the menu text
    // is localized (Polish: "Plik", German: "Datei", etc.). The contract is
    // "click action works against an Invoke-pattern element", and any menu
    // item exposes Invoke.
    const menuRef = Object.entries(snap.refs.entries).find(
      ([, e]) => e.role === "menu-item",
    )?.[0];
    expect(menuRef, "no menu-item refs found in Notepad's tree").toBeDefined();

    const result = await client!.act(session, { kind: "click", ref_id: menuRef! });
    expect(result.ok).toBe(true);

    await client!.closeSession(session);
  }, 120_000);
});

// ---------- helpers ----------

function findRefs(snap: Snapshot, role: string): string[] {
  return Object.entries(snap.refs.entries)
    .filter(([, e]) => e.role === role)
    .map(([refId]) => refId);
}

/** Refs that point at user-editable text regions, regardless of which UIA
 *  ControlType the host app chose to use. */
function findEditableRefs(snap: Snapshot): string[] {
  return Object.entries(snap.refs.entries)
    .filter(([, e]) => e.role === "text-field" || e.role === "document")
    .map(([refId]) => refId);
}

function findNodeByRef(snap: Snapshot, refId: string): { value?: string } | null {
  const stack: Array<{ ref_id?: string; value?: string; children?: unknown[] }> = [
    snap.root as unknown as { ref_id?: string; value?: string; children?: unknown[] },
  ];
  while (stack.length > 0) {
    const node = stack.pop()!;
    if (node.ref_id === refId) return node;
    if (Array.isArray(node.children)) {
      for (const c of node.children) {
        stack.push(c as { ref_id?: string; value?: string; children?: unknown[] });
      }
    }
  }
  return null;
}
