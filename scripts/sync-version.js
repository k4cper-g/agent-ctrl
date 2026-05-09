#!/usr/bin/env node

/**
 * Syncs the version from package.json to Cargo.toml's [workspace.package].
 * Run before building or releasing so the npm package and Rust binary
 * report the same version.
 */

import { execSync } from "child_process"
import { readFileSync, writeFileSync } from "fs"
import { dirname, join } from "path"
import { fileURLToPath } from "url"

const __dirname = dirname(fileURLToPath(import.meta.url))
const rootDir = join(__dirname, "..")

const packageJson = JSON.parse(
  readFileSync(join(rootDir, "package.json"), "utf-8"),
)
const version = packageJson.version

console.log(`Syncing version ${version} ...`)

const cargoTomlPath = join(rootDir, "Cargo.toml")
let cargoToml = readFileSync(cargoTomlPath, "utf-8")

// Match the first line beginning with `version = "..."` — this is the
// [workspace.package] version. Inter-crate path deps include `version` mid-line
// (`{ path = "...", version = "..." }`) and are not affected by `^version`.
const cargoVersionRegex = /^version\s*=\s*"[^"]*"/m
const newCargoVersion = `version = "${version}"`

let cargoTomlUpdated = false
const oldMatch = cargoToml.match(cargoVersionRegex)?.[0]

if (!oldMatch) {
  console.error("  Could not find [workspace.package] version field in Cargo.toml")
  process.exit(1)
}

if (oldMatch !== newCargoVersion) {
  cargoToml = cargoToml.replace(cargoVersionRegex, newCargoVersion)
  writeFileSync(cargoTomlPath, cargoToml)
  console.log(`  Updated Cargo.toml: ${oldMatch} -> ${newCargoVersion}`)
  cargoTomlUpdated = true
} else {
  console.log("  Cargo.toml already up to date")
}

// Update Cargo.lock to match. Best-effort — if cargo isn't on PATH or the
// network is unavailable, surface the error but don't fail version:sync.
if (cargoTomlUpdated) {
  try {
    execSync("cargo update --workspace --offline", {
      cwd: rootDir,
      stdio: "pipe",
    })
    console.log("  Updated Cargo.lock (offline)")
  } catch {
    try {
      execSync("cargo update --workspace", {
        cwd: rootDir,
        stdio: "pipe",
      })
      console.log("  Updated Cargo.lock")
    } catch (e) {
      console.warn(`  Warning: Could not update Cargo.lock: ${e.message}`)
    }
  }
}

console.log("Version sync complete.")
