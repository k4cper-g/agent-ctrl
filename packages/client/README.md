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
    await ctrl.waitFor(session, {
      predicate: { kind: "stable", idle_ms: 250 },
      timeout_ms: 5_000,
      poll_ms: 250,
    });
  }

  const name = firstRef ? await ctrl.get(session, "name", firstRef) : null;
  console.log(name?.value);

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

`v0.1` - paired with the daemon's mock surface for protocol validation and
both the Windows UIA surface and the macOS Accessibility (AX) surface for
real native-app automation. Linux, Android, and iOS are not implemented yet.
The client itself is platform-agnostic - it spawns whatever `agent-ctrl`
binary is on PATH and talks to it over stdio JSON-RPC.

## Real UIA Tests (Windows)

The default test suite uses the mock surface. The opt-in Windows UIA test uses
the deterministic `agent-ctrl-uia-fixture`, not a built-in Windows app:

```powershell
cargo build -p agent-ctrl-cli -p agent-ctrl-uia-fixture
$env:RUN_UIA_TESTS = "1"
npm run test --workspace=@agent-ctrl/client
```

## Real AX Tests (macOS)

The macOS counterpart uses `agent-ctrl-ax-fixture` (a Cocoa app) and
runs through the Rust integration test rather than the npm suite,
because it needs Accessibility + Screen Recording grants on the
`agent-ctrl` binary running it:

```bash
cargo build -p agent-ctrl-cli -p agent-ctrl-ax-fixture
RUN_AX_TESTS=1 cargo test -p agent-ctrl-cli --test macos_ax_fixture
```

The TypeScript surface itself is identical on Windows and macOS - same
methods, same JSON shapes. See [`docs/macos-ax-reliability.md`](../../docs/macos-ax-reliability.md)
for production notes specific to macOS (TCC permissions, sheets, IME).

## API Notes

The client uses stdio daemon transport, so TCP session auth tokens are not
needed. The shell CLI uses TCP session files and sends the per-session token
automatically.

Main methods: `openSession`, `snapshot`, `act`, `find`, `get`, `is`,
`waitFor`, `listWindows`, `batch`, `closeSession`, and `close`.

Action types are shared across surfaces and include check-state actions,
clipboard operations, raw mouse events, screenshot targets, drag, scroll,
select, switch-app, and highlight requests. See
[`src/types.ts`](src/types.ts) for the exact wire shapes.

The TypeScript wire types in [`src/types.ts`](src/types.ts) are hand-maintained.
A future commit will generate them from the Rust source via `ts-rs`.
