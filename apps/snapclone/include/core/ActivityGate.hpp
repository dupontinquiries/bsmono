#pragma once
// ─── core/ActivityGate.hpp ───────────────────────────────────────────────────
//
// Process-wide broadcaster of "should periodic work run, and how often?".
//
// Before this, the "is the app in the foreground?" check was reimplemented
// ad-hoc in each widget that cared (AmbientBlobs, WayfinderPage), each wiring
// its own QGuiApplication::applicationStateChanged handler — and none of them
// also looked at screen-off, so a decorative animation kept ticking with the
// screen dark. ActivityGate unifies the inputs (app foreground state, sourced
// here from QGuiApplication; screen-on / battery-saver / thermal, fed in from
// PowerManager by the app) behind one EnergyPolicy and emits transitions, so
// any periodic timer can gate itself in one connect:
//
//     connect(ActivityGate::instance(), &ActivityGate::interactiveChanged,
//             this, [this](bool on){ on ? m_timer->start() : m_timer->stop(); });
//
// It is a singleton because timers live all over the widget tree and threading
// PowerManager through every constructor would be far more invasive than the
// gating is worth. Foreground state works with zero wiring; to also gate on
// screen-off / battery-saver / thermal, the owner connects PowerManager's
// signals to the slots below once at startup (see AppController).
//
// Defaults are conservative: screen on, foreground, not battery-saver, normal
// thermal — i.e. interactive() == true until told otherwise, so an
// unconfigured gate never wrongly suppresses work.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>

#include "core/EnergyPolicy.hpp"

namespace SnapClone::Core {

class ActivityGate : public QObject
{
    Q_OBJECT
public:
    /// Lazily-created, parented to qApp. Safe to call from any thread that the
    /// GUI thread has already touched; intended for GUI-thread use.
    static ActivityGate *instance();

    [[nodiscard]] bool interactive()           const { return m_policy.interactive(); }
    [[nodiscard]] bool backgroundWorkAllowed() const { return m_policy.backgroundWorkAllowed(); }
    [[nodiscard]] int  cadenceScale()          const { return m_policy.cadenceScale(); }

public slots:
    // Fed from PowerManager by the owner (optional — defaults keep things on).
    void setScreenActive(bool on);
    void setBatterySaver(bool on);
    void setThermalCritical(bool critical);

signals:
    /// Foreground∧screen-on transitioned. Decorative / viewfinder / high-freq
    /// timers should start() on true and stop() on false.
    void interactiveChanged(bool interactive);

    /// The maintenance-cadence multiplier changed (1 normal … 8 max, 0 paused).
    /// Maintenance timers that want to slow down rather than stop can multiply
    /// their base interval by this.
    void cadenceScaleChanged(int scale);

private:
    explicit ActivityGate(QObject *parent = nullptr);
    void recompute();

    EnergyPolicy m_policy;
    bool m_lastInteractive = true;
    int  m_lastScale       = 1;
};

} // namespace SnapClone::Core
