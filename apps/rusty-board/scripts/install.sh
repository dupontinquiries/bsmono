#!/usr/bin/env bash
# ============================================================
# RustyBoard - install.sh
#
# Builds the project (release), installs the binary, resources,
# desktop file, and (optionally) systemd user service.
#
# Run:    ./scripts/install.sh
# Uninstall:  ./scripts/uninstall.sh
# ============================================================
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# Default install prefix: ~/.local for user installs (no sudo), /usr/local
# if you pass --system.
PREFIX="${PREFIX:-$HOME/.local}"
SYSTEM_INSTALL=0
SKIP_BUILD=0
WITH_MODEL=0
MODEL="ggml-base.en.bin"

usage() {
    cat <<EOF
RustyBoard installer

Usage: $0 [options]

Options:
  --prefix DIR     Install prefix (default: \$HOME/.local)
  --system         Install to /usr/local (requires sudo)
  --skip-build     Skip cargo build (use existing target/release/)
  --with-model     Download a Whisper model after install
  --model NAME     Whisper model to download (default: ggml-base.en.bin)
                     options: tiny.en, base.en, small.en, medium.en, large
  -h, --help       Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)       PREFIX="$2"; shift 2 ;;
        --system)       SYSTEM_INSTALL=1; PREFIX="/usr/local"; shift ;;
        --skip-build)   SKIP_BUILD=1; shift ;;
        --with-model)   WITH_MODEL=1; shift ;;
        --model)        MODEL="$2"; shift 2 ;;
        -h|--help)      usage; exit 0 ;;
        *)              echo "Unknown option: $1" >&2; usage; exit 2 ;;
    esac
done

SUDO=""
if [[ "$SYSTEM_INSTALL" -eq 1 ]]; then
    if [[ "$EUID" -ne 0 ]]; then
        SUDO="sudo"
    fi
fi

BIN_DIR="$PREFIX/bin"
SHARE_DIR="$PREFIX/share/rusty-board"
APPLICATIONS_DIR="$PREFIX/share/applications"
SYSTEMD_USER_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
ICONS_DIR="$PREFIX/share/icons/hicolor/scalable/apps"

echo "==> RustyBoard install"
echo "    prefix:       $PREFIX"
echo "    project root: $PROJECT_ROOT"
echo "    system:       $SYSTEM_INSTALL"
echo

# 1. Check toolchain
if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not found. Install Rust from https://rustup.rs/" >&2
    exit 1
fi

# 2. Check system dependencies (best effort)
echo "==> Checking system dependencies..."
MISSING=()
for pkg in pkg-config; do
    if ! command -v "$pkg" >/dev/null 2>&1; then
        MISSING+=("$pkg")
    fi
done
if ! pkg-config --exists gtk4 2>/dev/null; then
    MISSING+=("libgtk-4-dev (or gtk4-devel)")
fi
if ! pkg-config --exists libadwaita-1 2>/dev/null; then
    MISSING+=("libadwaita-1-dev (or libadwaita-devel)")
fi
if ! pkg-config --exists xkbcommon 2>/dev/null; then
    MISSING+=("libxkbcommon-dev (or libxkbcommon-devel) — keymap synthesis for Wayland virtual keyboard")
fi
if ! pkg-config --exists gtk4-layer-shell-0 2>/dev/null; then
    MISSING+=("libgtk4-layer-shell-dev (or gtk4-layer-shell-devel) — needed for Wayland docking")
