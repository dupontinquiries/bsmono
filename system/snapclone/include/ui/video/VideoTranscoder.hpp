#pragma once

// ─── ui/video/VideoTranscoder.hpp ────────────────────────────────────────────
//
// Transcodes an H.264/Opus .mkv (from VideoRecorder) into AV1/Opus .webm
// (or whatever the active backend produces — see VideoEncoderBackend.hpp).
// Audio is re-muxed without re-encode (Opus passthrough).
//
// Why AV1 for transmission:
//   • Roughly 2x better compression than H.264 at equivalent quality.
//     A 30s 720p clip drops from ~12 MB to ~5 MB → faster send over cellular.
//   • Mature decoder support via libdav1d (Qt's FFmpeg backend includes it).
//
// Why this is async / progress-reporting:
//   • libsvtav1 on FP5-class ARM is ~2-5fps at 720p. A 30-second clip takes
//     1.5-3 minutes. The UI must show progress so the user doesn't think
//     the app froze.
//
// Backend selection: VideoTranscoder picks an encoder backend from
// VideoEncoderBackend.cpp's priority list at startup. The first available
// backend wins. Future codec binaries (in development) take priority over
// FFmpeg, which takes priority over gst-launch+svtav1enc, with VP9/VP8 as
// last-resort fallbacks. See VideoEncoderBackend.hpp for the full chain.
//
// On devices where no encoder is available, start() fails and the caller
// can fall back to sending the original H.264 .mkv directly.
//
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/video/VideoEncoderBackend.hpp"

#include <QObject>
#include <QString>
#include <QRect>      // V32.34+: detectSalientRoi return type
#include <QRegularExpression>
#include <memory>

QT_BEGIN_NAMESPACE
class QProcess;
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::UI::Video {

class VideoTranscoder : public QObject
{
    Q_OBJECT
public:
    explicit VideoTranscoder(QObject *parent = nullptr);
    ~VideoTranscoder() override;

    /// Begin transcoding using an explicit EncodeSpec (preferred API).
    /// This is the entry point the AppController's tiered delivery flow
    /// calls — it specifies a tier (Thumbnail / Preview / Standard /
    /// HighQuality) and the backend translates that to its encoder args.
    ///
    /// Returns false if no backend is available OR the input is missing.
    bool startWithSpec(const EncodeSpec &spec);

    /// Convenience overload preserving the old positional-arg API.
    /// Maps to EncodeTier::Standard with the given width/height/bitrate.
    /// `targetWidth` = 0 means "keep source width" (full quality default).
    /// `targetHeight` is a hint only — height now floats with aspect ratio.
    ///
    /// Resolution policy: defaults preserve full quality. The caller
    /// (AppController + tiered delivery, when we route a snap as Preview
    /// or Thumbnail) is responsible for passing a smaller target if a
    /// downscale is appropriate for THIS PARTICULAR delivery. The
    /// transcoder itself does not downscale by default — high quality
    /// is the product goal, and network-adaptive downscaling is opt-in.
    bool start(const QString &inputPath,
               const QString &outputPath,
               int targetWidth      = 0,
               int targetHeight     = 0,
               int targetBitrateKbps = 2500);

    /// Cancel an in-progress transcode. transcodeFailed() will fire.
    void cancel();

    [[nodiscard]] bool isTranscoding() const { return m_running; }

    /// Name of the backend that was selected, or empty if no backend
    /// available. Useful for diagnostic logging.
    [[nodiscard]] QString activeBackendName() const;

    /// V32.32: kind (Cpu/Gpu/Hardware) of the selected backend. Default
    /// Cpu when no backend is available. Surface this in the UI's
    /// transcode-progress label so the user knows whether the CPU is
    /// being hammered or the GPU/dedicated hw is handling it.
    [[nodiscard]] IVideoEncoderBackend::Kind activeBackendKind() const;

signals:
    /// Emitted at 0-100 as encoding progresses.
    void progress(int percent);

    /// Transcode succeeded; output file is at the path passed to start().
    void transcodeFinished(const QString &outputPath);

    /// Transcode failed (gst exit nonzero, output missing/empty, etc.).
    void transcodeFailed(const QString &reason);

private:
    /// Detect whether the input has an audio track using gst-discoverer.
    /// Returns true if audio is present (or detection failed, defaulting
    /// to "probably yes" — adding an audio branch to a video-only input
    /// is what hangs the muxer, but ASSUMING audio when there isn't
    /// usually fails fast at link time).
    [[nodiscard]] bool detectHasAudio(const QString &inputPath) const;

    /// V32.34+ face/saliency ROI. Extract one representative frame from the
    /// input, run the Rust saliency detector on it, and return the salient
    /// (face/subject) bounding box in SOURCE pixel coordinates. Returns an
    /// invalid (null) QRect if extraction fails, saliency is unavailable
    /// (SNAPCLONE_RUST_SALIENCY=OFF), or no salient region is found — callers
    /// then simply skip the ROI boost. Synchronous: spawns a short ffmpeg
    /// single-frame extract (a few hundred ms), acceptable as a one-time
    /// pre-encode step on a transcode that already takes seconds.
    [[nodiscard]] QRect detectSalientRoi(const QString &inputPath) const;

    /// V32.34+ frame dims (display pixels) the most recent detectSalientRoi()
    /// resolved for. The ROI bit-allocation planner needs the frame size to
    /// clamp its background-penalty + padded-face-boost rectangles. mutable so
    /// the const detectSalientRoi can record them; read at the spec-fill site.
    /// 0 = unknown (planner falls back to the ROI's own extent).
    mutable int m_lastRoiFrameW = 0;
    mutable int m_lastRoiFrameH = 0;

    /// Find and cache the first available backend on first call. The
    /// returned pointer is owned by the static cache; do not delete.
    [[nodiscard]] IVideoEncoderBackend *selectBackend();

    QProcess *m_proc    = nullptr;
    bool      m_running = false;
    QString   m_outputPath;
    QString   m_inputPath;          // for size-based progress estimation
    qint64    m_inputSize    = 0;
    int       m_lastProgress = 0;   // last value emitted (monotonic)
    QTimer   *m_estimateTimer = nullptr;
    IVideoEncoderBackend *m_backend = nullptr; ///< borrowed, see selectBackend()
    QRegularExpression    m_progressRx;        ///< from m_backend

    // V32.34+ RUNTIME FALLBACK. The chosen backend is picked by an availability
    // probe, but a backend can be "available" yet fail at RUNTIME on a given
    // device — e.g. on FP5 the gst-svtav1enc pipeline can't negotiate
    // matroskademux ("not-negotiated (-4)") and exits 1, even though ffmpeg
    // handles the same .mkv fine. Previously a runtime failure went straight to
    // "send raw .mkv". Now we remember the spec + which backends already failed
    // this transcode, and on a runtime failure we transparently retry with the
    // NEXT available backend before giving up. Cleared on a fresh start().
    EncodeSpec            m_activeSpec;          ///< spec of the in-flight transcode
    bool                  m_haveActiveSpec = false;
    QStringList           m_failedBackends;      ///< names that failed at runtime
    bool                  m_cancelled = false;   ///< user cancel; suppress retry
};

} // namespace SnapClone::UI::Video
