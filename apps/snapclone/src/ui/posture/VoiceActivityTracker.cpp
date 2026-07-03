#include "ui/posture/VoiceActivityTracker.hpp"

#include <QDateTime>
#include <algorithm>

namespace SnapClone::UI {

VoiceActivityTracker &VoiceActivityTracker::instance()
{
    static VoiceActivityTracker s_instance;
    return s_instance;
}

VoiceActivityTracker::VoiceActivityTracker(QObject *parent) : QObject(parent)
{
    m_lapseTimer.setSingleShot(true);
    connect(&m_lapseTimer, &QTimer::timeout, this, [this] {
        Q_EMIT activityChanged();   // a window just lapsed
        rescheduleLapse();          // arm for the next-soonest, if any remain
    });
}

void VoiceActivityTracker::noteVoice(const QString &conversationId)
{
    m_lastVoiceMs[conversationId] = QDateTime::currentMSecsSinceEpoch();
    rescheduleLapse();
    // A fresh stamp can newly-arm (was inactive) or reset a window that was about
    // to lapse — either way consumers should re-evaluate.
    Q_EMIT activityChanged();
}

bool VoiceActivityTracker::isActive(const QString &conversationId) const
{
    const auto it = m_lastVoiceMs.constFind(conversationId);
    if (it == m_lastVoiceMs.constEnd()) return false;
    return (QDateTime::currentMSecsSinceEpoch() - it.value()) < m_windowMs;
}

bool VoiceActivityTracker::anyActive() const
{
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    for (auto it = m_lastVoiceMs.constBegin(); it != m_lastVoiceMs.constEnd(); ++it)
        if ((now - it.value()) < m_windowMs) return true;
    return false;
}

void VoiceActivityTracker::setWindowMs(int ms)
{
    m_windowMs = std::max(1000, ms);
    rescheduleLapse();
}

void VoiceActivityTracker::rescheduleLapse()
{
    // Fire when the earliest still-active entry crosses the window edge.
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    qint64 soonestRemaining = -1;
    for (auto it = m_lastVoiceMs.constBegin(); it != m_lastVoiceMs.constEnd(); ++it) {
        const qint64 remaining = m_windowMs - (now - it.value());
        if (remaining <= 0) continue;                       // already lapsed
        if (soonestRemaining < 0 || remaining < soonestRemaining)
            soonestRemaining = remaining;
    }
    if (soonestRemaining < 0) { m_lapseTimer.stop(); return; }
    m_lapseTimer.start(int(std::min<qint64>(soonestRemaining + 50, m_windowMs)));
}

} // namespace SnapClone::UI
