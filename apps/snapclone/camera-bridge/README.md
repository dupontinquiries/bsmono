# camera-bridge

A Rust daemon that captures video from a PipeWire camera, runs lightweight YOLO face detection, and outputs a smoothly tracked/cropped video stream to a virtual camera (`/dev/video10`) via v4l2loopback.

```
Camera (PipeWire / libcamera)
        ↓
  GStreamer appsink
        ↓
  YOLO inference  ←── periodic (default: 1 FPS)
        ↓
  Exponential smoothing
        ↓
  Crop + resize
        ↓
  /dev/video10  (v4l2loopback)
        ↓
  Browser / OBS / any V4L2 consumer
```

---

## Requirements

| Component | Version |
|---|---|
| Rust | 1.77+ (`rustup update`) |
| GStreamer | 1.20+ |
| v4l2loopback | kernel module |
| An ONNX face model | see `models/README.md` |

### Install system dependencies (Ubuntu / Debian)

```bash
# GStreamer
sudo apt install \
  gstreamer1.0-tools \
  gstreamer1.0-plugins-base \
  gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad \
  gstreamer1.0-pipewire \
  libgstreamer1.0-dev \
  libgstreamer-plugins-base1.0-dev

# v4l2loopback
sudo apt install v4l2loopback-dkms v4l-utils

# PipeWire (usually already installed on modern distros)
sudo apt install pipewire pipewire-pulse wireplumber
```

### Install system dependencies (Arch Linux)

```bash
sudo pacman -S gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad \
               gst-plugin-pipewire v4l2loopback-dkms v4l-utils pipewire pipewire-pulse
```

---

## Build

```bash
git clone <this-repo>
cd camera-bridge

# Release build (recommended for performance)
cargo build --release
```

The binary will be at `target/release/camera-bridge`.

---

## Setup

### 1. Load v4l2loopback

```bash
chmod +x scripts/setup_v4l2loopback.sh
./scripts/setup_v4l2loopback.sh
```

This creates `/dev/video10`.  Re-run after every reboot (or add a `modprobe`
line to `/etc/modules`):

```
# /etc/modules-load.d/v4l2loopback.conf
v4l2loopback

# /etc/modprobe.d/v4l2loopback.conf
options v4l2loopback devices=1 video_nr=10 card_label="VirtualCam" exclusive_caps=1
```

### 2. Download a face model

See `models/README.md` for model options and how to export to ONNX.
Place the model at `models/face.onnx` (or set `MODEL_PATH`).

### 3. Run

```bash
# With defaults
./target/release/camera-bridge

# With custom settings
CAMERA_PIPELINE="pipewiresrc target-object=42 ! videoconvert ! video/x-raw,format=RGB,width=640,height=360 ! appsink name=sink" \
MODEL_PATH=./models/face.onnx \
OUTPUT_DEVICE=/dev/video10 \
DETECTION_INTERVAL=0.5 \
ZOOM_FACTOR=1.4 \
RUST_LOG=debug \
  ./target/release/camera-bridge
```

### Find your camera's PipeWire object serial

```bash
pw-cli list-objects | grep -A5 "node.name.*camera"
# or
wpctl status
```

Use the object serial number in `pipewiresrc target-object=<serial>`.

---

## Install as a systemd user service

```bash
# Copy binary
install -Dm755 target/release/camera-bridge ~/.local/bin/camera-bridge

# Copy model
mkdir -p ~/.config/camera-bridge/models
cp models/face.onnx ~/.config/camera-bridge/models/

# Install service
mkdir -p ~/.config/systemd/user
cp systemd/camera-bridge.service ~/.config/systemd/user/

# Edit paths / environment if needed
$EDITOR ~/.config/systemd/user/camera-bridge.service

# Enable and start
systemctl --user daemon-reload
systemctl --user enable --now camera-bridge

# Check status
systemctl --user status camera-bridge
journalctl --user -u camera-bridge -f
```

---

## Configuration (environment variables)

| Variable | Default | Description |
|---|---|---|
| `CAMERA_PIPELINE` | `pipewiresrc ! videoconvert ! appsink name=sink` | Full GStreamer pipeline string |
| `MODEL_PATH` | `models/face.onnx` | Path to ONNX model |
| `OUTPUT_DEVICE` | `/dev/video10` | V4L2 loopback device |
| `OUTPUT_WIDTH` | `640` | Output frame width |
| `OUTPUT_HEIGHT` | `360` | Output frame height |
| `DETECTION_INTERVAL` | `1.0` | Seconds between YOLO runs |
| `ZOOM_FACTOR` | `1.5` | Crop expansion around face |
| `RUST_LOG` | `info` | Log level |

---

## Architecture

| Module | Responsibility |
|---|---|
| `capture.rs` | GStreamer pipeline, RGB frame extraction |
| `detection.rs` | ONNX inference, NMS, bounding box output |
| `tracking.rs` | Exponential smoothing, zoom-out fallback |
| `processing.rs` | Sub-image crop + Lanczos resize |
| `output.rs` | RGB→YUYV conversion, V4L2 device write |
| `main.rs` | Configuration, main loop, wiring |

---

## Troubleshooting

**`Cannot open virtual camera device /dev/video10`**
→ Run `setup_v4l2loopback.sh` first, or check `ls /dev/video*`.

**`Failed to load ONNX model`**
→ Ensure `models/face.onnx` exists and matches the input shape in `detection.rs`.

**Pipeline fails to start**
→ Test your GStreamer pipeline independently:
```bash
gst-launch-1.0 pipewiresrc ! videoconvert ! autovideosink
```

**No face detected**
→ Try lowering `CONF_THRESHOLD` in `detection.rs` (default 0.45) or check model compatibility.

**High CPU usage**
→ Increase `DETECTION_INTERVAL`, reduce resolution, or use a smaller model variant.

---

## License

MIT
