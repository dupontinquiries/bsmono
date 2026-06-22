// ─── core/ActivityGate.cpp ───────────────────────────────────────────────────
// See ActivityGate.hpp.
// ─────────────────────────────────────────────────────────────────────────────

#include "core/ActivityGate.hpp"

#include <QGuiApplication>
#include <QCoreApplication>

namespace SnapClone::Core {

ActivityGate *ActivityGate::instance()
{
    static ActivityGate *g = nullptr;
    if (!g)
        g = new ActivityGate(qApp);   // parented to the application object
    return g;
}

ActivityGate::ActivityGate(QObject *parent)
    : QObject(parent)
{
    // Foreground state is self-sourced — no wiring needed for the common case.
    // ApplicationInactive (transient focus loss, e.g. the notification shade)
    // counts as foreground: the user is a tap away from returning, so we keep
    // interactive work alive rather than thrash it off and back on.
    if (auto *gui = qobject_cast<QGuiApplication *>(QGuiApplication::instance())) {
        const Qt::ApplicationState s = gui->applicationState();
        m_policy.setAppForeground(s == Qt::ApplicationActive
                                  || s == Qt::ApplicationInactive);
        connect(gui, &QGuiApplication::applicationStateChanged,
                this, [this](Qt::ApplicationState st) {
                    m_policy.setAppForeground(st == Qt::ApplicationActive
                                              || st == Qt::ApplicationInactive);
                    recompute();
                });
    }
    m_lastInteractive = m_policy.interactive();
    m_lastScale       = m_policy.cadenceScale();
}

void ActivityGate::setScreenActive(bool on)
{
    m_policy.setScreenOn(on);
    recompute();
}

void ActivityGate::setBatterySaver(bool on)
{
    m_policy.setBatterySaver(on);
    recompute();
}

void ActivityGate::setThermalCritical(bool critical)
{
    m_policy.setThermal(critical ? EnergyThermal::Critical : EnergyThermal::Normal);
    recompute();
}

void ActivityGate::recompute()
{
    const bool nowInteractive = m_policy.interactive();
    if (nowInteractive != m_lastInteractive) {
        m_lastInteractive = nowInteractive;
        emit interactiveChanged(nowInteractive);
    }
    const int nowScale = m_policy.cadenceScale();
    if (nowScale != m_lastScale) {
        m_lastScale = nowScale;
        emit cadenceScaleChanged(nowScale);
    }
}

} // namespace SnapClone::Core
