#pragma once
// ─── PowerManager.hpp ─────────────────────────────────────────────────────────
//
// Three-tier battery and thermal integration for SnapClone on Linux.
//
// Tier 1 — KDE PowerDevil (org.kde.Solid.PowerManagement)
//   Only on Plasma desktops (detected via PLASMA_PLATFORM / XDG_SESSION_DESKTOP).
//   Provides: instant battery signals, suspend inhibit (flags=7), power profile
//   detection, screen-lock/idle signals via org.kde.screensaver.
//
// Tier 2 — upower (org.freedesktop.UPower)
//   Works on postmarketOS + Alpine even without KDE.
//   Provides: PropertiesChanged on the battery device, remaining time (TimeToEmpty),
//   energy rate (Watts), AC adapter detection, battery health and cycle count.
//
// Tier 3 — /sys + systemd-logind (universal fallback)
//   /sys/class/power_supply/*  — capacity, status, health, cycle_count, threshold.
//   /sys/class/thermal/*       — with zone-type filtering (cpu/soc/tsens preferred).
//   logind Inhibit("sleep:idle") — blocks sleep during active P2P sessions.
//   logind IdleHint signal       — screen-off detection without PowerDevil.
//
// Suspend inhibit is SESSION-GATED:
//   onSessionStarted() — acquires inhibit on first call.
//   onSessionEnded()   — releases inhibit when last session closes.
//
// Power profile awareness:
//   On Plasma Mobile / KDE Plasma the active power profile (performance /
//   balanced / power-saver) is read at startup and monitored for changes.
//   "power-saver" reduces CPU/modem clocks which can stall STUN and
//   hole-punching — the app warns the user and avoids expensive probing.
//
// Screen-off awareness:
//   Detects screen-off via logind IdleHint or org.kde.screensaver.
//   Emits screenActiveChanged(false) when the screen turns off so the
//   camera bridge can be paused before PipeWire drops its nodes.

#include <QObject>
#include <QTimer>
#include <QString>

namespace SnapClone::Core {

enum class ThermalState {
    Unknown,    // no /sys data readable
    Normal,     // < 45°C — full capability
    Warm,       // 45–60°C — reduce camera resolution
    Hot,        // 60–75°C — stop background scans
    Critical,   // > 75°C — emergency throttle
};

enum class PowerProfile {
    Unknown,
    Performance,   // maximum CPU/modem clocks — all features allowed
    Balanced,      // default — all features allowed
    PowerSaver,    // reduced clocks — avoid expensive network probing
};

enum class BatteryHealth {
    Unknown,
    Good,
    Fair,          // capacity degraded, warn user
    Poor,          // significant degradation
    Dead,          // battery reporting problems
};

class PowerManager : public QObject
{
    Q_OBJECT
public:
    explicit PowerManager(QObject *parent = nullptr);
    ~PowerManager() override;

    // ── State accessors ───────────────────────────────────────────────────
    [[nodiscard]] ThermalState  thermalState()    const { return m_thermalState; }
    [[nodiscard]] int           batteryPct()      const { return m_batteryPct; }
    [[nodiscard]] bool          isCharging()      const { return m_charging; }
    [[nodiscard]] bool          isOnAc()          const { return m_onAc; }
    [[nodiscard]] float         lastTempC()       const { return m_lastTemp; }
    [[nodiscard]] int           chargeThreshold() const { return m_chargeThreshold; }
    [[nodiscard]] int           cycleCount()      const { return m_cycleCount; }
    [[nodiscard]] BatteryHealth batteryHealth()   const { return m_batteryHealth; }
    [[nodiscard]] int           remainingSecs()   const { return m_remainingSecs; }
    [[nodiscard]] float         energyRateW()     const { return m_energyRateW; }
    [[nodiscard]] PowerProfile  powerProfile()    const { return m_powerProfile; }
    [[nodiscard]] bool          isScreenActive()  const { return m_screenActive; }
    [[nodiscard]] bool          upowerConnected() const { return m_upowerConnected; }
    [[nodiscard]] bool          isPlasma()        const { return m_isPlasma; }

    /// True if conditions allow expensive background work
    /// (NAT traversal, gossip scans, camera init, STUN probing, hole-punch).
    [[nodiscard]] bool allowsBackgroundWork() const {
        return m_thermalState != ThermalState::Critical
            && m_thermalState != ThermalState::Hot
            && m_powerProfile  != PowerProfile::PowerSaver
            && (m_batteryPct > 15 || m_onAc || m_batteryPct == 0);
    }

    /// True if only lightweight maintenance should run (no probing/scanning).
    [[nodiscard]] bool isBatterySaver() const {
        return !m_onAc && m_batteryPct > 0 && m_batteryPct <= 20;
    }

    /// Estimated minutes of battery life remaining (-1 = unknown/charging).
    [[nodiscard]] int remainingMinutes() const {
        return (m_remainingSecs > 0 && !m_charging)
            ? m_remainingSecs / 60 : -1;
    }

    // ── Session lifecycle — gates suspend inhibit ─────────────────────────
    void onSessionStarted();
    void onSessionEnded();

signals:
    // Thermal
    void thermalStateChanged(SnapClone::Core::ThermalState state);
    void thermalHot();       ///< 60–75°C — reduce workload
    void thermalCritical();  ///< >75°C   — emergency throttle

    // Battery
    void batteryLevelChanged(int pct);
    void batteryLow();               ///< pct ≤ 10%
    void batteryCritical();          ///< pct ≤ 5%
    void chargingChanged(bool charging);
    void onAcChanged(bool onAc);     ///< AC adapter plugged/unplugged
    void chargeThresholdChanged(int pct);
    void batteryHealthChanged(SnapClone::Core::BatteryHealth health);
    void remainingTimeChanged(int seconds);  ///< TimeToEmpty from upower

    // Power profile
    void powerProfileChanged(SnapClone::Core::PowerProfile profile);

    // Screen state
    void screenActiveChanged(bool active); ///< false = screen turned off

private slots:
    void pollSys();
    void pollThermal();
    void pollBattery();
    void onKdeBatteryChanged();
    void onKdeChargingChanged();
    void onUpowerPropertiesChanged();
    void onLogindIdleHintChanged();
    void onKdeProfileChanged();

private:
    void subscribeKdeBattery();
    void subscribeUpower();
    void subscribeScreenState();
    void subscribePowerProfile();
    void readUpowerBatteryDetails();
    void tryInhibitSuspend();
    void releaseInhibit();
    BatteryHealth healthFromString(const QString &s) const;

    QTimer        *m_watchdog          = nullptr;
    ThermalState   m_thermalState      = ThermalState::Unknown;
    int            m_batteryPct        = 0;
    bool           m_charging          = false;
    bool           m_onAc              = false;
    float          m_lastTemp          = 0.f;
    int            m_chargeThreshold   = 100;
    int            m_cycleCount        = -1;
    BatteryHealth  m_batteryHealth     = BatteryHealth::Unknown;
    int            m_remainingSecs     = -1;
    float          m_energyRateW       = 0.f;
    PowerProfile   m_powerProfile      = PowerProfile::Unknown;
    bool           m_screenActive      = true;
    int            m_activeSessions    = 0;

    uint           m_kdeInhibitCookie  = 0;
    int            m_logindInhibitFd   = -1;

    bool           m_isPlasma          = false;
    bool           m_upowerConnected   = false;
    QString        m_upowerBatteryPath;
};

} // namespace SnapClone::Core
