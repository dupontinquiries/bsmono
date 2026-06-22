#pragma once

// ─── ui/video/ShutterButton.hpp ──────────────────────────────────────────────
//
// Custom shutter widget that handles three gestures from a single touch:
//
//   • Quick tap (release within ~500ms, no motion)
//       → emits clicked() — photo capture, exactly like the old QPushButton
//
//   • Hold (>500ms, no motion)
//       → emits recordStarted() — video recording begins
//       → button changes color (red ring) to indicate live recording
//
//   • Hold + swipe up (>40px upward while held)
//       → emits recordLocked() — recording continues hands-free
//       → button changes to "stop" icon; tap to end
//
//   • Release while recording (and not locked)
//       → emits recordStopped(durationMs) — finalize recording
//
//   • Tap while locked
//       → emits recordStopped(durationMs) — finalize recording
//
// We subclass QWidget and handle raw mouse + touch events ourselves rather
// than using QPushButton + an event filter, because Qt's click-detection
// logic in QPushButton would fight us — it auto-emits clicked on release
// regardless of how long the press lasted.
//
// Touch flow caveat: this widget IS a leaf so SwipeNavigator's app-level
// filter will not interfere (SwipeNavigator's TouchBegin returns false for
// direct touches, deferring to Qt's mouse synthesis). We rely on Qt's
// synthesis: when SwipeNavigator's filter passes the event through, Qt
// generates mousePressEvent, mouseMoveEvent, mouseReleaseEvent for us.
// We use those directly — no need for native touch handling.
//
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QPoint>
#include <QElapsedTimer>

QT_BEGIN_NAMESPACE
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::UI::Video {

class ShutterButton : public QWidget
{
    Q_OBJECT
public:
    explicit ShutterButton(QWidget *parent = nullptr);
    ~ShutterButton() override;

    /// Visually mark the button as "currently recording" — caller uses this
    /// to keep the UI in sync if recording was started/stopped externally
    /// (e.g. max-duration timeout fires from VideoRecorder).
    void setRecording(bool recording);
    void setLocked(bool locked);

    [[nodiscard]] bool isRecording() const { return m_recording; }
    [[nodiscard]] bool isLocked() const    { return m_locked; }

signals:
    /// Quick tap — photo capture. Same semantics as old QPushButton::clicked.
    void clicked();
    /// Hold detected — start recording.
    void recordStarted();
    /// Swipe-up while holding — recording continues hands-free.
    void recordLocked();
    /// Release-while-recording or tap-when-locked — stop recording.
    void recordStopped(int durationMs);
    /// Swipe-down while idle (not recording) — open the voice/text mode picker.
    void voiceModeRequested();

protected:
    void paintEvent(QPaintEvent *e) override;
    void mousePressEvent(QMouseEvent *e) override;
    void mouseMoveEvent(QMouseEvent *e) override;
    void mouseReleaseEvent(QMouseEvent *e) override;
    void changeEvent(QEvent *e) override;  ///< repaint on theme change
    bool event(QEvent *e) override;        ///< route touch → mouse handlers
    QSize sizeHint() const override;

private:
    void onHoldTimeout();
    /// V32.31: 1s-after-record-started safety net. If the user released
    /// during the brief window the OS / window system / touch driver
    /// dropped the release event, m_pressed stays stuck true and the
    /// recorder runs forever. This watchdog re-checks m_pressed at
    /// kPressWatchdogMs after recordStarted and fires a stop if the
    /// press has gone away. Does NOT fire if the recording got locked
    /// (lock deliberately clears m_pressed; hands-free recording is the
    /// user's choice and runs until they tap again).
    void onPressWatchdogTick();

    QTimer       *m_holdTimer = nullptr;  // fires at kHoldThresholdMs
    QTimer       *m_pressWatchdog = nullptr; // fires kPressWatchdogMs after recordStarted
    QElapsedTimer m_pressTimer;           // measures THIS press duration
    QElapsedTimer m_recordingTimer;       // measures total recording duration
                                          // (started at hold-fire, used at stop)
    QPoint        m_pressPos;             // press location for swipe detection
    bool          m_pressed   = false;
    bool          m_recording = false;
    bool          m_locked    = false;

    static constexpr int kHoldThresholdMs   = 500;  // press > this = hold.
                                                     // Raised from 250: on a
                                                     // touchscreen, input
                                                     // latency + a natural tap
                                                     // can exceed 250ms and
                                                     // trip video. 500ms is the
                                                     // conventional long-press
                                                     // and sits well clear of
                                                     // any real tap.
    static constexpr int kSwipeUpThresholdPx = 40;   // upward motion to lock
    static constexpr int kSwipeDownThresholdPx = 40; // downward motion (idle) → mode picker
    static constexpr int kTapMotionTolerancePx = 8;  // max motion for "tap"
    /// V32.31: how long after recordStarted to re-verify m_pressed.
    /// 1s gives a comfortable buffer over the 500ms hold threshold —
    /// real users always still have their finger down 1s in. Anything
    /// shorter risks false-stops during normal recording. Anything
    /// longer is more time stuck in a forever-record state.
    static constexpr int kPressWatchdogMs   = 1000;
};

} // namespace SnapClone::UI::Video
