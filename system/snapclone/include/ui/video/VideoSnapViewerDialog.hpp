#pragma once

// ─── ui/video/VideoSnapViewerDialog.hpp ──────────────────────────────────────
//
// Plays back a received video snap. Persistent (file remains until snap
// expiry) and exportable to user's chosen folder.
//
// Playback strategy: two backends, picked at runtime.
//
//   1. Qt media (QMediaPlayer + QVideoWidget) — first attempt.
//      Native Qt path, hardware-friendly. But Qt 6 + FFmpeg 7/8 has
//      observed issues:
//        - sws_getContext rejects the "negative dstH" flip idiom
//          → errorOccurred fires before the first frame paints.
//        - Some H.264 streams from our recorder include rotation
//          metadata that triggers the broken flip path.
//      When QMediaPlayer errors out BEFORE the first frame paints,
//      the dialog auto-fails-over to backend 2.
//
//   2. GStreamer fallback (gst-launch as child process).
//      Pipeline: decodebin → videoconvert → jpegenc → multifilesink
//                       \--→ audioconvert → autoaudiosink
//      Video frames written to a tmpfs JPEG file we poll at 20 Hz
//      and paint to a QLabel (same pattern as the recorder's preview
//      branch in V32.6). Audio goes to system sink directly.
//      Audio/video sync isn't pixel-perfect — video lags audio by
//      up to one JPEG-poll tick (50 ms). For 5-30 s snaps that's
//      imperceptible.
//
// Controls:
//   • Tap viewfinder → toggle play/pause (Qt backend only; gst backend
//     restarts the pipeline on play, no in-process pause).
//   • Position slider → scrub (Qt backend only).
//   • Save button → QFileDialog, copies file to user's path.
//   • Close → reject() (file stays in snap storage until expiry).
//
// Differs from SnapViewerDialog (image-only) which deletes after first view.
// Per user spec: persistent + exportable, not view-once.
//
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/MobileDialog.hpp"
#include "core/Types.hpp"

#include <QString>

QT_BEGIN_NAMESPACE
class QMediaPlayer;
class QVideoWidget;
class QAudioOutput;
class QSlider;
class QPushButton;
class QLabel;
class QProcess;
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::UI::Video {

class VideoSnapViewerDialog : public MobileDialog
{
    Q_OBJECT
public:
    explicit VideoSnapViewerDialog(const SnapClone::Core::Snap &snap,
                                    QWidget *parent = nullptr);
    ~VideoSnapViewerDialog() override;

signals:
    /// Emitted when the user successfully saves to an external path.
    /// Wires up to status-bar confirmation in MainWindow.
    void videoExported(QString destPath);

    /// V32.42+: emitted when the user saves the video snap — the host marks it
    /// kept in storage so it's exempt from auto-expiry.
    void snapKept(SnapClone::Core::SnapId id);

private slots:
    void onSaveClicked();
    void onPlayPauseToggle();
    void onSeek(int sliderPos);
    void onMediaPositionChanged(qint64 ms);
    void onMediaDurationChanged(qint64 ms);

private:
    void buildUi();
    /// Try QMediaPlayer. Connects errorOccurred to switch to gst
    /// fallback if Qt's pipeline fails before the first frame.
    void startQtMediaPlayback();
    /// Tear down QMediaPlayer (called when failing over to gst).
    void teardownQtMedia();
    /// Spawn gst-launch with a decodebin → jpegenc → multifilesink
    /// (video) plus → autoaudiosink (audio) pipeline. Frames land in
    /// m_gstPreviewPath which a poll timer reads into m_gstFrameLabel.
    /// Returns false if gst-launch isn't available — caller falls back
    /// to the hard-failure overlay.
    bool startGstPlayback();
    /// Stop the gst child process and the poll timer.
    void teardownGst();
    /// Display the "can't play, save instead" overlay. Final fallback.
    void showHardFailureOverlay(const QString &reason);

    SnapClone::Core::Snap m_snap;

    // ── Qt media backend (preferred) ────────────────────────────────────────
    QMediaPlayer  *m_player    = nullptr;
    QAudioOutput  *m_audioOut  = nullptr;
    QVideoWidget  *m_video     = nullptr;

    // ── GStreamer fallback backend ──────────────────────────────────────────
    /// gst-launch-1.0 child running the playback pipeline. Owned.
    QProcess      *m_gstProc          = nullptr;
    /// Path to the multifilesink JPEG file (atomically replaced per frame).
    QString        m_gstPreviewPath;
    /// Poll timer driving the QLabel paint when the gst backend is active.
    QTimer        *m_gstPollTimer     = nullptr;
    /// mtime mirror — skip JPEG decode when the file hasn't changed.
    qint64         m_gstPreviewMtime  = 0;
    /// Label that displays decoded JPEG frames in gst-backend mode.
    /// Same widget area as m_video but used only when m_video has been
    /// torn down. Lazy-created in startGstPlayback.
    QLabel        *m_gstFrameLabel    = nullptr;
    /// V32.34+: rolling capture of gst-launch stderr so the failure
    /// path can show a human-readable diagnosis instead of just
    /// "Playback pipeline failed (exit 1)". Trimmed on every read
    /// to a few KB max. Reset in startGstPlayback.
    QByteArray     m_gstStderrBuf;

    // ── Shared UI ───────────────────────────────────────────────────────────
    QSlider       *m_seekSlider = nullptr;
    QPushButton   *m_playBtn   = nullptr;
    QPushButton   *m_saveBtn   = nullptr;
    QLabel        *m_timeLabel = nullptr;
    /// Overlay shown when BOTH backends have failed. Final user-facing
    /// "save it and play in another app" message. Lazy-created.
    QLabel        *m_errorOverlay = nullptr;

    /// True after we've switched to the gst backend. Prevents bouncing
    /// back-and-forth on transient Qt errors.
    bool           m_gstActive       = false;
    /// Has QMediaPlayer ever painted a frame? Used to decide whether an
    /// error from Qt is "fail-over worthy" (no frame yet → try gst) or
    /// "ignore" (frame painted → user just hit a transient hiccup).
    bool           m_qtFramePainted  = false;
};

} // namespace SnapClone::UI::Video
