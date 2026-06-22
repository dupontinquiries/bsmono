# PhotoSync Camera GUI

Snapchat-style camera interface with aggressive camera detection, libcamera integration, and full manual controls for Linux phones and laptops.

## Features

### 🎥 Advanced Camera Detection

**Aggressive Multi-Source Scanning:**
- **V4L2 devices** - Scans all /dev/video* devices (0-63)
- **libcamera** - Native libcamera device enumeration
- **PipeWire** - Camera sources via PipeWire (when available)
- **Automatic discovery** on app startup

**Device Capabilities:**
- Query supported formats (YUYV, MJPEG, etc.)
- Enumerate resolutions (from 640x480 to 4K+)
- Detect available controls (focus, white balance, exposure, etc.)
- Time-of-Flight (ToF) sensor detection

### 📸 Snapchat-Style Interface

**Touch Controls:**
- **Tap** - Capture photo
- **Long press** - Record video
- **Pinch** - Zoom (1x - 10x)
- **Swipe up/down** - Adjust exposure compensation
- **Double tap** - Switch front/back camera

**UI Elements:**
- Large circular capture button
- Real-time preview with libcamera
- Camera selector drawer (swipe from left)
- Settings drawer (swipe from bottom)
- Minimal, fullscreen design

### 🎛️ Professional Camera Controls

**Focus Modes:**
- Auto Focus (AF)
- Manual Focus (MF)
- Continuous Auto Focus (CAF)
- Macro Focus

**White Balance:**
- Auto
- Daylight (5500K)
- Cloudy (6500K)
- Tungsten (3200K)
- Fluorescent (4000K)
- Manual

**Advanced Features:**
- HDR mode (multi-exposure)
- Night mode (extended exposure)
- Exposure compensation (-5 to +5 EV)
- Digital zoom (1x - 10x)
- Time-of-Flight distance measurement

### ⚡ Performance Optimizations

**Threading:**
- Camera scanning on background thread
- Photo processing on worker thread
- Non-blocking UI interactions

**Hardware Acceleration:**
- Mesa/DRM integration for GPU rendering
- GBM (Generic Buffer Management) for zero-copy
- Hardware-accelerated video decode
- SIMD optimizations (via -march=native)

**Memory Efficiency:**
- Zero-copy frame buffers where possible
- Efficient QML/C++ integration
- Optimized texture uploads

## Building

### Dependencies

**Fedora/RHEL:**
```bash
sudo dnf install \
    cmake \
    qt5-qtbase-devel \
    qt5-qtdeclarative-devel \
    qt5-qtquickcontrols2-devel \
    qt5-qtmultimedia-devel \
    libcamera-devel \
    mesa-libgbm-devel \
    mesa-libEGL-devel \
    libdrm-devel
```

**Alpine Linux:**
```bash
sudo apk add \
    cmake \
    qt5-qtbase-dev \
    qt5-qtdeclarative-dev \
    qt5-qtquickcontrols2-dev \
    qt5-qtmultimedia-dev \
    libcamera-dev \
    mesa-gbm \
    mesa-egl \
    libdrm-dev
```

### Build Instructions

```bash
cd gui

# Create build directory
mkdir build && cd build

# Configure with CMake
cmake .. \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=/usr

# Build (with all cores)
cmake --build . -j$(nproc)

# Install
sudo cmake --install .
```

### Quick Build Script

```bash
#!/bin/bash
cd gui
rm -rf build
mkdir build && cd build
cmake .. -DCMAKE_BUILD_TYPE=Release
make -j$(nproc)
sudo make install
```

## Usage

### Launch the App

```bash
# From desktop
# Click "PhotoSync Camera" icon

# From terminal
photosync-camera

# For mobile (Phosh/Plasma Mobile)
# App appears in app drawer
```

### Camera Selection

1. **Automatic Selection** - First available camera is selected on startup
2. **Manual Selection**:
   - Tap camera name in top-left
   - Drawer shows all detected cameras
   - Select different camera
   - Preview switches immediately

### Capture Photos

1. **Point and shoot**:
   - Tap white circular button
   - Photo saves to ~/Pictures/PhotoSync
   - Automatic compression and deduplication

