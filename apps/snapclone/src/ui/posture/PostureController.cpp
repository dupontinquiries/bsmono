#include "ui/posture/PostureController.hpp"

#include "ui/posture/DimOverlay.hpp"
#include "ui/posture/IioSensorProxySource.hpp"
#include "ui/posture/VoiceActivityTracker.hpp"
#include "ui/SwipeNavigator.hpp"

#include <QSettings>

namespace SnapClone::UI {

namespace {
// Matches the page order set up in MainWindow (Camera, Inbox, Friends, …).
constexpr int kInboxPage = 1;
} // namespace

PostureController::PostureController(QWidget *overlayParent, SwipeNavigator *swiper,
                                     QObject *parent)
    : QObject(parent), m_swiper(swiper)
{
    m_engine  = new PostureEngine(this);
    m_overlay = new DimOverlay(overlayParent);
    m_source  = new IioSensorProxySource(this);

    loadConfigFromSettings();

    // engine → screen / call
    connect(m_engine, &PostureEngine::dimTargetChanged,
            m_overlay, &DimOverlay::animateTo);
    connect(m_engine, &PostureEngine::screensaverChanged,
            this, &PostureController::screensaverChanged);
    connect(m_engine, &PostureEngine::gatedModeRequested,
            this, &PostureController::gatedModeRequested);

    // sensors → engine
    connect(m_source, &PostureSource::proximityChanged,
            m_engine, &PostureEngine::setProximityNear);
    connect(m_source, &PostureSource::motionChanged,
            m_engine, &PostureEngine::setMoving);

    // arming inputs
    if (m_swiper)
        connect(m_swiper, &SwipeNavigator::pageChanged,
                this, [this](int){ recomputeArmed(); });
    connect(&VoiceActivityTracker::instance(), &VoiceActivityTracker::activityChanged,
            this, &PostureController::recomputeArmed);

    recomputeArmed();
}

void PostureController::loadConfigFromSettings()
{
    QSettings s;
    PostureEngine::Config c;   // defaults
    c.dimLevel1    = s.value(QStringLiteral("posture/dimLevel1"),    c.dimLevel1).toReal();
    c.dimLevel2    = s.value(QStringLiteral("posture/dimLevel2"),    c.dimLevel2).toReal();
    c.fadeToDim1Ms = s.value(QStringLiteral("posture/fadeToDim1Ms"), c.fadeToDim1Ms).toInt();
    c.stillnessMs  = s.value(QStringLiteral("posture/stillnessMs"),  c.stillnessMs).toInt();
    c.fadeToDim2Ms = s.value(QStringLiteral("posture/fadeToDim2Ms"), c.fadeToDim2Ms).toInt();
    c.wakeMs       = s.value(QStringLiteral("posture/wakeMs"),       c.wakeMs).toInt();
    c.enabled      = s.value(QStringLiteral("posture/enabled"),      c.enabled).toBool();
    m_engine->setConfig(c);
    m_enabled = c.enabled;
}

void PostureController::setEnabled(bool on)
{
    m_enabled = on;
    PostureEngine::Config c = m_engine->config();
    c.enabled = on;
    m_engine->setConfig(c);            // master-disables inside the engine too
    QSettings().setValue(QStringLiteral("posture/enabled"), on);
    recomputeArmed();
}

void PostureController::recomputeArmed()
{
    const bool onInbox = (m_swiper && m_swiper->currentIndex() == kInboxPage);
    const bool want = m_enabled
                   && m_engine->config().enabled
                   && onInbox
                   && VoiceActivityTracker::instance().anyActive();
    if (want == m_armed) return;
    m_armed = want;
    m_engine->setArmed(want);
    // Claim sensors only while armed. If the source can't claim (no proxy / no
    // proximity), it just reports unavailable and emits nothing — harmless.
    if (m_source) { if (want) m_source->start(); else m_source->stop(); }
}

void PostureController::notifySnap()
{
    if (m_engine) m_engine->notifySnapArrived();
}

} // namespace SnapClone::UI
