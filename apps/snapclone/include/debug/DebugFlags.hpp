#pragma once
// ─── DebugFlags.hpp ──────────────────────────────────────────────────────────
// Process-wide debug toggles that can be flipped at runtime (via settings)
// AND set at startup via env vars or CLI flags. Every flip is logged so the
// final log includes a session summary of toggle activity.
//
// Why a centralised debug-flags module: piling `#ifdef SNAPCLONE_NO_X` into
// every site that respects a debug toggle was getting unwieldy, and the
// flags were hard to flip mid-session. Centralising means:
//
//   • One place to look up "what debug toggles exist"
//   • Live toggling from Settings page (or QSettings on disk)
//   • CLI flags for test pipelines (e.g. `./SnapClone --no-monitoring-blocking`)
//   • Env vars for CI (`SNAPCLONE_NO_FSM_TUNING=1`)
//   • Logged transitions ("Debug flag X turned ON at 12:34:56")
//   • End-of-session summary of how long each was active
//
// Thread safety: flags read on hot paths use std::atomic<bool> so no
// locking is needed for reads. Writes go through `set()` which logs and
// notifies any listening Qt signal targets.
//
// Default values: all OFF. ON means "disable the named subsystem" — flag
// names read like a negative ("noFsmTuning = true" means "FSM tuning
// is OFF"). This is intentional: every flag's default behaviour is the
// production-correct behaviour; toggling ON is opting OUT of normal.
//
// Adding a new flag:
//   1. Add an enum value to DebugFlag
//   2. Add a `name()` mapping
//   3. Read with `is(DebugFlag::X)` on the hot path; write with `set(...)`
//   4. Document why it exists, here and at the use site
//
// ─────────────────────────────────────────────────────────────────────────────
#include <QObject>
#include <QString>
#include <QDateTime>
#include <QLoggingCategory>
#include <atomic>
#include <array>

namespace SnapClone::Debug {

/// All known debug flags. Keep small — this is for actual debugging
/// surfaces, not general user preferences.
enum class DebugFlag : quint8 {
    NoFsmTuning           = 0,  ///< Don't apply learned per-peer routing
    NoMonitoringBlocking  = 1,  ///< Validators log but never reject
    VerboseFsm            = 2,  ///< Log every FSM transition, not just bad ones
    // ─ keep last ─
    Count                 = 3,
};

[[nodiscard]] constexpr const char *name(DebugFlag f) noexcept
{
    switch (f) {
        case DebugFlag::NoFsmTuning:          return "no_fsm_tuning";
        case DebugFlag::NoMonitoringBlocking: return "no_monitoring_blocking";
        case DebugFlag::VerboseFsm:           return "verbose_fsm";
        case DebugFlag::Count:                return "";
    }
    return "";
}

/// Singleton holder. Allocated on first use; lives for the process
/// lifetime. Listeners receive `flagChanged` to update UI / cached state.
class DebugFlags : public QObject
{
    Q_OBJECT
public:
    static DebugFlags &instance();

    /// Fast read for hot-path use. Acquires no lock.
    [[nodiscard]] bool is(DebugFlag flag) const noexcept;

    /// Set a flag. Logs the change. Emits `flagChanged`. The `source` string
    /// is what shows up in the log ("settings page", "CLI", "env", etc.).
    void set(DebugFlag flag, bool on, const QString &source);

    /// Load initial state from env vars and command-line argument vector.
    /// Pass argv from main. Recognised env: SNAPCLONE_NO_FSM_TUNING,
    /// SNAPCLONE_NO_MONITORING_BLOCKING, SNAPCLONE_VERBOSE_FSM.
    /// Recognised flags: --no-fsm-tuning, --no-monitoring-blocking,
    /// --verbose-fsm.
    void loadFromEnvAndArgs(const QStringList &args);

    /// Persistent state via QSettings. Call after QApplication exists.
    void loadFromSettings();
    void saveToSettings() const;

    /// Per-flag activity stats since process start.
    struct ActivityStats {
        int      timesEnabled  = 0;     ///< Number of OFF→ON transitions
        int      timesDisabled = 0;     ///< Number of ON→OFF transitions
        qint64   totalEnabledMs = 0;    ///< Cumulative time flag was ON
        qint64   currentSinceMs = 0;    ///< If currently ON, when it became ON
    };
    [[nodiscard]] ActivityStats stats(DebugFlag flag) const;

    /// Multi-line summary suitable for end-of-session logging.
    /// Format:
    ///   Debug flag summary at shutdown (session 12345 ms):
    ///     no_fsm_tuning           OFF (was ON for 200 ms across 1 toggle)
    ///     no_monitoring_blocking  OFF (never enabled)
    [[nodiscard]] QString sessionSummary() const;

signals:
    void flagChanged(DebugFlag flag, bool on);

private:
    DebugFlags();
    Q_DISABLE_COPY_MOVE(DebugFlags)

    static constexpr std::size_t kCount = static_cast<std::size_t>(DebugFlag::Count);
    std::array<std::atomic<bool>, kCount> m_state{};
    mutable std::array<ActivityStats, kCount> m_stats{};
    qint64 m_processStartMs = 0;
};

/// Convenience: fast inline read. Use this at hot-path call sites.
[[nodiscard]] inline bool isDebug(DebugFlag flag) noexcept
{
    return DebugFlags::instance().is(flag);
}

} // namespace SnapClone::Debug
