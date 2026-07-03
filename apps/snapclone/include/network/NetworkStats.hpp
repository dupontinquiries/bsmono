#pragma once
// ─── NetworkStats.hpp ────────────────────────────────────────────────────────
// Per-peer connectivity profile: which transport / address class typically
// works for a given peer, how long it usually takes, and how reliable it
// has been across past sessions.
//
// Sits on top of:
//   • FSM transition signals (Established / Idle) from UdpChannel and
//     WebSocketTransport — successful connections produce Idle→Established
//   • AddressClassification (which class of address actually completed?)
//   • Storage (persistent so the profile survives across runs)
//
// The shape of what we learn, per (peerId, addressClass):
//
//   struct Profile {
//     int    attempts;       // total connect attempts to this class
//     int    successes;      // attempts that reached Established
//     qint64 medianMs;       // p50 connect latency on success
//     qint64 lastSuccessMs;  // wall-clock of most recent success (epoch ms)
//   };
//
// Why this is useful (the "good double-check via other methods" angle the
// user raised in conversation):
//
//   • SAME-LAN-FIRST address ordering is currently a pure heuristic
//     ("if peer.host in our /24, prefer that"). It assumes the LAN
//     address is fast. If Alice's phone has a flaky WiFi NIC and the
//     LAN attempt usually times out, our heuristic actively makes
//     things worse. With learned stats we know "Alice's LAN succeeds
//     20% of the time, takes 8s; public succeeds 95%, takes 1s" — and
//     can reorder accordingly.
//
//   • CGNAT peers — we currently INFO-log that hairpin "may" fail.
//     With stats we know whether THIS specific peer's hairpin works
//     (some CGNAT providers do support it) or not.
//
//   • Anomaly detection — a peer whose successful-class suddenly
//     changes (was always LAN, now always relay) is a signal of
//     environment change worth surfacing.
//
// Toggled by DebugFlag::NoFsmTuning. When NoFsmTuning is ON, the stats
// are still RECORDED but never USED — useful for A/B-testing the
// learned-routing vs default-heuristic paths.
//
// Functional design: NetworkStats is a value-typed module. Functions
// take in (peerId, addressClass, outcome) and return updated profiles.
// The persistence layer just round-trips QVariantMap; the learning logic
// has no Qt-specific dependencies beyond QString/QHash.
//
// Persistence: kept in SQLite via StorageManager (gradual rollout — for
// now in-memory only; persist behind a settings flag).
//
// ─────────────────────────────────────────────────────────────────────────────
#include "network/AddressClassification.hpp"
#include "core/Types.hpp"
#include <QHash>
#include <QString>
#include <QDateTime>
#include <QObject>
#include <array>

namespace SnapClone::Network::Stats {

/// One learned profile for a (peerId, addressClass) pair.
struct Profile {
    int      attempts      = 0;
    int      successes     = 0;
    qint64   medianMs      = 0;   ///< p50 connect latency in ms on success
    qint64   lastSuccessMs = 0;   ///< wall-clock of most recent success

    [[nodiscard]] double successRate() const noexcept {
        return attempts == 0 ? 0.0
            : static_cast<double>(successes) / static_cast<double>(attempts);
    }

    /// Recency: 0 = never, ~1 = within an hour, smaller = older.
    /// Used to weight stale profiles less in scoring.
    [[nodiscard]] double recencyScore(qint64 nowMs) const noexcept {
        if (lastSuccessMs == 0) return 0.0;
        const qint64 ageMs = nowMs - lastSuccessMs;
        if (ageMs < 0)         return 1.0;        // clock skew — treat as recent
        if (ageMs < 60'000)    return 1.0;        // <1min
        if (ageMs < 3'600'000) return 0.9;        // <1h
        if (ageMs < 86'400'000)return 0.7;        // <1d
        if (ageMs < 604'800'000) return 0.4;      // <1w
        return 0.2;                               // older
    }
};

/// Outcome of a connection attempt. Recorded by the network layer after
/// each transport handshake completes (success) or times out (failure).
enum class Outcome : quint8 {
    Success   = 0,
    Timeout   = 1,
    Refused   = 2,   ///< Other end actively refused (RST, refused 4xx, etc.)
    Aborted   = 3,   ///< We gave up before completion (user navigated away)
};

[[nodiscard]] constexpr const char *outcomeName(Outcome o) noexcept
{
    switch (o) {
        case Outcome::Success: return "success";
        case Outcome::Timeout: return "timeout";
        case Outcome::Refused: return "refused";
        case Outcome::Aborted: return "aborted";
    }
    return "?";
}

/// Top-level holder. Singleton lifetime, per-process. Thread-safe reads
/// (atomic snapshots); writes via the recordAttempt() entry point which
/// serialises updates internally.
class NetworkStats : public QObject
{
    Q_OBJECT
public:
    static NetworkStats &instance();

