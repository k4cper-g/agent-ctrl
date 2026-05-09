#!/usr/bin/env bash
# install-macos.sh - download and install the latest agent-ctrl release
# binary for macOS. Mirrors scripts/install-windows.ps1.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/k4cper-g/agent-ctrl/main/scripts/install-macos.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/k4cper-g/agent-ctrl/main/scripts/install-macos.sh | bash -s -- --version v0.2.0 --no-path
#
# Flags:
#   --version <tag>   Install a specific tag (default: latest).
#   --install-dir <p> Where to copy `agent-ctrl` (default: ~/.local/bin).
#   --no-path         Skip the PATH-update reminder.

set -euo pipefail

REPO="k4cper-g/agent-ctrl"
VERSION="latest"
INSTALL_DIR="$HOME/.local/bin"
NO_PATH=0

while (( "$#" )); do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --install-dir) INSTALL_DIR="$2"; shift 2 ;;
        --no-path) NO_PATH=1; shift ;;
        -h|--help)
            grep '^#' "$0" | sed 's/^# \?//'
            exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "install-macos.sh: this installer only runs on macOS (got $(uname -s))" >&2
    exit 1
fi

ARCH="$(uname -m)"
case "$ARCH" in
    arm64|aarch64) RUST_TARGET="aarch64-apple-darwin" ;;
    x86_64) RUST_TARGET="x86_64-apple-darwin" ;;
    *) echo "install-macos.sh: unsupported macOS arch: $ARCH" >&2; exit 1 ;;
esac

if [[ "$VERSION" == "latest" ]]; then
    RELEASE_URL="https://api.github.com/repos/$REPO/releases/latest"
else
    TAG="${VERSION#v}"
    RELEASE_URL="https://api.github.com/repos/$REPO/releases/tags/v${TAG}"
fi

echo "==> Resolving release: $RELEASE_URL"
ASSET_PATTERN="${RUST_TARGET}.tar.gz"

# Pull the matching asset's browser_download_url. We don't depend on jq;
# the released asset names are stable enough to grep for.
ASSET_URL="$(curl -fsSL "$RELEASE_URL" \
    | grep -oE '"browser_download_url": *"[^"]+"' \
    | sed -E 's/.*"browser_download_url": *"([^"]+)".*/\1/' \
    | grep -F "$ASSET_PATTERN" \
    | head -1)"

if [[ -z "$ASSET_URL" ]]; then
    echo "install-macos.sh: no release asset matching *${ASSET_PATTERN} found" >&2
    exit 1
fi

TMP="$(mktemp -d -t agent-ctrl-install)"
trap 'rm -rf "$TMP"' EXIT

echo "==> Downloading $(basename "$ASSET_URL")"
curl -fsSL "$ASSET_URL" -o "$TMP/agent-ctrl.tar.gz"

echo "==> Extracting"
tar -xzf "$TMP/agent-ctrl.tar.gz" -C "$TMP"

BINARY="$(find "$TMP" -type f -name 'agent-ctrl' -perm -u+x | head -1)"
if [[ -z "$BINARY" ]]; then
    echo "install-macos.sh: tarball did not contain a runnable agent-ctrl binary" >&2
    exit 1
fi

mkdir -p "$INSTALL_DIR"
cp "$BINARY" "$INSTALL_DIR/agent-ctrl"
chmod +x "$INSTALL_DIR/agent-ctrl"

echo "==> Verifying"
"$INSTALL_DIR/agent-ctrl" info

echo
echo "Installed agent-ctrl to $INSTALL_DIR/agent-ctrl"

if [[ "$NO_PATH" -eq 0 ]]; then
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) ;;
        *)
            echo
            echo "Add $INSTALL_DIR to your PATH:"
            echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.zshrc"
            echo "  source ~/.zshrc"
            ;;
    esac
fi

echo
echo "Next steps:"
echo "  1. Grant Accessibility permission in System Settings > Privacy & Security > Accessibility (add the binary at $INSTALL_DIR/agent-ctrl)."
echo "  2. (Optional) Grant Screen Recording for the screenshot verb in the same pane."
echo "  3. Run \`agent-ctrl doctor\` to verify the install."
