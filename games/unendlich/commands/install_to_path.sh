#!/usr/bin/env bash
#
# install_to_path.sh — symlink built unendlich binaries into ~/.local/bin/
# so commands like `unendlich-launcher` and `unendlich-analyze` work
# without explicit cargo/path prefixes.
#
# Usage:
#   ./commands/install_to_path.sh
#
# What it does:
#   1. Builds release binaries (cargo build --release -p ...).
#   2. Symlinks target/release/{unendlich-launcher,unendlich-analyze}
#      into ~/.local/bin/.
#   3. Confirms ~/.local/bin is in PATH and prints a reminder if not.
#
# After running this once, `unendlich-launcher run foo.unendtest --version @HEAD`
# works from anywhere on your system.
#
# Symlinks not copies: re-running `cargo build --release` after engine
# changes updates the binary in-place; the symlink follows.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR/.."

if ! git rev-parse --show-toplevel >/dev/null 2>&1; then
    echo "Error: not inside a git repository." >&2
    exit 1
fi
cd "$(git rev-parse --show-toplevel)"

REPO_ROOT="$(pwd)"
INSTALL_DIR="$HOME/.local/bin"
BINS=(
    "unendlich-launcher"
    "unendlich-analyze"
)

echo "==> Building release binaries..."
for b in "${BINS[@]}"; do
    cargo build --release -p "$b"
done

mkdir -p "$INSTALL_DIR"

echo
echo "==> Symlinking into $INSTALL_DIR/"
for b in "${BINS[@]}"; do
    src="$REPO_ROOT/target/release/$b"
    dst="$INSTALL_DIR/$b"
    if [ ! -f "$src" ]; then
        echo "  WARNING: $src missing (build may have failed)"
        continue
    fi
    # Remove any existing symlink or file in our way so re-running is idempotent.
    if [ -L "$dst" ] || [ -e "$dst" ]; then
        rm -f "$dst"
    fi
    ln -s "$src" "$dst"
    echo "  $b -> $src"
done

echo
echo "==> Done."
echo
case ":$PATH:" in
    *":$INSTALL_DIR:"*)
        echo "Verified: $INSTALL_DIR is on your PATH."
        echo "You can now run, e.g.,"
        echo "  unendlich-launcher run tests/specs/rd2_smoke.unendtest --version @HEAD"
        echo "from any directory."
        ;;
    *)
        echo "NOTE: $INSTALL_DIR is NOT currently in your PATH."
        echo "Add this to your shell rc (~/.zshrc, ~/.bashrc, etc.):"
        echo
        echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
        echo
        echo "Then restart your shell or run:"
        echo "    source ~/.zshrc   # or ~/.bashrc"
        ;;
esac