    /// Record an outcome for (peerId, addressClass). `latencyMs` only
    /// meaningful when outcome == Success. Updates profile in-memory.
    void recordAttempt(const Core::UserId &peerId,
                       AddressClass::Class klass,
                       Outcome outcome,
                       qint64 latencyMs = 0);

    /// Read snapshot of one profile. Returns default-constructed Profile
    /// if no data.
    [[nodiscard]] Profile profileFor(const Core::UserId &peerId,
                                      AddressClass::Class klass) const;

    /// Score-ordered list of address classes for a given peer. Highest
    /// score first. Score blends success rate, recency, and latency.
    /// Caller uses this to reorder candidate addresses before trying them.
    ///
    /// When DebugFlag::NoFsmTuning is ON, returns an empty list — callers
    /// fall through to the default heuristic ordering.
    [[nodiscard]] QList<AddressClass::Class>
    rankClassesForPeer(const Core::UserId &peerId) const;

    /// One-line summary for diagnostic logging.
    [[nodiscard]] QString summaryFor(const Core::UserId &peerId) const;

    /// All known peers (for end-of-session report).
    [[nodiscard]] QList<Core::UserId> knownPeers() const;

    /// Multi-line session summary suitable for end-of-session logging.
    [[nodiscard]] QString sessionSummary() const;

signals:
    /// Emitted after every recordAttempt that changes a profile (i.e. not
    /// duplicates of an existing perfect profile). UI can subscribe to
    /// surface "Alice is now reliably reaching you on LAN" hints.
    void profileChanged(SnapClone::Core::UserId peerId,
                        SnapClone::Network::AddressClass::Class klass);

private:
    NetworkStats();
    Q_DISABLE_COPY_MOVE(NetworkStats)

    using ClassArray = std::array<Profile,
        static_cast<std::size_t>(AddressClass::Class::Broadcast) + 1>;
    QHash<QString, ClassArray> m_byPeer; ///< Keyed by peerId.value
    qint64                     m_processStartMs = 0;
};

/// Helper: compute a score for a profile. Higher is better.
/// Exposed for testing; production callers use rankClassesForPeer().
[[nodiscard]] inline double scoreProfile(const Profile &p, qint64 nowMs) noexcept
{
    if (p.attempts == 0) return 0.0;
    const double rate = p.successRate();
    const double rec  = p.recencyScore(nowMs);
    // Latency factor: 100ms = 1.0, 1s = 0.5, 5s = 0.1, slower → 0.0+epsilon.
    // Only used when success rate is positive.
    //
    // All arithmetic stays in double — explicit casts on qint64 inputs
    // prevent -Wconversion warnings (long long → double could lose
    // precision for very large values; medianMs values are always small
    // here, but the compiler can't see that).
    double lat = 1.0;
    if (p.successes > 0 && p.medianMs > 0) {
        const double m = static_cast<double>(p.medianMs);
        if      (m <= 100.0)  lat = 1.0;
        else if (m <= 1000.0) lat = 0.5 + 0.5 * (1000.0 - m) / 900.0;
        else if (m <= 5000.0) lat = 0.1 + 0.4 * (5000.0 - m) / 4000.0;
        else                  lat = qMax(0.05, 5000.0 / m);
    }
    // Weights chosen so success rate dominates (it's the most important
    // signal); recency comes second (a profile we haven't validated this
    // week is less trustworthy); latency is the tiebreaker.
    return (0.6 * rate) + (0.25 * rec) + (0.15 * lat);
}

} // namespace SnapClone::Network::Stats
