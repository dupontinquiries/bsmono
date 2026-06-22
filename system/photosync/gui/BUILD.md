# PhotoSync Camera GUI - Complete Build Guide

## Overview

The PhotoSync Camera GUI is a Snapchat-style camera application with:
- **Aggressive camera detection** (V4L2, libcamera, PipeWire)
- **libcamera integration** with full manual controls
- **Hardware acceleration** via Mesa/DRM/GBM
- **Multi-threaded architecture** for performance
- **Professional controls** (focus, white balance, ToF, HDR)
- **Touch-optimized UI** for Linux phones

## Prerequisites

### System Requirements

**Minimum:**
- Linux kernel 4.14+ (for V4L2)
- Qt 5.15+
- libcamera 0.0.1+
- Mesa 21.0+
- 2 GB RAM
- Any camera (USB, CSI, MIPI)

**Recommended:**
- Linux 6.0+ (for latest camera drivers)
- Qt 5.15.2+
- libcamera 0.1.0+
- Mesa 23.0+ (for better GPU support)
- 4 GB RAM
- Multiple cameras for testing

### Platform-Specific Notes

**PinePhone / PinePhone Pro:**
- Use Megi's kernel for better camera support
- Enable MIPI CSI camera in device tree
- Install libcamera-pinephone package

**Librem 5:**
- Use PureOS camera drivers
- Enable i.MX8 camera support
- Test with both front and rear cameras

**x86_64 Laptops:**
- Most USB cameras work out of the box
- Some built-in cameras need firmware
- Check `lsusb` and `v4l2-ctl --list-devices`

## Installation

### Method 1: Package Manager (Recommended)

**Fedora/RHEL:**
```bash
# Install main package + GUI
sudo dnf install photosync-system photosync-system-gui

# Or build RPM with GUI
cd photosync-system
./build.sh --rpm
sudo dnf install ~/rpmbuild/RPMS/*/photosync-system-*.rpm
```

**Alpine Linux:**
```bash
# Install main package + GUI subpackage
sudo apk add photosync-system photosync-system-gui

# Or build APK with GUI
cd photosync-system
./build.sh --apk
sudo apk add --allow-untrusted ~/packages/photosync-system/*.apk
```

### Method 2: Build from Source

#### Step 1: Install Dependencies

**Fedora/RHEL:**
```bash
sudo dnf install -y \
    cmake \
    qt5-qtbase-devel \
    qt5-qtdeclarative-devel \
    qt5-qtquickcontrols2-devel \
    qt5-qtmultimedia-devel \
    libcamera-devel \
    mesa-libgbm-devel \
    mesa-libEGL-devel \
    mesa-dri-drivers \
    libdrm-devel \
    gcc-c++
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
    mesa-dev \
    mesa-gbm \
    mesa-egl \
    mesa-dri-gallium \
    libdrm-dev \
    g++
```

**Ubuntu/Debian:**
```bash
sudo apt install -y \
    cmake \
    qtbase5-dev \
    qtdeclarative5-dev \
    qml-module-qtquick-controls2 \
    qtmultimedia5-dev \
    libcamera-dev \
    libgbm-dev \
    libegl1-mesa-dev \
    libdrm-dev \
    g++
```

#### Step 2: Build

```bash
cd photosync-system/gui

# Create build directory
mkdir build && cd build

# Configure
cmake .. \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=/usr/local

# Build (use all CPU cores)
make -j$(nproc)

# Install
sudo make install
```

#### Step 3: Verify Installation

```bash
# Check binary
which photosync-camera

# Check desktop entry
ls -la /usr/local/share/applications/photosync-camera.desktop

# Test launch
photosync-camera --help
```

## Development Setup

### IDE Configuration

**Qt Creator:**
```bash
# Open CMakeLists.txt as project
qtcreator gui/CMakeLists.txt

# Configure kit:
# - Qt 5.15+
# - GCC/Clang with C++17
# - CMake 3.16+

# Build settings:
# - Release for testing
# - Debug for development
```

**VSCode:**
```json
// .vscode/settings.json
{
    "cmake.configureSettings": {
        "CMAKE_BUILD_TYPE": "Debug",
        "CMAKE_EXPORT_COMPILE_COMMANDS": "ON"
    },
    "C_Cpp.default.configurationProvider": "ms-vscode.cmake-tools"
}
```

