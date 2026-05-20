# Linux dev / CI environment

A container with a headless desktop accessibility stack (Xvfb + a private
session D-Bus + the AT-SPI registry), the Rust toolchain, and GTK4, for
developing and testing the `surface-atspi` (Linux) implementation from any host
- including Windows/macOS, where AT-SPI itself doesn't exist.

## Build

```bash
docker build -t agent-ctrl-linux-dev docker/linux-dev/
```

## Run

Bind-mount the repo at `/work`; keep the cargo target dir and registry on
Docker volumes so a Linux build doesn't clobber a Windows/macOS `target/` and
dep compiles are cached across runs:

```bash
docker run --rm -it \
  -v "$PWD:/work" -w /work \
  -v agent-ctrl-cargo-target:/cargo-target -e CARGO_TARGET_DIR=/cargo-target \
  -v agent-ctrl-cargo-home:/cargo-home -e CARGO_HOME=/cargo-home \
  agent-ctrl-linux-dev <command...>
```

On a Windows host (Git Bash / MSYS), prefix with `MSYS_NO_PATHCONV=1` so the
`/work` paths aren't mangled, and use a `C:/...` style mount source:

```bash
MSYS_NO_PATHCONV=1 docker run --rm -it \
  -v "C:/path/to/agent-ctrl:/work" -w /work \
  -v agent-ctrl-cargo-target:/cargo-target -e CARGO_TARGET_DIR=/cargo-target \
  -v agent-ctrl-cargo-home:/cargo-home -e CARGO_HOME=/cargo-home \
  agent-ctrl-linux-dev <command...>
```

The entrypoint brings the a11y stack up and then `exec`s your command inside
it, so AT-SPI clients (the daemon, the integration test) see a live registry.

Examples:

```bash
# build + check, like CI
... agent-ctrl-linux-dev cargo clippy --workspace --all-targets -- -D warnings
... agent-ctrl-linux-dev cargo test --workspace

# the AT-SPI fixture integration test (opt-in, like RUN_UIA_TESTS)
... agent-ctrl-linux-dev bash -c 'RUN_ATSPI_TESTS=1 cargo test -p agent-ctrl-cli --test linux_atspi_fixture'

# poke at the live tree manually (the fixture is a GTK4 app under crates/atspi-fixture/)
... agent-ctrl-linux-dev bash -c '
  python3 crates/atspi-fixture/main.py --ready-file /tmp/fx.ready --auto-close-ms 30000 &
  until [ -f /tmp/fx.ready ]; do sleep 0.1; done
  cargo run -q -p agent-ctrl-cli -- open atspi
  cargo run -q -p agent-ctrl-cli -- snapshot --target-process agent-ctrl-atspi-fixture
'
```

## Why this shape

- **A deterministic fixture**, not a distro app. `crates/atspi-fixture/main.py`
  is a small GTK4 app with stable widgets - the AT-SPI analog of
  `agent-ctrl-uia-fixture` / `agent-ctrl-ax-fixture`. It's Python (GTK-rs would
  bloat the Rust workspace) and runs in this container.
- **Headless.** No display manager, no real desktop session: `Xvfb` provides a
  virtual X server, `dbus-run-session` a private session bus, and
  `at-spi-bus-launcher` the AT-SPI registry on it. Same recipe GTK's own CI
  uses.
- **CI-ready.** The headless stack is plain `apt` packages plus the
  `entrypoint.sh` preamble, so a CI job can run the same `RUN_ATSPI_TESTS=1`
  fixture test inline without building the full image. Today the test is
  opt-in, like the Windows UIA (`RUN_UIA_TESTS`) and macOS AX (`RUN_AX_TESTS`)
  fixture tests; wiring an `ubuntu-latest` AT-SPI job into
  `.github/workflows/ci.yml` is a follow-up.
