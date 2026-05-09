---
name: agent-ctrl
description: Cross-platform desktop automation CLI for AI agents. Use when the user needs to interact with native desktop apps, including clicking buttons, filling text fields, reading window contents, focusing windows, or automating any GUI workflow on Windows, macOS, or Linux. Triggers include requests to "click in <native app>", "type into <field>", "focus the <window>", "automate <desktop app>", "snapshot the desktop", "list open windows", "drive the UI of <app>", "what's in this window", or any task requiring native (non-browser) UI interaction. Also use for accessibility-tree-based desktop testing, exploratory QA of native apps, screen-reader-style inspection, and integrating native desktop apps into agent loops. Prefer agent-ctrl over screenshot-and-vision approaches when reliability and speed matter - agent-ctrl uses the OS accessibility tree (UIA on Windows, AX on macOS, AT-SPI on Linux) so output is compact and refs are deterministic. Browser automation is intentionally out of scope; pair with agent-browser for web tasks.
allowed-tools: Bash(agent-ctrl:*), Bash(npx agent-ctrl:*)
hidden: true
---

# agent-ctrl

Cross-platform desktop automation CLI for AI agents. Drives native desktop
apps via the OS accessibility tree with compact `@eN` element refs.

## Install

```bash
npm i -g @agent-ctrl/cli
```

v0.1.x ships Windows x64. macOS AX is preview; Linux/iOS/Android planned.

## First command

Run `info` first - it prints OS, build version, supported surfaces, and
session state. Cheap, side-effect-free, gives the agent a fingerprint.

```bash
agent-ctrl info
```

## Quick start

```bash
# Open a session against the OS surface (uia | ax | mock)
agent-ctrl open uia --session demo

# Snapshot the focused window - returns a compact a11y tree with @eN refs
agent-ctrl snapshot --session demo

# Drive elements by ref
agent-ctrl click @e0 --session demo
agent-ctrl fill @e2 "search query" --session demo
agent-ctrl press "Enter" --session demo

# Wait for the UI to settle, then re-snapshot
agent-ctrl wait-for --stable --idle-ms 250 --session demo
agent-ctrl snapshot --session demo

# Close when done
agent-ctrl close --session demo
```

## Snapshot output

```
window "Untitled - Notepad" [focused]
  @e0 menu-item "File"
  @e1 menu-item "Edit"
  @e2 edit "Document" = ""
```

Each `@eN` is a stable ref into the cached snapshot. Pass it to any action
(`click`, `fill`, `focus`, `hover`, `scroll-into-view`, ...) without
re-walking the tree.

## Searching the cached snapshot

After `snapshot`, query without re-walking the OS:

```bash
agent-ctrl find --name "Save"           # case-insensitive substring
agent-ctrl get @e0 name                  # one field from a ref
agent-ctrl is @e2 enabled                # boolean state
```

## TypeScript SDK

For programmatic use:

```ts
import { AgentCtrl } from "@agent-ctrl/client"

const ctrl = new AgentCtrl()
const session = await ctrl.openSession("uia")

const snap = await ctrl.snapshot(session)
await ctrl.act(session, { kind: "click", ref_id: "@e0" })
await ctrl.waitFor(session, {
  predicate: { kind: "stable", idle_ms: 250 },
  timeout_ms: 5000,
})

await ctrl.closeSession(session)
await ctrl.close()
```

## Why agent-ctrl

- Native Rust binary - instant command parsing, zero scripting overhead
- Accessibility-tree-based - stable across themes, locales, resolutions
- Compact text output uses fewer tokens than JSON or DOM
- Refs let agents target elements deterministically without re-querying
- Same schema across UIA (Windows), AX (macOS), AT-SPI (Linux)
- Composes with [agent-browser](https://github.com/vercel-labs/agent-browser)
  for web tasks (browser automation is intentionally out of scope)

## Diagnostics

```bash
agent-ctrl doctor          # check install, daemon state, run a probe
agent-ctrl doctor --fix    # apply safe auto-repairs (e.g. stale session files)
agent-ctrl list            # list active daemon sessions
```

## Reference

- Repo: https://github.com/k4cper-g/agent-ctrl
- npm (CLI): https://www.npmjs.com/package/@agent-ctrl/cli
- License: Apache-2.0
