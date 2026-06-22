#!/usr/bin/env bash
# install.sh — copy built binaries, QML assets, systemd unit, and
# default configs into the user's home directory. No root required;
# everything lives under ~/.local and ~/.config.
#
# Layout after install:
#   ~/.local/bin/bo-controller       # the GUI controller
#   ~/.local/bin/bo-muxd             # the soundscape daemon
#   ~/.local/share/bo-controller/    # QML + assets (read at runtime)
#   ~/.config/systemd/user/bo-mux.service   # systemd socket-activated service
#   ~/.config/systemd/user/bo-mux.socket    # systemd socket unit
#   ~/.config/bo-mux/                # daemon configs
#
# Usage:
#   ./scripts/install.sh                # interactive
#   ./scripts/install.sh --systemd      # also enable bo-muxd user service
#   ./scripts/install.sh --uninstall    # remove everything we installed

set -euo pipefail

cd "$(dirname "$0")/.."

PREFIX="${HOME}/.local"
SHARE="${PREFIX}/share/bo-controller"
BIN="${PREFIX}/bin"
SYSTEMD_USER="${HOME}/.config/systemd/user"
MUX_CONFIG="${HOME}/.config/bo-mux"

ACTION="install"
WITH_SYSTEMD=0
for arg in "$@"; do
    case "$arg" in
        --uninstall) ACTION="uninstall" ;;
        --systemd|-s) WITH_SYSTEMD=1 ;;
    esac
done

if [[ "$ACTION" == "uninstall" ]]; then
    echo ">>> Uninstalling bo-suite from $PREFIX"
    rm -f "${BIN}/bo-controller" "${BIN}/bo-muxd"
    # Remove the old QML/assets share directory if it exists from a
    # pre-bundle install. The binary no longer needs it (QML +
    # assets are now compiled into the binary itself), so leaving
    # it behind would just be clutter.
    rm -rf "${SHARE}"
    if [[ -f "${SYSTEMD_USER}/bo-mux.service" ]] || [[ -f "${SYSTEMD_USER}/bo-mux.socket" ]]; then
        systemctl --user stop bo-mux.service 2>/dev/null || true
        systemctl --user stop bo-mux.socket  2>/dev/null || true
        systemctl --user disable bo-mux.socket 2>/dev/null || true
        rm -f "${SYSTEMD_USER}/bo-mux.service" "${SYSTEMD_USER}/bo-mux.socket"
        systemctl --user daemon-reload 2>/dev/null || true
    fi
    rm -f "${HOME}/.local/share/applications/bo-controller.desktop"
    echo ">>> Removed. (Personal configs in ~/.config/bo-mux/ left intact.)"
    exit 0
fi

# Check that we have built binaries.
for bin in bo-controller bo-muxd; do
    if [[ ! -x "target/release/$bin" ]]; then
        echo "ERROR: target/release/$bin not found. Run ./scripts/build.sh first."
        exit 1
    fi
done

echo ">>> Installing to $PREFIX"
mkdir -p "$BIN" "$MUX_CONFIG"

install -m 755 target/release/bo-controller "$BIN/bo-controller"
install -m 755 target/release/bo-muxd       "$BIN/bo-muxd"

# IMPORTANT: as of this build, QML files and assets are compiled
# INTO the bo-controller binary via include_dir!. The binary
# extracts them to a per-PID temp dir on launch. So:
#
#   - No more `cp -r bo-controller/qml $SHARE/qml`
#   - No more `BO_QML_DIR` env var in the desktop entry
#   - The "stale install" footgun is fundamentally gone — the QML
#     in the binary always matches the source tree that produced
#     the binary.
#
# Clean up any prior `$SHARE` from a previous-version install so the
# new behavior isn't shadowed if the user re-runs install.sh.
if [[ -d "$SHARE" ]]; then
    echo "    Removing legacy QML install at $SHARE (now bundled in binary)"
    rm -rf "$SHARE"
fi

# Desktop entry — makes the controller show up in KDE's app launcher.
# No BO_QML_DIR needed; the binary is self-contained.
DESKTOP="${HOME}/.local/share/applications/bo-controller.desktop"
mkdir -p "$(dirname "$DESKTOP")"
cat > "$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=B&O Controller
Comment=Bang & Olufsen volume + media controller
Exec=$BIN/bo-controller
Icon=audio-card
Categories=AudioVideo;Audio;
EOF

# Soundscape example
if [[ -d bo-mux/examples ]]; then
    cp -r bo-mux/examples/* "$MUX_CONFIG/" 2>/dev/null || true
fi

if [[ $WITH_SYSTEMD -eq 1 ]]; then
    echo
    echo ">>> Installing bo-muxd as a user systemd service"
    mkdir -p "$SYSTEMD_USER"
    SVC="${SYSTEMD_USER}/bo-mux.service"
    SOCK="${SYSTEMD_USER}/bo-mux.socket"
    if [[ -f bo-mux/systemd/bo-mux.service ]]; then
        # Substitute the installed path so the user copy points at
        # ~/.local/bin instead of wherever the source unit assumed.
        sed "s|/usr/local/bin/bo-muxd|${BIN}/bo-muxd|g" \
            bo-mux/systemd/bo-mux.service > "$SVC"
        cp bo-mux/systemd/bo-mux.socket "$SOCK" 2>/dev/null || true
    else
        cat > "$SVC" <<EOF
[Unit]
Description=B&O Mux Soundscape Daemon
After=pulseaudio.service network.target

[Service]
ExecStart=${BIN}/bo-muxd
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
EOF
        cat > "$SOCK" <<EOF
[Unit]
Description=B&O Mux Control Socket

[Socket]
ListenStream=%t/bo-mux/control.sock
SocketMode=0600

[Install]
WantedBy=sockets.target
EOF
    fi
    systemctl --user daemon-reload
    systemctl --user enable --now bo-mux.socket 2>/dev/null || true
    echo "    Run:  systemctl --user status bo-mux.socket"
fi

echo
echo ">>> Installed:"
echo "    $BIN/bo-controller   (QML + assets bundled inside)"
echo "    $BIN/bo-muxd"
echo "    $DESKTOP   (in KDE app launcher)"
if [[ $WITH_SYSTEMD -eq 1 ]]; then
    echo "    $SYSTEMD_USER/bo-mux.service + bo-mux.socket"
fi
echo
echo ">>> Make sure $BIN is on your PATH. If not, add to ~/.bashrc / ~/.zshrc:"
echo "    export PATH=\"\$HOME/.local/bin:\$PATH\""
echo
echo ">>> Launch the controller: bo-controller"
echo
echo "Note: as of this build, the binary is self-contained — QML and"
echo "assets are compiled INTO bo-controller via include_dir!. Re-running"
echo "this script just copies a new binary; the embedded QML matches the"
echo "source tree at compile time. The 'stale install' confusion is gone."
