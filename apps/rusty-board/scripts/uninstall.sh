#!/usr/bin/env bash
# ============================================================
# RustyBoard - uninstall.sh
# Removes everything install.sh installed.
# ============================================================
set -euo pipefail

PREFIX="${PREFIX:-$HOME/.local}"
SYSTEM_INSTALL=0
KEEP_CONFIG=0
KEEP_MODELS=0

usage() {
    cat <<EOF
RustyBoard uninstaller

Usage: $0 [options]

Options:
  --prefix DIR     Install prefix used at install time (default: \$HOME/.local)
  --system         Uninstall from /usr/local (requires sudo)
  --keep-config    Keep user config in ~/.config/rusty-board
  --keep-models    Keep downloaded Whisper models
  -h, --help       Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)       PREFIX="$2"; shift 2 ;;
        --system)       SYSTEM_INSTALL=1; PREFIX="/usr/local"; shift ;;
        --keep-config)  KEEP_CONFIG=1; shift ;;
        --keep-models)  KEEP_MODELS=1; shift ;;
        -h|--help)      usage; exit 0 ;;
        *)              echo "Unknown option: $1" >&2; usage; exit 2 ;;
    esac
done

SUDO=""
if [[ "$SYSTEM_INSTALL" -eq 1 && "$EUID" -ne 0 ]]; then
    SUDO="sudo"
fi

BIN_DIR="$PREFIX/bin"
SHARE_DIR="$PREFIX/share/rusty-board"
APPLICATIONS_DIR="$PREFIX/share/applications"
ICONS_DIR="$PREFIX/share/icons/hicolor/scalable/apps"
SYSTEMD_USER_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"
USER_CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/rusty-board"

echo "==> RustyBoard uninstall"
echo "    prefix:       $PREFIX"
echo

# 1. Stop the systemd user service (best effort)
if command -v systemctl >/dev/null 2>&1; then
    if systemctl --user is-active rusty-board.service >/dev/null 2>&1; then
        echo "==> Stopping rusty-board.service..."
        systemctl --user stop rusty-board.service || true
    fi
    systemctl --user disable rusty-board.service >/dev/null 2>&1 || true
fi

# 2. Remove systemd unit
if [[ -f "$SYSTEMD_USER_DIR/rusty-board.service" ]]; then
    echo "    rm $SYSTEMD_USER_DIR/rusty-board.service"
    rm -f "$SYSTEMD_USER_DIR/rusty-board.service"
    systemctl --user daemon-reload >/dev/null 2>&1 || true
fi

# 3. Optionally save models before nuking the share dir
if [[ "$KEEP_MODELS" -eq 1 && -d "$SHARE_DIR/models" ]]; then
    BACKUP_DIR="$HOME/rusty-board-models-backup"
    echo "==> Backing up models to $BACKUP_DIR"
    mkdir -p "$BACKUP_DIR"
    cp -R "$SHARE_DIR/models/." "$BACKUP_DIR/" || true
fi

# 4. Remove binaries
for f in rusty-board rusty-board-config; do
    if [[ -e "$BIN_DIR/$f" ]]; then
        echo "    rm $BIN_DIR/$f"
        $SUDO rm -f "$BIN_DIR/$f"
    fi
done

# 5. Remove resources
if [[ -d "$SHARE_DIR" ]]; then
    echo "    rm -r $SHARE_DIR"
    $SUDO rm -rf "$SHARE_DIR"
fi

# 6. Remove desktop entry and icon
DESKTOP_FILE="$APPLICATIONS_DIR/io.rustyboard.Keyboard.desktop"
if [[ -e "$DESKTOP_FILE" ]]; then
    echo "    rm $DESKTOP_FILE"
    $SUDO rm -f "$DESKTOP_FILE"
fi

ICON_FILE="$ICONS_DIR/io.rustyboard.Keyboard.svg"
if [[ -e "$ICON_FILE" ]]; then
    echo "    rm $ICON_FILE"
    $SUDO rm -f "$ICON_FILE"
fi

# 7. User config (default: remove; --keep-config preserves it)
if [[ "$KEEP_CONFIG" -eq 0 && -d "$USER_CONFIG_DIR" ]]; then
    echo "    rm -r $USER_CONFIG_DIR"
    rm -rf "$USER_CONFIG_DIR"
elif [[ -d "$USER_CONFIG_DIR" ]]; then
    echo "    keeping user config at $USER_CONFIG_DIR"
fi

# 8. Update caches (ignore failures)
update-desktop-database "$APPLICATIONS_DIR" >/dev/null 2>&1 || true
gtk-update-icon-cache -t "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true

echo
echo "==> Uninstall complete."
