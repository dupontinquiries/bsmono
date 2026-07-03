#!/usr/bin/env bash
# setup_v4l2loopback.sh
# Loads the v4l2loopback kernel module and creates /dev/video10.
#
# Run once before starting camera-bridge (or add to your init system).

set -euo pipefail

DEVICE_NR="${VIDEO_NR:-10}"
CARD_LABEL="${CARD_LABEL:-VirtualCam}"

echo "Loading v4l2loopback (device /dev/video${DEVICE_NR})..."

sudo modprobe v4l2loopback \
    devices=1 \
    video_nr="${DEVICE_NR}" \
    card_label="${CARD_LABEL}" \
    exclusive_caps=1

echo "Virtual camera device /dev/video${DEVICE_NR} is ready."

# Optional: set format via v4l2-ctl (install v4l-utils if needed).
if command -v v4l2-ctl &>/dev/null; then
    OUTPUT_WIDTH="${OUTPUT_WIDTH:-640}"
    OUTPUT_HEIGHT="${OUTPUT_HEIGHT:-360}"
    v4l2-ctl \
        --device="/dev/video${DEVICE_NR}" \
        --set-fmt-video="width=${OUTPUT_WIDTH},height=${OUTPUT_HEIGHT},pixelformat=YUYV" \
        2>/dev/null || true
    echo "Format set to ${OUTPUT_WIDTH}x${OUTPUT_HEIGHT} YUYV."
fi

echo "Done. You can now start camera-bridge."
