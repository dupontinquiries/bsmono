#pragma once
// ─── UI/camera/FocusReticle.hpp ──────────────────────────────────────────────
// V32.34+: tap-to-focus visual feedback overlay.
//
// A small square reticle that appears at the tap point on the viewfinder,
// pulses while the CDAF sweep is running, and flashes the result state
// briefly before fading out:
//   • Searching (default) → cyan, pulsing
//   • Locked  (sweep succeeded, sharpness improved) → green, steady
//   • Failed  (no sharp peak found / VCM unavailable) → red-orange, steady
//
// Lifecycle: created once as a child of the viewfinder label, hidden by
// default. CameraWidget calls `triggerAt(point)` on tap, and `setState()`
// from the focus-sweep completion handler. The reticle handles its own
// pulse animation + fade-out timing.
//
// Click-through: WA_TransparentForMouseEvents so it never absorbs the
// subsequent tap on the viewfinder.
// ─────────────────────────────────────────────────────────────────────────────

#include <QWidget>
#include <QColor>
#include <QPointF>

QT_BEGIN_NAMESPACE
class QTimer;
class QPaintEvent;
class QPropertyAnimation;
QT_END_NAMESPACE

namespace SnapClone::UI::Camera {

class FocusReticle : public QWidget
{
    Q_OBJECT
    /// Pulse phase for the searching state (0..1, looped by m_pulseTimer).
    Q_PROPERTY(qreal pulsePhase READ pulsePhase WRITE setPulsePhase)
public:
    enum class State {
        Searching,  ///< Sweep in progress — cyan, pulsing
        Locked,     ///< Sweep completed — green, steady
        Failed,     ///< Sweep failed — red-orange, steady
    };
    Q_ENUM(State)

    explicit FocusReticle(QWidget *parent = nullptr);
    ~FocusReticle() override;

    /// Show the reticle at the given point (in PARENT coordinates) and
    /// start in the Searching state. The point is the center; the reticle's
    /// geometry is positioned so this point is its midpoint.
    /// Idempotent — repeated triggerAt before a sweep finishes just moves
    /// the reticle without restarting the pulse phase.
    void triggerAt(const QPointF &center);

    /// Transition to a result state. Triggers a 1.2s linger then fade-out.
    /// If the reticle isn't visible, this is a no-op (we shouldn't be
    /// hearing about a focus result if we never triggered a sweep).
    void setState(State s);

    /// Pulse phase property (animation).
    [[nodiscard]] qreal pulsePhase() const { return m_pulsePhase; }
    void setPulsePhase(qreal v) { m_pulsePhase = v; update(); }

protected:
    void paintEvent(QPaintEvent *e) override;

private:
    void startPulse();
    void stopPulse();
    void beginLingerThenFade();

    /// Reticle box side length. 72px is comfortable for a 1080-class
    /// viewfinder — visible without obscuring the subject. Adjust here
    /// if the viewfinder ever scales unusually small.
    static constexpr int kSize = 72;

    State              m_state       = State::Searching;
    qreal              m_pulsePhase  = 0.0;  // 0..1
    QTimer            *m_pulseTimer  = nullptr;
    QTimer            *m_lingerTimer = nullptr;
    QPropertyAnimation *m_fadeAnim   = nullptr;
};

} // namespace SnapClone::UI::Camera
