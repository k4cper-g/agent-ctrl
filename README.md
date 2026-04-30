# agent-ctrl

Cross-platform computer-use framework for AI agents.

`agent-ctrl` exposes a unified, accessibility-tree based schema across operating systems, so an AI agent can drive Windows, macOS, Android, iOS, and the browser through a single consistent interface. The schema is modeled on [agent-browser](https://github.com/vercel-labs/agent-browser) and extended with the cross-platform concepts (apps, windows, native handles) that desktop and mobile require.

> **Status:** early scaffolding. The mock surface and protocol are wired up end-to-end (Rust daemon ↔ TypeScript client). Real surfaces (UIA, AX, CDP) are stubbed and land surface-by-surface.

## Workspace layout

The repository is a **dual workspace** — a Cargo workspace for the Rust engine and an npm workspace for the TypeScript client.

### Rust crates

| Crate | Purpose |
|---|---|
| [`crates/core`](crates/core) | Shared types and the `Surface` trait every platform implements. Schema, role taxonomy, action vocabulary, errors. |
| [`crates/daemon`](crates/daemon) | Long-running process that owns surface sessions and dispatches actions. |
| [`crates/cli`](crates/cli) | The `agent-ctrl` binary — user-facing entrypoint. |
| [`crates/surface-cdp`](crates/surface-cdp) | Chromium-via-CDP surface (browser parity, cross-platform). |
| [`crates/surface-uia`](crates/surface-uia) | Windows UI Automation surface (compiled only on Windows). |
| [`crates/surface-ax`](crates/surface-ax) | macOS Accessibility surface (compiled only on macOS). |

### TypeScript packages

| Package | Purpose |
|---|---|
| [`packages/client`](packages/client) | `@agent-ctrl/client` — typed TS wrapper that spawns the Rust daemon and talks JSON-RPC over stdio. |

## Platforms and surfaces

A **surface** is one accessibility protocol — UIA, AX, CDP, etc. A **platform** is an operating system. The two are not 1-to-1: most platforms can be driven by more than one surface (e.g. on Windows you can use UIA for native apps *and* CDP when you're driving Chrome), and CDP is a single protocol that spans every OS Chrome runs on. That's why the crates are named by surface, not by platform.

| Platform | Native surface | Browser surface | Status |
|---|---|---|---|
| Windows | [`surface-uia`](crates/surface-uia) — UI Automation | [`surface-cdp`](crates/surface-cdp) — Chrome / Edge via CDP | both scaffolded |
| macOS | [`surface-ax`](crates/surface-ax) — Accessibility / AX | [`surface-cdp`](crates/surface-cdp) — Chrome via CDP | both scaffolded |
| Linux | _planned_: `surface-atspi` (AT-SPI / D-Bus) | [`surface-cdp`](crates/surface-cdp) — Chrome via CDP | cdp scaffolded |
| Android | _planned_: `surface-accessibility-service` (AccessibilityService + JNI) | [`surface-cdp`](crates/surface-cdp) — Chrome via CDP | cdp scaffolded |
| iOS | _planned_: `surface-xcuitest` (XCUITest / WebDriverAgent) | [`surface-cdp`](crates/surface-cdp) — limited; iOS Chrome shares WebKit | cdp scaffolded |

Acronyms in one line: **UIA** = Microsoft UI Automation, **AX** = macOS Accessibility, **AT-SPI** = the Linux GNOME accessibility bus, **CDP** = Chrome DevTools Protocol, **XCUITest** = Apple's UI test automation framework.

## Build

Rust:

```bash
cargo check --workspace
cargo build --release -p agent-ctrl-cli
```

Surfaces gated by `target_os` compile to empty crates on other platforms, so the workspace builds on any host.

TypeScript:

```bash
npm install
npm run build --workspace=@agent-ctrl/client
npm run test  --workspace=@agent-ctrl/client
```

The TS test suite spawns the Rust daemon under `cargo run` and exercises the full protocol against the mock surface.

## Try it

The mock surface returns a fake two-button window — useful for exercising the protocol end-to-end without any platform-specific code:

```bash
# From Rust:
cargo run -p agent-ctrl-cli -- snapshot --surface mock
```

```typescript
// From TypeScript:
import { AgentCtrl } from "@agent-ctrl/client";

const ctrl = new AgentCtrl();
const session = await ctrl.openSession("mock");
const snap = await ctrl.snapshot(session);
console.log(snap.refs.entries);
await ctrl.close();
```

## License

Apache-2.0. See [LICENSE](LICENSE).
