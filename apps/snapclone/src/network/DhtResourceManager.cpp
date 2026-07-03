#include "network/DhtResourceManager.hpp"
#include "network/JamiAdapter.hpp"
#include "core/PowerManager.hpp"
#include <QLoggingCategory>
#include <QTimer>

Q_LOGGING_CATEGORY(lcDhtRes, "snapclone.dht.resources")

namespace SnapClone::Network {

// Re-announce intervals per mode. Values expire on the DHT after ~60min.
// We always re-announce before expiry; the question is how much MORE often.
//   Full:        55min — near-maximum contribution (values always fresh)
//   Maintenance: 3h    — value refreshed twice per half-day
//   Minimal:     8h    — value refreshed before it expires, barely
static constexpr int kReannounceFullMs        = 55 * 60 * 1000;
static constexpr int kReannounceMaintenanceMs =  3 * 60 * 60 * 1000;
static constexpr int kReannounceMinimalMs     =  8 * 60 * 60 * 1000; // just before 9h expiry

// How long to wait in Off mode before attempting reconnect
static constexpr int kReconnectCheckMs        = 5 * 60 * 1000; // 5 minutes

DhtResourceManager::DhtResourceManager(
    DhtDiscovery       *dht,
    Core::PowerManager *power,
    QObject            *parent)
    : QObject(parent)
    , m_dht(dht)
    , m_power(power)
{
    // Re-evaluate the target mode every 60 seconds.
    // Conditions that trigger immediate re-evaluation are handled via signals.
    m_evalTimer = new QTimer(this);
    m_evalTimer->setInterval(60 * 1000);
    m_evalTimer->setTimerType(Qt::VeryCoarseTimer);
    connect(m_evalTimer, &QTimer::timeout, this, &DhtResourceManager::evaluate);
    m_evalTimer->start();

    // V32.34+ battery: stats-emit timer fires every 60s to publish the
    // aggregated DhtStats. Previously a 1Hz timer incremented uptime
    // counter and emitted stats every 60th tick — but the increment is
    // pointless (we compute uptime from m_uptimeElapsed on demand) and
    // the 59 untaken paths each cost a CPU wakeup. VeryCoarseTimer for
    // power efficiency (Qt batches with other VeryCoarse timers up to
    // ±500ms, which is fine for stats publication).
    m_statsEmitTimer = new QTimer(this);
    m_statsEmitTimer->setInterval(60 * 1000);
    m_statsEmitTimer->setTimerType(Qt::VeryCoarseTimer);
    connect(m_statsEmitTimer, &QTimer::timeout, this,
            &DhtResourceManager::onStatsEmitTick);

    // React immediately to power state changes
    connect(power, &Core::PowerManager::batteryLevelChanged,
            this,  &DhtResourceManager::onPowerChanged);
    connect(power, &Core::PowerManager::chargingChanged,
            this,  &DhtResourceManager::onPowerChanged);
    connect(power, &Core::PowerManager::onAcChanged,
            this,  &DhtResourceManager::onPowerChanged);
    connect(power, &Core::PowerManager::thermalStateChanged,
            this,  &DhtResourceManager::onPowerChanged);
    connect(power, &Core::PowerManager::powerProfileChanged,
            this,  &DhtResourceManager::onPowerChanged);
    connect(power, &Core::PowerManager::screenActiveChanged,
            this,  &DhtResourceManager::onPowerChanged);

    // Initial evaluation — sets the starting mode
    evaluate();

    qCInfo(lcDhtRes) << "DhtResourceManager started, contribution="
                     << static_cast<int>(m_contribution)
                     << "mode=" << static_cast<int>(m_mode);
}

// ── Public control ────────────────────────────────────────────────────────────

void DhtResourceManager::setContributionLevel(ContributionLevel level)
{
    if (m_contribution == level) return;
    m_contribution = level;
    qCInfo(lcDhtRes) << "Contribution level set to" << static_cast<int>(level);
    evaluate();
}

void DhtResourceManager::setSessionActive(bool active)
{
    if (m_sessionActive == active) return;
    m_sessionActive = active;
    qCInfo(lcDhtRes) << "Session" << (active ? "started" : "ended");
    evaluate(); // active session may upgrade mode immediately
}

void DhtResourceManager::setAggressiveScreenOff(bool on)
{
    if (m_aggressiveScreenOff == on) return;
    m_aggressiveScreenOff = on;
    qCInfo(lcDhtRes) << "Aggressive screen-off"
                     << (on ? "ENABLED — screen-off forces DHT Off"
                            : "disabled — screen-off uses Minimal/Maintenance");
    evaluate(); // may change mode immediately if screen is currently off
}

int DhtResourceManager::reannounceIntervalMs() const
{
    switch (m_mode) {
    case DhtMode::Full:        return kReannounceFullMs;
    case DhtMode::Maintenance: return kReannounceMaintenanceMs;
    case DhtMode::Minimal:     return kReannounceMinimalMs;
    case DhtMode::Off:         return kReannounceMaintenanceMs; // not used, but safe
    }
    return kReannounceFullMs;
}

// ── Threshold helpers (vary by user contribution preference) ──────────────────

int DhtResourceManager::batteryThresholdFull() const
{
    // Minimum battery % to stay in Full mode (when not on AC)
    switch (m_contribution) {
    case ContributionLevel::Minimal:  return 50; // aggressive power saving
    case ContributionLevel::Balanced: return 30;
    case ContributionLevel::Full:     return 15;
    }
    return 30;
}

int DhtResourceManager::batteryThresholdMaintenance() const
{
    switch (m_contribution) {
    case ContributionLevel::Minimal:  return 30;
    case ContributionLevel::Balanced: return 20;
    case ContributionLevel::Full:     return 10;
    }
    return 20;
}

int DhtResourceManager::batteryThresholdMinimal() const
{
    switch (m_contribution) {
    case ContributionLevel::Minimal:  return 20;
    case ContributionLevel::Balanced: return 15;
    case ContributionLevel::Full:     return  8;
    }
    return 15;
}

// ── Mode computation ──────────────────────────────────────────────────────────

DhtMode DhtResourceManager::computeTargetMode() const
{
    const int  bat      = m_power->batteryPct();
    const bool charging = m_power->isCharging();
    const bool onAc     = m_power->isOnAc();
    const bool powered  = charging || onAc || bat == 0; // bat==0 = unknown (desktop/VM)
    const bool screen   = m_power->isScreenActive();
    const auto thermal  = m_power->thermalState();
    const auto profile  = m_power->powerProfile();

    // Thermal critical: go offline regardless of everything else.
    // The CPU can't spare cycles for DHT routing.
    if (thermal == Core::ThermalState::Critical)
        return DhtMode::Off;

    // Active P2P session: stay in Full regardless of battery
    // (user is actively using the app — DHT reachability matters most)
    if (m_sessionActive && thermal != Core::ThermalState::Hot)
        return DhtMode::Full;

    // AC power: always at least Maintenance, Full if screen is on
    if (powered) {
        if (screen || m_sessionActive) return DhtMode::Full;
        return DhtMode::Maintenance; // charging but screen off = conserve a bit
    }

    // V32.34+ POWER: aggressive screen-off. By this point we've already
    // guaranteed: not thermal-Critical, no active session, and not on AC
    // power (the powered branch returned above). So if the screen is off and
    // the policy is enabled, fully disconnect — the Minimal/Maintenance modes
    // below would keep the OpenDHT node doing background routing-table and
    // query-serving chatter (the dominant screen-off radio cost); only Off
    // silences the port. We reconnect + re-announce on screen-on, and the
    // mailbox poll can briefly bring DHT up to receive while pocketed.
    if (m_aggressiveScreenOff && !screen)
        return DhtMode::Off;

    // Power-saver profile: drop a tier
    const bool powerSaver = (profile == Core::PowerProfile::PowerSaver);

    // Battery-based thresholds
    if (bat >= batteryThresholdFull() && !powerSaver) {
        return screen ? DhtMode::Full : DhtMode::Maintenance;
    }
    if (bat >= batteryThresholdMaintenance()) {
        return screen ? DhtMode::Maintenance : DhtMode::Minimal;
    }
    if (bat >= batteryThresholdMinimal()) {
        return DhtMode::Minimal;
    }

    // Below minimum threshold: disconnect
    return DhtMode::Off;
}

QString DhtResourceManager::describeModeReason(DhtMode mode) const
{
    const int  bat    = m_power->batteryPct();
    const bool screen = m_power->isScreenActive();
    const bool onAc   = m_power->isOnAc() || m_power->isCharging();

    switch (mode) {
    case DhtMode::Full:
        if (m_sessionActive) return QStringLiteral("active P2P session");
        if (onAc)            return QStringLiteral("charging + screen on");
        return QStringLiteral("battery sufficient + screen on");
    case DhtMode::Maintenance:
        if (onAc)   return QStringLiteral("charging, screen off — reduced announce rate");
        if (!screen) return QStringLiteral("screen off, battery %1%").arg(bat);
        return QStringLiteral("battery %1% — maintenance mode").arg(bat);
    case DhtMode::Minimal:
        if (!screen) return QStringLiteral("screen off, battery %1%").arg(bat);
        return QStringLiteral("battery %1% — minimal contribution").arg(bat);
    case DhtMode::Off:
        if (m_power->thermalState() == Core::ThermalState::Critical)
            return QStringLiteral("thermal critical");
        if (m_aggressiveScreenOff && !screen && !onAc)
            return QStringLiteral("screen off — DHT parked (aggressive), "
                                  "reconnect on wake");
        return QStringLiteral("battery %1% — disconnected to save power").arg(bat);
    }
    return {};
}

// ── Mode application ──────────────────────────────────────────────────────────

void DhtResourceManager::applyMode(DhtMode target)
{
    const DhtMode prev = m_mode;
    if (target == prev) return;

    m_mode        = target;
    m_modeReason  = describeModeReason(target);
    m_stats.currentMode = target;

    qCInfo(lcDhtRes) << "DHT mode:"
                     << static_cast<int>(prev) << "→" << static_cast<int>(target)
                     << "(" << m_modeReason << ")";

    if (target == DhtMode::Off) {
        // Disconnect — saves ~15–30mW of idle modem power on Fairphone 5.
        // The DhtDiscovery::stop() call shuts down the runner cleanly.
#if defined(SNAPCLONE_OPENDHT_BACKEND)
        if (m_dht) m_dht->stop();
#endif
        // V32.34+: bank current online interval into the accumulator and
        // stop the stats emit timer. Uptime stays correct across the
        // off/on cycle because m_uptimeAccumSecs holds the history.
        if (m_uptimeElapsed.isValid()) {
            m_uptimeAccumSecs += m_uptimeElapsed.elapsed() / 1000;
            m_uptimeElapsed.invalidate();
        }
        m_statsEmitTimer->stop();
        m_stats.uptimeSecs  = m_uptimeAccumSecs;
        m_stats.isConnected = false;
        qCInfo(lcDhtRes) << "DHT offline — reconnect check in"
                         << kReconnectCheckMs/1000 << "s";
    } else {
        bool wasOff = (prev == DhtMode::Off);
        if (wasOff) {
            // Reconnect — start DHT and bootstrap
#if defined(SNAPCLONE_OPENDHT_BACKEND)
            if (m_dht) m_dht->start();
#endif
            // V32.34+: start a fresh online interval. The accumulator
            // carries forward; the elapsed timer measures THIS interval.
            m_uptimeElapsed.start();
            m_statsEmitTimer->start();
            m_stats.isConnected = true;
            ++m_stats.reconnectCount;
            qCInfo(lcDhtRes) << "DHT online (reconnect #" << m_stats.reconnectCount << ")";
        }
        // Update re-announce interval on the discovery object
#if defined(SNAPCLONE_OPENDHT_BACKEND)
        if (m_dht) {
            const int intervalMs = reannounceIntervalMs();
            m_dht->setReannounceInterval(intervalMs);
            qCInfo(lcDhtRes) << "Re-announce interval:"
                             << intervalMs / 60000 << "min";
        }
#endif
    }

    emit modeChanged(target, prev);
    emit statsUpdated(m_stats);
    if (target != DhtMode::Off)
        emit reannounceIntervalChanged(reannounceIntervalMs());
}

// ── Slots ─────────────────────────────────────────────────────────────────────

void DhtResourceManager::evaluate()
{
    const DhtMode target = computeTargetMode();
    applyMode(target);
}

void DhtResourceManager::onPowerChanged()
{
    // Debounce: power signals can burst (e.g., charging plugged in triggers
    // both chargingChanged and batteryLevelChanged within milliseconds).
    // A 2s single-shot timer ensures we only call evaluate() once per burst.
    static QTimer *debounce = nullptr;
    if (!debounce) {
        debounce = new QTimer(this);
        debounce->setSingleShot(true);
        debounce->setInterval(2000);
        connect(debounce, &QTimer::timeout, this, &DhtResourceManager::evaluate);
    }
    debounce->start();
}

void DhtResourceManager::onStatsEmitTick()
{
    // V32.34+: refresh uptimeSecs from the live elapsed timer (or just
    // the accumulator if we're currently offline) and publish stats.
    // Called once per minute when DHT is online; the on-demand uptime
    // calc means readers always see a fresh value without us having
    // to wake up 60×/minute.
    qint64 uptimeSecs = m_uptimeAccumSecs;
    if (m_uptimeElapsed.isValid()) {
        uptimeSecs += m_uptimeElapsed.elapsed() / 1000;
    }
    m_stats.uptimeSecs = uptimeSecs;
    emit statsUpdated(m_stats);
}

void DhtResourceManager::onPutCompleted()
{
    ++m_stats.putCount;
    // V32.34+: refresh uptime before emit so subscribers see a current
    // value. Cheap — one elapsed() call (a few ns).
    qint64 uptimeSecs = m_uptimeAccumSecs;
    if (m_uptimeElapsed.isValid()) uptimeSecs += m_uptimeElapsed.elapsed() / 1000;
    m_stats.uptimeSecs = uptimeSecs;
    emit statsUpdated(m_stats);
}

void DhtResourceManager::onGetCompleted()
{
    ++m_stats.getCount;
}

} // namespace SnapClone::Network
