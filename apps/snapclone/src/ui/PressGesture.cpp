// ─── UI/PressGesture.cpp ─────────────────────────────────────────────────────
// V32.30 — see header.
// ─────────────────────────────────────────────────────────────────────────────

#include "ui/PressGesture.hpp"

#include <QWidget>
#include <QTimer>
#include <QEvent>
#include <QMouseEvent>
#include <QTouchEvent>
#include <QCoreApplication>  // V32.32: qApp for watchdog timer parent
#include <QDateTime>         // V32.32: registry timestamps
#include <QLoggingCategory>

namespace {
Q_LOGGING_CATEGORY(lcGesture, "snapclone.ui.gesture")
}

namespace SnapClone::UI {

// V32.32: gesture-registry static storage. One QHash + one QTimer
// shared across ALL PressGesture instances. The timer is lazy-inited
// on the first call to ensureWatchdogRunning() and runs forever after
// (it ticks once per second; if the registry is empty the work is a
// single empty-hash check, so the cost is negligible).
QHash<quint32, PressGesture::RegistryEntry> PressGesture::s_registry;
QTimer *PressGesture::s_watchdogTimer = nullptr;

quint32 PressGesture::registryKey(const QWidget *target, const QPoint &pos)
{
    // Combine target pointer + press position into a 32-bit key.
    // Pointer bits: top half. Position bits: bottom half (10 bits each).
    // Collisions are acceptable per the design rationale in the header.
    const auto ptrBits  = static_cast<quint32>(
        reinterpret_cast<quintptr>(target) & 0xFFFFu) << 16;
    const auto posBits  = (static_cast<quint32>(pos.x() & 0xFF) << 8)
                        |  static_cast<quint32>(pos.y() & 0xFF);
    return ptrBits | posBits;
}

void PressGesture::ensureWatchdogRunning()
{
    if (s_watchdogTimer) return;
    // Parent the timer to qApp so it lives for the app lifetime. We
    // can't parent it to any one PressGesture instance because that
    // instance can be destroyed while other gestures still need the
    // watchdog. qApp is the natural app-scope owner.
    s_watchdogTimer = new QTimer(qApp);
    s_watchdogTimer->setInterval(kWatchdogIntervalMs);
    QObject::connect(s_watchdogTimer, &QTimer::timeout,
                     qApp, &PressGesture::watchdogSweep);
    s_watchdogTimer->start();
    qCDebug(lcGesture) << "Gesture watchdog started, interval="
                       << kWatchdogIntervalMs << "ms";
}

void PressGesture::watchdogSweep()
{
    if (s_registry.isEmpty()) return;
    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    // Collect keys to remove BEFORE mutating the hash to avoid
    // iterator invalidation. We don't modify entries we're keeping.
    QList<quint32> toReset;
    for (auto it = s_registry.constBegin(); it != s_registry.constEnd(); ++it) {
        const auto &e = it.value();
        const qint64 ageMs = nowMs - e.startedAtMs;
        if (ageMs < kWatchdogStuckMs) continue;
        toReset.append(it.key());
    }
    for (const quint32 key : toReset) {
        auto it = s_registry.find(key);
        if (it == s_registry.end()) continue;
        const auto &e = it.value();
        if (e.gesture) {
            qCWarning(lcGesture) << "Stuck gesture detected — forcing reset."
                                  << "target=" << (e.gesture->m_target
                                                   ? e.gesture->m_target->objectName()
                                                   : QString())
                                  << "pressPos=" << e.pressPos
                                  << "state=" << static_cast<int>(e.gesture->m_fsm.state());
            // Force-reset the FSM. This unblocks future taps on the same
            // widget — without it, the FSM would stay Pressing or Held
            // forever and the next press would be ignored (the
            // isIdle() guard in MouseButtonPress).
            e.gesture->enterIdle();
            // Fire dragCanceled so callers can clean up any visual
            // mid-gesture state. (longPress already fired if state was
            // Held — that's not a stuck case if the timer ran, so this
            // signal is most often the right tear-down hook.)
            emit e.gesture->dragCanceled();
        }
        s_registry.erase(it);
    }
}

PressGesture::PressGesture(QWidget *target)
    : QObject(target), m_target(target)
{
    if (!target) {
        qCWarning(lcGesture) << "PressGesture: nullptr target — gesture inert";
        return;
    }
    target->installEventFilter(this);
    // NOTE: we deliberately do NOT setAttribute(WA_AcceptTouchEvents).
    // On most Qt platforms, mouse-event synthesis works when the widget
    // does NOT have that attribute — Qt sees the widget can't handle
    // touch natively and falls back to mouse. By staying in the
    // mouse-synth path AND also peeking at the touch events that flow
    // through the chain (some platforms still deliver them to filters),
    // we cover all platforms with one FSM. If a platform delivers ONLY
    // touch and never synthesizes, the TouchBegin/End handlers below
    // pick it up directly.

    m_timer = new QTimer(this);
    m_timer->setSingleShot(true);
    m_timer->setInterval(m_fsm.longPressMs());
    connect(m_timer, &QTimer::timeout, this, [this]() {
        if (m_fsm.onLongPressTimeout() == PressOutcome::LongPress) {
            enterHeld();
            emit longPress();
        }
    });
}

PressGesture::~PressGesture()
{
    // V32.32: ensure we leave no orphan registry entry behind. If we're
    // destroyed mid-gesture (target widget destroyed, e.g. card recycled),
    // the watchdog would otherwise sweep a stale entry with a dangling
    // QPointer. Cheap defensive cleanup.
    if (m_registryKey != 0) {
        s_registry.remove(m_registryKey);
        m_registryKey = 0;
    }
}

void PressGesture::setLongPressMs(int ms)
{
    m_fsm.setLongPressMs(ms);
    if (m_timer) m_timer->setInterval(m_fsm.longPressMs());
}

void PressGesture::setMoveCancelPx(int px)
{
    m_fsm.setMoveCancelPx(px);
}

void PressGesture::enterPressing(const QPoint &localPos)
{
    m_fsm.onPress(localPos.x(), localPos.y());   // Idle → Pressing (ignored otherwise)
    if (m_timer) m_timer->start();
    qCDebug(lcGesture) << "enterPressing @" << localPos
                       << "(target:" << (m_target ? m_target->objectName() : QString()) << ")";

    // V32.32: register with the class-static watchdog. The watchdog
    // will force-reset us if no resolution arrives within 1s. Belt-
    // and-braces against missing release events / dropped touches
    // / Qt event-routing glitches that leave the FSM stuck.
    ensureWatchdogRunning();
    // If we somehow still had a stale entry (shouldn't happen — every
    // path through enterIdle/enterHeld removes it — but defensive),
    // overwrite it.
    if (m_registryKey != 0) {
        s_registry.remove(m_registryKey);
    }
    m_registryKey = registryKey(m_target.data(), localPos);
    s_registry.insert(m_registryKey, RegistryEntry{
        QPointer<PressGesture>(this),
        QDateTime::currentMSecsSinceEpoch(),
        localPos
    });
}

void PressGesture::enterIdle()
{
    m_fsm.reset();
    if (m_timer) m_timer->stop();
    // V32.32: deregister from the watchdog. Gesture resolved cleanly.
    if (m_registryKey != 0) {
        s_registry.remove(m_registryKey);
        m_registryKey = 0;
    }
}

void PressGesture::enterHeld()
{
    // The FSM already transitioned to Held in onLongPressTimeout(); here we do
    // the Qt-side cleanup that accompanies it.
    if (m_timer) m_timer->stop();
    qCDebug(lcGesture) << "enterHeld";
    // V32.32: deregister from the watchdog. longPress fired = gesture
    // resolved. The Held state then awaits a release/cancel to go
    // Idle, but that's normal lifecycle, not a stuck gesture.
    if (m_registryKey != 0) {
        s_registry.remove(m_registryKey);
        m_registryKey = 0;
    }
}

void PressGesture::checkMoveCancel(const QPoint &localPos)
{
    if (m_fsm.onMove(localPos.x(), localPos.y()) == PressOutcome::DragCanceled) {
        enterIdle();            // FSM is already Idle; this stops the timer + deregisters
        emit dragCanceled();
    }
}

bool PressGesture::eventFilter(QObject *watched, QEvent *event)
{
    if (watched != m_target) return false;
    auto *target = m_target.data();
    if (!target) return false;

    switch (event->type()) {
    case QEvent::TouchBegin: {
        auto *te = static_cast<QTouchEvent *>(event);
        if (te->points().isEmpty()) return false;
        const QPointF p = te->points().first().position();
        enterPressing(p.toPoint());
        // Don't accept/consume — let Qt synthesize mouse events so the
        // widget's own pressed-state visual feedback still works. Our
        // m_fsm.onPress() ignores a press while already Pressing, so the same
        // finger's touch + synthesized mouse can't double-enter Pressing.
        return false;
    }
    case QEvent::TouchUpdate: {
        auto *te = static_cast<QTouchEvent *>(event);
        if (te->points().isEmpty()) return false;
        const QPointF p = te->points().first().position();
        checkMoveCancel(p.toPoint());
        return false;
    }
    case QEvent::TouchEnd: {
        auto *te = static_cast<QTouchEvent *>(event);
        // Touch-only path (no mouse synth happened): handle release here.
        // If mouse synth WILL also arrive, the MouseButtonRelease handler will
        // see an Idle FSM and no-op cleanly — both check the same condition.
        QPoint p;
        if (!te->points().isEmpty()) {
            p = te->points().first().position().toPoint();
        }
        const bool resolving = !m_fsm.isIdle();
        const PressOutcome o = m_fsm.onRelease(target->rect().contains(p));
        if (resolving) {
            enterIdle();
            if      (o == PressOutcome::Tap)          emit tap();
            else if (o == PressOutcome::DragCanceled) emit dragCanceled();
        }
        return false;
    }
    case QEvent::TouchCancel: {
        if (m_fsm.onCancel() == PressOutcome::DragCanceled) {
            enterIdle();
            emit dragCanceled();
        }
        return false;
    }

    // Mouse fallback for desktop. We also handle these even when touch is
    // accepted, because some platforms emit mouse events anyway (e.g. when
    // the user actually IS using a mouse on a touch-capable display).
    case QEvent::MouseButtonPress: {
        auto *me = static_cast<QMouseEvent *>(event);
        if (me->button() != Qt::LeftButton) return false;
        // Avoid double-press when touch already accepted — if we're
        // already in Pressing or Held, ignore this mouse synth.
        if (!m_fsm.isIdle()) return false;
        enterPressing(me->position().toPoint());
        return false; // don't consume — let the widget's own press behaviour
                       // (visual feedback, focus) still run
    }
    case QEvent::MouseMove: {
        auto *me = static_cast<QMouseEvent *>(event);
        checkMoveCancel(me->position().toPoint());
        return false;
    }
    case QEvent::MouseButtonRelease: {
        auto *me = static_cast<QMouseEvent *>(event);
        if (me->button() != Qt::LeftButton) return false;
        const QPoint p = me->position().toPoint();
        const bool resolving = !m_fsm.isIdle();
        const PressOutcome o = m_fsm.onRelease(target->rect().contains(p));
        if (resolving) {
            enterIdle();
            if      (o == PressOutcome::Tap)          emit tap();
            else if (o == PressOutcome::DragCanceled) emit dragCanceled();
        }
        return false;
    }
    default:
        break;
    }
    return false;
}

} // namespace SnapClone::UI
