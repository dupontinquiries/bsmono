#!/bin/sh
# Fetch YuNet, the 230 KB face detector (MIT, https://github.com/opencv/opencv_zoo, models/face_detection_yunet).
# It is not in the repo: usage  tools/fetch_yunet.sh [dest]   (default ~/.local/share/eye-kit/models/yunet.onnx)
set -e
DEST=${1:-$HOME/.local/share/eye-kit/models/yunet.onnx}
mkdir -p "$(dirname "$DEST")"
curl -fsSL -o "$DEST" "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx"
[ "$(wc -c < "$DEST")" -eq 232589 ] || { echo "unexpected size for $DEST"; exit 1; }
echo "$DEST"
