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
    // Win11 Notepad's UIA tree (XAML) populates lazily after the window is
    // visible — the editable Document and the tab bar arrive on a separate
    // tick from the top-level window. Give it a moment, then poll inside the
    // test if a snapshot still misses them.
    await new Promise((r) => setTimeout(r, 750));
    client = new AgentCtrl({ command: DAEMON_COMMAND, stderr: "ignore" });
  });

  /**
   * Snapshot Notepad, retrying until the editable Document ref shows up.
   * Win11 Notepad emits the document into its UIA tree a beat after the
   * window appears; without this, fast snapshots intermittently catch a
   * window-only tree.
   */
  async function snapshotReady(session: SessionId): Promise<Snapshot> {
    return waitFor(async () => {
      const s = await client!.snapshot(session, NOTEPAD_TARGET);
      return findEditableRefs(s).length > 0 ? s : null;
    });
  }

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
    const snap: Snapshot = await snapshotReady(session);

    expect(snap.surface_kind).toBe("uia");
    expect(snap.app.name.toLowerCase()).toMatch(/notepad/);

    // Notepad's edit surface is a TextField on classic Notepad (UIA `Edit`)
    // or a Document on Win11 Notepad (UIA `Document` + ValuePattern). Either
    // is a legitimate "user-editable text region" and should produce a ref.
    const editableRefs = findEditableRefs(snap);
    expect(editableRefs.length).toBeGreaterThan(0);

    // Every emitted ref should carry a UIA NativeHandle with a non-empty
    // RuntimeId (4 LE bytes per i32 slot, so length is a positive multiple
    // of 4). AutomationId is optional — Notepad's controls usually omit it.
    const editEntry = snap.refs.entries[editableRefs[0]!]!;
    expect(editEntry.native?.platform).toBe("uia");
    if (editEntry.native?.platform === "uia") {
      expect(editEntry.native.runtime_id.length).toBeGreaterThan(0);
      expect(editEntry.native.runtime_id.length % 4).toBe(0);
    }

    await client!.closeSession(session);
  }, 120_000);

  it("fills the edit area and reads the value back", async () => {
    const session = await client!.openSession("uia");
    const snap = await snapshotReady(session);

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
    const snap = await snapshotReady(session);

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

  it("types text via SendInput without erroring", async () => {
    const session = await client!.openSession("uia");
    const snap = await snapshotReady(session);

    const editRef = findEditableRefs(snap)[0];
    expect(editRef).toBeDefined();

    bringToForeground("Notepad");
    const focusRes = await client!.act(session, { kind: "focus", ref_id: editRef! });
    expect(focusRes.ok).toBe(true);

    // We deliberately do NOT round-trip the typed value through a
    // re-snapshot. Win11 Notepad's WinUI 3 input layer is unreliable about
    // reflecting injected `KEYEVENTF_UNICODE` keystrokes (under load it can
    // drop, reorder, or substitute characters), and what `surface-uia`
    // actually owns is "the events were inserted into the OS input queue".
    // For guaranteed text delivery against an editable field, agents should
    // use `Fill` — covered above and in the `clears typed text` test below.
    const typeRes = await client!.act(session, { kind: "type", text: "hello" });
    expect(typeRes.ok).toBe(true);

    // The daemon must stay responsive after the SendInput batch — a stuck
    // worker would surface here as a snapshot timeout.
    const snap2 = await client!.snapshot(session, NOTEPAD_TARGET);
    expect(snap2.surface_kind).toBe("uia");

    await client!.closeSession(session);
  }, 120_000);

  it("reads selection state from the active Notepad tab", async () => {
    const session = await client!.openSession("uia");
    const snap = await snapshotReady(session);

    // Win11 Notepad's tab bar contains a selectable TabItem per open file.
    // With a fresh Notepad there's exactly one tab, and it's the active one,
    // so its SelectionItemPattern.IsSelected must surface as true.
    const tabs = collectNodesByRole(snap, "tab");
    expect(tabs.length, "no tab elements found in Notepad's tree").toBeGreaterThan(0);
    const selectedTabs = tabs.filter((t) => t.state.selected === true);
    expect(selectedTabs.length, "expected at least one tab with selected: true").toBeGreaterThan(0);

    await client!.closeSession(session);
  }, 120_000);

  it("clears typed text with Ctrl+A then Delete", async () => {
    const session = await client!.openSession("uia");
    const snap = await snapshotReady(session);

    const editRef = findEditableRefs(snap)[0];
    expect(editRef).toBeDefined();

    // Seed via Fill (path already verified in earlier test) so the Press
    // assertions are about the chord, not about typing.
    const seed = "to be deleted";
    const fillRes = await client!.act(session, {
      kind: "fill",
      ref_id: editRef!,
      value: seed,
    });
    expect(fillRes.ok).toBe(true);

    // Wait until the Fill is reflected before clearing — otherwise a fast
    // Press could race with Notepad's value-pattern apply.
    await waitFor(async () => {
      const s = await client!.snapshot(session, NOTEPAD_TARGET);
      return findNodeByRef(s, editRef!)?.value?.includes(seed) ? s : null;
    });

    bringToForeground("Notepad");
    await client!.act(session, { kind: "focus", ref_id: editRef! });

    const selectRes = await client!.act(session, { kind: "press", keys: "Ctrl+A" });
    expect(selectRes.ok).toBe(true);
    const deleteRes = await client!.act(session, { kind: "press", keys: "Delete" });
    expect(deleteRes.ok).toBe(true);

    const snap2 = await waitFor(async () => {
      const s = await client!.snapshot(session, NOTEPAD_TARGET);
      const node = findNodeByRef(s, editRef!);
      // Notepad reports an empty Document value as undefined or "".
      return !node?.value ? s : null;
    });

    const node = findNodeByRef(snap2, editRef!);
    expect(node?.value ?? "").toBe("");

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

interface NodeWithState {
  role: string | { unknown: string };
  state: { selected?: boolean; checked?: string; expanded?: boolean; required?: boolean };
  children?: unknown[];
}

function collectNodesByRole(snap: Snapshot, role: string): NodeWithState[] {
  const found: NodeWithState[] = [];
  const stack: NodeWithState[] = [snap.root as unknown as NodeWithState];
  while (stack.length > 0) {
    const node = stack.pop()!;
    if (node.role === role) found.push(node);
    if (Array.isArray(node.children)) {
      for (const c of node.children) {
        stack.push(c as NodeWithState);
      }
    }
  }
  return found;
}
