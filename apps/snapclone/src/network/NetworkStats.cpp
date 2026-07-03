#include "network/NetworkStats.hpp"
#include "debug/DebugFlags.hpp"
#include <QLoggingCategory>
#include <algorithm>

Q_LOGGING_CATEGORY(lcNetStats, "snapclone.netstats")

namespace SnapClone::Network::Stats {

NetworkStats &NetworkStats::instance()
{
    static NetworkStats inst;
    return inst;
}

NetworkStats::NetworkStats()
    : m_processStartMs(QDateTime::currentMSecsSinceEpoch())
{
}

void NetworkStats::recordAttempt(const Core::UserId &peerId,
                                  AddressClass::Class klass,
                                  Outcome outcome,
                                  qint64 latencyMs)
{
    if (peerId.value.isEmpty()) return;
    const auto idx = static_cast<std::size_t>(klass);
    auto &arr = m_byPeer[peerId.value];
    if (idx >= arr.size()) return;
    auto &p = arr[idx];

    p.attempts++;
    if (outcome == Outcome::Success) {
        p.successes++;
        // Running median approximation: nudge toward the new sample. For
        // p50 estimation we use a small step size (10% of the gap). This
        // converges to the actual median without needing to store the
        // full sample history.
        if (p.medianMs == 0) {
            p.medianMs = latencyMs;
        } else {
            const qint64 delta = latencyMs - p.medianMs;
            p.medianMs += delta / 10;
        }
        p.lastSuccessMs = QDateTime::currentMSecsSinceEpoch();
    }
    qCDebug(lcNetStats) << "Record:" << peerId.value << "via"
                        << AddressClass::className(klass)
                        << "-" << outcomeName(outcome)
                        << "(" << latencyMs << "ms)"
                        << "→ " << p.successes << "/" << p.attempts
                        << "successes, median" << p.medianMs << "ms";
    emit profileChanged(peerId, klass);
}

Profile NetworkStats::profileFor(const Core::UserId &peerId,
                                  AddressClass::Class klass) const
{
    const auto it = m_byPeer.constFind(peerId.value);
    if (it == m_byPeer.constEnd()) return {};
    const auto idx = static_cast<std::size_t>(klass);
    if (idx >= it->size()) return {};
    return (*it)[idx];
}

QList<AddressClass::Class>
NetworkStats::rankClassesForPeer(const Core::UserId &peerId) const
{
    // When FSM tuning is disabled (debug flag), return empty so callers
    // fall back to the default heuristic ordering. This is the A/B
    // switch: production builds use learned data; --no-fsm-tuning
    // bypasses it entirely.
    if (Debug::isDebug(Debug::DebugFlag::NoFsmTuning)) return {};

    const auto it = m_byPeer.constFind(peerId.value);
    if (it == m_byPeer.constEnd()) return {};

    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    struct Scored { AddressClass::Class klass; double score; };
    QList<Scored> scored;
    for (std::size_t i = 0; i < it->size(); ++i) {
        const auto &p = (*it)[i];
        if (p.attempts == 0) continue;
        const auto klass = static_cast<AddressClass::Class>(i);
        scored.append({ klass, scoreProfile(p, nowMs) });
    }
    std::sort(scored.begin(), scored.end(),
              [](const Scored &a, const Scored &b) { return a.score > b.score; });

    QList<AddressClass::Class> out;
    out.reserve(scored.size());
    for (const auto &s : scored) out.append(s.klass);
    return out;
}

QString NetworkStats::summaryFor(const Core::UserId &peerId) const
{
    const auto it = m_byPeer.constFind(peerId.value);
    if (it == m_byPeer.constEnd()) return QStringLiteral("(no history)");
    QStringList parts;
    for (std::size_t i = 0; i < it->size(); ++i) {
        const auto &p = (*it)[i];
        if (p.attempts == 0) continue;
        const auto klass = static_cast<AddressClass::Class>(i);
        parts << QStringLiteral("%1 %2/%3 (%4ms)")
                     .arg(QString::fromLatin1(AddressClass::className(klass)))
                     .arg(p.successes).arg(p.attempts).arg(p.medianMs);
    }
    return parts.isEmpty() ? QStringLiteral("(no history)")
                            : parts.join(QStringLiteral("; "));
}

QList<Core::UserId> NetworkStats::knownPeers() const
{
    QList<Core::UserId> out;
    out.reserve(m_byPeer.size());
    for (auto it = m_byPeer.constBegin(); it != m_byPeer.constEnd(); ++it)
        out.append(Core::UserId{ it.key() });
    return out;
}

QString NetworkStats::sessionSummary() const
{
    const qint64 sessionMs = QDateTime::currentMSecsSinceEpoch() - m_processStartMs;
    QString out = QStringLiteral(
        "Network stats summary at shutdown (session %1 ms, %2 peer(s) observed):\n")
            .arg(sessionMs).arg(m_byPeer.size());
    for (auto it = m_byPeer.constBegin(); it != m_byPeer.constEnd(); ++it) {
        out += QStringLiteral("  %1: %2\n")
                   .arg(it.key(), summaryFor(Core::UserId{ it.key() }));
    }
    return out;
}

} // namespace SnapClone::Network::Stats
