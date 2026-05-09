#!/usr/bin/env node

/**
 * Postinstall for agent-ctrl.
 *
 * For v0.1.x, only Windows x64 is supported.
 *  - On Windows: verify the native binary is present and (for global installs)
 *    rewrite npm's .cmd/.ps1 shims to invoke the .exe directly with zero
 *    overhead.
 *  - On other platforms: print a friendly notice and exit cleanly so npm
 *    install does not fail.
 *
 * This script must never throw — postinstall failures break installs.
 */

import { existsSync, writeFileSync } from "fs"
import { dirname, join } from "path"
import { fileURLToPath } from "url"
import { platform, arch } from "os"
import { execSync } from "child_process"

const __dirname = dirname(fileURLToPath(import.meta.url))
const projectRoot = join(__dirname, "..")
const binDir = join(projectRoot, "bin")

function noticeUnsupported() {
  console.log("")
  console.log("agent-ctrl: this platform is not supported in v0.1.x.")
  console.log(
    `  Detected: ${platform()}-${arch()}. v0.1.x ships Windows x64 only.`,
  )
  console.log(
    "  macOS and Linux are on the roadmap. The CLI wrapper will exit",
  )
  console.log("  with a helpful message if you try to invoke agent-ctrl.")
  console.log("")
}

async function main() {
  if (platform() !== "win32" || arch() !== "x64") {
    noticeUnsupported()
    return
  }

  const binaryPath = join(binDir, "agent-ctrl-win32-x64.exe")
  if (!existsSync(binaryPath)) {
    console.log("agent-ctrl: native binary not present in package.")
    console.log("  Build from source with: npm run build:native")
    return
  }

  await fixWindowsShims()
}

/**
 * On global installs (npm i -g), npm generates .cmd/.ps1 shims that go through
 * /bin/sh — broken on Windows. Overwrite them to invoke the .exe directly.
 */
async function fixWindowsShims() {
  let npmBinDir
  try {
    npmBinDir = execSync("npm prefix -g", { encoding: "utf8" }).trim()
  } catch {
    return
  }

  const cmdShim = join(npmBinDir, "agent-ctrl.cmd")
  const ps1Shim = join(npmBinDir, "agent-ctrl.ps1")

  // npm creates shims AFTER lifecycle scripts run for some installer paths,
  // so a missing shim is not an error. The JS launcher handles all cases.
  if (!existsSync(cmdShim)) {
    return
  }

  const relBinary =
    "node_modules\\@agent-ctrl\\cli\\bin\\agent-ctrl-win32-x64.exe"
  const absBinary = join(npmBinDir, relBinary)
  if (!existsSync(absBinary)) {
    return
  }

  try {
    const cmdContent = `@ECHO off\r\n"%~dp0${relBinary}" %*\r\n`
    writeFileSync(cmdShim, cmdContent)

    const ps1Content =
      `#!/usr/bin/env pwsh\r\n` +
      `$basedir = Split-Path $MyInvocation.MyCommand.Definition -Parent\r\n` +
      `& "$basedir\\${relBinary}" $args\r\n` +
      `exit $LASTEXITCODE\r\n`
    writeFileSync(ps1Shim, ps1Content)

    console.log("agent-ctrl: shims patched for direct binary invocation.")
  } catch (err) {
    console.log(
      `agent-ctrl: could not optimize shims (${err.message}); JS wrapper still works.`,
    )
  }
}

main().catch((err) => {
  // Never fail npm install from postinstall.
  console.warn(`agent-ctrl postinstall warning: ${err.message}`)
})
