#!/usr/bin/env node

/**
 * Cross-platform CLI wrapper for agent-ctrl.
 *
 * The wrapper detects platform/arch and spawns the matching native binary
 * shipped by postinstall (`agent-ctrl-<os>-<arch>[.exe]` in this directory).
 * It enables `npx @agent-ctrl/cli` on Windows where shell scripts don't work
 * and lets the native binary run directly when postinstall has patched the
 * global shims.
 *
 * Supported in v0.1.x: Windows x64, macOS arm64, macOS x64.
 * Linux is on the roadmap but not in this release.
 */

import { spawn } from "child_process"
import { existsSync } from "fs"
import { dirname, join } from "path"
import { fileURLToPath } from "url"
import { platform, arch } from "os"

const __dirname = dirname(fileURLToPath(import.meta.url))

function getBinaryName() {
  const os = platform()
  const cpuArch = arch()

  if (os === "win32" && cpuArch === "x64") return "agent-ctrl-win32-x64.exe"
  if (os === "darwin" && cpuArch === "arm64") return "agent-ctrl-darwin-arm64"
  if (os === "darwin" && cpuArch === "x64") return "agent-ctrl-darwin-x64"

  return null
}

function main() {
  const binaryName = getBinaryName()

  if (!binaryName) {
    console.error(
      `agent-ctrl: ${platform()}-${arch()} is not yet supported.`,
    )
    console.error(
      "v0.1.x supports Windows x64, macOS arm64, and macOS x64. Linux is",
    )
    console.error(
      "on the roadmap; see https://github.com/k4cper-g/agent-ctrl",
    )
    process.exit(1)
  }

  const binaryPath = join(__dirname, binaryName)

  if (!existsSync(binaryPath)) {
    console.error(`agent-ctrl: native binary missing at ${binaryPath}.`)
    console.error("")
    console.error("Reinstall the package, or build from source:")
    console.error("  git clone https://github.com/k4cper-g/agent-ctrl")
    console.error("  cd agent-ctrl && npm run build:native")
    process.exit(1)
  }

  const child = spawn(binaryPath, process.argv.slice(2), {
    stdio: "inherit",
    windowsHide: false,
  })

  child.on("error", (err) => {
    console.error(`agent-ctrl: failed to launch binary: ${err.message}`)
    process.exit(1)
  })

  child.on("close", (code) => {
    process.exit(code ?? 0)
  })
}

main()
