#include "ui/posture/PostureEngine.hpp"

namespace SnapClone::UI {

PostureEngine::PostureEngine(QObject *parent) : QObject(parent)
{
    m_stillTimer.setSingleShot(true);
    connect(&m_stillTimer, &QTimer::timeout, this, [this] {
        if (m_armed && m_state == State::Dim)
            enterScreensaver();
    });

    m_redimTimer.setSingleShot(true);
    connect(&m_redimTimer, &QTimer::timeout, this, [this] {
        // The light-up after a snap has finished; if we're still covered, restart
        // the dim cycle from Normal.
        if (m_armed && m_near && m_state == State::Normal)
            enterDim();
    });
}

void PostureEngine::setConfig(const Config &c)
{
    m_cfg = c;
    if (!m_cfg.enabled && m_armed)
        setArmed(false);   // master switch off → stand down
}

void PostureEngine::setArmed(bool armed)
{
    if (!m_cfg.enabled) armed = false;
    if (m_armed == armed) return;
    m_armed = armed;
    if (!m_armed) {
        // Full reset: cancel timers, fade any overlay away, drop screensaver.
        m_stillTimer.stop();
        m_redimTimer.stop();
        if (m_state != State::Normal) wakeToNormal();
        m_state = State::Normal;
    }
    updateGated();   // arming/disarming changes whether we hold the gated flip
    // When arming we stay Normal until proximity reads near — nothing to emit.
}

void PostureEngine::setInCall(bool inCall)
{
    if (m_inCall == inCall) return;
    m_inCall = inCall;
    updateGated();
}

void PostureEngine::setProximityNear(bool near)
{
    if (m_near == near) return;
    m_near = near;
    if (m_armed) {
        if (m_near) {
            if (m_state == State::Normal) enterDim();
        } else {
            m_redimTimer.stop();
            if (m_state != State::Normal) wakeToNormal();
        }
    }
    updateGated();
}

void PostureEngine::setMoving(bool moving)
{
    if (m_moving == moving) return;
    m_moving = moving;
    if (m_armed) evaluateStillness();
}

void PostureEngine::notifySnapArrived()
{
    if (!m_armed) return;
    if (m_state != State::Normal) {
        wakeToNormal();
        // If we're still covered, light up briefly then re-dim (restart timers).
        if (m_near) m_redimTimer.start(m_cfg.wakeMs);
    }
}

// ── transitions ───────────────────────────────────────────────────────────────

void PostureEngine::enterDim()
{
    m_state = State::Dim;
    Q_EMIT dimTargetChanged(m_cfg.dimLevel1, m_cfg.fadeToDim1Ms);
    evaluateStillness();
}

void PostureEngine::enterScreensaver()
{
    m_state = State::Screensaver;
    Q_EMIT dimTargetChanged(m_cfg.dimLevel2, m_cfg.fadeToDim2Ms);
    Q_EMIT screensaverChanged(true);
}

void PostureEngine::wakeToNormal()
{
    const bool wasSaver = (m_state == State::Screensaver);
    m_stillTimer.stop();
    m_state = State::Normal;
    Q_EMIT dimTargetChanged(0.0, m_cfg.wakeMs);
    if (wasSaver)
        Q_EMIT screensaverChanged(false);
}

void PostureEngine::evaluateStillness()
{
    // The stillness timer only runs while we're dimmed-but-not-yet-screensaver.
    // Motion stops it (reset); going still (re)starts a fresh countdown.
    if (m_state == State::Dim && !m_moving) {
        if (!m_stillTimer.isActive())
            m_stillTimer.start(m_cfg.stillnessMs);
    } else {
        m_stillTimer.stop();
    }
}

void PostureEngine::updateGated()
{
    // Request gated whenever we're armed and covered. Whether a call is actually
    // live (and thus whether the request does anything) is enforced where it's
    // routed — the active conversation only applies it if it's in a call — so the
    // engine doesn't need to track call state itself. setInCall() stays as an
    // advisory hint for future use.
    const bool gated = (m_armed && m_near);
    if (gated == m_gated) return;
    m_gated = gated;
    Q_EMIT gatedModeRequested(m_gated);
}

} // namespace SnapClone::UI
