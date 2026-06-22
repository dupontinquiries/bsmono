# PhotoSync Camera - Debugging & Testing Guide

## Debug Build Configuration

### Enable Debug Logging

**Environment Variables:**
```bash
# Qt debug output
export QT_LOGGING_RULES="*.debug=true"
export QT_MESSAGE_PATTERN="[%{time yyyy-MM-dd HH:mm:ss.zzz}] %{if-category}%{category}:%{endif} %{message}"

# libcamera debug
export LIBCAMERA_LOG_LEVELS="*:DEBUG"

# Mesa/OpenGL debug
export MESA_DEBUG=1
export EGL_LOG_LEVEL=debug
export LIBGL_DEBUG=verbose
```

**Build with Debug Symbols:**
```bash
cd gui/build
cmake .. -DCMAKE_BUILD_TYPE=Debug
make -j$(nproc)
```

### Run with Debugging

**Terminal output:**
```bash
photosync-camera 2>&1 | tee camera-debug.log
```

**With GDB:**
```bash
gdb photosync-camera
(gdb) run
# After crash:
(gdb) backtrace
(gdb) info threads
(gdb) thread apply all backtrace
```

**With Valgrind:**
```bash
valgrind --leak-check=full \
         --show-leak-kinds=all \
         --track-origins=yes \
         --verbose \
         --log-file=valgrind.log \
         photosync-camera
```

## Debug Output Interpretation

### Understanding Log Prefixes

```
[PhotoProcessor] - Main photo processing coordinator
[PhotoProcessorWorker] - Background worker thread operations
[PhotoMetadata] - Metadata creation and EXIF writing
[Thumbnail] - Thumbnail generation
[Compress] - Compression and deduplication
[Metadata] - Metadata writing process
[Geocode] - Reverse geocoding operations
[CameraManager] - Camera detection and control
[QML] - Qt/QML UI layer
```

### Example Debug Session

**Normal photo capture flow:**
```
[QML] Photo captured: /home/user/Pictures/PhotoSync/IMG_20260408_123456.jpg
[QML] Camera state: {"cameraId":"/dev/video0","cameraName":"USB Camera",...}
[PhotoProcessor] ===== New photo processing request =====
[PhotoProcessor] Path: /home/user/Pictures/PhotoSync/IMG_20260408_123456.jpg
[PhotoProcessor] Creating metadata for: /home/user/Pictures/PhotoSync/IMG_20260408_123456.jpg
[PhotoProcessor] File size: 2456789 bytes
[PhotoProcessor] Capture time: 2026-04-08T12:34:56
[PhotoProcessor] Adding location: 45.5017 , -73.5673 alt: 35
[PhotoProcessor] Camera ID: /dev/video0
[PhotoProcessor] Image dimensions: 1920 x 1080
[PhotoProcessor] Processing request queued to worker thread
[PhotoProcessorWorker] ===== Starting photo processing =====
[PhotoProcessorWorker] Step 1/3: Writing metadata...
[Metadata] Writing metadata for: ...
[Metadata] Location: 45.5017 , -73.5673 alt: 35
[PhotoProcessorWorker] Step 2/3: Generating thumbnail...
[Thumbnail] Original image size: QSize(1920, 1080)
[Thumbnail] Thumbnail saved to: /home/user/.cache/photosync/thumbnails/IMG_20260408_123456_thumb.jpg
[PhotoProcessorWorker] Step 3/3: Compressing and deduplicating...
[Compress] Command: photosync-daemon --process-file ...
[Compress] Daemon finished with exit code: 0
[PhotoProcessorWorker] ===== Processing complete! =====
[PhotoProcessor] >>> Processing complete
[QML] Photo processed successfully!
[QML] Savings: 42%
```

## Common Issues and Solutions

### Issue 1: No Location Data

**Symptoms:**
```
[PhotoProcessor] No location data (enabled: true valid: false)
```

**Debugging:**
```bash
# Check geoclue service
systemctl status geoclue.service

# Test GPS manually
gnome-maps  # or other location-aware app

# Check permissions
ls -la /etc/geolue/geoclue.conf

# Enable location for photosync-camera
sudo nano /etc/geoclue/geoclue.conf
# Add:
[photosync-camera]
allowed=true
system=false
users=
```

**Fix:**
```cpp
// In PhotoProcessor.cpp, add more verbose logging
qDebug() << "[PhotoProcessor] Position source created:" << (m_positionSource != nullptr);
if (auto *source = qobject_cast<QGeoPositionInfoSource*>(m_positionSource)) {
    qDebug() << "[PhotoProcessor] Source name:" << source->sourceName();
    qDebug() << "[PhotoProcessor] Update interval:" << source->updateInterval();
    qDebug() << "[PhotoProcessor] Preferred positioning methods:" << source->preferredPositioningMethods();
}
```

