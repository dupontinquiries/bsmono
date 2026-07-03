#pragma once
// ─── DhtResourceManager.hpp ──────────────────────────────────────────────────
//
// Balances DHT network contribution against battery and thermal constraints.
//
// # The problem
//
// OpenDHT contributes to the network by:
//   1. Storing values for other nodes (routing table participation)
//   2. Responding to GET requests from peers
//   3. Refreshing our own PUT values so they don't expire
//
// All three cost battery and data. On a phone with an active P2P session, the
// cost is justified. In the background with 12% battery, it isn't.
//
// # Uptime modes
//
// Mode::Full        — full contribution, 55-min re-announce
//   Condition: active P2P session OR screen on AND battery > 30% OR charging
//   DHT stays connected, re-announces at normal rate
//
// Mode::Maintenance — reduced contribution, 3-hour re-announce
//   Condition: no active session, screen on, battery 20–30%
//   DHT stays connected, but re-announces less often
//   Our routing table contribution is lower but we're still reachable
//
// Mode::Minimal     — minimal contribution, 8-hour re-announce
//   Condition: screen off OR battery 15–20% (not charging)
//   DHT stays connected but only refreshes before values would expire (1h)
//   Covers the "phone charging overnight" use case
//
// Mode::Off         — DHT disconnected
//   Condition: battery < 15% AND not charging, OR thermal Critical
//   Saves ~15–30mW of modem idle power on the Fairphone 5
//   We reconnect and re-announce as soon as conditions improve
//
// # User contribution preference
//
// A slider in Settings maps to ContributionLevel:
//   Minimal  — uses Mode::Off more aggressively (battery threshold = 25%)
//   Balanced — default thresholds above
//   Full     — stays connected unless thermal Critical or battery < 10%
//
// This is the user's explicit choice to contribute more/less to the network.
//
// # Statistics tracking
//
// Since the opendht C API doesn't expose routing table stats directly, we
// estimate contribution from:
//   - put_count:         how many values we've published for ourselves
//   - get_count:         how many lookups we've served (inferred from callbacks)
//   - uptime_secs:       how long the DHT has been connected this session
//   - reconnect_count:   how many times we've reconnected (Off→Full transitions)

#include <QObject>
#include <QTimer>
#include <QElapsedTimer>

namespace SnapClone::Core  { class PowerManager; }

namespace SnapClone::Network {

class DhtDiscovery;

// ── Contribution level — set by the user ─────────────────────────────────────

enum class ContributionLevel {
    Minimal,   // Prioritise battery — disconnect aggressively
    Balanced,  // Default — reasonable network citizen
    Full,      // Prioritise network — stay connected as long as possible
};

// ── Operating mode — computed from power state + contribution level ───────────

enum class DhtMode {
    Full,        // 55-min re-announce, full routing participation
    Maintenance, // 3-hour re-announce, stay connected but quieter
    Minimal,     // 8-hour re-announce, barely-there but still reachable
    Off,         // Disconnected — not contributing, saves modem power
};

// ── Runtime statistics ────────────────────────────────────────────────────────

struct DhtStats {
    int    uptimeSecs     = 0;   // seconds DHT has been connected this session
    int    reconnectCount = 0;   // Off→Full transitions (high = unstable conditions)
    int    putCount       = 0;   // DHT PUT operations performed
    int    getCount       = 0;   // DHT GET operations performed
    DhtMode currentMode   = DhtMode::Off;
    bool   isConnected    = false;
};

// ─────────────────────────────────────────────────────────────────────────────

class DhtResourceManager : public QObject
{
    Q_OBJECT
public:
    explicit DhtResourceManager(
        DhtDiscovery          *dht,
        Core::PowerManager    *power,
        QObject               *parent = nullptr);
    ~DhtResourceManager() override = default;

    // ── Control ───────────────────────────────────────────────────────────
    void setContributionLevel(ContributionLevel level);
    ContributionLevel contributionLevel() const { return m_contribution; }

    /// Called by AppController when a P2P session becomes active/inactive.
    void setSessionActive(bool active);

    /// V32.34+ POWER: when true, screen-off on battery (no active session,
    /// not charging) forces DHT fully OFF instead of Minimal/Maintenance —
    /// the node keeps doing background routing-table / query-serving chatter
    /// in those modes, which is the dominant screen-off radio cost; only Off
    /// truly silences the port. We reconnect + re-announce on screen-on (~1
    /// announce), and the mailbox poll can briefly bring DHT up to receive
    /// while pocketed. Default OFF (opt-in) so existing behavior is unchanged
    /// until validated on-device. See docs/TODO.md "Limit DHT PORT traffic
    /// when screen-off".
    void setAggressiveScreenOff(bool on);
    bool aggressiveScreenOff() const { return m_aggressiveScreenOff; }

    // ── State ─────────────────────────────────────────────────────────────
    DhtMode  currentMode()  const { return m_mode; }
    DhtStats stats()        const { return m_stats; }
    bool     isConnected()  const { return m_mode != DhtMode::Off; }

    /// Re-announce interval for the current mode (milliseconds).
    int reannounceIntervalMs() const;

    /// Human-readable description of why the current mode was chosen.
    QString modeReason() const { return m_modeReason; }

signals:
    void modeChanged(SnapClone::Network::DhtMode newMode,
                     SnapClone::Network::DhtMode oldMode);
    void statsUpdated(SnapClone::Network::DhtStats stats);
    void reannounceIntervalChanged(int ms);

public slots:
    /// Called by DhtDiscovery when a PUT completes.
    void onPutCompleted();
    /// Called by DhtDiscovery when a GET completes.
    void onGetCompleted();

private slots:
    void evaluate();          // recompute target mode from all signals
    /// V32.34+ battery: previously a 1-second counter that did
    /// ++uptimeSecs and emitted stats every 60th tick. Now a 60-second
    /// timer that just emits stats; uptime is computed on-demand from
    /// m_uptimeElapsed.elapsed() / 1000. Saves ~59 CPU wakeups/minute
    /// for the whole time DHT is on (which is most of the time).
    void onStatsEmitTick();
    void onPowerChanged();    // any PowerManager signal

private:
    void applyMode(DhtMode target);
    DhtMode computeTargetMode() const;
    QString describeModeReason(DhtMode mode) const;

    DhtDiscovery            *m_dht;
    Core::PowerManager      *m_power;
    ContributionLevel        m_contribution  = ContributionLevel::Balanced;
    DhtMode                  m_mode          = DhtMode::Off;
    QString                  m_modeReason;
    bool                     m_sessionActive = false;
    /// V32.34+ POWER: opt-in aggressive screen-off (see setAggressiveScreenOff).
    bool                     m_aggressiveScreenOff = false;
    QTimer                  *m_evalTimer     = nullptr;  // re-evaluate every 60s
    /// V32.34+ battery: 60-second stats emitter. Replaces the old
    /// 1Hz uptime tick. See onStatsEmitTick docstring.
    QTimer                  *m_statsEmitTimer = nullptr;
    /// V32.34+: monotonic clock for on-demand uptime calculation. Started
    /// when DHT goes online; checked when caller reads m_stats.uptimeSecs.
    /// Accumulator m_uptimeAccumSecs survives stop/start cycles so
    /// reported uptime is "total time DHT spent online this session".
    QElapsedTimer            m_uptimeElapsed;
    qint64                   m_uptimeAccumSecs = 0;
    DhtStats                 m_stats;

    // Thresholds — vary by contribution level
    int batteryThresholdFull()        const;
    int batteryThresholdMaintenance() const;
    int batteryThresholdMinimal()     const;
};

} // namespace SnapClone::Network
