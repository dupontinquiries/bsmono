#include "ui/video/VideoRecorder.hpp"
#include "ui/video/RecordingMetadata.hpp"  // V32.32: write audio-track presence to cache

#include <QProcess>
#include <QStandardPaths>
#include <QMutex>   // V32.34+ audio: element-probe cache guard
#include <QHash>    // V32.34+ audio: element-probe memo
#include <QTimer>
#include <QFile>
#include <QFileInfo>
#include <QDir>
#include <QStringList>
#include <QLoggingCategory>
#include <QSettings>
#include <QCoreApplication>
#include <QDateTime>     // V32.25: stop() output-grace deadline arithmetic
#include <QEventLoop>    // V32.32: stop() async wait on pipelineEnteredPlaying

namespace SnapClone::UI::Video {

Q_LOGGING_CATEGORY(lcRec, "snapclone.video.recorder")

namespace {

/// Probe which H.264-class encoder is available on this system.
/// Returns the gst element name (and optional element params) to use.
///
/// On Fedora the situation is messy:
///   • x264enc lives in gstreamer1-plugins-ugly (RPM Fusion only, not default repos)
///   • openh264enc lives in gstreamer1-plugin-openh264 (Cisco repo, enabled by default since F33)
///   • vp8enc is in gstreamer1-plugins-good (ALWAYS present on any Linux)
///
/// We probe `gst-inspect-1.0 NAME` for each candidate in priority order and
/// return the first that exists. Cached after the first call.
struct EncoderChoice {
    QString element;          ///< e.g. "x264enc"
    QStringList properties;   ///< e.g. {"tune=zerolatency", "speed-preset=ultrafast"}
    QString containerExt;     ///< ".mkv" for h264, ".webm" for vp8
    QString muxer;            ///< "matroskamux" or "webmmux"
    /// Element to insert between encoder and muxer (empty = none needed).
    /// openh264enc emits raw byte-stream H.264 which matroskamux can't accept
    /// without h264parse converting to AVC format. x264enc auto-negotiates so
    /// it works either way, but h264parse is harmless to include defensively.
    QString parser;
};

EncoderChoice probeEncoder()
{
    // Cache successful probes so we don't pay the 2-6s gst-inspect cost on
    // every record. But DON'T cache failures — if the user discovers no
    // encoder is available and installs gstreamer1-plugins-ugly mid-session,
    // the next record attempt should pick it up without an app restart.
    static EncoderChoice cached;
    if (!cached.element.isEmpty()) return cached;

    auto haveElement = [](const QString &name) -> bool {
        QProcess p;
        p.start(QStringLiteral("gst-inspect-1.0"), {name});
        p.waitForFinished(2000);
        return p.exitCode() == 0;
    };

    // V32.32: Priority 0: v4l2h264enc — hardware H.264 encoder via the
    // SoC's dedicated video accelerator (e.g. Qualcomm Venus on FP5,
    // Cedrus on Allwinner). Uses near-zero CPU and far less battery
    // than software encode at the same bitrate.
    //
    // OPT-IN ONLY via SC_USE_HW_ENCODER=1. The element being installed
    // doesn't guarantee the underlying driver actually produces valid
    // H.264 — on pmOS edge, Venus firmware versions vary and some
    // produce corrupt streams. Defaulting to hardware would silently
    // break recording for affected users. The opt-in lets people who
    // can verify it works on their hardware get the battery benefit.
    //
    // Availability prerequisites (probe-detectable):
    //   - Kernel exposes /dev/video<N> as a V4L2 M2M encoder device
    //   - gstreamer1.0-plugins-good built with V4L2 support
    //   - gst-inspect-1.0 reports "v4l2h264enc"
    //
    // Properties match x264enc's intent (8Mbps source bitrate for AV1
    // transcode) but use the hardware encoder's controls:
    //   - extra-controls "controls,video_bitrate=8000000,..."
    //     is how v4l2h264enc exposes hardware encoder knobs
    //   - h264parse converts the byte-stream output to AVC format
    //     for matroskamux (same reason as openh264enc)
    const bool hwEncoderRequested =
        qEnvironmentVariable("SC_USE_HW_ENCODER", QStringLiteral("0"))
            != QLatin1String("0");
    if (hwEncoderRequested && haveElement(QStringLiteral("v4l2h264enc"))) {
        cached = { QStringLiteral("v4l2h264enc"),
                   { QStringLiteral(
                       "extra-controls=\"controls,video_bitrate=8000000,"
                       "video_gop_size=30,h264_profile=4\"") },
                       // h264_profile=4 = High (best quality the SoC supports)
                       // video_gop_size=30 = key frame every second at 30fps
                   QStringLiteral(".mkv"),
                   QStringLiteral("matroskamux"),
                   QStringLiteral("h264parse") };
        return cached;
    }

    // Priority 1: x264enc — best quality + speed for our use case
    if (haveElement(QStringLiteral("x264enc"))) {
        // 8 Mbps source bitrate: this is the SOURCE for AV1 second-pass
        // transcoding. Higher bitrate here = more visual information for
        // the AV1 encoder to compress optimally. Intermediate file is
        // deleted post-transcode so storage cost is bounded.
        cached = { QStringLiteral("x264enc"),
                   { QStringLiteral("tune=zerolatency"),
                     QStringLiteral("speed-preset=ultrafast"),
                     QStringLiteral("bitrate=8000"),   // 8 Mbps (was 4)
                     QStringLiteral("key-int-max=30") },
                   QStringLiteral(".mkv"),
                   QStringLiteral("matroskamux"),
                   QStringLiteral("h264parse") };  // defensive
        return cached;
    }
    // Priority 2: openh264enc — Cisco's H.264, default-available on Fedora 33+
    // No "tune"/"speed-preset" properties; just bitrate.
    if (haveElement(QStringLiteral("openh264enc"))) {
        // openh264enc emits raw byte-stream H.264 which matroskamux can't
        // ingest directly. h264parse converts byte-stream → AVC packets
        // with proper NAL length prefixes, which matroskamux accepts.
        // Without this, gst-launch errors:
        //   "could not link openh264enc0 to mux"
        //
        // Bitrate: 8 Mbps (was 4 Mbps). Recording is the SOURCE for the
        // AV1 second-pass transcode — higher bitrate here means the
        // transcoder has more visual information to compress, so the
        // final AV1 looks better at the SAME output size. Storage is
        // cheap (file is deleted post-transcode); quality preservation
        // through the pipeline matters.
        //
        // Why complexity=low not medium: openh264enc is realtime-bound
        // (must keep up with camera frame rate or buffers drop). low is
        // the only setting that reliably hits 30 fps on FP5-class ARM.
        // The added preview-branch CPU cost (tee + jpegenc @ 10 fps) is
        // ~5%, which we're using BUDGET for; we can't also bump
        // complexity without risking dropped frames on slower hardware.
        cached = { QStringLiteral("openh264enc"),
                   { QStringLiteral("bitrate=8000000"),  // 8 Mbps source
                     QStringLiteral("complexity=low") },  // realtime constraint
                   QStringLiteral(".mkv"),
                   QStringLiteral("matroskamux"),
                   // V32.35+ (device-logs): config-interval=-1 makes h264parse
                   // re-insert SPS/PPS before every keyframe. WHY: device logs
                   // showed an openh264enc-recorded .mkv FAILING to transcode —
                   // the gst transcoder's `filesrc ! decodebin` choked with
                   // matroskademux "not-negotiated (-4)", then vp9/vp8 also
                   // failed, falling back to sending the raw (bigger, un-AV1'd)
                   // .mkv. The x264enc-recorded file on the other phone
                   // transcoded fine. Leading cause: openh264enc's byte-stream
                   // output, parsed to AVC without periodic SPS/PPS, produces a
                   // matroska track whose codec config isn't self-describing on
                   // re-demux. config-interval=-1 injects the parameter sets so
                   // the recorded file is re-decodable by the transcode path.
                   // (Device-bound: this targets the gst-transcode branch taken
                   // only when ffmpeg is absent; recording itself already worked.)
                   QStringLiteral("h264parse config-interval=-1") };
        return cached;
    }
    // Priority 3: vp8enc — always present in plugins-good. We pivot to webm
    // container since matroska + vp8 isn't the conventional pairing.
    if (haveElement(QStringLiteral("vp8enc"))) {
        cached = { QStringLiteral("vp8enc"),
                   { QStringLiteral("deadline=1"),       // realtime
                     QStringLiteral("target-bitrate=8000000") },  // 8 Mbps source
                   QStringLiteral(".webm"),
                   QStringLiteral("webmmux"),
                   QString() };  // VP8 doesn't need a parser
        return cached;
    }
    // No encoder found — return empty WITHOUT caching, so a subsequent
    // attempt can re-probe (user may have installed an encoder package
    // since this attempt).
    return EncoderChoice{};
}


/// Probe whether autoaudiosrc has at least one usable device. Returns true
/// if audio capture is likely to work, false if we should record video-only.
///
/// Reasons audio capture might fail at runtime:
///   • No microphone hardware (desktop without mic, headless server)
///   • PulseAudio/PipeWire not running (e.g. fresh postmarketOS install
///     before `apk add pipewire-pulse`)
///   • Microphone permission denied (sandboxed apps via xdg-desktop-portal)
///   • Microphone busy in another app (call in progress, etc.)
///
/// We use `gst-device-monitor-1.0 Audio/Source` which lists what's actually
/// available right now. Empty output means no audio devices.
///
/// We deliberately do NOT cache the result — mic state can change at runtime
/// (user grants/revokes permission, external mic hot-plug, another app takes
/// the device, etc.). The probe is only ~50-200ms so re-running per record
/// is acceptable.
bool probeAudioAvailable()
{
    QProcess p;
    p.start(QStringLiteral("gst-device-monitor-1.0"),
            { QStringLiteral("Audio/Source") });
    // 5s timeout — was 2s but observed timing out on a desktop with
    // pipewire+wireplumber where the device enumeration scan takes
    // ~3s on first call after boot. Timing out and "assuming available"
    // means we add the audiosrc branch optimistically — if there's no
    // mic, gst-launch fails the whole pipeline (no audio) instead of
    // recording video-only. The 5s wait at startup is worth the
    // improved correctness.
    if (!p.waitForFinished(5000)) {
        p.kill();
        qCWarning(lcRec) << "gst-device-monitor probe timed out — assuming audio available";
        return true;  // optimistic: pipeline failure is the next safety net
    }
    const QByteArray out = p.readAllStandardOutput();
    // Look for "Device found:" lines — at least one means we have a source
    const bool hasDevice = out.contains("Device found");
    if (!hasDevice) {
        qCInfo(lcRec) << "No audio source detected — will record video-only";
    }
    return hasDevice;
}

/// V32.34+ audio: probe-once whether a gst element is installed, for the
/// optional capture-side loudness compressor. Memoized so repeated
/// records don't re-spawn gst-inspect. Mirrors the camera backend's
/// element-detection pattern. `audiodynamic` lives in gst-plugins-good
/// so it's usually present, but minimal pmOS images may omit it — we
/// fall back to a plain audio branch when absent.
bool audioElementAvailable(const QString &element)
{
    static QMutex mu;
    static QHash<QString, bool> cache;
    QMutexLocker lock(&mu);
    const auto it = cache.constFind(element);
    if (it != cache.constEnd()) return it.value();

    bool present = false;
    const QString inspect = QStandardPaths::findExecutable(
        QStringLiteral("gst-inspect-1.0"));
    if (!inspect.isEmpty()) {
        QProcess p;
        p.start(inspect, { element });
        if (p.waitForFinished(2000)) {
            present = (p.exitStatus() == QProcess::NormalExit
                       && p.exitCode() == 0);
        } else {
            p.kill();
            p.waitForFinished(500);
        }
    }
    cache.insert(element, present);
    qCInfo(lcRec) << "audio gst element" << element
                  << (present ? "available" : "NOT available");
    return present;
}

} // anon



VideoRecorder::VideoRecorder(QObject *parent)
    : QObject(parent)
{
}

void VideoRecorder::setUserRotation(int degrees)
{
    // Normalize to 0/90/180/270. Anything else (e.g. user accidentally
    // passed a value computed from a sensor) clamps to 0 — safer than
    // letting an invalid value drop into the pipeline and produce
    // GStreamer parse errors at launch time.
    if (degrees != 0 && degrees != 90 && degrees != 180 && degrees != 270) {
        qCWarning(lcRec) << "setUserRotation: invalid degrees" << degrees
                          << "— clamping to 0";
        degrees = 0;
    }
    m_userRotation = degrees;
}

VideoRecorder::~VideoRecorder()
{
    if (m_recording) stop();
    if (m_proc) {
        // Same pattern as GstLaunchBackend::stop — disconnect ALL signals
        // before kill so no readyRead*/finished signals fire into our slots
        // during destruction. Without this, a queued signal could deref
        // a half-destroyed VideoRecorder. Don't waitForFinished — that
        // runs a local event loop and re-enters our handler stack.
        m_proc->QObject::disconnect();
        m_proc->kill();
        // V32.34+ crash fix (same as GstLaunchBackend / VideoSnapViewerDialog):
        // the reparent+deleteLater orphan path relies on the event loop pumping
        // again to honor the deferred delete. At SHUTDOWN (this teardown runs
        // from the destructor's base-class chain) the loop may never spin again
        // → the orphaned, parentless QProcess is torn down inconsistently by
        // static teardown → heap corruption / segfault on exit. When closing
        // down, reap (SIGKILL already sent) and delete DIRECTLY; signals were
        // disconnected just above so no slot can fire on it. Keep the
        // orphan+deleteLater path during normal operation, where the loop WILL
        // pump and the synchronous-delete-during-dtor-chain concern applies.
        if (QCoreApplication::closingDown()) {
            if (m_proc->state() != QProcess::NotRunning)
                m_proc->waitForFinished(200);
            delete m_proc;
        } else {
            m_proc->setParent(nullptr);
            m_proc->deleteLater();
        }
        m_proc = nullptr;
    }
}

qint64 VideoRecorder::elapsedMs() const
{
    return m_recording ? m_timer.elapsed() : 0;
}

bool VideoRecorder::start(const QString &pwNode, const QString &outputPath, int maxDurationMs)
{
    if (m_recording) {
        qCWarning(lcRec) << "start: already recording";
        return false;
    }

    const QString gstBin = QStandardPaths::findExecutable(QStringLiteral("gst-launch-1.0"));
    if (gstBin.isEmpty()) {
        emit recordingFailed(tr("gst-launch-1.0 not in PATH"));
        return false;
    }

    // ── Probe encoder FIRST ─────────────────────────────────────────────────
    // We need `enc` to know the muxer + container before we can build the
    // pipeline OR finalize the output path. x264enc isn't in default Fedora
    // repos (RPM Fusion only) so we fall back to openh264enc → vp8enc.
    const EncoderChoice enc = probeEncoder();
    if (enc.element.isEmpty()) {
        emit recordingFailed(tr("No H.264-class encoder available. Install one of: "
            "gstreamer1-plugins-ugly (x264enc), gstreamer1-plugin-openh264, "
            "or gstreamer1-plugins-good (vp8enc)."));
        return false;
    }
    qCInfo(lcRec) << "Using encoder:" << enc.element
                  << "(container:" << enc.containerExt << ")";

    // ── Resolve output path (may swap extension to match muxer) ─────────────
    QString actualOutputPath = outputPath;
    {
        const qsizetype dot = outputPath.lastIndexOf(QLatin1Char('.'));
        const QString currentExt = (dot > 0) ? outputPath.mid(dot) : QString();
        if (currentExt != enc.containerExt) {
            actualOutputPath = (dot > 0 ? outputPath.left(dot) : outputPath)
                             + enc.containerExt;
            qCInfo(lcRec) << "Swapped output extension to" << enc.containerExt
                          << "→" << actualOutputPath;
        }
    }

    // ── Make sure output directory exists ───────────────────────────────────
    const QFileInfo fi(actualOutputPath);
    if (!fi.absoluteDir().exists()) {
        if (!QDir().mkpath(fi.absolutePath())) {
            emit recordingFailed(tr("Cannot create output directory: %1")
                                  .arg(fi.absolutePath()));
            return false;
        }
    }
    if (QFile::exists(actualOutputPath)) QFile::remove(actualOutputPath);

    // ── Live-preview file path ──────────────────────────────────────────────
    // The recorder's gst-launch writes a low-res JPEG to this path at ~10 Hz
    // via a tee branch. The CameraWidget polls this file (via
    // VideoRecorder::previewFilePath()) to display live frames while the
    // recorder owns /dev/video0. multifilesink atomically replaces the file
    // on each frame, so a reader never sees a half-written JPEG.
    //
    // Why a file rather than a socket / shared memory: dead simple, works on
    // every platform, no IPC failure modes. The cost is one tmpfs write per
    // preview frame (~5-15 KB JPEG), negligible compared to camera capture.
    //
    // TempLocation maps to /tmp (or %TEMP% on Windows) which is almost always
    // tmpfs on Linux — so this never hits a real disk.
    m_previewPath = QStandardPaths::writableLocation(
                        QStandardPaths::TempLocation)
                  + QStringLiteral("/snapclone-preview-")
                  + QString::number(QCoreApplication::applicationPid())
                  + QStringLiteral(".jpg");
    // Clean any stale preview file from a prior crashed run so the
    // viewfinder doesn't briefly show the wrong recording's last frame.
    if (QFile::exists(m_previewPath)) QFile::remove(m_previewPath);

    // ── Build pipeline ──────────────────────────────────────────────────────
    // gst-launch parses left-to-right; named elements must be DECLARED before
    // they're REFERENCED. Order:
    //   1. Muxer + sink (declares "mux")
    //   2. Video source → tee name=t (declares "t")
    //   3. Encoder branch (refs "t." and "mux.")
    //   4. Preview branch (refs "t." → JPEG → multifilesink)
    //   5. Audio branch (refs "mux." — only when audio source detected)
    //
    // The tee is what enables live preview during recording. Without it
    // the viewfinder shows a stale frozen frame for the full recording
    // duration because v4l2 is single-consumer — only the recorder can
    // have /dev/video0 open at a time. Tee splits the v4l2 stream so the
    // SAME pipeline that's recording also produces preview frames.
    QStringList args;
    args << QStringLiteral("-q") << QStringLiteral("-e");  // -e: EOS on SIGINT/SIGTERM

    // Muxer + sink (declared FIRST so "mux" name resolves in branches below)
    args << enc.muxer << QStringLiteral("name=mux")
         << QStringLiteral("!") << QStringLiteral("filesink")
         << QStringLiteral("location=") + actualOutputPath;

    // Video source — pick element based on path format. tee splits the
    // source stream so we can encode AND preview from one camera open.
    if (pwNode.isEmpty()) {
        args << QStringLiteral("autovideosrc");
    } else if (pwNode.startsWith(QStringLiteral("/dev/video"))) {
        // io-mode=mmap forces memory-mapped buffers (universally supported)
        // instead of letting v4l2src negotiate USERPTR mode, which fails on
        // many drivers including the Asahi/Apple Silicon FaceTime HD path.
        // Without this, recording dies with "Failed to allocate required
        // memory. Buffer pool activation failed."
        args << QStringLiteral("v4l2src")
             << QStringLiteral("device=") + pwNode
             << QStringLiteral("io-mode=mmap")
             << QStringLiteral("do-timestamp=true");
    } else if (pwNode.startsWith(QStringLiteral("/"))) {
        // An absolute path that ISN'T a /dev/video node is a libcamera camera id
        // (e.g. /base/soc@0/cci@ac4a000/i2c-bus@1/camera@29), as produced by the
        // direct LibcameraBackend. The PipeWire/libcamera bridge exposes the SAME
        // sensor as a node named "libcamera_input.<id with '/' and '@' → '_'>",
        // and the device-probe path drives exactly that node through pipewiresrc
        // successfully on this hardware. So map the id to the node name and use
        // pipewiresrc — proven on-device, and (unlike libcamerasrc) needs no
        // extra gstreamer plugin. Feeding pipewiresrc the raw "/base/…camera@29"
        // id is what fails with "Format negotiation failed / not-negotiated".
        QString node = pwNode;
        node.replace(QLatin1Char('@'), QLatin1Char('_'));
        node.replace(QLatin1Char('/'), QLatin1Char('_'));
        node = QStringLiteral("libcamera_input.") + node;
        qCInfo(lcRec) << "Mapped libcamera id" << pwNode << "→ PipeWire node" << node << "for recording";
        args << QStringLiteral("pipewiresrc")
             << QStringLiteral("target-object=") + node
             << QStringLiteral("do-timestamp=true");
    } else {
        args << QStringLiteral("pipewiresrc")
             << QStringLiteral("target-object=") + pwNode
             << QStringLiteral("do-timestamp=true");
    }

    // V32.32: user-rotation pass. Inserted BEFORE the tee so both the
    // encoder branch (which writes the actual .mkv recipients see)
    // AND the preview branch (the in-app viewfinder JPEG) get the
    // same rotated frames. Without this, the camera widget's
    // applyUserRotation only affects the photo capture path and the
    // live-display path — the recording was being written in raw
    // camera-sensor orientation, so a Fairphone rear camera (which
    // is mounted landscape but used portrait) produced sideways
    // videos at the recipient even though the viewfinder showed
    // upright content.
    //
    // videoconvert before videoflip: videoflip needs raw video in a
    // pixel format it understands. v4l2src can emit YUY2/NV12/etc;
    // videoflip handles all common raw formats but a defensive
    // videoconvert ensures format negotiation succeeds across the
    // device matrix we encounter.
    //
    // 0° → "none" is a passthrough (zero-copy), so leaving this
    // inserted unconditionally has no cost when rotation is off.
    {
        const char *method = "none";
        switch (m_userRotation) {
        case 90:  method = "clockwise";        break;
        case 180: method = "rotate-180";       break;
        case 270: method = "counterclockwise"; break;
        default:  method = "none";             break;
        }
        args << QStringLiteral("!") << QStringLiteral("videoconvert")
             << QStringLiteral("!") << QStringLiteral("videoflip")
             << QStringLiteral("method=") + QString::fromLatin1(method);
    }

    args << QStringLiteral("!") << QStringLiteral("tee")
         << QStringLiteral("name=t");

    // ── Encoder branch (full quality) ──────────────────────────────────────
    // queue between tee and downstream is critical: it gives this branch
    // its own thread of execution. Without it, the preview branch's
    // backpressure (or vice versa) would stall the entire pipeline.
    args << QStringLiteral("t.")
         << QStringLiteral("!") << QStringLiteral("queue")
         << QStringLiteral("!") << QStringLiteral("videoconvert")
         << QStringLiteral("!") << QStringLiteral("video/x-raw,format=I420")
         << QStringLiteral("!") << enc.element;
    for (const QString &prop : enc.properties) {
        args << prop;
    }
    // Insert encoder-specific parser before muxer if needed (e.g. h264parse
    // for openh264enc which emits byte-stream that matroskamux refuses).
    // V32.35+: the parser may now carry a property (e.g.
    // "h264parse config-interval=-1"), so split on spaces — QProcess args are
    // individual tokens, and gst-launch needs the element and each property as
    // separate argv entries.
    if (!enc.parser.isEmpty()) {
        args << QStringLiteral("!");
        const auto parserTokens =
            enc.parser.split(QLatin1Char(' '), Qt::SkipEmptyParts);
        for (const QString &tok : parserTokens)
            args << tok;
    }
    args << QStringLiteral("!") << QStringLiteral("mux.");

    // ── Preview branch (low-res JPEG for viewfinder display) ───────────────
    //
    //   t. ! queue (leaky=downstream max-buffers=1)   ← drop old frames
    //      ! videorate                                 ← drop to 10 fps
    //      ! video/x-raw,framerate=10/1
    //      ! videoscale
    //      ! video/x-raw,width=320,pixel-aspect-ratio=1/1  ← downscale
    //      ! videoconvert                              ← JPEG needs YUY2/I420
    //      ! jpegenc quality=70
    //      ! multifilesink location=<path> max-files=1
    //
    // Why leaky=downstream + max-buffers=1: if the viewfinder isn't
    // reading the preview file fast enough (or the JPEG encoder stalls),
    // we want to DROP frames rather than backpressure the source. The
    // encoder branch must NEVER stall — that drops actual recording
    // frames the user cares about. Preview is the disposable branch.
    //
    // Why videorate to 10 fps: human eye sees ~10 fps as "live preview"
    // motion. At 30 fps we'd be writing 30 JPEGs/sec to tmpfs for no
    // perceptual gain. 10 fps = ~10 KB/sec on tmpfs.
    //
    // multifilesink with max-files=1 keeps a single file at the location,
    // overwriting atomically each frame. Polling reads always see a
    // coherent JPEG.
    args << QStringLiteral("t.")
         << QStringLiteral("!") << QStringLiteral("queue")
         << QStringLiteral("leaky=downstream")
         << QStringLiteral("max-size-buffers=1")
         << QStringLiteral("max-size-bytes=0")
         << QStringLiteral("max-size-time=0")
         << QStringLiteral("!") << QStringLiteral("videorate")
         << QStringLiteral("!") << QStringLiteral("video/x-raw,framerate=10/1")
         << QStringLiteral("!") << QStringLiteral("videoscale")
         // V32.31: add-borders=true + explicit square caps. Was producing
         // visibly stretched preview on some sources because width-only
         // caps with no add-borders meant videoscale's fixate path picked
         // an output height that didn't preserve source aspect for all
         // source resolutions/PARs we encounter in PipeWire (FP5 480x640
         // portrait, USB 1920x1080 landscape, virtual-cam 720p).
         // The actual recorded video file is fine — only this preview
         // branch was wrong.
         //
         // Square output (320x320) + add-borders=true gives uniform
         // behaviour: landscape sources letterbox with top/bottom bars,
         // portrait sources pillar-box with left/right bars. Content
         // aspect always preserved. The viewfinder label scales this
         // square JPEG with KeepAspectRatio so the bars are invisible
         // against the black background of m_viewfinderLabel.
         //
         // The previous "width=320 only" caps without PAR triggered
         // varying fixate behavior across GStreamer versions — what was
         // working on one machine stretched on another. The integer-
         // overflow bug noted earlier (with width=320 + PAR=1/1) only
         // happens when BOTH width is fixed AND PAR is constrained
         // explicitly — not when width and height are both fixed with
         // add-borders=true, which uses a different code path.
         //
         // Fallback if 320x320 overflows on some new device: drop the
         // size caps entirely (just `videoscale add-borders=true` with
         // no caps) and let JPEG encoder consume source-resolution
         // frames. Costs more bytes-per-tick to tmpfs but is bulletproof.
         << QStringLiteral("add-borders=true")
         << QStringLiteral("!") << QStringLiteral("video/x-raw,width=320,height=320")
         << QStringLiteral("!") << QStringLiteral("videoconvert")
         << QStringLiteral("!") << QStringLiteral("jpegenc")
         << QStringLiteral("quality=70")
         << QStringLiteral("!") << QStringLiteral("multifilesink")
         << QStringLiteral("location=") + m_previewPath
         << QStringLiteral("max-files=1");


    // Audio branch — only added if we can detect at least one audio source.
    // Without this guard, a missing mic causes the WHOLE pipeline to error
    // out at startup ("Could not open audio device") and gst exits, which
    // is much worse than recording video-only.
    //
    // Caller can also explicitly disable audio via QSettings("video/recordAudio")
    // — useful for users who want quieter recordings or have noisy environments.
    QSettings s;
    const bool audioEnabled = s.value(QStringLiteral("video/recordAudio"), true).toBool();
    const bool audioOk = audioEnabled && probeAudioAvailable();
    if (audioOk) {
        args << QStringLiteral("autoaudiosrc")
             << QStringLiteral("!") << QStringLiteral("queue")
             << QStringLiteral("!") << QStringLiteral("audioconvert")
             << QStringLiteral("!") << QStringLiteral("audioresample");
        // V32.34+ audio: optional capture-side loudness compression.
        // Raw mic level varies wildly between recordings (windy outdoors
        // vs quiet room vs arm's-length), so playback volume is
        // unpredictable. A soft-knee compressor + makeup gain brings
        // every recording toward a more consistent perceived loudness —
        // not true EBU-R128 (that's a transcode-side two-pass job, filed
        // in the audio design brief), but a cheap real-time floor that
        // makes quiet recordings audible.
        //
        // Gated on:
        //   • QSettings audio/normalizeLoudness (default ON)
        //   • audiodynamic being installed (gst-plugins-good; absent on
        //     minimal pmOS images — fall back to plain passthrough)
        //
        // audiodynamic compressor params: soft-knee, ratio 0.5 (2:1),
        // threshold 0.5 — gentle. Followed by audioamplify makeup gain.
        // These are conservative defaults; the audio design brief tracks
        // tuning against real recorded samples.
        const bool normalize =
            s.value(QStringLiteral("audio/normalizeLoudness"), true).toBool();
        if (normalize && audioElementAvailable(QStringLiteral("audiodynamic"))) {
            args << QStringLiteral("!") << QStringLiteral("audiodynamic")
                 << QStringLiteral("mode=compressor")
                 << QStringLiteral("characteristics=soft-knee")
                 << QStringLiteral("ratio=0.5")
                 << QStringLiteral("threshold=0.5");
            // Makeup gain — audioamplify is in gst-plugins-good too, but
            // probe it independently in case of a partial install.
            if (audioElementAvailable(QStringLiteral("audioamplify"))) {
                args << QStringLiteral("!") << QStringLiteral("audioamplify")
                     << QStringLiteral("amplification=1.5")
                     // method=none: don't hard-clip the makeup gain. The
                     // upstream compressor already holds peaks down, and
                     // opusenc tolerates the occasional transient. clip/
                     // wrap modes would introduce audible distortion.
                     << QStringLiteral("clipping-method=none");
            }
            qCInfo(lcRec) << "Audio: capture-side loudness compression ENABLED";
        } else if (normalize) {
            qCInfo(lcRec) << "Audio: normalization requested but audiodynamic "
                             "unavailable — recording at raw level";
        }
        args << QStringLiteral("!") << QStringLiteral("opusenc")
             << QStringLiteral("!") << QStringLiteral("mux.");
    } else {
        qCInfo(lcRec) << "Recording WITHOUT audio (probe failed or disabled in settings)";
    }
    // V32.32: tell the metadata cache whether this recording will have
    // an audio track. The transcoder later checks this instead of
    // re-running gst-discoverer-1.0 (which costs ~5s on FP5-class ARM
    // and blocks the GUI thread). audioOk is authoritative — we
    // decided here whether to wire the audio branch into the
    // pipeline, so the resulting file's audio-track presence matches
    // this exactly.
    RecordingMetadata::instance().setHasAudio(actualOutputPath, audioOk);

    m_proc = new QProcess(this);
    m_proc->setProcessChannelMode(QProcess::SeparateChannels);

    // V32.32: reset state-change tracking for this session. m_proc is
    // freshly constructed but the recorder instance is reused across
    // recordings, so the flags must be cleared each time.
    m_pipelinePlaying = false;
    m_lastStdout.clear();

    // Accumulate stderr so we can include the real failure reason in
    // recordingFailed() rather than just "gst exit 1". gst-launch always
    // emits its actual error message (missing element, EBUSY, codec not
    // installed, etc.) on stderr just before exiting non-zero — that's the
    // useful diagnostic we want to surface.
    connect(m_proc, &QProcess::readyReadStandardError, this, [this]() {
        const QByteArray err = m_proc->readAllStandardError();
        if (err.isEmpty()) return;
        qCInfo(lcRec) << "gst-launch stderr:" << err.trimmed();
        m_lastStderr.append(err);
        // Cap so a runaway log doesn't eat memory
        if (m_lastStderr.size() > 8192) {
            m_lastStderr = m_lastStderr.right(8192);
        }
    });
    // V32.32: stdout watcher for the PLAYING state transition.
    //
    // gst-launch prints "Setting pipeline to PLAYING ..." to stdout when
    // the pipeline successfully reaches the PLAYING state. That's the
    // canonical "you can safely terminate me and get a valid output
    // file" signal — once PLAYING is reached, the muxer has written
    // its header and any SIGTERM produces a complete (if short) file.
    //
    // Before V32.32, stop() used a 1500ms file-size busy-poll
    // (QThread::msleep(100) loop) to approximate this. That worked but
    // blocked the GUI thread; this stdout-based detection is precise
    // AND async — pipelineEnteredPlaying() is emitted on the GUI
    // thread, stop() waits via QEventLoop without freezing the UI.
    //
    // Falls back gracefully: if we never see "PLAYING" (older gst-launch
    // version, weird locale, stdout buffering), stop() still terminates
    // after its bounded wait — same outcome as before, no regression.
    connect(m_proc, &QProcess::readyReadStandardOutput, this, [this]() {
        const QByteArray chunk = m_proc->readAllStandardOutput();
        if (chunk.isEmpty()) return;
        m_lastStdout.append(chunk);
        if (m_lastStdout.size() > 8192) {
            m_lastStdout = m_lastStdout.right(8192);
        }
        // Edge-trigger only — emit once per session even if "PLAYING"
        // appears again later (e.g. seek-driven state transitions in
        // future pipelines that might pause and resume).
        if (!m_pipelinePlaying.load()
            && m_lastStdout.contains("Setting pipeline to PLAYING")) {
            qCInfo(lcRec) << "gst-launch pipeline reached PLAYING at"
                          << m_timer.elapsed() << "ms";
            m_pipelinePlaying = true;
            emit pipelineEnteredPlaying();
        }
    });
    connect(m_proc, QOverload<int, QProcess::ExitStatus>::of(&QProcess::finished),
            this, [this, actualOutputPath](int code, QProcess::ExitStatus) {
        const int dur = m_recording ? static_cast<int>(m_timer.elapsed()) : 0;
        m_recording = false;
        if (m_tickTimer) { m_tickTimer->stop(); m_tickTimer->deleteLater(); m_tickTimer = nullptr; }

        // Verify the file exists and has sensible size before declaring success.
        // gst-launch sometimes exits 0 even when its pipeline failed at startup,
        // so a "success" exit + missing/tiny file means failure.
        QFile f(actualOutputPath);
        if (!f.exists() || f.size() < 1024) {
            qCWarning(lcRec) << "Recording exited code" << code
                             << "but output is missing or empty (size:"
                             << (f.exists() ? f.size() : -1) << ")";
            // Extract a useful tail of stderr for the user-facing message.
            // gst-launch errors look like: "ERROR: from element ...: ... : Could not open device"
            QString tail = QString::fromUtf8(m_lastStderr).trimmed();
            // Try to find the most informative line — last "ERROR:" line wins
            const QStringList lines = tail.split(QLatin1Char('\n'),
                                                  Qt::SkipEmptyParts);
            QString errLine;
            for (auto it = lines.crbegin(); it != lines.crend(); ++it) {
                if (it->contains(QLatin1String("ERROR"))
                 || it->contains(QLatin1String("Could not"))) {
                    errLine = it->trimmed();
                    break;
                }
            }
            if (errLine.isEmpty() && !lines.isEmpty()) errLine = lines.last();
            // Cap length so the toast/dialog doesn't get unwieldy
            if (errLine.length() > 200) errLine = errLine.left(200) + QStringLiteral("…");

            // V32.34+: human-readable translation of common gst failures.
            // The raw "ERROR: from element /GstPipeline:.../matroskademux:
            // ... not-negotiated (-4)" message tells a gstreamer dev
            // exactly where the failure is, but the average user only
            // sees a wall of brackets and codec acronyms.
            //
            // Pattern: matroska/webm demux + not-negotiated = decoder
            // missing or upstream produced an unsupported format. For
            // recordings, the most common culprit is the h264 decoder
            // not being installed (we encode to h264 in the .mkv but
            // some pmOS edge builds don't ship the decoder package).
            //
            // Pattern: pipewiresrc + not-negotiated = caps constraint
            // failed (the bug I shipped earlier asking for 1280×720).
            // Already mostly handled by defaulting the source-target
            // caps to off, but if a user enables them on incompatible
            // hardware, this matches.
            QString humanReason;
            const QString errLower = errLine.toLower();
            if (errLower.contains(QLatin1String("matroskademux"))
                && errLower.contains(QLatin1String("not-negotiated"))) {
                humanReason = tr("Recording failed: no decoder available "
                                  "for the recorded format. Install h264 "
                                  "decoder support (apk add "
                                  "gst-plugins-bad or libde265 / openh264 "
                                  "depending on your distro) and try "
                                  "again.");
            } else if (errLower.contains(QLatin1String("pipewiresrc"))
                       && errLower.contains(QLatin1String("not-negotiated"))) {
                humanReason = tr("Recording failed: camera does not "
                                  "support the requested capture format. "
                                  "If you enabled Camera → Source Target "
                                  "Resolution, set it back to 0 in "
                                  "Settings.");
            }

            const QString reason = !humanReason.isEmpty() ? humanReason
                : errLine.isEmpty()
                    ? tr("Recording produced no output (gst exit %1)").arg(code)
                    : tr("Recording failed: %1").arg(errLine);
            // Preview file may exist if recording got partway through —
            // remove so subsequent recordings don't see a stale frame.
            if (!m_previewPath.isEmpty()) {
                QFile::remove(m_previewPath);
                m_previewPath.clear();
            }
            emit recordingFailed(reason);
            m_lastStderr.clear();
            return;
        }
        qCInfo(lcRec) << "Recording finished:" << actualOutputPath
                      << "size:" << f.size() << "duration:" << dur << "ms";
        // Clean up the preview file — recording is done, the viewfinder
        // shouldn't keep showing the last preview frame.
        if (!m_previewPath.isEmpty()) {
            QFile::remove(m_previewPath);
            m_previewPath.clear();
        }
        emit recordingStopped(actualOutputPath, dur);
    });

    qCInfo(lcRec) << "Recording to" << actualOutputPath
                  << "from" << (pwNode.isEmpty() ? QStringLiteral("autovideosrc") : pwNode);
    qCDebug(lcRec) << "Pipeline args:" << args;

    m_proc->start(gstBin, args);
    if (!m_proc->waitForStarted(2000)) {
        emit recordingFailed(tr("gst-launch failed to start"));
        m_proc->deleteLater();
        m_proc = nullptr;
        return false;
    }

    m_recording     = true;
    m_outputPath    = actualOutputPath;  // may differ from caller's outputPath
                                          // if the encoder needed a different container
    m_maxDurationMs = maxDurationMs;
    m_timer.restart();
    m_lastStderr.clear();  // reset for this session's stderr capture

    // Tick timer for UI elapsed counter
    m_tickTimer = new QTimer(this);
    m_tickTimer->setInterval(100);
    connect(m_tickTimer, &QTimer::timeout, this, [this]() {
        const int e = static_cast<int>(m_timer.elapsed());
        emit elapsedTick(e);
        if (m_maxDurationMs > 0 && e >= m_maxDurationMs) {
            qCInfo(lcRec) << "Max duration reached — stopping";
            stop();
        }
    });
    m_tickTimer->start();
    return true;
}

void VideoRecorder::stop()
{
    if (!m_recording || !m_proc) return;

    // V32.32 (revision): wait for the pipeline to reach PLAYING state
    // before terminating. This guards against the fast-stop-corrupts-
    // webm bug — user starts recording, immediately taps stop, the
    // pipeline hasn't yet wired up its muxer header, SIGTERM produces
    // a malformed file.
    //
    // BEFORE (V32.25): busy-poll loop with QThread::msleep(100), checking
    // the output file size. Blocked the GUI thread for up to 1500ms.
    //
    // NOW: wait on the pipelineEnteredPlaying() signal via QEventLoop.
    // The signal fires when our stdout watcher sees "Setting pipeline to
    // PLAYING" — the same canonical signal gst-launch itself uses for
    // major state transitions. A QTimer provides the timeout fallback.
    //
    // The UI thread keeps painting and responding to events during this
    // wait because QEventLoop pumps the event queue — same model as
    // QDialog::exec() or QMessageBox::information(). Bounded by the
    // timeout so a broken pipeline (driver hung, plugin missing) doesn't
    // hold the UI forever. After the timeout, we proceed to terminate
    // regardless — the post-mortem "size < 1024" check in the finished
    // handler still catches genuinely-broken pipelines and reports
    // them as recording failures.
    //
    // Common case (recording for >half a second): m_pipelinePlaying is
    // already true by the time stop() is called and we skip the wait
    // entirely. Zero overhead.
    constexpr int kGraceMs = 1500;
    if (!m_pipelinePlaying.load()) {
        const qint64 elapsed = m_timer.elapsed();
        const qint64 deadline = qMax<qint64>(0, kGraceMs - elapsed);
        if (deadline > 0) {
            qCInfo(lcRec) << "stop: pipeline not yet PLAYING after"
                          << elapsed << "ms — waiting up to" << deadline
                          << "ms for state transition";
            QEventLoop loop;
            QTimer timeout;
            timeout.setSingleShot(true);
            connect(&timeout, &QTimer::timeout, &loop, &QEventLoop::quit);
            connect(this, &VideoRecorder::pipelineEnteredPlaying,
                    &loop, &QEventLoop::quit);
            // If the process exits during the wait (gst-launch crash,
            // missing element, codec init failure), QProcess::finished
            // fires — quit the loop immediately rather than waiting out
            // the timeout for nothing.
            connect(m_proc, QOverload<int, QProcess::ExitStatus>::of(
                                &QProcess::finished),
                    &loop, &QEventLoop::quit);
            timeout.start(static_cast<int>(deadline));
            loop.exec();  // returns on EITHER signal
            if (m_pipelinePlaying.load()) {
                qCInfo(lcRec) << "stop: pipeline reached PLAYING after"
                              << m_timer.elapsed() << "ms total — proceeding";
            } else {
                qCWarning(lcRec) << "stop: pipeline never reached PLAYING within"
                                    " grace window — stopping anyway, file will"
                                    " be flagged as failed in finished() handler";
            }
        }
    }

    // Send EOS via terminate (handled by `-e` flag above) so matroskamux
    // finalizes the file properly. SIGKILL would leave it unmuxed/corrupt.
    m_proc->terminate();
    if (!m_proc->waitForFinished(3000)) {
        qCWarning(lcRec) << "stop: gst-launch didn't exit gracefully — killing";
        m_proc->kill();
        m_proc->waitForFinished(500);
    }
    // The finished() signal will fire and clean up state via the lambda above.
}

} // namespace SnapClone::UI::Video