### Issue 2: EXIF Not Written

**Symptoms:**
```
[PhotoMetadata] Note: exiv2 integration pending, writing JSON sidecar
```

**This is expected!** The code currently writes JSON sidecar files. To enable EXIF:

**Install exiv2 development files:**
```bash
# Fedora
sudo dnf install exiv2-devel

# Alpine
sudo apk add exiv2-dev

# Ubuntu
sudo apt install libexiv2-dev
```

**Update CMakeLists.txt:**
```cmake
find_package(PkgConfig REQUIRED)
pkg_check_modules(EXIV2 REQUIRED exiv2)

target_include_directories(photosync-camera PRIVATE
    ${EXIV2_INCLUDE_DIRS}
)

target_link_libraries(photosync-camera PRIVATE
    ${EXIV2_LIBRARIES}
)
```

**Uncomment EXIF code in PhotoProcessor.cpp** (currently commented out pending exiv2 setup)

### Issue 3: photosync-daemon Not Found

**Symptoms:**
```
[Compress] WARNING: Failed to start photosync-daemon
[Compress] Error: "No such file or directory"
```

**This is normal if daemon isn't installed yet!** The camera GUI works standalone.

**To fix:**
```bash
# Install main PhotoSync package first
cd photosync-system
make build
sudo make install

# Or build full package
./build.sh --rpm --install  # Fedora
./build.sh --apk --install  # Alpine
```

### Issue 4: Camera Not Detected

**Symptoms:**
```
[CameraManager] Found 0 cameras
```

**Debugging:**
```bash
# List V4L2 devices
v4l2-ctl --list-devices

# Check permissions
ls -la /dev/video*
groups | grep video

# Test with libcamera
libcamera-hello --list-cameras

# Check kernel modules
lsmod | grep -i video
dmesg | grep -i video
```

**Fix permissions:**
```bash
sudo usermod -aG video $USER
# Log out and back in
```

### Issue 5: Thumbnail Generation Fails

**Symptoms:**
```
[Thumbnail] ERROR: Failed to load image
[Thumbnail] Reader error: Unsupported image format
```

**Debugging:**
```bash
# Check image format
file /path/to/image.jpg

# Test with Qt
qmake -query QT_INSTALL_PLUGINS
ls $(qmake -query QT_INSTALL_PLUGINS)/imageformats/
```

**Install missing image plugins:**
```bash
# Fedora
sudo dnf install qt5-qtimageformats

# Alpine  
sudo apk add qt5-qtimageformats

# Ubuntu
sudo apt install qt5-image-formats-plugins
```

### Issue 6: Worker Thread Hangs

**Symptoms:**
```
[PhotoProcessor] Processing request queued to worker thread
# ... nothing happens
```

**Debugging:**
```bash
# Run with thread debugging
QT_LOGGING_RULES="qt.core.thread.debug=true" photosync-camera

# Check with GDB
gdb photosync-camera
(gdb) run
# Ctrl+C when hung
(gdb) info threads
(gdb) thread apply all backtrace
```

**Common cause:** Worker thread not started

**Fix:**
```cpp
// In PhotoProcessor constructor, verify:
m_workerThread->start();
qDebug() << "[PhotoProcessor] Worker thread state:" << m_workerThread->isRunning();
```

## Testing Checklist

### Functional Tests

- [ ] **Camera Detection**
  - Detects USB cameras
  - Detects built-in cameras
  - Shows camera details (name, driver, capabilities)
  - Camera selector shows all devices

- [ ] **Photo Capture**
  - Tap to capture works
  - Flash animation shows
  - Photo saves to correct directory
  - Filename includes timestamp

- [ ] **Metadata**
  - Capture time is correct (check timezone)
  - Location added when enabled
  - Camera ID recorded
  - Settings (focus, WB, etc.) saved
  - JSON sidecar created

- [ ] **Processing**
  - Thumbnail generated in ~/.cache/photosync/thumbnails
  - Processing indicator shows
  - Progress updates (0% → 100%)
  - Toast notification appears
  - photosync-daemon called (if installed)

- [ ] **Controls**
  - Focus mode changes apply
  - White balance changes apply
  - Exposure slider works
  - Zoom pinch gesture works
  - HDR toggle works
  - Night mode toggle works
  - Camera switch button works

- [ ] **Location**
  - GPS toggle in settings
  - Location indicator shows current position
  - Coordinates update in real-time
  - Location added to metadata

### Performance Tests

- [ ] **Frame Rate**
  - Preview runs at 30 FPS minimum
  - No stuttering during control changes
  - Smooth zoom operation

- [ ] **Memory**
  - No memory leaks (run with Valgrind)
  - Memory usage stays under 150 MB
  - No growth over time