2. **Manual controls**:
   - Swipe from bottom for settings
   - Adjust focus mode, white balance, etc.
   - Changes apply in real-time

3. **Advanced features**:
   - Enable HDR for high-contrast scenes
   - Enable night mode for low light
   - Use macro focus for close-ups
   - Check ToF distance (if available)

### Record Videos

1. **Long press** the capture button
2. Recording starts (button turns red, pulsing)
3. **Release** to stop recording
4. Video saves to ~/Pictures/PhotoSync

### Zoom

**Method 1: Pinch gesture**
- Use two fingers to pinch in/out
- Zoom slider appears on right
- Range: 1x to 10x

**Method 2: Zoom slider**
- Slider appears during pinch
- Drag up/down to adjust
- Shows current zoom level (e.g., "2.5x")

### Exposure Compensation

**Swipe up/down anywhere on screen:**
- Swipe up: Increase exposure (+)
- Swipe down: Decrease exposure (-)
- Range: -5 EV to +5 EV
- Exposure indicator shows on left

## Integration with PhotoSync

### Automatic Processing

Every captured photo is automatically:

1. **Thumbnail generated** (256x256 JPEG)
2. **Compressed** with zstd level 19
3. **Deduplicated** via block hashing
4. **Registered** in SQLite database
5. **Synced** via Syncthing (if enabled)

### Processing Pipeline

```
Camera Capture
    │
    ├─> Save original to ~/Pictures/PhotoSync/
    │
    ├─> Generate thumbnail (256x256)
    │   └─> Store in ~/.cache/photosync/thumbnails/
    │
    ├─> Call photosync-daemon
    │   ├─> Chunk into 1 MiB blocks
    │   ├─> Hash each block (SHA-256)
    │   ├─> Check for duplicates
    │   ├─> Compress new blocks (zstd -19)
    │   └─> Update database
    │
    └─> Syncthing detects change
        └─> Sync to other devices
```

### Viewing Processing Stats

In QML interface:
- Progress indicator during processing
- Toast notification on completion
- Shows original size vs. compressed size
- Displays space savings percentage

## libcamera Integration Details

### Camera Initialization

```cpp
// 1. Create camera manager
auto cm = std::make_shared<CameraManager>();
cm->start();

// 2. Acquire camera
for (const auto &cam : cm->cameras()) {
    if (cam->id() == selectedId) {
        camera = cam;
        break;
    }
}
camera->acquire();

// 3. Generate configuration
auto config = camera->generateConfiguration({StreamRole::Viewfinder});

// 4. Configure stream
StreamConfiguration &cfg = config->at(0);
cfg.pixelFormat = formats::YUV420;
cfg.size = Size(1920, 1080);

// 5. Apply configuration
camera->configure(config.get());
```

### Frame Capture

```cpp
// 1. Allocate buffers
auto allocator = new FrameBufferAllocator(camera);
allocator->allocate(stream);

// 2. Create request
auto request = camera->createRequest();
request->addBuffer(stream, buffers[0].get());

// 3. Set controls
ControlList &controls = request->controls();
controls.set(controls::AfMode, controls::AfModeAuto);
controls.set(controls::AwbMode, controls::AwbAuto);

// 4. Queue request
camera->queueRequest(request.get());

// 5. Start capture
camera->start();
```

### Control Mapping

| libcamera Control | UI Element | Values |
|-------------------|------------|--------|
| `AfMode` | Focus mode selector | Auto, Manual, Continuous, Macro |
| `AwbMode` | White balance selector | Auto, Daylight, Cloudy, Tungsten, Fluorescent |
| `ExposureValue` | Exposure slider | -5.0 to +5.0 EV |
| `HdrMode` | HDR toggle | Off, MultiExposure |
| `AfRange` | Macro focus | Normal, Macro |

## Performance Tuning

### Compiler Optimizations

The CMakeLists.txt includes aggressive optimizations:

```cmake
-O3              # Maximum optimization
-march=native    # Use CPU-specific instructions (SIMD, etc.)
-mtune=native    # Tune for current CPU
-flto            # Link-time optimization
```

