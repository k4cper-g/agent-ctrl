#!/usr/bin/env node

/**
 * Postinstall for agent-ctrl.
 *
 * The npm tarball does NOT bundle native binaries. On install we detect the
 * platform/arch, download the matching prebuilt binary from this version's
 * GitHub Release, drop it into bin/, chmod it on POSIX, and (for Windows
 * global installs) rewrite npm's .cmd/.ps1 shims so the .exe runs directly
 * with zero Node overhead.
 *
 * Supported targets in v0.1.x: Windows x64, macOS arm64, macOS x64.
 *
 * The script must never throw - postinstall failures break installs.
 */

import {
  chmodSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  unlinkSync,
  writeFileSync,
} from "fs"
import { dirname, join } from "path"
import { fileURLToPath } from "url"
import { platform, arch } from "os"
import { execSync } from "child_process"
import { get } from "https"

const __dirname = dirname(fileURLToPath(import.meta.url))
const projectRoot = join(__dirname, "..")
const binDir = join(projectRoot, "bin")

const GITHUB_REPO = "k4cper-g/agent-ctrl"

const packageJson = JSON.parse(
  await import("fs").then((m) =>
    m.readFileSync(join(projectRoot, "package.json"), "utf8"),
  ),
)
const version = packageJson.version

function getBinaryName() {
  const os = platform()
  const cpuArch = arch()

  if (os === "win32" && cpuArch === "x64") return "agent-ctrl-win32-x64.exe"
  if (os === "darwin" && cpuArch === "arm64") return "agent-ctrl-darwin-arm64"
  if (os === "darwin" && cpuArch === "x64") return "agent-ctrl-darwin-x64"

  return null
}

function noticeUnsupported() {
  console.log("")
  console.log("agent-ctrl: this platform is not supported in v0.1.x.")
  console.log(
    `  Detected: ${platform()}-${arch()}.`,
  )
  console.log(
    "  v0.1.x supports Windows x64, macOS arm64, and macOS x64.",
  )
  console.log("  Linux/iOS/Android are on the roadmap.")
  console.log("")
}

function downloadFile(url, dest) {
  return new Promise((resolve, reject) => {
    const file = createWriteStream(dest)
    const request = (currentUrl) => {
      get(currentUrl, (res) => {
        if (res.statusCode === 301 || res.statusCode === 302 || res.statusCode === 307) {
          request(res.headers.location)
          return
        }
        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode} for ${currentUrl}`))
          return
        }
        res.pipe(file)
        file.on("finish", () => file.close(() => resolve()))
      }).on("error", (err) => {
        try {
          unlinkSync(dest)
        } catch {}
        reject(err)
      })
    }
    request(url)
  })
}

async function main() {
  const binaryName = getBinaryName()

  if (!binaryName) {
    noticeUnsupported()
    return
  }

  if (!existsSync(binDir)) {
    mkdirSync(binDir, { recursive: true })
  }

  const binaryPath = join(binDir, binaryName)

  if (existsSync(binaryPath)) {
    if (platform() !== "win32") {
      try {
        chmodSync(binaryPath, 0o755)
      } catch {}
    }
    console.log(`agent-ctrl: native binary already present (${binaryName}).`)
    if (platform() === "win32") await fixWindowsShims(binaryName)
    return
  }

  const url = `https://github.com/${GITHUB_REPO}/releases/download/v${version}/${binaryName}`
  console.log(`agent-ctrl: downloading ${binaryName} for v${version} ...`)
  console.log(`  ${url}`)

  try {
    await downloadFile(url, binaryPath)
    if (platform() !== "win32") {
      chmodSync(binaryPath, 0o755)
    }
    console.log(`agent-ctrl: ready (${binaryName}).`)
  } catch (err) {
    console.warn(
      `agent-ctrl: could not download native binary (${err.message}).`,
    )
    console.warn(
      `  Install will still complete; running 'agent-ctrl' will fail until you`,
    )
    console.warn(
      `  reinstall or run 'npm run build:native' from a source checkout.`,
    )
    return
  }

  if (platform() === "win32") {
    await fixWindowsShims(binaryName)
  }
}

/**
 * On global installs (npm i -g), npm generates .cmd/.ps1 shims that go through
 * /bin/sh - broken on Windows. Overwrite them to invoke the .exe directly.
 */
async function fixWindowsShims(binaryName) {
  let npmBinDir
  try {
    npmBinDir = execSync("npm prefix -g", { encoding: "utf8" }).trim()
  } catch {
    return
  }

  const cmdShim = join(npmBinDir, "agent-ctrl.cmd")
  const ps1Shim = join(npmBinDir, "agent-ctrl.ps1")

  if (!existsSync(cmdShim)) {
    return
  }

  const relBinary = `node_modules\\@agent-ctrl\\cli\\bin\\${binaryName}`
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
  console.warn(`agent-ctrl postinstall warning: ${err.message}`)
})
