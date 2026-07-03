#pragma once

// ─── UI/SnapViewerDialog.hpp ──────────────────────────────────────────────────
//
// Full-screen snap viewer with explicit Save and Close controls.
//
// Product philosophy (not a Snapchat clone):
//   • The viewer gives the recipient full agency — they can save the image
//     to ~/Pictures/SnapClone/ with a timestamped filename that carries the
//     sender's ID and the snap ID as metadata.
//   • An auto-close countdown timer shows how long the snap will remain
//     open on screen.  When it expires the dialog closes itself.
//     The recipient may dismiss early or save before the timer runs out.
//   • Saving does NOT notify the sender (no surveillance model).
//   • The snap record in local storage is marked viewed; the raw bytes are
//     cleared from the in-memory Snap struct after the dialog closes.
//
// Auto-close:
//   durationSecs is taken from the Snap struct (default 5s for images,
//   longer for video in future).  A QProgressBar counts down visually.
//   AppController::snapViewExpired() also fires at the same time, which
//   causes InboxWidget to update the row appearance even if the dialog
//   was already closed by the user.
//
// Saved file naming convention:
//   ~/Pictures/SnapClone/<timestamp>_from_<senderId>_<snapId(8chars)>.jpg
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/MobileDialog.hpp"
#include <QPixmap>
#include <QList>
#include <QElapsedTimer>
#include "core/Types.hpp"

QT_BEGIN_NAMESPACE
class QLabel;
class QPushButton;
class QProgressBar;
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::UI {

class SnapViewerDialog : public MobileDialog
{
    Q_OBJECT

public:
    /// Construct the viewer for a received snap.
    /// \param snap        The fully decrypted snap (encryptedBlob holds JPEG bytes).
    /// \param parent      Parent widget (typically InboxWidget or MainWindow).
    explicit SnapViewerDialog(const Core::Snap &snap, QWidget *parent = nullptr);

    /// V32.45+: turn the viewer into a browsable feed player over an ordered
    /// list of image/text snaps. Switches off the view-once auto-expiry timer
    /// and shows ◀ / position / ▶ navigation; \a startIndex is the snap tapped.
    void setPlaylist(const QList<Core::Snap> &snaps, int startIndex);

signals:
    /// Emitted when the user explicitly saves the image to disk.
    /// Carries the path it was saved to so the caller can show a status message.
    void imageSaved(QString path);

    /// V32.42+: emitted when the user saves the snap — the host marks it kept in
    /// storage so it's exempt from auto-expiry.
    void snapKept(SnapClone::Core::SnapId id);

    /// Emitted when the timer expires (mirrors AppController::snapViewExpired).
    void viewTimerExpired(SnapClone::Core::SnapId id);

    /// V32.45+: emitted each time a snap is shown in playlist/feed mode, so the
    /// host can mark it viewed (browsing the feed counts as opening each one).
    void snapViewed(SnapClone::Core::SnapId id);

private slots:
    void onSaveClicked();
    void onTimerTick();
    void navPrev();
    void navNext();

protected:
    bool eventFilter(QObject *obj, QEvent *e) override;
    void keyPressEvent(QKeyEvent *e)   override;
    void resizeEvent  (QResizeEvent *e) override;

private:
    void buildUi(const Core::Snap &snap);
    void loadImage(const QByteArray &jpegBytes);
    void loadSnap(const Core::Snap &snap);   // populate media + meta for a snap
    void showSnapAt(int index);              // navigate the playlist
    void updateNavState();                   // position label + button enable
    [[nodiscard]] QString buildSavePath(const Core::Snap &snap) const;

    Core::Snap   m_snap;
    QPixmap      m_fullPixmap;           // full-resolution decoded image for rescaling
    QLabel      *m_imageLabel    = nullptr;
    QLabel      *m_metaLabel     = nullptr;  // sender + timestamp
    QPushButton *m_saveBtn       = nullptr;
    QPushButton *m_closeBtn      = nullptr;
    QProgressBar *m_countdown    = nullptr;
    QTimer       *m_timer        = nullptr;
    /// V32.34+: debounce timer for the smooth rescale on resize. Every
    /// resizeEvent does a SmoothTransformation scale of m_fullPixmap
    /// (potentially 4032×3024 → screen size, ~50ms). During an active
    /// keyboard-popup or window-resize gesture, those events fire
    /// continuously and freeze the UI. We now do a FAST rescale
    /// immediately + arm this timer to follow up with a smooth one
    /// 80ms after the last resize stops.
    QTimer       *m_smoothResizeTimer = nullptr;

    qint64       m_remainingMs   = 0;
    qint64       m_totalMs       = 0;      // total duration for progress bar range
    QElapsedTimer m_elapsed;               // measures actual wall-clock time elapsed
    bool         m_saved         = false;    // prevents double-save on rapid clicks

    // V32.45+: feed-player playlist (image/text snaps). Empty == single-snap
    // view-once mode (legacy behaviour).
    QList<Core::Snap> m_playlist;
    int          m_index         = -1;
    bool         m_audioPlaying  = false;   // V32.47+: current voice note is playing
    qint64       m_audioTotalMs  = 0;       // V32.49+: duration, for scrub→seek mapping
    qint64       m_lastScrubAtMs = 0;       // V32.49+: throttle seeks during a drag
    QWidget     *m_navBar        = nullptr;
    QPushButton *m_prevBtn       = nullptr;
    QPushButton *m_nextBtn       = nullptr;
    QLabel      *m_posLabel      = nullptr;
};

} // namespace SnapClone::UI
