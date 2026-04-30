// End-to-end test: drives the real Rust daemon (mock surface) from TypeScript.
//
// Spawns the daemon via `cargo run`. Cargo will recompile on first invocation
// after a Rust source change, so the first run can be slow; subsequent runs
// reuse the cached binary.

import { afterEach, describe, expect, it } from "vitest";

import { AgentCtrl, type Snapshot } from "../src/index.js";

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

describe("AgentCtrl driving the mock surface", () => {
  let client: AgentCtrl | null = null;

  afterEach(async () => {
    if (client) {
      await client.close();
      client = null;
    }
  });

  it("opens a mock session, snapshots it, acts on it, then closes", async () => {
    client = new AgentCtrl({ command: DAEMON_COMMAND, stderr: "ignore" });

    const session = await client.openSession("mock");
    expect(typeof session).toBe("string");
    expect(session.length).toBeGreaterThan(0);

    const snap: Snapshot = await client.snapshot(session);
    expect(snap.surface_kind).toBe("mock");
    expect(snap.app.id).toBe("agent.ctrl.mock");
    expect(snap.root.children?.length).toBe(2);

    const buttonRefs = Object.entries(snap.refs.entries)
      .filter(([, e]) => e.role === "button")
      .map(([refId]) => refId);
    expect(buttonRefs).toHaveLength(2);

    const okRef = buttonRefs[0]!;
    const result = await client.act(session, { kind: "click", ref_id: okRef });
    expect(result.ok).toBe(true);

    await client.closeSession(session);
  }, 120_000); // first cargo build can be slow

  it("rejects calls after close()", async () => {
    client = new AgentCtrl({ command: DAEMON_COMMAND, stderr: "ignore" });
    await client.close();
    await expect(client.openSession("mock")).rejects.toThrow();
  }, 120_000);

  it("returns an error result for unknown sessions", async () => {
    client = new AgentCtrl({ command: DAEMON_COMMAND, stderr: "ignore" });
    await expect(
      client.snapshot("00000000-0000-0000-0000-000000000000"),
    ).rejects.toThrow(/unknown session/);
  }, 120_000);
});
