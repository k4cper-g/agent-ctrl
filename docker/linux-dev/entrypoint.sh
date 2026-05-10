#!/usr/bin/env bash
# Bring up a headless desktop accessibility stack, then exec the given command
# inside it (so AT-SPI clients in the command see a live registry):
#
#   - Xvfb on $DISPLAY (default :99): a virtual X server, so GTK apps can open
#     real windows that the AT-SPI registry can introspect.
#   - dbus-run-session: a private session bus for this invocation.
#   - at-spi-bus-launcher: claims org.a11y.Bus on the session bus and spawns the
#     AT-SPI registry on its own (the "a11y") bus. (org.a11y.Bus is D-Bus
#     auto-activatable, so a client connecting would start it anyway; we start
#     it eagerly and wait for it so the first client doesn't race.)
#
# Usage (the Dockerfile sets this as ENTRYPOINT):
#   docker run ... agent-ctrl-linux-dev <command...>
set -euo pipefail

export DISPLAY="${DISPLAY:-:99}"
export GDK_BACKEND="${GDK_BACKEND:-x11}"
# GTK4 enables its AT-SPI backend when an a11y bus is present; be explicit so
# the fixture always registers regardless of distro defaults.
export GTK_A11Y="${GTK_A11Y:-atspi}"
export NO_AT_BRIDGE=0
export GSETTINGS_BACKEND="${GSETTINGS_BACKEND:-memory}"

# --- virtual X server -------------------------------------------------------
Xvfb "$DISPLAY" -screen 0 1280x1024x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
xvfb_pid=$!
for _ in $(seq 1 100); do
  xdpyinfo -display "$DISPLAY" >/dev/null 2>&1 && break
  sleep 0.05
done
if ! xdpyinfo -display "$DISPLAY" >/dev/null 2>&1; then
  echo "entrypoint: Xvfb did not come up on $DISPLAY" >&2
  cat /tmp/xvfb.log >&2 || true
  exit 1
fi
trap 'kill "$xvfb_pid" 2>/dev/null || true' EXIT

# Locate the AT-SPI bus launcher (path differs across distros / versions).
atspi_launcher=""
for candidate in \
    "$(command -v at-spi-bus-launcher 2>/dev/null || true)" \
    /usr/libexec/at-spi-bus-launcher \
    /usr/lib/at-spi2-core/at-spi-bus-launcher \
    /usr/libexec/at-spi2/at-spi-bus-launcher; do
  if [ -n "$candidate" ] && [ -x "$candidate" ]; then atspi_launcher="$candidate"; break; fi
done

# --- session bus + AT-SPI registry, then the user's command -----------------
# Everything from here runs under a fresh session D-Bus.
exec dbus-run-session -- bash -c '
  set -euo pipefail
  launcher="$1"; shift
  if [ -n "$launcher" ]; then
    "$launcher" --launch-immediately >/tmp/at-spi-bus-launcher.log 2>&1 &
    atspi_pid=$!
    trap "kill $atspi_pid 2>/dev/null || true" EXIT
  fi
  # Wait until org.a11y.Bus answers (either our eager launcher or D-Bus
  # auto-activation). GetAddress returning the registry address means the
  # a11y bus is up.
  for _ in $(seq 1 100); do
    if gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
         --method org.a11y.Bus.GetAddress >/dev/null 2>&1; then
      break
    fi
    sleep 0.05
  done
  if ! gdbus call --session --dest org.a11y.Bus --object-path /org/a11y/bus \
       --method org.a11y.Bus.GetAddress >/dev/null 2>&1; then
    echo "entrypoint: AT-SPI bus (org.a11y.Bus) did not become available" >&2
    cat /tmp/at-spi-bus-launcher.log >&2 || true
    exit 1
  fi
  exec "$@"
' bash "$atspi_launcher" "$@"
