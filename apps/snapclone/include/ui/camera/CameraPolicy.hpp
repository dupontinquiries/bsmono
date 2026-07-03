#pragma once

// ─── ui/camera/CameraPolicy.hpp ───────────────────────────────────────────────
//
// CameraPolicy — named timeouts and limits for the camera capture path.
//
// History: V32.32-V32.33 had timeouts and dimensions buried inline:
//   • 2000ms waitForStarted in GstLaunchBackend::start
//   • 5000ms first-frame watchdog (added V32.34+)
//   • 2000ms early-exit window for alternate-node retry
//   • 480×640 BGRx hard-coded into the gst-launch caps
//   • SNAPCLONE_CAMERA_ROTATION as the only knob exposed to users
//
// That layout had three problems:
//   1. No central place to look when tuning for new hardware
//   2. No way to expose configuration in the UI without one ad-hoc
//      Settings binding per constant
//   3. No way for test code to override the policy for fast-fail
//      probing (probes want short timeouts; real captures want long)
//
// V32.34+: CameraPolicy holds these as named fields, loaded from
// QSettings on construction, mutable at runtime, with a singleton
// accessor. Backends read m_policy at start time. Settings page
// edits fields and emits changed() so live backends can adapt
// (today: only the dropdown rebuilds; future: re-tune in-flight).
//
// Threading: CameraPolicy is NOT thread-safe by design. Edit only
// from the GUI thread (Settings page). Backend threads read a
// snapshot captured at start() time and don't observe later changes
// — restart the backend to pick up new timeouts. This avoids needing
// a lock per timeout-check.

#include <QObject>
#include <QSettings>

namespace SnapClone::UI::Camera {

/// Default value rationale documented at each field. All durations
/// are milliseconds; all dimensions are pixels.
struct CameraPolicy {
    // ── Startup / health ──────────────────────────────────────────────

    /// How long to wait for the capture subprocess (gst-launch) to
    /// announce that it's running. Failure here is "the launcher binary
    /// isn't on PATH" or "the kernel was so loaded that fork+exec took
    /// >2s". The value matches the historical hard-coded constant.
    int  startupTimeoutMs = 2000;

    /// How long to wait between "subprocess running" and "first frame
    /// produced". Failure here is "libcamera opened the sensor but the
    /// kernel driver is stuck producing buffers" — happens on FP5 after
    /// suspend/resume. Empirically 5s is enough margin for cold-boot
    /// libcamera initialization (which can do non-trivial sensor
    /// calibration), without making the user stare at a frozen viewfinder
    /// indefinitely.
    int  firstFrameTimeoutMs = 5000;

    /// Window after start() during which an unexpected exit is treated
    /// as "wrong node, try the next one" rather than "broken environment".
    /// After this window any exit is reported as a hard failure. 2s is
    /// usually enough — early failures (no such element, missing format)
    /// fire within ms; legitimate playback typically survives many
    /// seconds before any natural exit.
    int  earlyExitWindowMs = 2000;

    /// Whether the early-exit handler should automatically try the next
    /// device in availableDevices() rather than reporting failure. Used
    /// by the default-device path (deviceId empty in start()); off when
    /// the user explicitly picked a device, since "your choice didn't
    /// work" shouldn't silently substitute "but this other one did".
    bool autoRetryAlternate = true;

    // ── Probe-mode overrides ──────────────────────────────────────────

    /// Probe mode shortens the timeouts dramatically — we want a quick
    /// yes/no per node, not a 5s wait per probe. These three values are
    /// USED INSTEAD OF the timeouts above when a backend is started
    /// with `probeMode=true`. CameraTester sets probeMode; the user-
    /// facing camera path leaves it false.
    int  probeStartupTimeoutMs   = 1000;
    int  probeFirstFrameTimeoutMs = 1500;
    int  probeEarlyExitWindowMs  = 800;

    // ── Pipeline capture format ──────────────────────────────────────

    /// Output frame width / height the BGRx caps clamp to. The current
    /// onStdoutData parser assumes constant frame size, so changing
    /// these requires restarting the backend. Width × Height × 4 bytes
    /// per frame is the per-frame buffer size on the pipe.
    ///
    /// **V32.34+: raised defaults from 480×640 to 1280×720.** User reported
    /// the viewfinder looked stuck at ~640px even on cameras capable of
    /// 1080p+. Root cause: this hardcoded default was the actual cap on
    /// frame quality (the pipeline scales the source DOWN to this size
    /// via videoscale). 1280×720 is the natural sweet spot for FP5-class
    /// hardware — high enough to look sharp on a 1080p display, low
    /// enough that frame buffers stay around 3.7MB and the BGRx pipe
    /// runs at 30fps without sustained CPU pressure.
    ///
    /// Capture quality matches viewfinder quality with the current
    /// requestCapture() implementation (emits m_lastFrame). For true
    /// multi-megapixel still-capture we'd need a separate libcamera-
    /// still-photo path; for now, raising these gives a 4× bump in
    /// snap photo resolution at acceptable memory cost.
    ///
    /// Users who need even higher resolution can override via
    /// QSettings camera/policy/captureWidth and captureHeight.
    int  captureWidth  = 1280;
    int  captureHeight = 720;

