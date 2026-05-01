# AGENTS.md

Instructions for AI coding agents working on this codebase.

## Project shape

`agent-ctrl` is a Rust workspace. The user-facing binary is `agent-ctrl` (in `crates/cli`). Library crates live in `crates/core`, `crates/daemon`, and `crates/surface-*`. Each `surface-*` crate implements the `Surface` trait from `agent-ctrl-core` for one platform.

Reference repo: `agent-browser/` at the workspace root is a clone of [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser) used as a structural template. It is **not** a dependency and is excluded from the build (`.gitignore`).

## Conventions

- **Rust edition 2021**, MSRV 1.85.
- Workspace lints in `Cargo.toml` are authoritative - `clippy::pedantic` and `clippy::nursery` are warnings. Do not silence with `#[allow(...)]` unless you have a concrete reason and add a one-line comment explaining why.
- `unsafe_code = "warn"` workspace-wide. Crates that genuinely need it (FFI surfaces) opt in per-crate with `#![allow(unsafe_code)]` at `lib.rs`.
- All public items must have rustdoc comments (`missing_docs = "warn"`).
- `unwrap()` / `expect()` are warnings - use `?` or pattern matching instead. Tests may use them freely.
- CLI flags use kebab-case (`--allow-file-access`, never `--allowFileAccess`).
- No emojis in code, output, or docs. Unicode symbols (✓ ✗ → ⚠) are acceptable in CLI output.
- No em-dashes (the U+2014 character) anywhere in the project's text - code comments, rustdoc, README, CHANGELOG, examples, CLI output. Use ASCII hyphens (`-`) or restructure the sentence.
- Format: `cargo fmt --all`. Lint: `cargo clippy --workspace --all-targets`.

## Architecture rules

- **The `Surface` trait is the contract.** Anything platform-specific lives behind it. The daemon, CLI, and any consumer code must talk only to `Surface` and the types in `agent-ctrl-core`.
- **Schema is single-snapshot stable.** A `RefId` is only valid for the snapshot that produced it. Surfaces must rediscover the underlying native element at action time via `RefMap` lookup (role + name + nth + optional native handle).
- **Surfaces are target-gated.** `surface-uia` only compiles on `target_os = "windows"`, etc. The crate must build (as an empty crate) on other platforms so the workspace stays cross-host.
- **Capabilities are negotiated, not assumed.** Each surface exposes a `CapabilitySet`. Callers must check `supports(...)` before invoking optional actions.

## Adding a feature

When adding or changing user-facing behavior (a new action, flag, schema field, etc.) update **all** of:

1. The relevant types in `crates/core/src/`
2. The `Surface` trait if it is a new operation
3. Every `surface-*` implementation (return `Error::Unsupported` if the platform can't do it)
4. The CLI entrypoint in `crates/cli/src/`
5. The README options table or architecture section
6. Inline rustdoc comments on the new public items

## Testing

```bash
cargo check --workspace                           # fast type-check
cargo build --workspace                           # full build
cargo test --workspace                            # run all tests
cargo clippy --workspace --all-targets -- -D warnings   # lint, fail on warnings
cargo fmt --all -- --check                        # format check
```

## License

Apache-2.0. New files inherit it via the workspace `[workspace.package]` license field - no per-file headers required.
