#!/bin/sh
# setup_backend.sh
#
# Sets up the camera-bridge PipeWire backend on postmarketOS / FairPhone 5.
#
# Creates THREE virtual camera nodes:
#   virtual-cam-rear      — raw ultrawide/main (IMX858)
#   virtual-cam-front     — raw front (S5KJN1)
#   virtual-cam-tracking  — front + YOLO face tracking
#
# camera-bridge creates these as Video/Source PipeWire nodes directly.
# Apps read whichever node they want.
#
# Usage:
#   sh scripts/setup_backend.sh [--no-v4l2loopback] [--dry-run]

set -e

log()  { printf '\033[1;32m[setup]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn ]\033[0m %s\n' "$*"; }
die()  { printf '\033[1;31m[error]\033[0m %s\n' "$*" >&2; exit 1; }
run()  {
    if [ "$DRY_RUN" = "1" ]; then
        printf '\033[1;34m[dry  ]\033[0m %s\n' "$*"
    else
        eval "$*"
    fi
}

SKIP_V4L2LOOPBACK=0
DRY_RUN=0
KERNEL_VER=$(uname -r)
KERNEL_PKG="linux-postmarketos-qcom-sc7280"
V4L2LB_VIDEO_NR=30
V4L2LB_LABEL="FaceTrackCam"
PW_CONF_DIR="$HOME/.config/pipewire/pipewire.conf.d"
SYSTEMD_DIR="$HOME/.config/systemd/user"
BRIDGE_CONF="$HOME/.config/camera-bridge"

for arg in "$@"; do
    case "$arg" in
        --no-v4l2loopback) SKIP_V4L2LOOPBACK=1 ;;
        --dry-run)         DRY_RUN=1 ;;
        -h|--help)
            echo "Usage: $0 [--no-v4l2loopback] [--dry-run]"
            exit 0
            ;;
    esac
done

log "Setting up camera-bridge backend for kernel $KERNEL_VER"

# ── 1. kernel-dev + v4l2loopback (optional) ───────────────────────────────────
if [ "$SKIP_V4L2LOOPBACK" = "0" ]; then
    log "Installing kernel dev package: ${KERNEL_PKG}-dev"
    if ! run "doas apk add --no-interactive ${KERNEL_PKG}-dev"; then
        warn "Could not install ${KERNEL_PKG}-dev — skipping v4l2loopback build"
        SKIP_V4L2LOOPBACK=1
    fi
fi

if [ "$SKIP_V4L2LOOPBACK" = "0" ]; then
    KDIR="/lib/modules/${KERNEL_VER}/build"
    if [ ! -d "$KDIR" ]; then
        warn "Kernel build dir $KDIR not found — skipping v4l2loopback"
        SKIP_V4L2LOOPBACK=1
    fi
fi

if [ "$SKIP_V4L2LOOPBACK" = "0" ]; then
    log "Installing v4l2loopback source"
    run "doas apk add --no-interactive v4l2loopback-src"
    V4L2SRC=$(find /usr/src -maxdepth 1 -name "v4l2loopback*" -type d | head -1)
    [ -z "$V4L2SRC" ] && die "v4l2loopback source not found in /usr/src"
    log "Building v4l2loopback from $V4L2SRC"
    run "doas make -C '$V4L2SRC' KDIR='$KDIR' KERNELRELEASE='$KERNEL_VER'"
    run "doas make -C '$V4L2SRC' KDIR='$KDIR' KERNELRELEASE='$KERNEL_VER' install"
    run "doas depmod -a '$KERNEL_VER'"
    log "Loading v4l2loopback (device /dev/video${V4L2LB_VIDEO_NR})"
    run "doas modprobe v4l2loopback \
        devices=1 \
        video_nr=${V4L2LB_VIDEO_NR} \
        card_label='${V4L2LB_LABEL}' \
        exclusive_caps=1" || warn "modprobe failed — may already be loaded"
    run "printf '%s\n' 'v4l2loopback' | doas tee '/etc/modules-load.d/v4l2loopback.conf' > /dev/null"
    run "printf '%s\n' \
        'options v4l2loopback devices=1 video_nr=${V4L2LB_VIDEO_NR} card_label=${V4L2LB_LABEL} exclusive_caps=1' \
        | doas tee '/etc/modprobe.d/v4l2loopback.conf' > /dev/null"
    ls /dev/video${V4L2LB_VIDEO_NR} >/dev/null 2>&1 \
        && log "v4l2loopback ready at /dev/video${V4L2LB_VIDEO_NR}" \
        || warn "/dev/video${V4L2LB_VIDEO_NR} not found — check dmesg"
else
    log "Skipping v4l2loopback (PipeWire-only mode)"
fi

# ── 2. PipeWire config — remove old loopback configs ─────────────────────────
# camera-bridge now creates Video/Source nodes itself via pipewiresink,
# so no loopback module config is needed.  Clean up any old configs.
log "Cleaning up old PipeWire loopback configs (no longer needed)"
run "mkdir -p '$PW_CONF_DIR'"
for old_conf in \
    "$PW_CONF_DIR/virtual-cameras.conf" \
    "$PW_CONF_DIR/virtual-camera.conf"; do
    if [ -f "$old_conf" ]; then
        log "Removing obsolete config: $old_conf"
        run "rm -f '$old_conf'"
    fi
done
log "Virtual camera nodes are now created directly by camera-bridge at runtime."

