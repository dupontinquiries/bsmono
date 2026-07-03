#!/bin/sh
# setup_webcam_client.sh — set up camera-client on a Linux desktop
#
# Installs prerequisites (v4l2loopback or PipeWire support) and walks
# through the key exchange with your FP5.
#
# Usage:
#   sh setup_webcam_client.sh [--phone-ip <ip>] [--no-v4l2]

set -e

GREEN='\033[1;32m'; YELLOW='\033[1;33m'; RED='\033[1;31m'; NC='\033[0m'
ok()   { printf "${GREEN}[ok]${NC}   %s\n" "$*"; }
warn() { printf "${YELLOW}[warn]${NC} %s\n" "$*"; }
fail() { printf "${RED}[fail]${NC} %s\n" "$*" >&2; exit 1; }
info() { printf "[....] %s\n" "$*"; }

PHONE_IP=""
PHONE_USER="${PHONE_USER:-user}"
PHONE_PORT="${PHONE_PORT:-7878}"
NO_V4L2=0

while [ $# -gt 0 ]; do
    case "$1" in
        --phone-ip) PHONE_IP="$2"; shift ;;
        --no-v4l2)  NO_V4L2=1 ;;
        *) warn "Unknown arg: $1" ;;
    esac
    shift
done

# ── 1. Install v4l2loopback ───────────────────────────────────────────────────
if [ "$NO_V4L2" = "0" ]; then
    info "Installing v4l2loopback..."
    if command -v dnf > /dev/null 2>&1; then
        sudo dnf install -y v4l2loopback kmod-v4l2loopback 2>/dev/null || \
        sudo dnf install -y v4l2loopback-utils 2>/dev/null || true
    elif command -v apt > /dev/null 2>&1; then
        sudo apt-get install -y v4l2loopback-dkms v4l2loopback-utils
    elif command -v pacman > /dev/null 2>&1; then
        sudo pacman -S --noconfirm v4l2loopback-dkms
    else
        warn "Unknown distro — install v4l2loopback manually"
    fi

    # Load the module
    if ! lsmod | grep -q v4l2loopback; then
        sudo modprobe v4l2loopback \
            devices=1 video_nr=30 \
            card_label="PhoneCamera" \
            exclusive_caps=1
    fi

    if ls /dev/video30 > /dev/null 2>&1; then
        ok "v4l2loopback ready at /dev/video30"
    else
        warn "/dev/video30 not found — camera-client --pipewire mode will still work"
    fi

    # Persist across reboots
    if [ ! -f /etc/modules-load.d/v4l2loopback.conf ]; then
        echo "v4l2loopback" | sudo tee /etc/modules-load.d/v4l2loopback.conf > /dev/null
        cat << 'MODCONF' | sudo tee /etc/modprobe.d/v4l2loopback.conf > /dev/null
options v4l2loopback devices=1 video_nr=30 card_label="PhoneCamera" exclusive_caps=1
MODCONF
        ok "v4l2loopback will load on next boot"
    fi
fi

# ── 2. Generate identity ──────────────────────────────────────────────────────
CAMERA_CLIENT=""
for p in \
    "$HOME/.local/bin/camera-client" \
    "./target/release/camera-client" \
    "./target/aarch64-unknown-linux-musl/release/camera-client"
do
    [ -x "$p" ] && CAMERA_CLIENT="$p" && break
done

if [ -z "$CAMERA_CLIENT" ]; then
    warn "camera-client binary not found."
    warn "Build it first: sh scripts/build.sh --desktop"
    warn "Then re-run this script."
    exit 1
fi
ok "camera-client: $CAMERA_CLIENT"

info "Generating/showing identity..."
HOSTNAME=$(hostname -s)
PUBKEY_LINE=$("$CAMERA_CLIENT" keygen --comment "$HOSTNAME" 2>/dev/null | \
    grep -A1 "authorized_keys:" | tail -1 | tr -d '"')

echo ""
ok "Your public key (copy this to the phone):"
echo ""
echo "  $PUBKEY_LINE"
echo ""

# ── 3. Trust the server ───────────────────────────────────────────────────────
if [ -n "$PHONE_IP" ]; then
    info "Getting fingerprint from phone..."
    FP=$(ssh -p 22 "${PHONE_USER}@${PHONE_IP}" \
        "~/.local/bin/camera-server show-fingerprint 2>/dev/null" | \
        grep "TLS fingerprint:" | awk '{print $3}')

    if [ -n "$FP" ]; then
        "$CAMERA_CLIENT" trust "$PHONE_IP" "$FP"
        ok "Trusted $PHONE_IP (fp: ${FP:0:16}...)"
    else
        warn "Could not get fingerprint automatically."
        warn "On the phone, run: camera-server show-fingerprint"
        warn "Then here: camera-client trust $PHONE_IP <fingerprint>"
    fi

    info "Adding your public key to phone..."
    ssh "${PHONE_USER}@${PHONE_IP}" \
        "~/.local/bin/camera-server add-key '${PUBKEY_LINE}'" 2>/dev/null && \
        ok "Public key added to phone" || \
        warn "Could not add key automatically — do it manually on the phone"
else
    echo "Next steps:"
    echo ""
    echo "  On the phone:"
    echo "    camera-server add-key \"${PUBKEY_LINE}\""
    echo ""
    echo "  Get the phone's TLS fingerprint:"
    echo "    (on phone) camera-server show-fingerprint"
    echo ""
    echo "  Trust it here:"
    echo "    camera-client trust <phone-ip> <fingerprint>"
fi

# ── 4. Usage summary ──────────────────────────────────────────────────────────
PHONE_REF="${PHONE_IP:-<phone-ip>}"
echo ""
ok "Setup complete!"
echo ""
echo "Usage:"
echo ""
echo "  # Connect and use phone as webcam (v4l2loopback /dev/video30):"
echo "  camera-client connect $PHONE_REF"
echo ""
echo "  # Use a specific camera:"
echo "  camera-client connect $PHONE_REF --stream virtual-cam-front"
echo "  camera-client connect $PHONE_REF --stream virtual-cam-tracking"
echo ""
echo "  # Use PipeWire output instead (appears as 'phone-camera' in any app):"
echo "  camera-client connect $PHONE_REF --pipewire"
echo ""
echo "  # Test in ffplay:"
echo "  ffplay /dev/video30"
echo ""
echo "  # Use in OBS: add V4L2 source → /dev/video30"
echo "  # Use in browser: the device appears as 'PhoneCamera'"