### Build Variants

**Debug Build (with symbols):**
```bash
cmake .. -DCMAKE_BUILD_TYPE=Debug
make -j$(nproc)
./photosync-camera
```

**Release Build (optimized):**
```bash
cmake .. -DCMAKE_BUILD_TYPE=Release
make -j$(nproc)
```

**Release with Debug Info:**
```bash
cmake .. -DCMAKE_BUILD_TYPE=RelWithDebInfo
make -j$(nproc)
```

**Minimum Size Release:**
```bash
cmake .. -DCMAKE_BUILD_TYPE=MinSizeRel
make -j$(nproc)
strip photosync-camera
```

## Performance Tuning

### Compiler Optimizations

The default CMakeLists.txt includes:
```cmake
-O3              # Aggressive optimization
-march=native    # CPU-specific instructions
-mtune=native    # Tune for current CPU
-flto            # Link-time optimization
```

**For cross-compilation (e.g., building for ARM on x86):**
```bash
cmake .. \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_C_COMPILER=aarch64-linux-gnu-gcc \
    -DCMAKE_CXX_COMPILER=aarch64-linux-gnu-g++ \
    -DCMAKE_C_FLAGS="-march=armv8-a" \
    -DCMAKE_CXX_FLAGS="-march=armv8-a"
```

### Runtime Optimizations

**Qt Quick Scene Graph:**
```bash
# Use threaded renderer
export QSG_RENDER_LOOP=threaded

# Force OpenGL backend
export QT_QUICK_BACKEND=

# Enable texture atlasing
export QSG_ATLAS_WIDTH=2048
export QSG_ATLAS_HEIGHT=2048
```

**libcamera:**
```bash
# Enable debug logs
export LIBCAMERA_LOG_LEVELS=*:DEBUG

# Disable debug logs (production)
export LIBCAMERA_LOG_LEVELS=*:ERROR
```

**Mesa/DRM:**
```bash
# Force specific GPU driver
export MESA_LOADER_DRIVER_OVERRIDE=panfrost  # For ARM Mali

# Enable debug
export MESA_DEBUG=1
export EGL_LOG_LEVEL=debug
```

## Testing

### Camera Detection

```bash
# List V4L2 devices
v4l2-ctl --list-devices

# Test with libcamera
libcamera-hello --list-cameras

# Query capabilities
v4l2-ctl -d /dev/video0 --all
```

### Manual Testing

```bash
# Launch with verbose logging
photosync-camera --verbose

# Test specific camera
photosync-camera --camera /dev/video0

# Force software rendering
QT_QUICK_BACKEND=software photosync-camera
```

### Performance Profiling

**Frame rate:**
```bash
# Enable FPS counter
QSG_VISUALIZE=overdraw photosync-camera
```

**CPU profiling:**
```bash
perf record -g photosync-camera
perf report
```

**Memory profiling:**
```bash
valgrind --tool=massif photosync-camera
massif-visualizer massif.out.*
```

## Troubleshooting

### Common Issues

**1. No cameras detected**

```bash
# Check permissions
ls -la /dev/video*
groups  # Should include 'video'

# Add yourself to video group
sudo usermod -aG video $USER
# Log out and back in

# Check kernel modules
lsmod | grep -i video
sudo modprobe uvcvideo
```

**2. Camera opens but black screen**

```bash
# Test with simple tool
libcamera-hello --camera 0

# Check if format is supported
v4l2-ctl -d /dev/video0 --list-formats-ext

# Try different backend
QT_QUICK_BACKEND=software photosync-camera
```

**3. Crashes on startup**

```bash
# Get backtrace
gdb photosync-camera
(gdb) run
# After crash:
(gdb) backtrace

# Check dependencies
ldd photosync-camera
```

**4. Poor performance**

```bash
# Check GPU driver
glxinfo | grep "OpenGL renderer"

# Force GPU rendering
export QT_QUICK_BACKEND=

# Reduce resolution
# Edit main.qml: cfg.size = Size(1280, 720)
```

**5. Controls don't work**

```bash
# Check available controls
v4l2-ctl -d /dev/video0 --list-ctrls

# Test control manually
v4l2-ctl -d /dev/video0 --set-ctrl=focus_auto=0
```

