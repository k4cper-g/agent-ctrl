# @agent-ctrl/client

TypeScript client for the [agent-ctrl](../..) daemon.

Spawns the Rust `agent-ctrl daemon` as a subprocess and talks JSON-RPC to it
over stdio. Provides a typed API over the wire protocol so you can write
agent code in TypeScript while the OS automation runs in native Rust.

## Install (workspace)

```bash
npm install
npm run build --workspace=@agent-ctrl/client
```

## Usage

```typescript
import { AgentCtrl } from "@agent-ctrl/client";

const ctrl = new AgentCtrl();
try {
  const session = await ctrl.openSession("mock");
  const snap = await ctrl.snapshot(session);
  console.log(`captured ${Object.keys(snap.refs.entries).length} refs`);

  // Click the first button
  const [firstRef] = Object.keys(snap.refs.entries);
  if (firstRef) {
    await ctrl.act(session, { kind: "click", ref_id: firstRef });
  }

  await ctrl.closeSession(session);
} finally {
  await ctrl.close();
}
```

## Configuration

```typescript
new AgentCtrl({
  // Full spawn command. Defaults to ["agent-ctrl", "daemon"].
  command: ["cargo", "run", "-q", "-p", "agent-ctrl-cli", "--", "daemon"],
  // What to do with daemon stderr: "inherit" (default) or "ignore".
  stderr: "ignore",
  // Working directory for the daemon process.
  cwd: process.cwd(),
});
```

## Status

`v0.1` - paired with the daemon's mock surface for protocol validation.
Real surfaces (UIA, AX, CDP) land in subsequent versions.

The TypeScript wire types in [`src/types.ts`](src/types.ts) are hand-maintained.
A future commit will generate them from the Rust source via `ts-rs`.
