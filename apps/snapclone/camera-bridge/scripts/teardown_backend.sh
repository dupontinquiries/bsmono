#!/bin/sh
# teardown_backend.sh — cleanly removes the camera-bridge backend.
#
# Usage:
#   sh scripts/teardown_backend.sh [--purge] [--dry-run]
#   --purge    also removes ~/.config/camera-bridge and installed binaries
#   --dry-run  print what would happen without doing it

set -e

log()  { printf '\033[1;32m[teardown]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn    ]\033[0m %s\n' "$*"; }
run()  {
    if [ "$DRY_RUN" = "1" ]; then
        printf '\033[1;34m[dry     ]\033[0m %s\n' "$*"
    else
        eval "$*"
    fi
}

PURGE=0
DRY_RUN=0
SYSTEMD_DIR="$HOME/.config/systemd/user"
PW_CONF_DIR="$HOME/.config/pipewire/pipewire.conf.d"
BRIDGE_CONF="$HOME/.config/camera-bridge"

for arg in "$@"; do
    case "$arg" in
        --purge)   PURGE=1 ;;
        --dry-run) DRY_RUN=1 ;;
        -h|--help)
            echo "Usage: $0 [--purge] [--dry-run]"
            exit 0
            ;;
    esac
done

log "Tearing down camera-bridge backend"

# ── 1. stop services ──────────────────────────────────────────────────────────
for svc in camera-bridge camera-view; do
    run "systemctl --user stop $svc 2>/dev/null || true"
    run "systemctl --user disable $svc 2>/dev/null || true"
    for f in "$SYSTEMD_DIR/${svc}.service"; do
        [ -f "$f" ] && run "rm -f '$f'" && log "Removed $f"
    done
done
run "systemctl --user daemon-reload 2>/dev/null || true"

# ── 2. remove PipeWire virtual camera configs ─────────────────────────────────
for conf in \
    "$PW_CONF_DIR/virtual-cameras.conf" \
    "$PW_CONF_DIR/virtual-camera.conf"; do    # old single-node config
    if [ -f "$conf" ]; then
        log "Removing $conf"
        run "rm -f '$conf'"
    fi
done
if [ -d "$PW_CONF_DIR" ] && [ "$DRY_RUN" != "1" ]; then
    log "Reloading PipeWire"
    systemctl --user restart pipewire 2>/dev/null || true
    sleep 1
fi

# ── 3. unload v4l2loopback ────────────────────────────────────────────────────
if lsmod 2>/dev/null | grep -q v4l2loopback; then
    log "Unloading v4l2loopback"
    run "doas modprobe -r v4l2loopback 2>/dev/null || true"
fi
for f in /etc/modules-load.d/v4l2loopback.conf /etc/modprobe.d/v4l2loopback.conf; do
    [ -f "$f" ] && run "doas rm -f '$f'" && log "Removed $f"
done

# ── 4. purge ──────────────────────────────────────────────────────────────────
if [ "$PURGE" = "1" ]; then
    if [ -d "$BRIDGE_CONF" ]; then
        log "Removing $BRIDGE_CONF"
        run "rm -rf '$BRIDGE_CONF'"
    fi
    for bin in camera-bridge camera-view; do
        f="$HOME/.local/bin/$bin"
        [ -f "$f" ] && run "rm -f '$f'" && log "Removed $f"
    done
    f="$HOME/.local/share/applications/camera-view.desktop"
    [ -f "$f" ] && run "rm -f '$f'" && log "Removed $f"
else
    log "Config preserved at $BRIDGE_CONF (use --purge to remove)"
fi

log "Teardown complete."
