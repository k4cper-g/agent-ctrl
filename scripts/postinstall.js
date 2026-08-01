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
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from "fs"
import { dirname, join } from "path"
import { fileURLToPath } from "url"
import { platform, arch } from "os"
import { execSync } from "child_process"
import { createHash } from "crypto"
import { get } from "https"

const __dirname = dirname(fileURLToPath(import.meta.url))
const projectRoot = join(__dirname, "..")
const binDir = join(projectRoot, "bin")

const GITHUB_REPO = "k4cper-g/agent-ctrl"

const packageJson = JSON.parse(readFileSync(join(projectRoot, "package.json"), "utf8"))
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

function downloadFile(url, dest, redirectsLeft = 5) {
  return new Promise((resolve, reject) => {
    get(url, (res) => {
      if ([301, 302, 307, 308].includes(res.statusCode ?? 0)) {
        res.resume()
        if (!res.headers.location || redirectsLeft === 0) {
          reject(new Error(`invalid redirect for ${url}`))
          return
        }
        downloadFile(res.headers.location, dest, redirectsLeft - 1).then(resolve, reject)
        return
      }
      if (res.statusCode !== 200) {
        res.resume()
        reject(new Error(`HTTP ${res.statusCode} for ${url}`))
        return
      }

      const file = createWriteStream(dest, { flags: "wx", mode: 0o600 })
      const fail = (err) => {
        file.destroy()
        try {
          unlinkSync(dest)
        } catch {}
        reject(err)
      }
      res.on("error", fail)
      file.on("error", fail)
      file.on("finish", () => file.close(resolve))
      res.pipe(file)
    }).on("error", reject)
  })
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex")
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
  const tempBinaryPath = `${binaryPath}.${process.pid}.download`
  const checksumPath = `${tempBinaryPath}.sha256`

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
    await downloadFile(url, tempBinaryPath)
    await downloadFile(`${url}.sha256`, checksumPath)
    const checksumText = readFileSync(checksumPath, "utf8").trim()
    const expected = checksumText.match(/^[a-fA-F0-9]{64}/)?.[0]?.toLowerCase()
    const actual = sha256(tempBinaryPath)
    if (!expected || actual !== expected) {
      throw new Error(`SHA-256 mismatch for ${binaryName}`)
    }
    renameSync(tempBinaryPath, binaryPath)
    unlinkSync(checksumPath)
    if (platform() !== "win32") {
      chmodSync(binaryPath, 0o755)
    }
    console.log(`agent-ctrl: ready (${binaryName}).`)
  } catch (err) {
    for (const path of [tempBinaryPath, checksumPath]) {
      try {
        unlinkSync(path)
      } catch {}
    }
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
