# PhotoSync Build Configuration & Feature Flags

## Feature Flags System

PhotoSync uses Cargo feature flags to enable/disable components at compile time. This allows you to:
- Build minimal installations for resource-constrained devices
- Enable experimental features for testing
- Reduce binary size by excluding unused features
- Compile only what you need

## Available Features

### Core Features (Always Enabled)

- **deduplication** - Block-level deduplication with XXH64/SHA-256
- **compression** - zstd compression at configurable levels
- **sqlite** - SQLite database for metadata
- **thumbnails** - Automatic thumbnail generation

### Optional Features

#### `websocket-sync` (Default: OFF)
Real-time synchronization via WebSocket protocol.

**Enables:**
- WebSocket server for remote sync
- Real-time file notifications
- Multi-device sync without Syncthing

**Dependencies:**
- `tokio`
- `tokio-tungstenite`
- `futures-util`

**Build:**
```bash
cargo build --features websocket-sync
```

**Use case:** Direct device-to-device sync over network

---

#### `content-defined-chunking` (Default: ON)
Advanced chunking using Rabin fingerprinting for better deduplication.

**Enables:**
- Rolling hash CDC
- Variable-size chunks
- Better dedup on similar files

**Build:**
```bash
# Disable for simpler fixed-size chunking:
cargo build --no-default-features --features fixed-chunking
```

**Use case:** Burst photos, edited versions, RAW+JPEG pairs

---

#### `video-recording` (Default: OFF)
Video capture and encoding via FFmpeg.

**Enables:**
- H.264/H.265/VP9/AV1 encoding
- Multiple quality presets
- Real-time video compression

**Dependencies:**
- `ffmpeg-sys`
- FFmpeg development libraries

**Build:**
```bash
cargo build --features video-recording
```

**Use case:** Video capture on camera GUI

---

#### `ai-features` (Default: OFF)
AI-powered scene detection and photo analysis.

**Enables:**
- TensorFlow Lite integration
- Scene classification
- Auto-settings adjustment
- Face detection

**Dependencies:**
- `tflite`
- TensorFlow Lite libraries
- Pre-trained models (~20 MB)

**Build:**
```bash
cargo build --features ai-features
```

**Use case:** Automatic photo enhancement

---

#### `raw-support` (Default: OFF)
RAW image format support with DNG output.

**Enables:**
- Capture RAW+JPEG
- DNG file writing
- LibRaw integration

**Dependencies:**
- `libraw-sys`
- `tinydng`

**Build:**
```bash
cargo build --features raw-support
```

**Use case:** Professional photography

---

#### `qr-scanner` (Default: OFF)
QR code and barcode scanning.

**Enables:**
- Real-time QR detection
- Barcode scanning
- ZXing integration

**Dependencies:**
- `zxing-cpp`

**Build:**
```bash
cargo build --features qr-scanner
```

**Use case:** Utility scanning features

---

#### `cloud-sync` (Default: OFF)
Cloud backup and synchronization.

**Enables:**
- WebDAV support
- Nextcloud integration
- S3-compatible storage

**Dependencies:**
- `reqwest`
- `aws-sdk-s3`

**Build:**
```bash
cargo build --features cloud-sync
```

**Use case:** Automatic cloud backup

---

#### `exif-full` (Default: OFF)
Complete EXIF metadata support with exiv2.

**Enables:**
- EXIF reading and writing
- GPS coordinate embedding
- Maker notes

**Dependencies:**
- `exiv2-sys`
- exiv2 development libraries

**Build:**
```bash
cargo build --features exif-full
```

**Use case:** Professional metadata handling

---

## Feature Combinations

### Minimal Build (Embedded Devices)
**Size:** ~2 MB binary
```bash
cargo build --release \
    --no-default-features \
    --features "fixed-chunking"
```

**Includes:**
- Basic deduplication
- zstd compression
- SQLite database
- Fixed-size chunking only

---

### Standard Build (Default)
**Size:** ~5 MB binary
```bash
cargo build --release
```

**Includes:**
- Content-defined chunking
- XXH64 and SHA-256 hashing
- Full deduplication
- Thumbnail generation

---

### Full Featured Build
**Size:** ~15-20 MB binary
```bash
cargo build --release \
    --features "websocket-sync,video-recording,raw-support,exif-full,qr-scanner"
```

**Includes:**
- All sync methods
- Video recording
- RAW support
- QR scanning
- Full EXIF

---

### Professional Photography Build
**Size:** ~12 MB binary
```bash
cargo build --release \
    --features "raw-support,exif-full,ai-features"
```

**Includes:**
- RAW+JPEG capture
- Full EXIF metadata
- AI scene detection
- Professional controls

---

### Server Build (Headless)
**Size:** ~8 MB binary
```bash
cargo build --release \
    --no-default-features \
    --features "content-defined-chunking,websocket-sync,cloud-sync"
```

**Includes:**
- Advanced deduplication
- WebSocket sync server
- Cloud backup
- No GUI components

---

## Build Profiles