    /// V32.39+ PERF: cap on the PREVIEW (and therefore capture) frame size.
    /// The pipeline scales DOWN to fit inside previewMaxWidth × previewMaxHeight
    /// (aspect preserved) on the GPU — i.e. BEFORE the gldownload GPU→CPU
    /// readback — so the readback, the CPU repack, and the per-frame QImage all
    /// shrink with the square of the linear scale. On the FP5 the readback of a
    /// 1280×720 BGRx frame is ~3.7 MB EVERY frame; at 1024×576 it's ~2.36 MB
    /// (64%), which is the single biggest lever on preview latency.
    ///
    /// The default 1024×576 is deliberately a clean 16:9 AND a multiple of the
    /// 16-px stride alignment, so the strideAlignPx rounding below is a no-op and
    /// the GPU downscale from a 16:9 sensor mode is perfectly uniform (no AR
    /// squish). For a bigger win try 768×432 (also clean 16:9/16-aligned, ~36%
    /// of 720p); for sharper snaps raise it or set 0 = no cap.
    ///
    /// IMPORTANT — capture coupling: a captured snap is the most-recent preview
    /// frame (requestCapture() returns lastFrame()), so this cap also bounds the
    /// resolution of SENT snaps. 1024×576 is plenty for ephemeral viewing on a
    /// phone. A decoupled high-res still-grab would need a separate capture
    /// pipeline (future work).
    ///
    /// 0 on EITHER dimension disables the cap. Logged loudly at start().
    /// Settings: camera/policy/previewMaxWidth, previewMaxHeight.
    int  previewMaxWidth   = 1024;
    int  previewMaxHeight  = 576;

    /// V32.39+ PERF: optional preview framerate cap. When > 0 a
    /// `videorate drop-only=true max-rate=N` is inserted BEFORE the GPU
    /// colorspace stage, so frames above N fps are dropped before they pay the
    /// glupload/convert/gldownload cost (drop-only never duplicates, so a source
    /// slower than N is untouched). The leaky queue already provides backpressure
    /// when the consumer falls behind; this is an explicit lever to trade preview
    /// smoothness for CPU/GPU headroom + battery. 0 = no cap (default — let the
    /// queue regulate). Capture quality is unaffected (only cadence changes).
    /// Settings: camera/policy/previewMaxFps.
    int  previewMaxFps     = 0;

    /// V32.34+: target SOURCE resolution requested from pipewiresrc.
    /// Distinct from captureWidth/Height (which is what we PRODUCE for
    /// the app). The source resolution is what we ask libcamera /
    /// pipewire to deliver to us BEFORE our pipeline scales it down.
    ///
    /// **DEFAULT: DISABLED (0)** as of V32.34+ after device testing.
    /// FP5 libcamera does not publish a 1280×720@30 mode that
    /// pipewiresrc can negotiate to, so a non-zero target caused
    /// "Format negotiation failed / not-negotiated (-4)" on both
    /// rear and front cameras and broke ALL camera startup. The
    /// fields stay for users / devices where explicit upstream caps
    /// help (some libcameras with multiple modes will pick a better
    /// one when asked), but enabling this is now an opt-in step
    /// via Settings or QSettings edit. The original motivation
    /// (sensor sticking in low-quality preview mode) needs a
    /// different mechanism — likely camera-bridge-side
    /// CAPTURE_WIDTH/HEIGHT env vars which DO work because the
    /// bridge negotiates directly with libcamera at session-open
    /// time, before pipewire is involved.
    ///
    /// To enable manually: edit ~/.config/SnapCloneProject/SnapClone.conf
    /// [camera/policy]
    /// sourceTargetWidth=1280
    /// sourceTargetHeight=720
    /// sourceTargetFps=30
    int  sourceTargetWidth   = 0;
    int  sourceTargetHeight  = 0;
    int  sourceTargetFps     = 0;

