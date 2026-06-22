# PhotoSync Camera - Complete Features Roadmap

## Implementation Status & Priorities

### ✅ Implemented (v0.1.1)

- [x] **Core camera functionality**
  - Aggressive camera detection (V4L2, libcamera, PipeWire)
  - Real-time preview with libcamera
  - Photo capture with flash animation
  - Camera switching (front/back)

- [x] **Manual controls**
  - Focus modes (Auto/Manual/Continuous/Macro)
  - White balance presets (Auto/Daylight/Cloudy/etc.)
  - Exposure compensation (-5 to +5 EV)
  - Digital zoom (1x-10x with pinch gesture)
  - HDR mode toggle
  - Night mode toggle

- [x] **Metadata system**
  - GPS coordinates with location toggle
  - Camera properties (ID, name, driver)
  - Camera settings at capture
  - Timestamp in ISO 8601 format
  - Image properties (resolution, format, size)
  - JSON sidecar files

- [x] **Processing pipeline**
  - Automatic thumbnail generation (256x256)
  - zstd compression integration
  - Block-level deduplication
  - Multi-threaded processing
  - Progress indicators

- [x] **UI/UX**
  - Snapchat-style fullscreen interface
  - Touch gestures (tap, long-press, pinch, swipe)
  - Processing indicator with progress bar
  - Toast notifications
  - Location indicator
  - Settings drawer

- [x] **Debugging**
  - Comprehensive logging with context
  - Thread tracking
  - Performance metrics
  - Error handling
  - Debug documentation

### 🚧 In Progress (v0.1.2)

#### Video Recording [HIGH PRIORITY]
**Status:** Code complete, needs integration
**Components:**
- [x] VideoRecorder class implemented
- [x] FFmpeg H.264/H.265/VP9/AV1 encoding
- [x] Quality presets (Low/Medium/High/VeryHigh/Ultra)
- [x] Frame rate control
- [x] Bitrate control
- [ ] Integration with CameraManager
- [ ] QML UI for video controls
- [ ] Video metadata (duration, codec, bitrate)

**Timeline:** 1-2 days
**Complexity:** Medium
**Dependencies:** FFmpeg development libraries

#### EXIF Integration [HIGH PRIORITY]
**Status:** Code ready, needs exiv2 library
**Features:**
- [ ] GPS coordinates in EXIF tags
- [ ] DateTime in standard EXIF fields
- [ ] Camera info (Make, Model)
- [ ] Maker notes for custom data
- [ ] EXIF editing tools

**Timeline:** 2-3 days
**Complexity:** Medium
**Dependencies:** exiv2-devel, libexif

#### Reverse Geocoding [MEDIUM PRIORITY]
**Status:** API integration pending
**Features:**
- [ ] Nominatim API integration
- [ ] Location name from coordinates
- [ ] Caching of geocoded locations
- [ ] Offline mode (cached only)

**Timeline:** 1-2 days
**Complexity:** Low
**Dependencies:** Qt Network, internet connection

### 📋 Planned (v0.2.0)

#### RAW Image Support [HIGH PRIORITY]
**Features:**
- [ ] Capture RAW+JPEG simultaneously
- [ ] DNG (Digital Negative) output
- [ ] RAW preview with embedded JPEG
- [ ] RAW metadata preservation
- [ ] LibRaw integration for processing

**Timeline:** 5-7 days
**Complexity:** High
**Dependencies:** libcamera RAW support, LibRaw/TinyDNG

**Impact:**
- Professional photography workflow
- Maximum image quality
- Post-processing flexibility
- Large file sizes (need compression!)

#### Burst Mode with Smart Selection [HIGH PRIORITY]
**Features:**
- [ ] Configurable burst count (5/10/20/50/100 shots)
- [ ] Configurable interval (fastest/0.5s/1s)
- [ ] Automatic best photo selection:
  - Sharpness detection (Laplacian variance)
  - Exposure quality
  - Face detection (OpenCV)
  - Motion detection
- [ ] Keep all or best only mode
- [ ] Burst gallery view

**Timeline:** 7-10 days
**Complexity:** High
**Dependencies:** OpenCV for face detection

**Impact:**
- Action/sports photography
- Higher keeper rate
- Deduplication shines (90%+ savings)

#### Time-Lapse & Interval Shooting [MEDIUM PRIORITY]
**Features:**
- [ ] Configurable interval (1s to 24h)
- [ ] Configurable duration or shot count
- [ ] Auto-exposure adjustment
- [ ] Video generation from frames (ffmpeg)
- [ ] Preview playback
- [ ] Background operation

**Timeline:** 3-5 days
**Complexity:** Medium
**Dependencies:** ffmpeg for video generation