**Expected performance improvements:**
- 20-30% faster frame processing
- Better NEON/SSE utilization on ARM/x86
- Reduced binary size with LTO

### Threading Configuration

**Camera Scanner Thread:**
- Runs camera detection in background
- Doesn't block UI during scan
- Emits signals when cameras found

**Photo Processor Thread:**
- Processes captured photos asynchronously
- Compression and deduplication
- Progress updates via signals

**Main UI Thread:**
- QML rendering and user input
- Camera preview display
- Always responsive

### Memory Optimization

**Zero-Copy Paths:**
- libcamera → GBM buffer → EGL texture
- No memcpy() for preview frames
- Direct GPU upload where possible

**Buffer Reuse:**
- Frame buffers allocated once
- Reused for each capture
- No per-frame allocation overhead

## Mobile Optimization

### For Linux Phones (PinePhone, Librem 5, etc.)

**Touch-Optimized UI:**
- Large touch targets (minimum 44x44 dp)
- Swipe gestures for common actions
- No hover-dependent controls

**Battery Efficiency:**
- Camera preview at 30 FPS (configurable)
- Aggressive sleep when idle
- Wake on screen touch

**Portrait Mode:**
- Default to portrait layout
- Auto-rotation support
- Optimized for 720x1440 screens

## Troubleshooting

### No Cameras Found

```bash
# Check V4L2 devices
ls -la /dev/video*

# Check libcamera
libcamera-hello --list-cameras

# Check permissions
sudo usermod -aG video $USER
# Log out and back in
```

### Camera Won't Open

```bash
# Check if another app is using it
lsof /dev/video0

# Kill blocking process
pkill -9 cheese  # or other camera app

# Try different camera
photosync-camera --camera /dev/video1
```

### Poor Performance

```bash
# Enable hardware acceleration
export QT_QUICK_BACKEND=software  # Try different backends

# Check GPU driver
glxinfo | grep OpenGL

# Rebuild with optimizations
cd build
cmake .. -DCMAKE_BUILD_TYPE=Release
make -j$(nproc)
```

### Black Screen / No Preview

```bash
# Check libcamera permissions
sudo chmod 666 /dev/video*

# Try software rendering
QT_QUICK_BACKEND=software photosync-camera

# Check logs
journalctl -f | grep photosync
```

## Development

### Adding New Camera Features

1. **Add control to CameraManager.h**:
```cpp
Q_PROPERTY(int newControl READ newControl WRITE setNewControl NOTIFY newControlChanged)
```

2. **Implement in CameraManager.cpp**:
```cpp
void CameraManager::setNewControl(int value) {
    m_newControl = value;
    emit newControlChanged();
    applyControlSettings();
}
```

3. **Apply to libcamera**:
```cpp
void CameraManager::applyControlSettings() {
    controls.set(controls::NewControl, m_newControl);
}
```

4. **Add UI in main.qml**:
```qml
Slider {
    from: 0
    to: 100
    value: cameraManager.newControl
    onValueChanged: cameraManager.setNewControl(value)
}
```

### Testing

```bash
# Unit tests (TODO)
cd build
ctest

# Manual testing
photosync-camera --verbose

# Memory leak check
valgrind --leak-check=full photosync-camera
```

## Architecture

```
┌─────────────────────────────────────────┐
│          Qt/QML Interface               │
│  (main.qml - Snapchat UI)               │
└────────────┬────────────────────────────┘
             │
             ├─> CameraManager (C++)
             │   ├─ Camera Scanner (threaded)
             │   ├─ libcamera integration
             │   └─ Controls (focus, WB, etc.)
             │
             └─> PhotoProcessor (C++)
                 ├─ Thumbnail generation
                 ├─ Compression (zstd)
                 └─ PhotoSync integration
                     └─> photosync-daemon
```

## License

Same as main PhotoSync system: MIT OR Apache-2.0

## See Also

- [Main PhotoSync README](../README.md)
- [libcamera Documentation](https://libcamera.org/docs.html)
- [Qt Documentation](https://doc.qt.io/)
