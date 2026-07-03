#pragma once
// ─── UI/PressGesture.hpp ─────────────────────────────────────────────────────
//
// V32.30: robust multi-modal press detection (tap vs long-press vs drag-cancel).
//
// Why this exists:
//   The previous tap-and-hold logic (ContactsPage edit button) used
//   QPushButton::pressed → QTimer → QPushButton::released signals. That
//   worked on desktop mouse but proved unreliable on touch — the user
//   reported "tap and hold to edit is never opening." Touch event flow
//   has too many subtle paths that can miss a signal:
//     • Qt's touch-to-mouse synthesis can be defeated by upstream
//       WA_AcceptTouchEvents widgets
//     • Some platforms deliver TouchBegin without ever firing pressed()
//     • Scroll-area drag-threshold detection can swallow the press
//   This class is an FSM that watches BOTH touch and mouse events and
//   emits clean, well-defined signals regardless of which path Qt picks.
//
// FSM states:
//
//     Idle ──[press]──▶ Pressing ──[500ms]──▶ Held(emit longPress)
//      ▲                  │
//      │                  ├──[release before 500ms]──▶ emit tap, → Idle
//      │                  │
//      │                  └──[move > kMoveCancelPx]──▶ emit dragCanceled, → Idle
//      │
//      └──[release|cancel from Held]──▶ Idle
//
// Move-cancel threshold: 16px. Larger than typical jitter; smaller than
// what a "swipe" looks like. Once the user moves their finger >16px from
// the initial touch point, we conclude they're either drag-scrolling or
// trying to swipe — not pressing — so we abandon the gesture without
// emitting longPress or tap.
//
// Usage:
//   auto *g = new PressGesture(myButton);
//   connect(g, &PressGesture::tap, this, [](){ ... });
//   connect(g, &PressGesture::longPress, this, [](){ ... });
//
// The gesture installs itself as an event filter on the watched widget.
// It honors the widget's hit-test geometry — only events INSIDE the
// widget rect (in local coords) count. Releases outside the rect emit
// dragCanceled.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QPoint>
#include <QPointer>
#include <QHash>

#include "ui/PressFsm.hpp"

QT_BEGIN_NAMESPACE
class QWidget;
class QTimer;
class QEvent;
QT_END_NAMESPACE

namespace SnapClone::UI {

class PressGesture : public QObject {
    Q_OBJECT
public:
    /// Attach to `target`. The gesture installs itself as an event filter,
    /// reparents to target, and emits its signals on the GUI thread.
    /// Target must outlive the gesture (or get destroyed first, in which
    /// case Qt parent-child cleanup tears the gesture down too).
    explicit PressGesture(QWidget *target);
    ~PressGesture() override;

    /// Long-press threshold. Default 500ms (Android standard).
    void setLongPressMs(int ms);

    /// Movement cancellation threshold in pixels. Default 16. Increase if
    /// you find legitimate long-presses on small targets getting canceled
    /// by minor finger drift.
    void setMoveCancelPx(int px);

signals:
    /// User pressed and released within longPressMs, without moving > cancel
    /// threshold. The equivalent of a "click" but reliably touch-aware.
    void tap();

    /// User held for >= longPressMs without moving > cancel threshold.
    /// Fires ONCE when the threshold is crossed; subsequent presses
    /// re-arm the FSM from Idle.
    void longPress();

    /// Press was abandoned (release outside the widget, or move > cancel
    /// threshold during press). Mostly useful for UI feedback like
    /// dismissing a "tap and hold to edit" hint.
    void dragCanceled();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    void enterPressing(const QPoint &localPos);
    void enterIdle();
    void enterHeld();
    void checkMoveCancel(const QPoint &localPos);

    // V32.32: gesture-registry watchdog. Per user request:
    // "1000ms delay timer to check if same tap gesture is registered
    // as at start (hash by position and name them then track them in
    // a table) so that we can unbreak partial transitions that are
    // abandoned."
    //
    // Each PressGesture instance registers itself in s_registry when
    // it enters Pressing state. A single class-static watchdog timer
    // sweeps s_registry every kWatchdogIntervalMs (1000ms). For each
    // entry that's been in flight longer than kWatchdogStuckMs
    // without resolution, the watchdog force-resets it and emits a
    // diagnostic log line.
    //
    // The "name/hash" key combines: target widget pointer + press
    // position. Two different presses on the same widget at the same
    // position within the same registry tick would collide — but
    // that's so rare in practice (humans don't tap the same pixel
    // twice in <1s) that we accept the collision rather than
    // tracking a monotonic counter.
    struct RegistryEntry {
        QPointer<PressGesture> gesture;  // weak — gesture may have been destroyed
        qint64                 startedAtMs;
        QPoint                 pressPos;
    };
    static QHash<quint32, RegistryEntry> s_registry;
    static QTimer                       *s_watchdogTimer;

    /// Compute the registry key for a (target, pressPos) pair.
    static quint32 registryKey(const QWidget *target, const QPoint &pos);
    /// Class-static watchdog tick — sweep the registry, force-reset
    /// stuck entries, log diagnostics. Called from s_watchdogTimer.
    static void watchdogSweep();
    /// Lazy-init the class-static watchdog timer on first use.
    static void ensureWatchdogRunning();

    /// This instance's current registry key (or 0 if not registered).
    /// Stored on the instance so we can deregister cleanly without
    /// re-hashing position (which might have shifted by then).
    quint32 m_registryKey = 0;

    QPointer<QWidget> m_target;
    QTimer  *m_timer = nullptr;
    // Pure decision core: owns the Idle/Pressing/Held state, the press origin,
    // and the long-press / move-cancel thresholds. PressGesture only does the
    // Qt-side work (event translation, the timer, the watchdog, emitting).
    PressFsm m_fsm;

    static constexpr int kWatchdogIntervalMs = 1000; // sweep frequency
    static constexpr int kWatchdogStuckMs    = 1000; // entries older than this = stuck
};

} // namespace SnapClone::UI
