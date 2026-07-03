#pragma once
// ─── UI/AmbientCameraDriver.hpp ──────────────────────────────────────────────
//
// V32.30: live-viewfinder driven theme derivation (Mode B / "Content Creator
// Mode"). Subscribes to a CameraWidget's preview-frame signal, stores the
// most recent frame, and ~1Hz re-derives the synthetic palette from it.
//
// Battery / CPU model:
//   • Storage of the latest frame is a cheap QImage assignment (Qt
//     shallow-copy). No allocation per frame.
//   • Derivation runs only on the 1Hz tick, NOT on every frame — saves
//     ~29 derivations per second relative to a naive subscription.
//   • Tick stops when:
//       - User picks a non-synthetic palette
//       - User leaves the camera page (CameraWidget hideEvent stops its
//         backend, frames stop arriving, latest frame stays stale)
//       - The driver itself is explicitly stopped
//
// Activation pattern:
//   • Settings "Ambient: Camera" carousel click → MainWindow calls
//     start(cameraWidget)
//   • Settings any other carousel click → MainWindow calls stop()
//   • Stop also called when MainWindow destructs (Qt parent cleanup is
//     fine; explicit stop is defensive for early-shutdown paths).
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QImage>
#include <QColor>
#include <QPointer>

QT_BEGIN_NAMESPACE
class QTimer;
QT_END_NAMESPACE

namespace SnapClone::UI {

class CameraWidget;

class AmbientCameraDriver : public QObject {
    Q_OBJECT
public:
    explicit AmbientCameraDriver(QObject *parent = nullptr);
    ~AmbientCameraDriver() override;

    /// Start consuming preview frames from `cam`. Idempotent — calling
    /// start() when already running just retargets the camera widget.
    /// Caller must ensure `cam` outlives this driver (or call stop()
    /// before `cam` is destroyed). QPointer is used internally as
    /// safety net.
    void start(CameraWidget *cam);

    /// Stop sampling. Disconnects from the camera widget and pauses the
    /// derivation timer. Safe to call when not running.
    void stop();

    /// True if start() has been called and stop() hasn't.
    bool isActive() const { return m_active; }

private slots:
    void onFrame(const QImage &frame);
    void onDeriveTick();

private:
    QPointer<CameraWidget> m_cam;
    QImage   m_latestFrame;
    QTimer  *m_timer = nullptr;
    bool     m_active = false;

    /// V32.30 (Wave 2): temporal smoothing state. Walking around with the
    /// camera produces visible palette jitter as the dominant hue jumps
    /// frame-to-frame. We apply an exponential moving average on the
    /// underlying accent/background QColors before building a Palette
    /// from them, so the chrome calmly tracks rather than strobes.
    ///
    /// Smoothing factor α: 0.15. With 1Hz ticks, that's a ~4.3s half-life
    /// for stale data to fade out. Tuned for "noticeable but not jumpy"
    /// — values 0.05-0.30 are reasonable; lower = smoother (more lag).
    /// First tick: m_smoothedValid is false → no blending, use raw values
    /// directly so the chrome reacts immediately at activation.
    QColor   m_smoothedAccent;
    QColor   m_smoothedBackground;
    bool     m_smoothedValid = false;
    static constexpr float kSmoothingAlpha = 0.15f;

    /// V32.34+: async-derive guard. deriveColors() iterates 4096 pixels
    /// with QColor + HSL conversion per-pixel — measured at 20-40ms on
    /// FP5, visible as jank when a derive tick coincides with a swipe.
    /// We now run the sample on a QThreadPool worker via QtConcurrent
    /// and apply the result on the GUI thread.
    ///
    /// m_deriveInFlight prevents stacking: if the previous derive hasn't
    /// returned by the next tick (slow device under load), we drop the
    /// new tick rather than spawning a parallel worker. The 2s tick
    /// interval means this almost never happens unless the device is
    /// thrashing, in which case skipping a tick is the right behavior.
    bool     m_deriveInFlight = false;

    /// V32.34+ battery: app-foreground tracking. The 2-second derive
    /// tick fires regardless of whether the app is on screen — when
    /// backgrounded, the camera widget stops feeding fresh frames
    /// but our timer keeps re-deriving the stale frame. With app-
    /// state gating we stop the timer entirely on background and
    /// restart on resume. Saves ~30 worker-pool wakeups per minute
    /// of backgrounded ambient-mode use.
    bool     m_appActive = true;
};

} // namespace SnapClone::UI