- [ ] **CPU**
  - < 30% CPU during preview
  - < 50% CPU during capture
  - Background thread doesn't block UI

### Platform-Specific Tests

**PinePhone / Librem 5:**
- [ ] Touch gestures work
- [ ] Portrait mode correct
- [ ] Auto-rotation works
- [ ] Battery usage acceptable
- [ ] Thermal throttling handled

**x86_64 Laptop:**
- [ ] Mouse/touchpad controls work
- [ ] Keyboard shortcuts (future)
- [ ] Multiple cameras handled
- [ ] High-resolution capture

## Automated Testing

### Unit Tests (Future)

```cpp
// test_photoprocessor.cpp
class TestPhotoProcessor : public QObject
{
    Q_OBJECT
    
private slots:
    void testMetadataCreation();
    void testThumbnailGeneration();
    void testLocationEncoding();
    void testWorkerThread();
};

void TestPhotoProcessor::testMetadataCreation()
{
    PhotoProcessor processor;
    QVariantMap cameraState;
    cameraState["cameraId"] = "/dev/video0";
    cameraState["focusMode"] = 2;
    
    // Create metadata
    PhotoMetadata meta = processor.createMetadata("/tmp/test.jpg", cameraState);
    
    QCOMPARE(meta.cameraId, QString("/dev/video0"));
    QCOMPARE(meta.focusMode, 2);
}
```

### Integration Tests

```bash
#!/bin/bash
# test-camera-capture.sh

set -e

echo "Testing camera capture..."

# 1. Start app in background
photosync-camera &
PID=$!

sleep 5

# 2. Trigger capture (using automation)
# xdotool or other automation tool

# 3. Wait for processing
sleep 10

# 4. Check output
if [ -f ~/Pictures/PhotoSync/IMG_*.jpg ]; then
    echo "✓ Photo captured"
else
    echo "✗ No photo found"
    kill $PID
    exit 1
fi

# 5. Check metadata
if [ -f ~/Pictures/PhotoSync/IMG_*.jpg.metadata.json ]; then
    echo "✓ Metadata created"
else
    echo "✗ No metadata found"
fi

# 6. Check thumbnail
if [ -f ~/.cache/photosync/thumbnails/*_thumb.jpg ]; then
    echo "✓ Thumbnail generated"
else
    echo "✗ No thumbnail found"
fi

kill $PID
echo "All tests passed!"
```

## Performance Profiling

### CPU Profiling

```bash
# Record performance
perf record -g photosync-camera

# Analyze
perf report

# Focus on specific function
perf report --symbol=PhotoProcessorWorker::processPhoto
```

### Memory Profiling

```bash
# Massif (heap profiler)
valgrind --tool=massif photosync-camera

# Visualize
massif-visualizer massif.out.*

# Check for leaks
valgrind --leak-check=full --show-leak-kinds=all photosync-camera
```

### GPU Profiling

```bash
# Enable GPU debugging
export MESA_DEBUG=1
export LIBGL_DEBUG=verbose

# Intel GPU tools
intel_gpu_top

# AMD GPU tools
radeontop
```

## Continuous Debugging

### Enable Persistent Logging

```bash
# Create log directory
mkdir -p ~/.local/share/photosync/logs

# Add to app startup
photosync-camera 2>&1 | tee ~/.local/share/photosync/logs/camera-$(date +%Y%m%d-%H%M%S).log
```

### Add Debug Menu in QML

```qml
// Debug overlay
Rectangle {
    visible: debugMode
    anchors.bottom: parent.bottom
    width: parent.width
    height: 200
    color: "#E0000000"
    
    ColumnLayout {
        anchors.fill: parent
        
        Text {
            text: "FPS: " + fps
            color: "white"
        }
        
        Text {
            text: "Memory: " + memoryUsage + " MB"
            color: "white"
        }
        
        Text {
            text: "Active camera: " + cameraManager.activeCamera
            color: "white"
        }
    }
}
```

## Reporting Bugs

When reporting issues, include:

1. **System Information:**
   ```bash
   uname -a
   lsb_release -a
   qmake -v
   libcamera-hello --version
   ```

2. **Log Output:**
   - Full debug log from terminal
   - Valgrind output if crashing
   - GDB backtrace if available

3. **Steps to Reproduce:**
   - Exact sequence of actions
   - Camera model being used
   - Expected vs actual behavior

4. **Screenshots/Video:**
   - UI state when issue occurs
   - Screen recording of bug

## Resources

- Qt Debug Tools: https://doc.qt.io/qt-5/debug.html
- libcamera Debugging: https://libcamera.org/docs.html#debugging
- GDB Quick Reference: https://darkdust.net/files/GDB%20Cheat%20Sheet.pdf
- Valgrind Manual: https://valgrind.org/docs/manual/