**Impact:**
- Creative photography
- Documenting processes
- Astronomy/nature photography

#### Live Filters & Effects [LOW PRIORITY]
**Features:**
- [ ] GPU-accelerated OpenGL shaders
- [ ] Real-time filter preview
- [ ] Filters:
  - Black & White
  - Sepia/Vintage
  - HDR effect
  - Vignette
  - Blur/Sharpen
  - Beauty mode
- [ ] Custom LUT (Look-Up Table) support
- [ ] Filter intensity slider

**Timeline:** 5-7 days
**Complexity:** High
**Dependencies:** OpenGL ES 3.0+, shader programming

**Impact:**
- Social media ready photos
- Creative expression
- Reduced post-processing time

### 🎯 Planned (v0.3.0)

#### QR Code / Barcode Scanner [MEDIUM PRIORITY]
**Features:**
- [ ] Real-time QR code detection
- [ ] Barcode scanning (EAN, UPC, Code128, etc.)
- [ ] ZXing library integration
- [ ] Scan history
- [ ] Auto-action on scan (open URL, add contact, etc.)
- [ ] Scan overlay with targeting box

**Timeline:** 3-5 days
**Complexity:** Medium
**Dependencies:** ZXing C++

**Impact:**
- Utility beyond photography
- Payment apps integration
- Contact sharing

#### AI Scene Detection [HIGH PRIORITY]
**Features:**
- [ ] TensorFlow Lite integration
- [ ] Pre-trained MobileNet model
- [ ] Scene classification:
  - Landscape
  - Portrait
  - Food
  - Sunset
  - Indoor
  - Sport
  - Night
  - Document
- [ ] Auto-adjust settings per scene
- [ ] Scene-specific optimizations

**Timeline:** 7-10 days
**Complexity:** High
**Dependencies:** TensorFlow Lite, pre-trained models

**Impact:**
- Better photos automatically
- Beginner-friendly
- Professional results

#### Panorama Mode [HIGH PRIORITY]
**Features:**
- [ ] Guided panorama capture (left to right)
- [ ] Real-time alignment preview
- [ ] Image stitching (OpenCV)
- [ ] 180° and 360° modes
- [ ] Vertical panoramas
- [ ] Auto-cropping

**Timeline:** 10-14 days
**Complexity:** Very High
**Dependencies:** OpenCV stitching module

**Impact:**
- Wide-angle photography without wide lens
- Landscapes, architecture
- High-resolution outputs

#### Pro Mode [MEDIUM PRIORITY]
**Features:**
- [ ] Manual shutter speed control
- [ ] Manual ISO control
- [ ] Manual focus with focus peaking
- [ ] Histogram display
- [ ] Zebra pattern (overexposure warning)
- [ ] Grid overlays (rule of thirds, golden ratio)
- [ ] Level indicator

**Timeline:** 5-7 days
**Complexity:** Medium
**Dependencies:** libcamera manual control support

**Impact:**
- Professional photographers
- Creative control
- Learning tool

### 🚀 Planned (v1.0.0)

#### Portrait Mode with Depth [HIGH PRIORITY]
**Features:**
- [ ] ToF sensor integration for depth map
- [ ] Background blur (bokeh simulation)
- [ ] Adjustable blur strength
- [ ] Edge detection for subject isolation
- [ ] Dual-camera depth estimation (if available)
- [ ] Portrait lighting effects

**Timeline:** 14-21 days
**Complexity:** Very High
**Dependencies:** ToF sensor, depth processing algorithms

**Impact:**
- Professional portrait photography
- DSLR-like results
- Social media appeal

#### Cloud Sync & Backup [MEDIUM PRIORITY]
**Features:**
- [ ] WebDAV integration
- [ ] Nextcloud sync
- [ ] Google Photos API
- [ ] Automatic upload
- [ ] Selective sync (starred photos only)
- [ ] Conflict resolution
- [ ] Bandwidth management

**Timeline:** 10-14 days
**Complexity:** High
**Dependencies:** Network access, cloud provider APIs

**Impact:**
- Automatic backup
- Access from anywhere
- Share with family

#### Photo Editor [MEDIUM PRIORITY]
**Features:**
- [ ] Crop and rotate
- [ ] Brightness/contrast/saturation
- [ ] Curves adjustment
- [ ] HSL sliders
- [ ] Local adjustments (dodge/burn)
- [ ] Healing brush
- [ ] Undo/redo history
- [ ] Save as copy

**Timeline:** 14-21 days
**Complexity:** Very High
**Dependencies:** Image processing library

**Impact:**
- All-in-one photo app
- No external editor needed
- Quick fixes