### Development
```toml
[profile.dev]
opt-level = 0
debug = true
```

```bash
cargo build --features "websocket-sync,video-recording"
```

---

### Release
```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

```bash
cargo build --release --features "content-defined-chunking"
```

---

### Size-Optimized Release
```toml
[profile.release-small]
inherits = "release"
opt-level = "z"
lto = true
codegen-units = 1
strip = true
panic = "abort"
```

```bash
cargo build --profile release-small --no-default-features
```

---

## Configuration File

Create `~/.config/photosync/features.toml`:

```toml
[features]
# Core deduplication
chunking_strategy = "content-defined"  # or "fixed-size"
hash_algorithm = "xxh64"               # or "sha256"
compression_level = 19                 # 1-22

# Optional features
websocket_sync = false
video_recording = false
raw_support = false
ai_features = false
qr_scanner = false
cloud_sync = false
exif_full = true  # Recommended

[chunking]
# Content-defined chunking settings
min_chunk_size = 262144   # 256 KB
max_chunk_size = 8388608  # 8 MB
target_chunk_size = 1048576  # 1 MB

[websocket]
# WebSocket sync settings (if enabled)
server_addr = "0.0.0.0:8765"
auto_sync = true
heartbeat_interval = 30  # seconds

[cloud]
# Cloud sync settings (if enabled)
provider = "nextcloud"  # or "webdav", "s3"
endpoint = "https://cloud.example.com"
username = "user"
# password stored in keyring

[ai]
# AI features settings (if enabled)
model_path = "/usr/share/photosync/models/mobilenet_v2.tflite"
confidence_threshold = 0.7
auto_adjust_settings = true
```

---

## Runtime Feature Detection

The daemon detects enabled features at runtime:

```rust
use photosync::features;

fn main() {
    println!("PhotoSync Features:");
    println!("  Chunking: {}", features::chunking_strategy());
    println!("  Hash: {}", features::hash_algorithm());
    
    #[cfg(feature = "websocket-sync")]
    println!("  WebSocket sync: enabled");
    
    #[cfg(feature = "video-recording")]
    println!("  Video recording: enabled");
    
    #[cfg(feature = "raw-support")]
    println!("  RAW support: enabled");
    
    #[cfg(feature = "ai-features")]
    println!("  AI features: enabled");
}
```

---

## Dependency Management

### Minimal Dependencies
```toml
[dependencies]
rusqlite = { version = "0.31", features = ["bundled"] }
zstd = "0.13"
sha2 = "0.10"
xxhash-rust = { version = "0.8", features = ["xxh64"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### Optional Dependencies
```toml
[dependencies]
# WebSocket sync
tokio = { version = "1.35", features = ["full"], optional = true }
tokio-tungstenite = { version = "0.21", optional = true }
futures-util = { version = "0.3", optional = true }

# Video recording
ffmpeg-sys = { version = "6.0", optional = true }

# RAW support
libraw-sys = { version = "0.2", optional = true }
tinydng = { version = "0.1", optional = true }

# AI features
tflite = { version = "0.2", optional = true }

# QR scanning
zxing-cpp = { version = "0.2", optional = true }

# Cloud sync
reqwest = { version = "0.11", features = ["json"], optional = true }
aws-sdk-s3 = { version = "1.0", optional = true }

# EXIF
exiv2-sys = { version = "0.1", optional = true }
```

---

## Testing Features

Test specific feature combinations:

```bash
# Test minimal build
cargo test --no-default-features

# Test with websocket
cargo test --features websocket-sync

# Test all features
cargo test --all-features
```

---

## Packaging

### Fedora RPM
```spec
# Base package (minimal)
Name: photosync-system
...

# Full package
Name: photosync-system-full
BuildRequires: ffmpeg-devel exiv2-devel
Features: --all-features
...
```

### Alpine APK
```bash
# Minimal for embedded
apk add photosync-minimal

# Full for workstation
apk add photosync-full
```

---

## Platform-Specific Builds

### PinePhone (ARM, Limited Resources)
```bash
cargo build --release \
    --target aarch64-unknown-linux-gnu \
    --no-default-features \
    --features "content-defined-chunking"
```

### Laptop (x86_64, Full Features)
```bash
cargo build --release \
    --features "websocket-sync,video-recording,raw-support,exif-full,ai-features"
```

### Server (Headless)
```bash
cargo build --release \
    --no-default-features \
    --features "websocket-sync,cloud-sync"
```

---

## Migration Guide

### From Fixed to Content-Defined Chunking

If you want to migrate existing data:

```bash
# Re-process all files with new chunking strategy
photosync-cli reprocess --chunking content-defined

# Garbage collect old blocks
photosync-cli gc
```

### Enabling New Features

To enable features after initial install:

```bash
# Rebuild with new features
cargo build --release --features "video-recording,raw-support"

# Reinstall
sudo make install

# Restart daemon
systemctl --user restart photosync.service
```

---

This feature flag system allows PhotoSync to scale from minimal embedded devices to full-featured workstations while maintaining a single codebase!
