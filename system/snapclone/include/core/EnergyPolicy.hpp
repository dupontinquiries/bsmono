#pragma once
// ─── core/EnergyPolicy.hpp ───────────────────────────────────────────────────
//
// Pure, Qt-free decision core for "given the device + app state, what should
// periodic work do?". This is the brain behind ActivityGate — kept separate so
// the policy table can be unit-tested with plain C++ (no GUI, no D-Bus, no
// device).
//
// Two kinds of periodic work want different answers:
//   • Interactive work — viewfinder polling, decorative animations, anything
//     whose only purpose is to update pixels the user is looking at. There is
//     no reason to run it when the screen is off or the app is backgrounded.
//     → gate on interactive().
//   • Background maintenance — keepalives, reconnect probes, discovery
//     announces. These must keep running even when not interactive (that's how
//     a snap arrives while the phone is in a pocket), but they can run LESS
//     often to save wakeups, and must stop entirely when the SoC is thermally
//     critical. → gate on backgroundWorkAllowed(); scale by cadenceScale().
//
// On mobile every timer expiry is a CPU (and sometimes modem) wakeup that
// blocks deep sleep, so the levers here are "don't fire at all" (interactive
// gating) and "fire less often / let the kernel batch" (cadence scaling +
// coarse timers at the call site).
// ─────────────────────────────────────────────────────────────────────────────

namespace SnapClone::Core {

// Coarse thermal tiers (a reduction of PowerManager's ThermalState — Normal
// folds in "Warm", Hot stays, Critical stays — so the policy only branches on
// the distinctions that change behaviour).
enum class EnergyThermal { Normal, Hot, Critical };

class EnergyPolicy {
public:
    // ── inputs ────────────────────────────────────────────────────────────────
    void setScreenOn(bool v)        { m_screenOn = v; }
    void setAppForeground(bool v)   { m_foreground = v; }
    void setBatterySaver(bool v)    { m_batterySaver = v; }
    void setThermal(EnergyThermal t){ m_thermal = t; }

    bool screenOn()      const { return m_screenOn; }
    bool appForeground() const { return m_foreground; }
    bool batterySaver()  const { return m_batterySaver; }
    EnergyThermal thermal() const { return m_thermal; }

    // ── decisions ───────────────────────────────────────────────────────────────

    /// The user can actually see the screen: foreground AND screen on. Gate
    /// decorative / viewfinder / high-frequency timers on this — when false
    /// they should stop completely.
    bool interactive() const { return m_screenOn && m_foreground; }

    /// Background maintenance may run. Always true except when the SoC is
    /// thermally critical, where everything non-essential must stop to shed
    /// heat. (Maintenance deliberately keeps running when merely backgrounded
    /// or screen-off — that's how snaps arrive in-pocket.)
    bool backgroundWorkAllowed() const { return m_thermal != EnergyThermal::Critical; }

    /// Multiplier to stretch a periodic-maintenance interval by, to cut the
    /// number of wakeups. 1 = run at the normal cadence. 0 = stop (paused —
    /// callers should treat 0 as "don't fire"). Factors compound:
    ///   • not interactive (backgrounded / screen-off): ×2
    ///   • battery-saver:                                ×2
    ///   • thermally hot:                                ×2
    /// capped at 8 so a maintenance task can't stall indefinitely.
    int cadenceScale() const {
        if (m_thermal == EnergyThermal::Critical) return 0;     // paused
        int s = 1;
        if (!interactive())                  s *= 2;
        if (m_batterySaver)                  s *= 2;
        if (m_thermal == EnergyThermal::Hot) s *= 2;
        return s > 8 ? 8 : s;
    }

private:
    bool          m_screenOn     = true;
    bool          m_foreground   = true;
    bool          m_batterySaver = false;
    EnergyThermal m_thermal      = EnergyThermal::Normal;
};

} // namespace SnapClone::Core
