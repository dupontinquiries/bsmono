#include "debug/DebugFlags.hpp"
#include <QSettings>
#include <QCoreApplication>

Q_LOGGING_CATEGORY(lcDebug, "snapclone.debug")

namespace SnapClone::Debug {

DebugFlags &DebugFlags::instance()
{
    static DebugFlags inst;
    return inst;
}

DebugFlags::DebugFlags()
    : m_processStartMs(QDateTime::currentMSecsSinceEpoch())
{
    for (auto &a : m_state) a.store(false, std::memory_order_relaxed);
}

bool DebugFlags::is(DebugFlag flag) const noexcept
{
    const auto idx = static_cast<std::size_t>(flag);
    if (idx >= kCount) return false;
    return m_state[idx].load(std::memory_order_relaxed);
}

void DebugFlags::set(DebugFlag flag, bool on, const QString &source)
{
    const auto idx = static_cast<std::size_t>(flag);
    if (idx >= kCount) return;
    const bool prev = m_state[idx].exchange(on, std::memory_order_relaxed);
    if (prev == on) return;  // no change

    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    auto &s = m_stats[idx];
    if (on) {
        s.timesEnabled++;
        s.currentSinceMs = now;
        qCWarning(lcDebug) << "Debug flag" << name(flag) << "turned ON"
                           << "(source:" << source << ")";
    } else {
        s.timesDisabled++;
        if (s.currentSinceMs > 0) {
            s.totalEnabledMs += (now - s.currentSinceMs);
            s.currentSinceMs = 0;
        }
        qCWarning(lcDebug) << "Debug flag" << name(flag) << "turned OFF"
                           << "(source:" << source << ")";
    }
    emit flagChanged(flag, on);
}

void DebugFlags::loadFromEnvAndArgs(const QStringList &args)
{
    // Env-var lookup table — keep in sync with the enum.
    struct Entry { DebugFlag flag; const char *env; const char *cli; };
    const std::array<Entry, kCount> table{{
        { DebugFlag::NoFsmTuning,          "SNAPCLONE_NO_FSM_TUNING",
          "--no-fsm-tuning" },
        { DebugFlag::NoMonitoringBlocking, "SNAPCLONE_NO_MONITORING_BLOCKING",
          "--no-monitoring-blocking" },
        { DebugFlag::VerboseFsm,           "SNAPCLONE_VERBOSE_FSM",
          "--verbose-fsm" },
    }};

    for (const auto &e : table) {
        const QByteArray ev = qgetenv(e.env);
        if (!ev.isEmpty() && ev != "0" && ev.toLower() != "false") {
            set(e.flag, true, QStringLiteral("env (%1)").arg(e.env));
            continue; // env takes priority over CLI in this design
        }
        if (args.contains(QString::fromLatin1(e.cli))) {
            set(e.flag, true, QStringLiteral("cli (%1)").arg(e.cli));
        }
    }
}

void DebugFlags::loadFromSettings()
{
    // Settings path is the same QSettings the rest of the app uses
    // (organization/application from QCoreApplication).
    QSettings s;
    s.beginGroup(QStringLiteral("debug"));
    for (std::size_t i = 0; i < kCount; ++i) {
        const auto f = static_cast<DebugFlag>(i);
        const QString key = QString::fromLatin1(name(f));
        if (!s.contains(key)) continue;
        const bool v = s.value(key, false).toBool();
        if (v && !is(f)) set(f, true, QStringLiteral("settings.ini"));
        // We don't auto-turn-off via settings — if env/CLI turned it ON,
        // settings can't override (avoids surprising the user mid-debug
        // with stale on-disk state).
    }
    s.endGroup();
}

void DebugFlags::saveToSettings() const
{
    QSettings s;
    s.beginGroup(QStringLiteral("debug"));
    for (std::size_t i = 0; i < kCount; ++i) {
        const auto f = static_cast<DebugFlag>(i);
        s.setValue(QString::fromLatin1(name(f)), is(f));
    }
    s.endGroup();
}

DebugFlags::ActivityStats DebugFlags::stats(DebugFlag flag) const
{
    const auto idx = static_cast<std::size_t>(flag);
    if (idx >= kCount) return {};
    ActivityStats out = m_stats[idx];
    // If currently ON, include time since became ON in the total.
    if (out.currentSinceMs > 0) {
        out.totalEnabledMs += QDateTime::currentMSecsSinceEpoch() - out.currentSinceMs;
    }
    return out;
}

QString DebugFlags::sessionSummary() const
{
    const qint64 sessionMs = QDateTime::currentMSecsSinceEpoch() - m_processStartMs;
    QString out = QStringLiteral("Debug flag summary at shutdown (session %1 ms):\n")
                      .arg(sessionMs);
    for (std::size_t i = 0; i < kCount; ++i) {
        const auto f = static_cast<DebugFlag>(i);
        const auto s = stats(f);
        const bool curOn = is(f);
        out += QStringLiteral("  %1  %2 ")
                   .arg(QString::fromLatin1(name(f)), -25)
                   .arg(curOn ? QStringLiteral("ON ") : QStringLiteral("OFF"));
        if (s.timesEnabled == 0 && !curOn) {
            out += QStringLiteral("(never enabled)\n");
        } else {
            out += QStringLiteral("(was ON for %1 ms across %2 toggle%3)\n")
                       .arg(s.totalEnabledMs)
                       .arg(s.timesEnabled)
                       .arg(s.timesEnabled == 1 ? QStringLiteral("") : QStringLiteral("s"));
        }
    }
    return out;
}

} // namespace SnapClone::Debug
