# Camera Bridge Integration

SnapClone integrates with `camera-bridge` for Fairphone 5 (postmarketOS) camera support.

## Architecture

The FP5 uses libcamera sensors not directly accessible via V4L2/Qt on Alpine.
`camera-bridge` exposes them via GStreamer + PipeWire virtual nodes:

    Physical sensor (libcamera/PipeWire)
        │
        ▼
    camera-bridge (Rust daemon)
        ├── virtual-cam-rear    (IMX858 back camera, 480×640 portrait)
        ├── virtual-cam-front   (S5KJN1 front camera, 480×640 portrait)
        └── virtual-cam-tracking (front + YOLO face tracking + crop)
        │
        ▼
    Qt6 CameraWidget (picks up PipeWire nodes via GStreamer backend)

## Setup on Fairphone 5 (postmarketOS)

```bash
# 1. Build and install the bridge
cargo build --release
install -m755 target/release/camera-bridge /usr/local/bin/
install -m755 target/release/camera-server /usr/local/bin/

# 2. Install systemd units
install -m644 systemd/camera-bridge.service /etc/systemd/user/
systemctl --user enable --now camera-bridge

# 3. Verify virtual cameras appear
pw-cli list-objects | grep virtual-cam
# Should show: virtual-cam-rear, virtual-cam-front, virtual-cam-tracking

# 4. SnapClone will auto-detect and prefer virtual-cam-front for selfie snaps
```

## Remote streaming (phone → laptop)

```bash
# On the phone:
camera-server --bind 0.0.0.0:7878

# On the laptop:
camera-client keygen                          # once
camera-client trust <phone-ip> <fingerprint>  # once
camera-client connect <phone-ip>              # stream!
```

## Qt6 backend requirement

SnapClone on postmarketOS must be built with:
- Qt6Multimedia with GStreamer backend (not FFmpeg)
- gstreamer-pipewire plugin installed
- PipeWire session running (pipewire, pipewire-pulse, wireplumber)

Check: `gst-inspect-1.0 pipewiresrc`