#### Gallery View [MEDIUM PRIORITY]
**Features:**
- [ ] Grid view with thumbnails
- [ ] Timeline organization (by date)
- [ ] Albums and collections
- [ ] Search by metadata (location, date, camera)
- [ ] Favorites/starring
- [ ] Slideshow mode
- [ ] Share to apps
- [ ] Delete/move operations

**Timeline:** 10-14 days
**Complexity:** High
**Dependencies:** Database for fast queries

**Impact:**
- Complete photo management
- Easier to find photos
- Photo organization

### 🌟 Future Ideas (v2.0+)

#### Advanced Features
- [ ] Astrophotography mode (star trails, long exposure stacking)
- [ ] Macro mode with focus stacking
- [ ] HDR video recording
- [ ] Slow-motion video (high frame rate)
- [ ] 360° photo/video support
- [ ] Light painting mode
- [ ] Multiple exposure blending
- [ ] RAW processing pipeline
- [ ] Watermarking
- [ ] EXIF editor
- [ ] GPS track recording
- [ ] Photo geotagging from GPX

#### AI-Powered Features
- [ ] Object detection and labeling
- [ ] Face recognition and tagging
- [ ] Smart photo suggestions
- [ ] Auto-enhance (AI-based)
- [ ] Style transfer filters
- [ ] Image upscaling (AI)
- [ ] Noise reduction (AI)
- [ ] Sky replacement
- [ ] Remove unwanted objects

#### Social & Sharing
- [ ] Direct share to social media
- [ ] Photo stories/albums
- [ ] Collaborative albums
- [ ] Print ordering integration
- [ ] Photo books creation
- [ ] Canvas prints

#### Platform Integration
- [ ] Waydroid support (Android in Linux)
- [ ] Desktop mode (windowed)
- [ ] Multiple camera support
- [ ] External camera (USB)
- [ ] Webcam mode
- [ ] Screen recording
- [ ] Presentation mode

## Priority Matrix

### Must Have (P0)
1. Video recording
2. EXIF integration
3. RAW support
4. Burst mode
5. AI scene detection

### Should Have (P1)
6. Time-lapse
7. Pro mode
8. Panorama
9. Portrait mode
10. QR scanner

### Nice to Have (P2)
11. Live filters
12. Cloud sync
13. Photo editor
14. Gallery view
15. Reverse geocoding

## Resource Requirements

### Development Time Estimate
- **v0.1.2** (Video, EXIF, Geocoding): 1-2 weeks
- **v0.2.0** (RAW, Burst, Time-lapse): 3-4 weeks
- **v0.3.0** (QR, AI, Panorama): 4-6 weeks
- **v1.0.0** (Portrait, Cloud, Editor): 6-8 weeks

### Hardware Requirements
- **ToF sensor**: Portrait mode, depth features
- **Dual cameras**: Depth estimation backup
- **GPU**: Filter processing, AI inference
- **Storage**: RAW files need significant space

### Library Dependencies
- **libcamera**: Core camera functionality (✓ have)
- **FFmpeg**: Video encoding (need)
- **exiv2**: EXIF metadata (need)
- **OpenCV**: Face detection, stitching (need)
- **TensorFlow Lite**: AI features (need)
- **ZXing**: QR/barcode scanning (need)
- **LibRaw**: RAW processing (need)
- **Qt Positioning**: GPS (✓ have)
- **Qt Multimedia**: Audio/video (✓ have)

### Disk Space Impact
- **Code**: ~5 MB
- **Libraries**: ~50 MB
- **AI models**: ~20 MB
- **Per photo (JPEG)**: 2-5 MB
- **Per photo (RAW)**: 20-40 MB
- **Per video (1min 1080p)**: 50-100 MB

## Testing Strategy

### Manual Testing (Each Release)
- [ ] Test on PinePhone
- [ ] Test on Librem 5
- [ ] Test on x86_64 laptop
- [ ] Test on ARM single-board computer
- [ ] Test with USB camera
- [ ] Test with built-in camera

### Automated Testing (Future)
- [ ] Unit tests for metadata
- [ ] Unit tests for image processing
- [ ] Integration tests for pipeline
- [ ] Performance benchmarks
- [ ] Memory leak detection
- [ ] Thread safety validation

### User Acceptance Testing
- [ ] Beta program
- [ ] Community feedback
- [ ] Bug bounty program
- [ ] Performance metrics collection

## Documentation Needs

### User Documentation
- [ ] Feature tutorials (video)
- [ ] Photography tips
- [ ] Troubleshooting FAQ
- [ ] Platform-specific guides

### Developer Documentation
- [ ] API reference
- [ ] Plugin system
- [ ] Contributing guide
- [ ] Code style guide

This roadmap will evolve based on community feedback and priorities. What would you like implemented next?
