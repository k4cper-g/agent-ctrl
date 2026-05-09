#!/usr/bin/env node

/**
 * Copies the cargo-built agent-ctrl binary into bin/ with platform-specific
 * naming so npm publish can ship it.
 *
 * The Cargo workspace target dir is at the repo root (target/release/), not
 * inside crates/cli/target/.
 */

import { copyFileSync, existsSync, mkdirSync } from "fs"
import { dirname, join } from "path"
import { fileURLToPath } from "url"
import { platform, arch } from "os"

const __dirname = dirname(fileURLToPath(import.meta.url))
const rootDir = join(__dirname, "..")

const ext = platform() === "win32" ? ".exe" : ""
const sourcePath = join(rootDir, "target", "release", `agent-ctrl${ext}`)
const binDir = join(rootDir, "bin")
const targetName = `agent-ctrl-${platform()}-${arch()}${ext}`
const targetPath = join(binDir, targetName)

if (!existsSync(sourcePath)) {
  console.error(`Error: cargo binary not found at ${sourcePath}`)
  console.error("Run: cargo build --release -p agent-ctrl-cli")
  process.exit(1)
}

if (!existsSync(binDir)) {
  mkdirSync(binDir, { recursive: true })
}

copyFileSync(sourcePath, targetPath)
console.log(`Copied native binary -> ${targetPath}`)