# ── 3. camera-bridge config ───────────────────────────────────────────────────
log "Creating camera-bridge config directory"
run "mkdir -p '$BRIDGE_CONF/models'"

ENV_FILE="$BRIDGE_CONF/env"
# Always (re)write the env file so it stays in sync with this script.
# Existing user edits will be overwritten — keep custom values in a
# separate override file and set EnvironmentFile in the service instead.
log "Writing env file to $ENV_FILE"
if [ "$DRY_RUN" != "1" ]; then
cat > "$ENV_FILE" << 'ENVEOF'
# camera-bridge runtime configuration

# ── Physical PipeWire camera node names ──────────────────────────────────────
# These are the libcamera nodes for the FP5 hardware.
PW_NODE_REAR=libcamera_input._base_soc_0_cci_ac4a000_i2c-bus_1_camera_29
PW_NODE_FRONT=libcamera_input._base_soc_0_cci_ac4b000_i2c-bus_1_camera_10

# ── Virtual output nodes ──────────────────────────────────────────────────────
# camera-bridge creates these automatically as Video/Source PipeWire nodes.
# Apps connect to: virtual-cam-rear | virtual-cam-front | virtual-cam-tracking

# ── Inference ─────────────────────────────────────────────────────────────────
MODEL_PATH=/home/user/.config/camera-bridge/models/face.onnx
# YOLO runs at most every DETECTION_INTERVAL seconds.
# Laplacian variance also triggers early if the scene changes suddenly.
DETECTION_INTERVAL=1.0
ZOOM_FACTOR=1.4

# ── Output resolution ─────────────────────────────────────────────────────────
# After videoflip corrects the 270° sensor rotation, frames are portrait.
# Use portrait dimensions (width < height) to match the corrected output.
OUTPUT_WIDTH=480
OUTPUT_HEIGHT=640

# ── Logging ───────────────────────────────────────────────────────────────────
RUST_LOG=info
ENVEOF
fi

# ── 4. systemd user services ──────────────────────────────────────────────────
log "Installing systemd user services"
run "mkdir -p '$SYSTEMD_DIR'"

if [ "$DRY_RUN" != "1" ]; then

# camera-bridge: feeds all three virtual nodes
cat > "$SYSTEMD_DIR/camera-bridge.service" << 'SVCEOF'
[Unit]
Description=Camera Bridge — feeds virtual PipeWire camera nodes
After=pipewire.service
Wants=pipewire.service

[Service]
Type=simple
Restart=on-failure
RestartSec=3
EnvironmentFile=%h/.config/camera-bridge/env
ExecStart=%h/.local/bin/camera-bridge
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=default.target
SVCEOF

# camera-view: fullscreen display reading from virtual-cam-rear
cat > "$SYSTEMD_DIR/camera-view.service" << 'SVCEOF'
[Unit]
Description=Fullscreen Camera View (transparent phone wallpaper)
After=pipewire.service camera-bridge.service graphical-session.target
Requires=camera-bridge.service
PartOf=graphical-session.target

[Service]
Type=simple
Restart=on-failure
RestartSec=2
ExecStart=%h/.local/bin/camera-view --no-cursor
Environment=PW_SOURCE_NODE=virtual-cam-rear
Environment=CAMERA_ROTATION=0
Environment=RUST_LOG=warn
StartLimitIntervalSec=30
StartLimitBurst=5
StandardOutput=journal
StandardError=journal

[Install]
WantedBy=graphical-session.target
SVCEOF

fi

# ── 5. reload PipeWire + systemd ──────────────────────────────────────────────
log "Reloading PipeWire"
{ run "systemctl --user restart pipewire" || warn "Could not restart PipeWire"; }
sleep 2
run "systemctl --user daemon-reload"

# ── 6. verify ─────────────────────────────────────────────────────────────────
# Virtual nodes are created by camera-bridge at runtime, not by PipeWire config.
# They only appear in pw-cli while camera-bridge is running.
# Just verify the service files and env file are in place.
log "Verifying service files..."
if [ "$DRY_RUN" != "1" ]; then
    for svc in camera-bridge camera-view; do
        if [ -f "$SYSTEMD_DIR/${svc}.service" ]; then
            log "  OK: ${svc}.service"
        else
            warn "  MISSING: ${svc}.service"
        fi
    done
    if [ -f "$ENV_FILE" ]; then
        log "  OK: $ENV_FILE"
    else
        warn "  MISSING: $ENV_FILE"
    fi
fi

log ""
log "Setup complete. Next steps:"
log ""
log "  1. Build both binaries:"
log "       cargo build --release"
log ""
log "  2. Install:"
log "       cp target/release/camera-bridge ~/.local/bin/"
log "       cp target/release/camera-view   ~/.local/bin/"
log ""
log "  3. Install ONNX face model (needed for tracking viewport):"
log "       cp face.onnx $BRIDGE_CONF/models/face.onnx"
log "       (see models/README.md)"
log ""
log "  4. Install desktop entry:"
log "       cp desktop/camera-view.desktop ~/.local/share/applications/"
log ""
log "  5. Start services:"
log "       systemctl --user enable --now camera-bridge"
log "       systemctl --user enable --now camera-view"
log ""
log "Virtual camera nodes (for apps to connect to):"
log "  virtual-cam-rear      — rear/ultrawide camera (IMX858)"
log "  virtual-cam-front     — front camera (S5KJN1)"
log "  virtual-cam-tracking  — face-tracked front camera"
