#pragma once

// ─── ui/video/VideoRecorder.hpp ──────────────────────────────────────────────
//
// Records video + audio to an H.264/Opus .mkv file using gst-launch as a
// child process. Captures from libcamera/PipeWire (whatever the camera-
// bridge or QtCameraBackend exposes — same source the viewfinder uses).
//
// We use gst-launch instead of QMediaRecorder because:
//   • QMediaRecorder doesn't reliably enumerate camera-bridge nodes on
//     postmarketOS (same reason CameraWidget needed GstLaunchBackend).
//   • gst-launch lets us hand-tune codec parameters per device.
//
// H.264 is chosen for capture (not AV1) because:
//   • x264enc speed-preset=ultrafast is real-time on the FP5.
//   • libsvtav1 even at preset 12 is too slow for real-time capture.
//   • The transcode step (H.264 → AV1) happens after recording stops,
//     so the user can put the phone down while it works.
//
// The output file goes to a temp path; caller is responsible for moving it
// or feeding it to VideoTranscoder.
//
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QString>
#include <QElapsedTimer>
#include <atomic>

QT_BEGIN_NAMESPACE
class QProcess;
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::UI::Video {

class VideoRecorder : public QObject
{
    Q_OBJECT
public:
    explicit VideoRecorder(QObject *parent = nullptr);
    ~VideoRecorder() override;

    /// Begin recording. `pwNode` is the PipeWire source node name (same
    /// format as GstLaunchBackend uses, e.g. "virtual-cam-rear").
    /// Empty string falls back to autovideosrc.
    /// `outputPath` is the target file (.mkv); will be overwritten.
    /// Returns false if the pipeline failed to launch.
    bool start(const QString &pwNode, const QString &outputPath, int maxDurationMs = 60000);

    /// V32.32: configure rotation applied to BOTH the recorded video AND
    /// the in-pipeline preview frames. Must be set BEFORE start() — the
    /// rotation is baked into the gst pipeline at launch and can't change
    /// mid-recording.
    ///
    /// Accepted values: 0, 90, 180, 270 (degrees, clockwise). Anything
    /// else is clamped to 0. Matches the convention used by
    /// CameraWidget's applyUserRotation (Qt's QTransform::rotate is
    /// clockwise too, so the same enum/setting carries over).
    ///
    /// Implementation: inserts `videoflip method=<N>` AFTER the source
    /// and BEFORE the tee, so both encoder and preview branches see
    /// rotated frames. 0° resolves to method=none which is a passthrough
    /// (zero-copy buffer pass), so calling this with 0 is free.
    void setUserRotation(int degrees);

    /// Stop recording cleanly. The file will be properly muxed/finalized
    /// before recordingStopped() fires.
    void stop();

    /// True if a recording is in progress.
    [[nodiscard]] bool isRecording() const { return m_recording; }

    /// Elapsed milliseconds since start() — for UI display.
    [[nodiscard]] qint64 elapsedMs() const;

    /// Path to the per-frame JPEG preview file that the recorder's tee
    /// branch writes during recording. Empty string when not recording.
    /// The viewfinder polls this file to display live frames while the
    /// recorder owns the camera. See VideoRecorder.cpp for the tee
    /// pipeline rationale.
    ///
    /// The file is updated at ~10 Hz and replaced atomically by
    /// multifilesink, so a reader that opens it gets a coherent JPEG
    /// (never a half-written file).
    [[nodiscard]] QString previewFilePath() const { return m_previewPath; }

signals:
    /// Emitted when recording successfully finishes and the file is finalized.
    /// `path` is the output file path (always the one passed to start()).
    /// `durationMs` is total recording time.
    void recordingStopped(const QString &path, int durationMs);

    /// Emitted if the pipeline fails (gst exited non-zero, file not created).
    /// VideoRecorder cleans up internally before emitting.
    void recordingFailed(const QString &reason);

    /// Emitted at ~10Hz while recording. Useful for UI elapsed counters.
    void elapsedTick(int ms);

    /// V32.32: emitted exactly once per recording session, on the
    /// transition from gst-launch-spawned to pipeline-actually-running.
    /// stop() uses this with a QEventLoop to wait for PLAYING before
    /// terminating, replacing the V32.25 file-size busy-poll which
    /// blocked the GUI thread for up to 1500ms.
    ///
    /// Wired internally — not intended for external subscribers; we
    /// expose it as a Qt signal rather than a callback because QEventLoop
    /// is the cleanest way to do a bounded async wait without blocking
    /// the UI thread.
    void pipelineEnteredPlaying();

private:
    QProcess      *m_proc        = nullptr;
    bool           m_recording   = false;
    QString        m_outputPath;
    QString        m_previewPath;     ///< current preview JPEG file (empty when idle)
    QElapsedTimer  m_timer;
    int            m_maxDurationMs = 60000;
    /// V32.32: rotation degrees set via setUserRotation. Baked into the
    /// pipeline at start() time. Reset to 0 on every fresh start so a
    /// stale value from a prior recording doesn't leak through (callers
    /// must set rotation fresh each time if they want it).
    int            m_userRotation = 0;
    QTimer        *m_tickTimer  = nullptr;
    /// Accumulated gst-launch stderr — fed to recordingFailed() reason.
    /// Capped at 8KB to bound memory in pathological cases.
    QByteArray     m_lastStderr;
    /// V32.32: stdout accumulator for state-change message parsing.
    /// We scan for the "Setting pipeline to PLAYING" line emitted by
    /// gst-launch on the major state transition. Capped same as stderr.
    QByteArray     m_lastStdout;
    /// V32.32: flips true on the PLAYING transition. Read by stop() so
    /// it can short-circuit the wait if PLAYING already happened.
    /// Atomic because future async paths may set this from worker
    /// threads; currently set only on the GUI thread via the QProcess
    /// stdout signal but the atomic costs nothing and future-proofs.
    std::atomic<bool> m_pipelinePlaying { false };
};

} // namespace SnapClone::UI::Video