fi
if [[ ${#MISSING[@]} -gt 0 ]]; then
    echo "WARNING: missing dependencies:"
    for m in "${MISSING[@]}"; do echo "  - $m"; done
    echo
    echo "On Debian/Ubuntu:"
    echo "    sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libgtk4-layer-shell-dev libxkbcommon-dev libasound2-dev"
    echo "On Fedora:"
    echo "    sudo dnf install gcc pkgconf-pkg-config gtk4-devel libadwaita-devel gtk4-layer-shell-devel libxkbcommon-devel alsa-lib-devel"
    echo "On Arch:"
    echo "    sudo pacman -S base-devel pkgconf gtk4 libadwaita gtk4-layer-shell libxkbcommon alsa-lib"
    echo "On Alpine (postmarketOS):"
    echo "    sudo apk add build-base pkgconf gtk4.0-dev libadwaita-dev gtk4-layer-shell-dev libxkbcommon-dev alsa-lib-dev pango-dev gdk-pixbuf-dev"
    echo
    read -r -p "Continue anyway? [y/N] " yn
    case "$yn" in [Yy]*) ;; *) exit 1 ;; esac
fi

# 3. Build
if [[ "$SKIP_BUILD" -eq 0 ]]; then
    echo "==> Building (release)..."
    cd "$PROJECT_ROOT"
    cargo build --release
fi

BIN_SRC="$PROJECT_ROOT/target/release/rusty-board"
BIN_CFG="$PROJECT_ROOT/target/release/rusty-board-config"

if [[ ! -x "$BIN_SRC" ]]; then
    echo "ERROR: $BIN_SRC not found. Did the build fail?" >&2
    exit 1
fi

# 4. Install binaries and resources
echo "==> Installing files..."
$SUDO install -d "$BIN_DIR" "$SHARE_DIR" "$APPLICATIONS_DIR" "$ICONS_DIR"
$SUDO install -m 0755 "$BIN_SRC" "$BIN_DIR/rusty-board"
if [[ -x "$BIN_CFG" ]]; then
    $SUDO install -m 0755 "$BIN_CFG" "$BIN_DIR/rusty-board-config"
fi

$SUDO cp -R "$PROJECT_ROOT/resources/." "$SHARE_DIR/"

# 5. Desktop entry — MUST include `X-KDE-Wayland-VirtualKeyboard=true`
# so KWin recognizes us as a virtual keyboard. Without that line, this
# .desktop file silently shadows any other rusty-board .desktop in
# /usr/share/applications/ (XDG user-level shadows system-level), and
# the keyboard disappears from SystemSettings → Virtual Keyboard.
# Past regression: this template lacked the magic line and broke
# registration after a re-install.
DESKTOP_FILE="$APPLICATIONS_DIR/io.rustyboard.Keyboard.desktop"
$SUDO tee "$DESKTOP_FILE" >/dev/null <<EOF
[Desktop Entry]
Type=Application
Name=RustyBoard
GenericName=On-screen Keyboard
Comment=Modern Rust-based on-screen keyboard with swipe and voice input
Exec=$BIN_DIR/rusty-board
Icon=io.rustyboard.Keyboard
Terminal=false
NoDisplay=true
Categories=Utility;Accessibility;
Keywords=keyboard;input;swipe;voice;
X-KDE-Wayland-VirtualKeyboard=true
StartupNotify=true
StartupWMClass=io.rustyboard.Keyboard
EOF

# Path-divergence warning: redeploy.sh installs to /usr/local/bin
# unconditionally. If the user just installed to ~/.local/bin (the
# default user-install prefix), then a future redeploy will overwrite
# the wrong path and break the registration.
if [[ "$BIN_DIR" != "/usr/local/bin" ]]; then
    echo
    echo "NOTE: This install put the binary at $BIN_DIR/rusty-board"
    echo "      but redeploy.sh always installs to /usr/local/bin/rusty-board."
    echo "      To keep them aligned, either:"
    echo "        - Re-run with --system:  ./scripts/install.sh --system"
    echo "        - Or remember to manually update the .desktop file's"
    echo "          Exec= line after running redeploy.sh."
fi

# 6. Optional icon (placeholder svg if real icon not present)
ICON_SRC="$PROJECT_ROOT/resources/icons/io.rustyboard.Keyboard.svg"
ICON_DST="$ICONS_DIR/io.rustyboard.Keyboard.svg"
if [[ -f "$ICON_SRC" ]]; then
    $SUDO install -m 0644 "$ICON_SRC" "$ICON_DST"
fi

# 7. Systemd user service (only for user installs; skip for system installs).
if [[ "$SYSTEM_INSTALL" -eq 0 ]]; then
    echo "==> Installing systemd user service..."
    install -d "$SYSTEMD_USER_DIR"
    sed "s|@BIN_DIR@|$BIN_DIR|g" \
        "$PROJECT_ROOT/systemd/rusty-board.service" \
        > "$SYSTEMD_USER_DIR/rusty-board.service"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload || true
        echo "    Enable autostart with:"
        echo "        systemctl --user enable --now rusty-board.service"
    fi
fi

# 8. Whisper model download (optional)
if [[ "$WITH_MODEL" -eq 1 ]]; then
    echo "==> Downloading Whisper model: $MODEL"
    MODEL_DIR="$SHARE_DIR/models"
    $SUDO install -d "$MODEL_DIR"
    BASE_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"
    URL="$BASE_URL/${MODEL}"
    if command -v curl >/dev/null 2>&1; then
        $SUDO curl -L --fail -o "$MODEL_DIR/$MODEL" "$URL"
    elif command -v wget >/dev/null 2>&1; then
        $SUDO wget -O "$MODEL_DIR/$MODEL" "$URL"
    else
        echo "ERROR: neither curl nor wget available; cannot fetch model." >&2
        echo "       Manually place the model at: $MODEL_DIR/$MODEL" >&2
        exit 1
    fi
fi

# 9. Update caches (best-effort; ignore failures)
update-desktop-database "$APPLICATIONS_DIR" >/dev/null 2>&1 || true
gtk-update-icon-cache -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true

echo
echo "==> Install complete."
echo
echo "Run with:    rusty-board"
echo "Edit config: rusty-board-config show"
echo "Config path: $(rusty-board-config path 2>/dev/null || echo "$HOME/.config/rusty-board/config.toml")"
if [[ "$SYSTEM_INSTALL" -eq 0 ]]; then
    case ":$PATH:" in
        *":$BIN_DIR:"*)  ;;
        *) echo
           echo "NOTE: $BIN_DIR is not in your PATH."
           echo "      Add this to your shell rc:"
           echo "          export PATH=\"$BIN_DIR:\$PATH\""
           ;;
    esac
fi
