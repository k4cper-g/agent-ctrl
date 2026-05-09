#!/usr/bin/env node

/**
 * Cross-platform CLI wrapper for agent-ctrl.
 *
 * For v0.1.x, only Windows x64 is supported. The wrapper enables npx on
 * Windows where shell scripts don't work; on global installs, postinstall.js
 * patches npm's shims to invoke the native binary directly with zero overhead.
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

  if (os !== "win32") return null
  if (cpuArch !== "x64") return null

  return "agent-ctrl-win32-x64.exe"
}

function main() {
  const binaryName = getBinaryName()

  if (!binaryName) {
    console.error(
      `agent-ctrl: ${platform()}-${arch()} is not yet supported.`,
    )
    console.error(
      "v0.1.x supports Windows x64 only. macOS and Linux are on the roadmap;",
    )
    console.error(
      "see https://github.com/k4cper-g/agent-ctrl for current status.",
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
