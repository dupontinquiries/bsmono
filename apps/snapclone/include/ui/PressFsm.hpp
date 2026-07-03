#pragma once
// ─── UI/PressFsm.hpp ─────────────────────────────────────────────────────────
//
// Pure, Qt-free decision core for tap / long-press / drag-cancel detection.
//
// This is the brain that used to live tangled inside PressGesture's
// eventFilter. Pulling it out buys three things:
//   • clarity   — the gesture rules are in one place, not spread across Qt
//                 touch/mouse event cases;
//   • testing   — the rules can be unit-tested with plain C++, no GUI;
//   • UX proof  — we can *simulate* the imprecise input a first-time phone
//                 user produces (slow taps, shaky fingers, lifting off the
//                 edge of a button) and check the gesture is still understood.
//
// PressGesture is now the thin Qt adapter around this: it translates touch and
// mouse events into the calls below, arms a QTimer for the long-press timeout,
// runs the stuck-gesture watchdog, and emits its signals from these outcomes.
//
// Rules (must stay in lock-step with PressGesture's documented FSM):
//
//     Idle ──onPress──▶ Pressing ──onLongPressTimeout──▶ Held  (LongPress)
//      ▲                   │
//      │                   ├── onRelease(inside=true)  ─▶ Idle (Tap)
//      │                   ├── onRelease(inside=false) ─▶ Idle (DragCanceled)
//      │                   └── onMove > moveCancelPx   ─▶ Idle (DragCanceled)
//      │
//      └── onRelease / onCancel from Held ──▶ Idle   (no signal: LongPress
//          already fired)
//
//   • A press while already Pressing or Held is ignored. This is what lets the
//     same physical finger arrive as BOTH a touch event and a synthesized
//     mouse event without firing two gestures.
//   • Movement is measured as Manhattan distance (|dx| + |dy|) from the press
//     origin — cheap, and a good enough proxy for "the finger wandered."
//   • onCancel models the platform yanking the touch out from under us
//     (TouchCancel) — always a drag-cancel if a gesture was in flight.
//
// No Qt, no allocation, no threads — just an integer state machine.
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::UI {

enum class PressState { Idle, Pressing, Held };

/// What a single event call resolved the gesture into, if anything.
enum class PressOutcome { None, Tap, LongPress, DragCanceled };

class PressFsm {
public:
    PressFsm() = default;
    PressFsm(int longPressMs, int moveCancelPx)
        : m_longPressMs(clampLongPress(longPressMs)),
          m_moveCancelPx(clampMoveCancel(moveCancelPx)) {}

    // ── tuning ───────────────────────────────────────────────────────────────
    // Long-press threshold (ms). Clamped to a 50 ms floor so a misconfigured
    // value can't make every touch an instant long-press.
    void setLongPressMs(int ms)  { m_longPressMs = clampLongPress(ms); }
    // Movement cancel threshold (px, Manhattan). Clamped to a 2 px floor so it
    // can't be set so tight that sub-pixel rounding cancels every press.
    void setMoveCancelPx(int px) { m_moveCancelPx = clampMoveCancel(px); }

    int  longPressMs()  const { return m_longPressMs; }
    int  moveCancelPx() const { return m_moveCancelPx; }

    // ── introspection ─────────────────────────────────────────────────────────
    PressState state()      const { return m_state; }
    bool       isPressing() const { return m_state == PressState::Pressing; }
    bool       isIdle()     const { return m_state == PressState::Idle; }

    // ── events (each returns the outcome it triggered, or None) ────────────────

    /// Finger / button went down at (x, y). Ignored unless we were Idle.
    PressOutcome onPress(int x, int y)
    {
        if (m_state != PressState::Idle)
            return PressOutcome::None;          // ignore re-press (touch + synth mouse)
        m_state = PressState::Pressing;
        m_originX = x;
        m_originY = y;
        return PressOutcome::None;
    }

    /// Finger moved to (x, y). Cancels the gesture if it wandered too far.
    PressOutcome onMove(int x, int y)
    {
        if (m_state != PressState::Pressing)
            return PressOutcome::None;
        const int dx = x - m_originX;
        const int dy = y - m_originY;
        const int manhattan = (dx < 0 ? -dx : dx) + (dy < 0 ? -dy : dy);
        if (manhattan > m_moveCancelPx) {
            m_state = PressState::Idle;
            return PressOutcome::DragCanceled;
        }
        return PressOutcome::None;
    }

    /// The long-press timer elapsed. Promotes a still-held press to a long-press.
    PressOutcome onLongPressTimeout()
    {
        if (m_state != PressState::Pressing)
            return PressOutcome::None;          // moved/released already — no-op
        m_state = PressState::Held;
        return PressOutcome::LongPress;
    }

    /// Finger / button came up. `insideTarget` = was the release within the
    /// widget's hit rect (the adapter computes this).
    PressOutcome onRelease(bool insideTarget)
    {
        if (m_state == PressState::Pressing) {
            m_state = PressState::Idle;
            return insideTarget ? PressOutcome::Tap : PressOutcome::DragCanceled;
        }
        if (m_state == PressState::Held) {
            m_state = PressState::Idle;          // long-press already fired
            return PressOutcome::None;
        }
        return PressOutcome::None;
    }

    /// The platform cancelled the touch sequence (TouchCancel, or the
    /// stuck-gesture watchdog force-resetting us).
    PressOutcome onCancel()
    {
        if (m_state != PressState::Idle) {
            m_state = PressState::Idle;
            return PressOutcome::DragCanceled;
        }
        return PressOutcome::None;
    }

    /// Hard reset to Idle with no signal (watchdog teardown path).
    void reset() { m_state = PressState::Idle; }

private:
    static int clampLongPress(int ms)  { return ms < 50 ? 50 : ms; }
    static int clampMoveCancel(int px) { return px < 2 ? 2 : px; }

    PressState m_state   = PressState::Idle;
    int        m_originX = 0;
    int        m_originY = 0;
    int        m_longPressMs  = 500;   // Android-standard long-press
    int        m_moveCancelPx = 16;    // > jitter, < a deliberate swipe
};

} // namespace SnapClone::UI
