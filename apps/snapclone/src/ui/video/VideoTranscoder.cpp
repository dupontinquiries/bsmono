#include "ui/video/VideoTranscoder.hpp"
#include "ui/video/VideoEncoderBackend.hpp"
#ifdef SNAPCLONE_RUST_SALIENCY
#  include "saliency/snapclone_saliency_ffi.h"  // V32.34+: face ROI boost
#  include <QImage>
#endif

#include <QProcess>
#include <QCoreApplication>  // V32.34+: closingDown() shutdown guard
#include <QStandardPaths>
#include <QFile>
#include <QFileInfo>
#include <QDir>
#include <QStringList>
#include <QRegularExpression>
#include <utility>   // std::swap (ROI W/H swap on rotated sources)
#include <QLoggingCategory>
#include <cmath>  // V32.32: std::exp for exponential progress estimate
#include <QTimer>
#include <QDateTime>

namespace SnapClone::UI::Video {

Q_LOGGING_CATEGORY(lcTranscode, "snapclone.video.transcoder")

namespace {

/// One-time backend probe. The list is materialised on first call (which
/// involves spawning a handful of small gst-inspect / which / ffmpeg
/// processes, costing ~50-300ms) and cached for the rest of the process
/// lifetime. Re-probing per encode would add that latency to every call.
///
/// Returns nullptr if NO backend is available — caller surfaces a
/// user-visible error and falls back to sending the raw .mkv directly.
IVideoEncoderBackend *findFirstAvailableBackend()
{
    static std::vector<std::unique_ptr<IVideoEncoderBackend>> s_all;
    static IVideoEncoderBackend *s_chosen = nullptr;
    static bool s_probed = false;
    if (s_probed) return s_chosen;
    s_probed = true;

    s_all = allBackendsInPriorityOrder();
    for (const auto &b : s_all) {
        const auto avail = b->checkAvailable();
        if (avail.available) {
            qCInfo(lcTranscode) << "Selected encoder backend:" << b->name();
            s_chosen = b.get();
            return s_chosen;
        }
        qCDebug(lcTranscode) << "Backend" << b->name() << "unavailable:"
                             << avail.reason;
    }
    qCWarning(lcTranscode) << "No video encoder backend is available";
    return nullptr;
}

/// V32.34+ RUNTIME FALLBACK selector. Like findFirstAvailableBackend() but
/// skips any backend whose name() is in `exclude` — used when a backend was
/// "available" yet failed at RUNTIME (e.g. FP5 gst-svtav1enc not-negotiated),
/// so we can transparently move to the next working encoder. Keeps its own
/// owned list (separate from the fast-path static cache) so the returned
/// pointers stay alive for the life of the process.
IVideoEncoderBackend *findAvailableBackendExcluding(const QStringList &exclude)
{
    static std::vector<std::unique_ptr<IVideoEncoderBackend>> s_allEx;
    if (s_allEx.empty()) s_allEx = allBackendsInPriorityOrder();
    for (const auto &b : s_allEx) {
        if (exclude.contains(b->name())) continue;
        const auto avail = b->checkAvailable();
        if (avail.available) {
            qCInfo(lcTranscode) << "Fallback encoder backend selected:"
                                << b->name() << "(excluded:" << exclude << ")";
            return b.get();
        }
        qCDebug(lcTranscode) << "Fallback skip" << b->name() << ":" << avail.reason;
    }
    return nullptr;
}

} // anon namespace

VideoTranscoder::VideoTranscoder(QObject *parent)
    : QObject(parent)
{
}

VideoTranscoder::~VideoTranscoder()
{
    if (m_estimateTimer) {
        m_estimateTimer->stop();
        m_estimateTimer->deleteLater();
        m_estimateTimer = nullptr;
    }
    if (m_proc) {
        // Disconnect signals BEFORE kill — same rationale as GstLaunchBackend
        // and VideoRecorder. Avoid waitForFinished's nested event loop.
        // Also reparent before deleteLater so the QObject parent-child
        // cleanup doesn't preempt the deferred delete and trigger
        // "QProcess: Destroyed while process running" warnings.
        m_proc->QObject::disconnect();
        m_proc->kill();
        // V32.34+ crash fix: at shutdown the event loop may never pump again,
        // so an orphaned (setParent(nullptr)) deleteLater'd QProcess is torn
        // down inconsistently → heap corruption on exit. Delete directly when
        // closing down (signals disconnected above, SIGKILL sent); keep the
        // orphan+deleteLater path during normal operation. (Same fix as
        // GstLaunchBackend / VideoRecorder / VideoSnapViewerDialog.)
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

IVideoEncoderBackend *VideoTranscoder::selectBackend()
{
    if (!m_backend) m_backend = findFirstAvailableBackend();
    return m_backend;
}

QString VideoTranscoder::activeBackendName() const
{
    return m_backend ? m_backend->name() : QString();
}

IVideoEncoderBackend::Kind VideoTranscoder::activeBackendKind() const
{
    // V32.32: default to Cpu when no backend is available — the indicator
    // will still show the system "is working" (transcode is queued) even
    // if backend probe was inconclusive at construction time.
    return m_backend ? m_backend->kind() : IVideoEncoderBackend::Kind::Cpu;
}

bool VideoTranscoder::detectHasAudio(const QString &inputPath) const
{
    // Without an audio-track check, the audio branch would block in PREROLL
    // waiting for an audio pad that never arrives, freezing transcode at 0%
    // forever — the mobile-freeze symptom. We probe via gst-discoverer-1.0.
    //
    // The recorder DOES capture audio when a mic is present, so most snaps
    // WILL have audio. We only skip the audio branch when we positively
    // detect there isn't one.
    bool hasAudio = true;  // default: better to fail loud than silently drop
    bool probeSucceeded = false;
    QProcess disc;
    disc.start(QStringLiteral("gst-discoverer-1.0"),
               { QStringLiteral("-v"), inputPath });
    if (disc.waitForFinished(8000)) {
        const QByteArray out = disc.readAllStandardOutput();
        const bool seenAudio =
            out.contains("Audio:")       ||
            out.contains("audio-codec:") ||
            out.contains("audio/x-")     ||
            out.contains("Audio stream");
        const bool noAudioStreams =
            !seenAudio &&
            (out.contains("Video:") || out.contains("video-codec:"));
        if (seenAudio) {
            hasAudio = true;  probeSucceeded = true;
        } else if (noAudioStreams) {
            hasAudio = false; probeSucceeded = true;
        }
    } else {
        disc.kill();
        qCWarning(lcTranscode) << "gst-discoverer timed out on" << inputPath
                               << "— assuming audio present";
    }
    qCInfo(lcTranscode) << "Audio detection: hasAudio=" << hasAudio
                        << "probeSucceeded=" << probeSucceeded;
    return hasAudio;
}

QRect VideoTranscoder::detectSalientRoi(const QString &inputPath) const
{
    m_lastRoiFrameW = 0;  // reset; set below once the display dims are known
    m_lastRoiFrameH = 0;
#ifndef SNAPCLONE_RUST_SALIENCY
    Q_UNUSED(inputPath);
    return {};  // saliency not built in — caller skips the ROI boost
#else
    // Extract ONE representative frame and run the classical saliency
    // detector to find the face/subject region. We grab a frame ~1s in
    // rather than the very first (which is often black/half-exposed while
    // the camera settles) via ffmpeg's -ss seek + -frames:v 1, decoded to a
    // small PNG in the temp dir. ffmpeg is the same binary the ffmpeg
    // backend uses, so if we got here on that backend it's present; if
    // ffmpeg is missing we just return null and skip the boost.
    const QString ff = QStandardPaths::findExecutable(QStringLiteral("ffmpeg"));
    if (ff.isEmpty()) return {};

    const QString framePath = QDir(QDir::tempPath()).filePath(
        QStringLiteral("sc_roi_%1.png")
            .arg(QDateTime::currentMSecsSinceEpoch()));

    QProcess grab;
    // -ss before -i = fast (keyframe) seek; 1s in. -frames:v 1 = one frame.
    // Downscale the extracted frame to <=512 wide: saliency is scale-
    // invariant for our purposes and a smaller frame keeps the detector
    // well under its sub-100ms target. -y to overwrite any stale temp.
    grab.start(ff, {
        QStringLiteral("-y"), QStringLiteral("-hide_banner"),
        QStringLiteral("-loglevel"), QStringLiteral("error"),
        QStringLiteral("-ss"), QStringLiteral("1"),
        QStringLiteral("-i"), inputPath,
        QStringLiteral("-frames:v"), QStringLiteral("1"),
        QStringLiteral("-vf"), QStringLiteral("scale=min(iw\\,512):-2"),
        framePath
    });
    QRect roi;  // null by default
    if (grab.waitForFinished(5000) && QFile::exists(framePath)) {
        QImage img(framePath);
        if (!img.isNull()) {
            const QImage rgba = img.convertToFormat(QImage::Format_RGBA8888);
            const SalRect sb = sc_saliency_bbox(
                rgba.constBits(),
                static_cast<uint32_t>(rgba.width()),
                static_cast<uint32_t>(rgba.height()));
            if (sb.w > 0 && sb.h > 0) {
                // The saliency box is in the EXTRACTED frame's space (≤512
                // wide). The encoder's addroi filter runs on the SOURCE frame
                // (it's first in the -vf chain, before scale). BOTH the extract
                // and addroi operate on ffmpeg's DECODED frames, which ffmpeg
                // auto-rotates per the stream's display matrix — so they share
                // one orientation. We map the box up to source DISPLAY pixels.
                //
                // V32.34+ ROTATION FIX: the previous probe read the first
                // "WxH" in ffmpeg's stderr, which is the CODED resolution
                // (e.g. 720x1280). But the FP5 recordings carry a -90° display
                // matrix, so the decoded/display frame is 1280x720 — axes
                // swapped. Scaling the box by coded dims while addroi sees
                // display dims produced an ROI height of 1248 on a 720-tall
                // frame → ffmpeg's "value 1248 > maximum 720" clamp warning and
                // a misplaced quality boost. We now (a) parse the Video: stream
                // line specifically, (b) read the rotation tag and swap W/H when
                // it's ±90/270, and (c) HARD-CLAMP the final box to the frame so
                // it can never exceed bounds regardless of probe quirks.
                roi = QRect(static_cast<int>(sb.x), static_cast<int>(sb.y),
                            static_cast<int>(sb.w), static_cast<int>(sb.h));
                const int extractW = rgba.width();
                const int extractH = rgba.height();
                int srcW = 0, srcH = 0, rotDeg = 0;
                QProcess dim;
                dim.start(ff, {
                    QStringLiteral("-hide_banner"),
                    QStringLiteral("-i"), inputPath
                });
                if (dim.waitForFinished(5000)) {
                    const QString err = QString::fromUtf8(dim.readAllStandardError());
                    // Anchor to the video stream line so we don't grab a stray
                    // number (bitrate, matrix coeffs, etc.). ffmpeg prints e.g.
                    //   Stream #0:0(eng): Video: h264 (...), yuv420p(...), 720x1280 [SAR ...]
                    static const QRegularExpression vidRx(
                        QStringLiteral(R"(Video:.*?(\d{2,5})x(\d{2,5}))"));
                    const auto vm = vidRx.match(err);
                    if (vm.hasMatch()) {
                        srcW = vm.captured(1).toInt();
                        srcH = vm.captured(2).toInt();
                    }
                    // Display rotation, if present:
                    //   Side data: ... Display Matrix: rotation of -90.00 degrees
                    static const QRegularExpression rotRx(
                        QStringLiteral(R"(rotation of\s*(-?\d+(?:\.\d+)?)\s*degrees)"));
                    const auto rm = rotRx.match(err);
                    if (rm.hasMatch()) {
                        // Normalize to 0/90/180/270.
                        int r = static_cast<int>(qRound(rm.captured(1).toDouble()));
                        r = ((r % 360) + 360) % 360;
                        rotDeg = r;
                    }
                }
                // For a ±90/270° display rotation, the DISPLAY frame ffmpeg's
                // filters see has W/H swapped vs the coded dims we just parsed.
                if (rotDeg == 90 || rotDeg == 270) std::swap(srcW, srcH);

                if (srcW > 0 && srcH > 0 && extractW > 0 && extractH > 0
                    && (srcW != extractW || srcH != extractH)) {
                    const double sx = static_cast<double>(srcW) / extractW;
                    const double sy = static_cast<double>(srcH) / extractH;
                    roi = QRect(
                        static_cast<int>(roi.x() * sx),
                        static_cast<int>(roi.y() * sy),
                        static_cast<int>(roi.width()  * sx),
                        static_cast<int>(roi.height() * sy));
                }
                // HARD CLAMP to the display frame so addroi can never receive an
                // out-of-bounds box (the safety net for any probe/rotation edge
                // case). If we couldn't determine the frame size, fall back to
                // the extract dims scaled — but only clamp when we have a frame.
                if (srcW > 0 && srcH > 0) {
                    int x = qBound(0, roi.x(), srcW - 1);
                    int y = qBound(0, roi.y(), srcH - 1);
                    int w = qBound(1, roi.width(),  srcW - x);
                    int h = qBound(1, roi.height(), srcH - y);
                    roi = QRect(x, y, w, h);
                    // Reject a degenerate/implausible box (covers ~the whole
                    // frame → no useful boost, just skip it).
                    if (w >= srcW - 2 && h >= srcH - 2) roi = QRect();
                }
                qCInfo(lcTranscode) << "Saliency ROI (display px):" << roi
                                    << "src:" << srcW << "x" << srcH
                                    << "rot:" << rotDeg;
                // V32.34+: remember the display frame dims so startWithSpec can
                // hand them to the ROI bit-allocation planner for clamping.
                if (srcW > 0 && srcH > 0) {
                    m_lastRoiFrameW = srcW;
                    m_lastRoiFrameH = srcH;
                }
            }
        }
    }
    QFile::remove(framePath);
    return roi;
#endif
}

bool VideoTranscoder::startWithSpec(const EncodeSpec &spec)
{
    if (m_running) {
        qCWarning(lcTranscode) << "start: already transcoding";
        return false;
    }
    const QFileInfo in(spec.inputPath);
    if (!in.exists() || in.size() < 1024) {
        emit transcodeFailed(tr("Input file missing or empty: %1").arg(spec.inputPath));
        return false;
    }
    m_inputPath    = spec.inputPath;
    m_inputSize    = in.size();
    m_lastProgress = 0;

    // V32.34+: remember the spec so a runtime failure can retry on another
    // backend. If this is a FRESH start (not an internal fallback retry), clear
    // the failed-backend list. We detect a retry by comparing input paths:
    // the fallback path re-enters startWithSpec with the same spec.
    if (!(m_haveActiveSpec && m_activeSpec.inputPath == spec.inputPath
          && m_activeSpec.outputPath == spec.outputPath)) {
        m_failedBackends.clear();
        m_cancelled = false;   // fresh transcode — clear any prior cancel
    }
    m_activeSpec     = spec;
    m_haveActiveSpec = true;

    // Select a backend, honoring any backends that already failed at runtime
    // for THIS transcode (so the retry moves forward, not in circles).
    IVideoEncoderBackend *backend = m_failedBackends.isEmpty()
        ? selectBackend()
        : findAvailableBackendExcluding(m_failedBackends);
    if (!backend) {
        emit transcodeFailed(tr("No video encoder backend available "
                                "(need ffmpeg+libsvtav1, gst-svtav1enc, "
                                "vp9enc, or vp8enc)."));
        return false;
    }

    // V32.34+: honor the backend's required container. Most backends use webm
    // (AV1/VP9/VP8), but a VVC backend needs mkv (webm can't carry H.266). If
    // the selected backend's container differs from the requested output's
    // extension, compute a corrected path. `spec` is const, so we stash the
    // corrected path and apply it to m_activeSpec now and to `resolved` below,
    // so directory creation, the existence check, and buildArgs all agree.
    QString correctedOutputPath = spec.outputPath;
    {
        const QString wantExt = backend->outputContainer(); // no dot
        const QFileInfo fi(spec.outputPath);
        if (fi.suffix().compare(wantExt, Qt::CaseInsensitive) != 0) {
            correctedOutputPath = fi.absolutePath() + QLatin1Char('/')
                                + fi.completeBaseName() + QLatin1Char('.') + wantExt;
            qCInfo(lcTranscode) << "Backend" << backend->name()
                                << "requires ." << wantExt << "container — output path"
                                << spec.outputPath << "→" << correctedOutputPath;
            m_activeSpec.outputPath = correctedOutputPath;
        }
    }

    // Ensure output directory exists.
    const QFileInfo out(correctedOutputPath);
    if (!QDir().mkpath(out.absolutePath())) {
        emit transcodeFailed(tr("Cannot create output dir: %1").arg(out.absolutePath()));
        return false;
    }
    if (QFile::exists(correctedOutputPath)) QFile::remove(correctedOutputPath);

    // Fill in EncodeSpec fields the backend will need.
    EncodeSpec resolved = spec;
    resolved.outputPath = correctedOutputPath;   // V32.34+: backend's container
    resolved.hasAudio = detectHasAudio(spec.inputPath);

    // V32.34+ FACE/SALIENCY ROI: only the ffmpeg backend honors the addroi
    // filter (gst-launch has no equivalent), and only the user-facing tiers
    // warrant the extra work. Compute the salient box once here so buildArgs
    // stays pure. A null roi (saliency off / extract failed / no subject)
    // leaves roiW=0 and the backend skips the boost.
    if (backend->name() == QLatin1String("ffmpeg-libsvtav1")
        && (resolved.tier == EncodeTier::Standard
            || resolved.tier == EncodeTier::HighQuality)) {
        const QRect roi = detectSalientRoi(spec.inputPath);
        if (roi.isValid() && roi.width() > 0 && roi.height() > 0) {
            resolved.roiX = roi.x();
            resolved.roiY = roi.y();
            resolved.roiW = roi.width();
            resolved.roiH = roi.height();
            // Frame dims the planner clamps against (display pixels).
            resolved.roiFrameW = m_lastRoiFrameW;
            resolved.roiFrameH = m_lastRoiFrameH;
        }
    }

    // Build the command line from the backend.
    const QString bin   = backend->binary();
    const QStringList args = backend->buildArgs(resolved);

    // Resolve `bin` against PATH (absolute paths pass through).
    QString execPath = bin;
    if (!bin.startsWith(QLatin1Char('/'))) {
        execPath = QStandardPaths::findExecutable(bin);
        if (execPath.isEmpty()) {
            emit transcodeFailed(tr("Backend %1 chose binary '%2' which is not in PATH")
                                     .arg(backend->name(), bin));
            return false;
        }
    }

    // Defensive: never leak a prior QProcess. The retry path already nulls
    // m_proc before re-entering, and a fresh start can't be running (guarded at
    // the top), but allocate-over-existing would orphan the old one — so clean
    // it up explicitly if it's somehow still set.
    if (m_proc) {
        m_proc->disconnect(this);
        if (m_proc->state() != QProcess::NotRunning) m_proc->kill();
        m_proc->deleteLater();
        m_proc = nullptr;
    }
    m_proc = new QProcess(this);
    m_proc->setProcessChannelMode(QProcess::SeparateChannels);
    m_progressRx = backend->progressPattern();

    // Stream backend stderr at INFO so a hang's diagnostic trail is visible.
    // For backends that emit progress on stderr (ffmpeg "frame=N", custom
    // codec "progress: N", gst's Progress: N%), parse into a percent.
    connect(m_proc, &QProcess::readyReadStandardError, this, [this](){
        if (!m_proc) return;
        const QByteArray err = m_proc->readAllStandardError();
        if (err.isEmpty()) return;
        qCInfo(lcTranscode) << "[encoder]" << err.trimmed();
        // V32.32: first-output trigger. As soon as the encoder produces
        // ANY stderr (banner, version line, library version dump, etc),
        // we know the process is alive and running its init phase.
        // Jump the progress bar to at least 5% so the user sees motion
        // immediately rather than staring at 0% for the multi-second
        // AV1 lookahead-buffer fill. The size/time estimators below
        // then take over and ratchet up from there.
        if (m_lastProgress < 5) {
            m_lastProgress = 5;
            emit progress(5);
        }
        // Generic gstreamer progress (legacy path — still useful if a
        // future backend's regex doesn't fire).
        static const QRegularExpression gstRx(
            QStringLiteral("[Pp]rogress[^\\d]*(\\d+)%"));
        auto applyMatch = [this](int p) {
            if (p >= 0 && p <= 100 && p > m_lastProgress) {
                m_lastProgress = p;
                emit progress(p);
            }
        };
        const QString s = QString::fromUtf8(err);
        if (!m_progressRx.pattern().isEmpty()) {
            const auto m = m_progressRx.match(s);
            if (m.hasMatch()) {
                bool ok = false;
                // Backend may emit either a percent OR a frame count
                // (ffmpeg). We can't reliably distinguish without input
                // duration; treat 0-100 as percent and >100 as a frame
                // count we can't translate (caller falls back to
                // size-based estimation, which is fine).
                const int v = m.captured(1).toInt(&ok);
                if (ok && v >= 0 && v <= 100) applyMatch(v);
            }
        }
        const auto gm = gstRx.match(s);
        if (gm.hasMatch()) {
            bool ok = false;
            const int p = gm.captured(1).toInt(&ok);
            if (ok) applyMatch(p);
        }
    });

    // Belt-and-suspenders progress estimator. AV1 encoders buffer many
    // frames internally before emitting any output (~30-60 frame lookahead),
    // so the output file stays at 0 bytes for the first ~10-30s of encoding
    // on slow hardware. Without compensation the UI sticks at 0% during the
    // buffering window even though the process is busy.
    //
    // Strategy: combine TWO sources, take the max each tick:
    //   1. File-size ratio: output_size / input_size × 2 (AV1 ≈ 50% of
    //      input bitrate). Reliable once bytes flow.
    //   2. V32.32: time-based EXPONENTIAL creep (was: linear 1%/3s capped 60).
    //      Curve = 60 * (1 - exp(-elapsedMs / kTimeConstantMs)). With
    //      kTimeConstantMs = 4000 the curve hits ~7% at 500ms, ~13% at 1s,
    //      ~24% at 2s, ~38% at 4s, asymptotes to 60% around 10s.
    //      This solves the "transcode finishes in <3s but user only ever
    //      saw 0%" symptom: front-loaded curve gives instant visible
    //      motion, and exits before the size-based estimate has even
    //      had time to read a non-empty output file.
    // We never emit > 95% from estimates — only the final 100% comes from
    // the process-exit handler, so the bar doesn't snap back at completion.
    m_estimateTimer = new QTimer(this);
    m_estimateTimer->setInterval(500);
    const qint64 startMs = QDateTime::currentMSecsSinceEpoch();
    const QString outputPath = resolved.outputPath;
    connect(m_estimateTimer, &QTimer::timeout, this, [this, outputPath, startMs]() {
        int sizeEst = 0;
        const QFileInfo fi(outputPath);
        if (fi.exists() && m_inputSize > 0) {
            const double ratio = static_cast<double>(fi.size())
                               / static_cast<double>(m_inputSize) * 2.0;
            sizeEst = qBound(0, static_cast<int>(ratio * 95.0), 95);
        }
        const qint64 elapsedMs = QDateTime::currentMSecsSinceEpoch() - startMs;
        // V32.32: exponential approach to 60% asymptote.
        constexpr double kTimeConstantMs = 4000.0;
        constexpr double kAsymptote     = 60.0;
        const double t = static_cast<double>(elapsedMs) / kTimeConstantMs;
        const int timeEst = qBound(0,
            static_cast<int>(kAsymptote * (1.0 - std::exp(-t))),
            60);
        const int est = qMax(sizeEst, timeEst);
        if (est > m_lastProgress) {
            m_lastProgress = est;
            emit progress(est);
        }
    });

    connect(m_proc, QOverload<int, QProcess::ExitStatus>::of(&QProcess::finished),
            this, [this, outputPath, backendName = backend->name()]
                  (int code, QProcess::ExitStatus) {
        m_running = false;
        if (m_estimateTimer) {
            m_estimateTimer->stop();
            m_estimateTimer->deleteLater();
            m_estimateTimer = nullptr;
        }
        QFile f(outputPath);
        if (code != 0 || !f.exists() || f.size() == 0) {
            // V32.34+: if the user cancelled, don't treat this as a backend
            // failure and don't retry — just clean up and report cancellation.
            if (m_cancelled) {
                if (f.exists() && f.size() == 0) f.remove();
                if (m_proc) { m_proc->deleteLater(); m_proc = nullptr; }
                m_haveActiveSpec = false;
                m_failedBackends.clear();
                qCInfo(lcTranscode) << "Transcode cancelled by user";
                emit transcodeFailed(tr("Transcode cancelled"));
                return;
            }
            // V32.34+ RUNTIME FALLBACK: this backend was available but failed
            // to actually encode (e.g. FP5 gst-svtav1enc matroskademux
            // not-negotiated). Mark it failed and try the NEXT available
            // backend with the same spec before surfacing a hard failure.
            if (f.exists() && f.size() == 0) f.remove();  // clean partial output
            if (!m_failedBackends.contains(backendName))
                m_failedBackends.append(backendName);
            if (m_proc) { m_proc->deleteLater(); m_proc = nullptr; }

            IVideoEncoderBackend *next =
                findAvailableBackendExcluding(m_failedBackends);
            if (next && m_haveActiveSpec) {
                qCWarning(lcTranscode)
                    << "Backend" << backendName << "exited" << code
                    << "— retrying with" << next->name();
                // m_backend caches the fast-path choice; clear it so the retry
                // doesn't reuse the failed one via selectBackend().
                m_backend = nullptr;
                startWithSpec(m_activeSpec);   // re-enters; keeps m_failedBackends
                return;
            }
            qCWarning(lcTranscode)
                << "Backend" << backendName << "exited" << code
                << "— no further backends to try";
            m_haveActiveSpec = false;   // terminal: next start() is fresh
            m_failedBackends.clear();
            emit transcodeFailed(tr("Encoder exited %1 with no/empty output").arg(code));
            return;
        }
        // 100% only on success — bar doesn't snap back from 99 to 100.
        m_haveActiveSpec = false;   // success: next start() is fresh
        m_failedBackends.clear();
        m_lastProgress = 100;
        emit progress(100);
        qCInfo(lcTranscode) << "Transcode complete:" << outputPath
                            << "size:" << f.size();
        emit transcodeFinished(outputPath);
    });

    m_outputPath = outputPath;
    qCInfo(lcTranscode) << "Transcoding" << resolved.inputPath << "→" << outputPath
                        << "(backend:" << backend->name()
                        << "tier:" << SnapClone::Core::Media::tierName(resolved.tier)
                        << "width:" << resolved.targetWidth
                        << "bitrate:" << resolved.bitrateKbps << "kbps)";
    // Log full args at INFO so the user can copy-paste to reproduce on hang.
    qCInfo(lcTranscode) << backend->name() << "command:"
                        << execPath << args.join(QLatin1Char(' '));

    m_proc->start(execPath, args);
    if (!m_proc->waitForStarted(2000)) {
        emit transcodeFailed(tr("Encoder process failed to start"));
        m_proc->deleteLater();
        m_proc = nullptr;
        return false;
    }
    m_running = true;
    if (m_estimateTimer) m_estimateTimer->start();
    emit progress(0);
    return true;
}

bool VideoTranscoder::start(const QString &inputPath, const QString &outputPath,
                            int targetWidth, int targetHeight, int targetBitrateKbps)
{
    // Map the positional-arg API onto an EncodeSpec at Standard tier.
    // Callers that want a specific tier (thumbnail / preview / high quality)
    // should use startWithSpec() directly.
    Q_UNUSED(targetHeight);
    EncodeSpec spec;
    spec.inputPath   = inputPath;
    spec.outputPath  = outputPath;
    spec.tier        = EncodeTier::Standard;
    spec.targetWidth = targetWidth;
    spec.bitrateKbps = targetBitrateKbps;
    return startWithSpec(spec);
}

void VideoTranscoder::cancel()
{
    if (!m_running || !m_proc) return;
    qCInfo(lcTranscode) << "Cancelling transcode";
    // V32.34+: mark cancelled so the finished() handler treats the resulting
    // non-zero exit as a user cancel, NOT a backend runtime failure — otherwise
    // the runtime-fallback logic would "helpfully" restart the transcode on the
    // next backend right after the user asked to stop.
    m_cancelled = true;
    m_proc->terminate();
    if (!m_proc->waitForFinished(2000)) m_proc->kill();
}

} // namespace SnapClone::UI::Video