    /// V32.34+ GPU accel: prefer GPU colorspace conversion + scaling over
    /// the CPU `videoconvert ! videoscale` path when the GL elements are
    /// installed. On FP5 (Adreno 619) this *could* offload the YUV→RGB +
    /// downscale work to the GPU.
    ///
    /// **Default flipped to FALSE in V32.34+ after device testing.** On the
    /// FP5 / pmOS Plasma Mobile stack the GL element set is incomplete:
    /// `gldownload` refused to link to fdsink even for a plain BGRx sysmem
    /// cap ("can't handle caps video/x-raw,format=BGRx,1280x720"), so the
    /// GPU pipeline failed to start at all and the camera was dead. The CPU
    /// dual-videoconvert path is proven working on the same device. The GPU
    /// pipeline construction has since been corrected (convert/scale now
    /// happen IN GL memory before gldownload, which only transfers to
    /// sysmem — see buildConvertScaleSegment), but whether THIS device's GL
    /// stack negotiates it is still unproven, so we ship CPU-by-default and
    /// leave GPU as opt-in for testing:
    ///   ~/.config/SnapCloneProject/SnapClone.conf
    ///   [camera/policy] preferGpuConvert=true
    /// Re-enable by default only once a device confirms the GPU path
    /// actually starts AND delivers frames.
    bool preferGpuConvert    = false;

    /// V32.34+ tint fix: force the output buffer stride to exactly
    /// width×4 bytes (no SIMD row padding). The fd pipe carries raw
    /// bytes with no stride metadata, and our QImage reader assumes
    /// width×4. When GStreamer pads rows for alignment, the reader
    /// drifts → tiled/sheared preview. A `capsfilter` can't express
    /// stride, so we instead constrain the output WIDTH to a multiple of
    /// this value at start() (rounding down), making width×4 land on the
    /// alignment boundary so tight stride == padded stride.
    ///
    /// V32.34+ TILING FIX: default raised 4 → 16. The old comment claimed
    /// "4 px = 16 bytes = GStreamer's alignment", but many elements —
    /// notably gldownload on the GPU path, and libcamera/v4l2 sources —
    /// align rows to 64 BYTES, not 16. With strideAlignPx=4 a width like
    /// 1284 gives a 5136-byte tight row that a 64-byte-aligning element
    /// pads to 5184, and the reader (assuming 5136) walks diagonally →
    /// tiled framebuffer. strideAlignPx=16 forces width to a multiple of
    /// 16, so width×4 is a multiple of 64 — tight == padded for the common
    /// 64-byte case, and still fine for 16/32. Costs ≤15 px off the width
    /// (invisible). Set to 1 to disable; the per-frame sink-caps re-sync
    /// and bloat diagnostic are the safety nets if a device aligns wider.
    int  strideAlignPx       = 16;

    /// V32.50+ PERF: skip the CPU repack `videoconvert` that follows
    /// `gldownload` on the GPU colorspace path. That repack exists only to
    /// guarantee a tight (width×4) row stride for the fd reader, and costs a
    /// full-frame CPU copy every frame (~W×H×4 bytes × fps of memory traffic).
    /// With strideAlignPx=16 the width is already a multiple of 16, so width×4
    /// lands on a 64-byte boundary — meaning on any GPU whose GL row alignment
    /// is ≤64 bytes the download is ALREADY tight and the repack is pure
    /// overhead. Enabling this drops it. Off by default because the safe
    /// outcome (correct preview) depends on the device's actual GL alignment:
    /// if a GPU aligns wider than width×4, the preview will tile/shear and you
    /// should turn this back off (or raise strideAlignPx to match).
    ///   [camera/policy] gpuSkipDownloadRepack=true
    bool gpuSkipDownloadRepack = false;

    /// V32.34+ GPU accel: allow higher source resolution when GPU
    /// conversion is active. When preferGpuConvert succeeds, the
    /// expensive conversion is off the CPU, so we can afford to ask
    /// the source for a higher resolution and downscale on the GPU.
    /// 0 = use sourceTargetWidth/Height as-is. When set and GPU path
    /// is active, overrides the source request. Off by default to keep
    /// behavior conservative until device-tested.
    int  gpuSourceWidth      = 0;
    int  gpuSourceHeight     = 0;

    /// V32.34+: if first-frame size is smaller than the target source
    /// resolution by more than this ratio, log a warning. The check
    /// runs ONCE on first frame. Helps diagnose "we asked for 720p but
    /// got 480p" without flooding logs. Ratio 0.75 means "if we got
    /// less than 75% of what we asked for in either dimension, warn".
    qreal lowQualityRatio    = 0.75;

    /// V32.34+: how aggressively to RESTART the pipeline when we detect
    /// it's been operating in low quality. Three levels:
    ///   • "off"     — only log, never auto-restart
    ///   • "first"   — restart ONCE after first frame if low quality.
    ///                  Most useful: catches sensor stuck in preview mode.
    ///   • "always"  — restart every time low quality is detected
    ///                  (after first frame OR via periodic re-check).
    ///                  Aggressive; only enable if "first" doesn't help.
    /// Default "first" because preview-mode-latch is the dominant
    /// failure pattern and one restart usually clears it.
    QString autoQualityRestart = QStringLiteral("first");