### Platform-Specific Issues

**PinePhone:**
```bash
# Enable camera in device tree
sudo nano /boot/dtb/allwinner/sun50i-a64-pinephone-1.2.dts
# Add camera node

# Recompile device tree
sudo dtc -I dts -O dtb -o /boot/dtb/...

# Load module
sudo modprobe sun6i_csi
```

**Librem 5:**
```bash
# Check i.MX8 camera
dmesg | grep -i imx8

# Load drivers
sudo modprobe imx8mq_mipi_csi2
sudo modprobe dcmipp
```

## Advanced Features

### Custom Camera Modes

Add custom resolutions in `CameraManager.cpp`:

```cpp
void CameraManager::configureCamera()
{
    // ...existing code...
    
    // Add 4K mode
    if (supportsResolution(3840, 2160)) {
        cfg.size = Size(3840, 2160);
    }
}
```

### HDR Implementation

The HDR mode uses multi-exposure:

```cpp
controls.set(controls::HdrMode, controls::HdrModeMultiExposure);
```

Captures multiple frames at different exposures and combines them.

### Night Mode

Increases exposure and ISO:

```cpp
void CameraManager::enableNightMode(bool enable)
{
    if (enable) {
        controls.set(controls::ExposureTime, 33333); // 1/30s
        controls.set(controls::AnalogueGain, 8.0);   // High ISO
    }
}
```

### ToF Integration

Time-of-Flight sensor provides distance:

```cpp
void CameraManager::updateToFDistance()
{
    // Read from ToF sensor (e.g., VL53L0X)
    int fd = open("/dev/i2c-1", O_RDWR);
    // I2C communication with ToF sensor
    m_tofDistance = readDistance();
    emit tofDistanceChanged();
}
```

## Packaging for Distribution

### Creating RPM

```bash
# Build with GUI
cd photosync-system
cp packaging/rpm/photosync-system-with-gui.spec ~/rpmbuild/SPECS/

# Create tarball
tar czf ~/rpmbuild/SOURCES/photosync-system-0.1.0.tar.gz .

# Build RPM
rpmbuild -bb ~/rpmbuild/SPECS/photosync-system-with-gui.spec

# Install
sudo dnf install ~/rpmbuild/RPMS/*/photosync-system-*.rpm
```

### Creating APK

```bash
# Copy updated APKBUILD
cd photosync-system
cp packaging/apk/APKBUILD-with-gui ~/packages/photosync-system/APKBUILD

# Build
cd ~/packages/photosync-system
abuild checksum
abuild -r

# Install
sudo apk add ~/packages/*/photosync-system-*.apk
```

### Creating Flatpak

```yaml
# org.photosync.Camera.yaml
app-id: org.photosync.Camera
runtime: org.kde.Platform
runtime-version: '5.15'
sdk: org.kde.Sdk
command: photosync-camera

modules:
  - name: libcamera
    # ... libcamera build config
  
  - name: photosync-camera
    buildsystem: cmake
    sources:
      - type: dir
        path: .
```

```bash
flatpak-builder build org.photosync.Camera.yaml
flatpak-builder --install build org.photosync.Camera.yaml
```

## Contributing

### Code Style

Follow Qt/KDE coding standards:
- Indent: 4 spaces
- Braces: K&R style
- Names: camelCase for methods, m_ prefix for members

### Adding Features

1. Create branch: `git checkout -b feature/new-control`
2. Implement feature
3. Test on at least 2 devices
4. Submit pull request with:
   - Description
   - Screenshots/video
   - Test results

### Reporting Bugs

Include:
- Distribution and version
- Qt version (`qmake -v`)
- libcamera version (`libcamera-hello --version`)
- Camera model
- Logs (`photosync-camera --verbose 2>&1 | tee camera.log`)

## Resources

- [libcamera Documentation](https://libcamera.org/docs.html)
- [Qt Quick Documentation](https://doc.qt.io/qt-5/qtquick-index.html)
- [V4L2 API](https://www.kernel.org/doc/html/latest/userspace-api/media/v4l/v4l2.html)
- [Mesa Documentation](https://docs.mesa3d.org/)

## License

MIT OR Apache-2.0 (same as PhotoSync)
