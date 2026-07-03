#pragma once
// ─── ui/posture/PostureEngine.hpp ─────────────────────────────────────────────
//
// Drives the proximity screensaver and the push-to-talk ↔ gated talk-mode flip
// for voice-active conversations. Pure logic: it consumes abstract posture
// signals (proximity near/far, device moving/still, a snap arrived) plus an
// "armed" gate, and emits what the UI should do — an overlay dim target, a
// screensaver on/off (to cap the render rate), and a talk-mode request. Hardware
// lives behind PostureSource; arming (the inbox / 5-minute-voice rule) lives in
// the app and is fed via setArmed().
//
// We deliberately never read the real display power state; proximity-near stands
// in for "phone is at your ear / not being looked at".
//
// Dim state machine (only while armed):
//
//   Normal ──proximity near──▶ Dim ──still for stillnessMs──▶ Screensaver
//     ▲                         │                                  │
//     └──── proximity far / snap arrived (fade back over wakeMs) ──┘
//
//   • Dim         — overlay fades toward dimLevel1 (50%) over fadeToDim1Ms.
//   • Screensaver — overlay deepens to dimLevel2 (75%) over fadeToDim2Ms and the
//                   render rate is capped. Motion before stillnessMs just resets
//                   the stillness timer; the dim holds.
//   • Wake        — proximity far, or a snap arrives, fades the overlay back to
//                   clear over wakeMs and restarts the cycle. A snap that arrives
//                   while still covered lights up then re-dims.
//
// The talk-mode flip is decoupled from the dim states: gated ⇔ (armed && near &&
// in a call). Putting the phone to your ear switches PTT→gated; taking it away
// switches back.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QTimer>

namespace SnapClone::UI {

class PostureEngine : public QObject {
    Q_OBJECT
public:
    struct Config {
        qreal dimLevel1    = 0.50;   ///< first dim (overlay opacity, 0..1)
        qreal dimLevel2    = 0.75;   ///< screensaver dim
        int   fadeToDim1Ms = 30000;  ///< Normal → Dim fade
        int   stillnessMs  = 60000;  ///< still this long in Dim → Screensaver
        int   fadeToDim2Ms = 3000;   ///< Dim → Screensaver fade (fast)
        int   wakeMs       = 1000;   ///< any → Normal fade
        bool  enabled      = true;   ///< master on/off (from settings)
    };

    enum class State { Normal, Dim, Screensaver };

    explicit PostureEngine(QObject *parent = nullptr);

    void setConfig(const Config &c);
    [[nodiscard]] const Config &config() const { return m_cfg; }
    [[nodiscard]] State state() const { return m_state; }
    [[nodiscard]] bool armed() const { return m_armed; }

public Q_SLOTS:
    /// The gate: true when the current view is a voice-active context (the inbox,
    /// or a conversation with a voice message in the last 5 minutes). Disarming
    /// fully resets to Normal and releases the talk-mode flip.
    void setArmed(bool armed);
    /// Whether a voice call is live right now (decides if we touch the talk mode).
    void setInCall(bool inCall);
    /// Posture inputs (wired from a PostureSource).
    void setProximityNear(bool near);
    void setMoving(bool moving);
    /// A snap arrived for the current context — wake the screen.
    void notifySnapArrived();

Q_SIGNALS:
    /// Drive the dim overlay to `opacity` (0..1) over `durationMs`.
    void dimTargetChanged(qreal opacity, int durationMs);
    /// Entered/left screensaver — cap or restore the render rate (camera preview).
    void screensaverChanged(bool active);
    /// Requested talk mode: true = gated (VOX), false = push-to-talk.
    void gatedModeRequested(bool gated);

private:
    void enterDim();          ///< proximity went near
    void enterScreensaver();  ///< stillness elapsed
    void wakeToNormal();      ///< fade overlay back, leave screensaver
    void evaluateStillness(); ///< (re)start or cancel the stillness timer
    void updateGated();       ///< recompute gated = armed && near && inCall

    Config m_cfg;
    State  m_state  = State::Normal;
    bool   m_armed  = false;
    bool   m_inCall = false;
    bool   m_near   = false;
    bool   m_moving = false;
    bool   m_gated  = false;   ///< last talk-mode we requested
    QTimer m_stillTimer;       ///< fires after stillnessMs of no motion
    QTimer m_redimTimer;       ///< after a snap wake while still covered, re-dim
};

} // namespace SnapClone::UI