    // ── Frame timing health ──────────────────────────────────────────

    /// V32.34+: if median frame interval over the last N samples
    /// exceeds (1000/fps) × this ratio, log a warning. Detects "camera
    /// works but at 15fps when we asked for 30". 1.5 means "warn if
    /// median frame interval is >1.5× target". The N (sample count)
    /// is fixed at 30 so the median responds to ~1s of frames.
    qreal frameRateDegradedRatio = 1.5;

    /// Rotation applied via videoflip in the pipeline. "none", "90l",
    /// "90r", "180". Empty / "none" means no rotation element inserted.
    /// SNAPCLONE_CAMERA_ROTATION env var still takes precedence so dev
    /// debugging via shell stays one-line.
    QString rotation;

    // ── Frame-drop watchdog ───────────────────────────────────────────

    /// If frames stop arriving for this duration AFTER first-frame
    /// success, the backend treats the source as having died and emits
    /// stopped(). Distinct from firstFrameTimeoutMs: this is the
    /// "running and then went silent" case (USB camera unplugged,
    /// kernel module unloaded). 3s is enough that brief stalls don't
    /// flap the camera, but short enough that disconnections are
    /// noticed before the user gives up.
    int  framedropTimeoutMs = 3000;

    // ── Diagnostic logging ────────────────────────────────────────────

    /// How much gst-launch stderr to retain for error-translation hints.
    /// 4KB is enough to capture the typical multi-line GStreamer error
    /// trail (element + pad + downgrade chain).
    int  stderrBufMaxBytes = 4096;

    /// Whether to log a per-frame dimension/timestamp at the first
    /// frame. Useful for confirming "yes the source is delivering
    /// 1920×1080 even though we caps'd to 480×640". On by default,
    /// rate-limited to first frame so it doesn't spam at 30Hz.
    bool logFirstFrameInfo = true;

    /// V32.34+ PiP FIX: whether this device can capture TWO cameras at once.
    /// Picture-in-Picture (rear viewfinder + front selfie overlay) needs two
    /// simultaneous `pipewiresrc` streams. On the FP5 — and most single-ISP
    /// mobile camera stacks — the front and rear sensors share one ISP pipeline
    /// and CANNOT both be live; opening the second fails negotiation ("Failed to
    /// build output pipeline"), which is why PiP appeared to silently do nothing.
    /// Default false so PiP is NOT offered (and, if forced, fails with a clear
    /// message) on hardware we can't prove supports it. Set true on desktops or
    /// devices with independent sensors / USB webcams, where simultaneous
    /// capture genuinely works. Settings key: camera/policy/simultaneousCapture.
    /// The richer per-source concurrency-group model lives in the Rust
    /// snapclone-videosrc crate; this single bool is the conservative C++ gate
    /// until that model is wired through CameraRegistry.
    bool simultaneousCaptureSupported = false;
};

/// Singleton-accessor + QSettings binding for CameraPolicy. The
/// Settings page reads fields, edits them, calls save() on commit.
/// Backends call instance().snapshot() at start time to capture a
/// thread-local copy that won't change underneath them.
class CameraPolicyStore : public QObject
{
    Q_OBJECT
public:
    static CameraPolicyStore &instance();

    /// Current values. Read-only access via snapshot is preferred from
    /// non-GUI threads; this returns a const reference so non-thread-safe
    /// callers don't accidentally hold a mutable handle.
    [[nodiscard]] const CameraPolicy &current() const { return m_policy; }

    /// Cheap copy for backends to capture at start() time. Backends keep
    /// THIS snapshot for the duration of one session; live changes to
    /// fields don't affect already-started backends.
    [[nodiscard]] CameraPolicy snapshot() const { return m_policy; }

    /// Replace the whole policy. Emits changed(). Persists to QSettings
    /// (group "camera/policy") so the next launch picks up the same
    /// values. Use this from Settings save handlers.
    void set(const CameraPolicy &p);

    /// Reset to compile-time defaults. Useful for "factory reset" or
    /// "I broke something in the settings, get me back". Persists.
    void resetToDefaults();

signals:
    /// Emitted after any field change. Settings UI listens to re-render
    /// editing controls if the policy was changed externally (rare —
    /// only really triggered by resetToDefaults). Backends do NOT need
    /// to listen — they capture snapshots at start time and rely on
    /// restart for picking up updates.
    void changed();

private:
    CameraPolicyStore();
    void load();
    void save() const;

    CameraPolicy m_policy;
};

} // namespace SnapClone::UI::Camera
