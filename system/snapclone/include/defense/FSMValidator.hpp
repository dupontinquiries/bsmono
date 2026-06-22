#pragma once
// ─── FSMValidator.hpp ─────────────────────────────────────────────────────────
// Tiny finite-state-machine validator. Use as a defense-in-depth filter on
// untrusted inbound protocol messages.
//
// The core idea: even if every individual field of a protocol message is
// well-formed (port in range, lengths bounded, magic number correct), the
// MESSAGE itself may be illegitimate because of WHEN it arrives. Examples
// the parse-level validators don't catch:
//
//   • A peer sends ACK before any CONNECT — we've never seen them, why
//     are we receiving acknowledgements?
//   • A peer sends CHUNK after they've cleanly disconnected — replay
//     attack or session confusion bug.
//   • A peer sends OFFER for a snap they're supposedly receiving from us
//     — protocol role confusion.
//   • A peer sends two CONNECTs in a row without us ACKing the first —
//     wastes our session state, possibly DoS attempt.
//
// Functional design: a transition table is just a free function returning
// bool, paired with the current state. No state owned by this header — the
// caller manages it. We just provide validation predicates and a
// rate-limited-warning helper so misalignments are visible without
// flooding logs.
//
// Usage pattern:
//   namespace MyFSM {
//     enum class Phase { Idle, Connecting, Active, Closing };
//     constexpr bool isLegal(Phase p, Event e) { ... }
//   }
//   if (!MyFSM::isLegal(peer.phase, evt)) {
//       FSMValidator::logMisalignment(lcMine, "MyFSM", peer.phase, evt, sender);
//       return; // drop and don't update state
//   }
//   peer.phase = MyFSM::nextPhase(peer.phase, evt);
//
// Why this is worth the abstraction (small as it is):
//   1. Forces every protocol handler to explicitly enumerate the legal
//      (state, event) pairs in ONE place — easier to audit than
//      "if (foo && !bar && peer.connected && session != 0) ..." checks
//      scattered across switch arms.
//   2. New bugs that introduce illegal transitions become VISIBLE rather
//      than silently corrupting state.
//   3. Future protocol versions can extend the table without touching
//      handler code.
//
// ─────────────────────────────────────────────────────────────────────────────
#include <QLoggingCategory>
#include <QDateTime>
#include <QString>

namespace SnapClone::Core { class AppContextProvider; struct AppContext; }

namespace SnapClone::Defense::FSMValidator {

/// V32.28: optional one-line summary of the current AppContext for
/// inclusion in misalignment logs. Returns empty string when ctx is
/// null. Format: " ctx[appActive=1,screenOn=1,page=camera,
/// conn=wifi,metered=0,userAct=true]" — designed for grep-friendly
/// inline append into existing log lines.
///
/// Inline so each TU that includes this header can use it without a
/// separate .cpp. Implementation defined below the namespace.
QString contextSummary(const SnapClone::Core::AppContextProvider *ctx);

/// Rate-limited misalignment logger. Returns true if it actually logged
/// this call (caller may want to also bump a counter etc.). The static
/// timestamp + count are keyed by `key` so different FSMs don't share
/// throttle state.
///
/// The 5-second window matches what we use for the SCUD/PUNCH loopback
/// drops — long enough not to spam, short enough that an attacker
/// flooding us still produces one log every 5 s.
///
/// Implementation note: function-local static is initialized once per
/// `key` instantiation. We avoid a global map (would need a mutex) by
/// templating on a compile-time tag — each call site gets its own
/// static. That means `logMisalignment<TagA>` and `logMisalignment<TagB>`
/// have independent rate limits. For dynamic keys (e.g. per-peer) the
/// caller should bucket externally.
///
/// V32.28: optional `ctx` parameter appends an AppContext snapshot
/// summary to the log line. Required for offline tuning — without it,
/// FSM-misalignment traces tell us WHAT broke but not WHY (was the
/// user in the foreground? on cellular? recent SSID change?). Pass
/// nullptr if context not available; the log line just omits the
/// summary in that case.
template <typename Tag>
inline bool logMisalignmentRateLimited(const QLoggingCategory &cat,
                                        const QString          &fsmName,
                                        const QString          &fromState,
                                        const QString          &event,
                                        const QString          &peerId,
                                        int                     thresholdMs = 5000,
                                        const SnapClone::Core::AppContextProvider *ctx = nullptr)
{
    static qint64 s_lastLogMs = 0;
    static int    s_droppedSinceLog = 0;
    ++s_droppedSinceLog;
    const qint64 now = QDateTime::currentMSecsSinceEpoch();
    if (now - s_lastLogMs <= thresholdMs) return false;
    s_lastLogMs = now;
    const int batch = s_droppedSinceLog;
    s_droppedSinceLog = 0;
    qCWarning(cat) << "FSM-MISALIGNED [" << fsmName << "]:"
                   << "got event" << event
                   << "in state" << fromState
                   << "from" << peerId
                   << "(" << batch << "illegal transition(s) in last"
                   << (thresholdMs / 1000) << "s)"
                   << contextSummary(ctx);
    return true;
}

/// Untagged version (single throttle for all callers). Use when the
/// callsite is in a single file and you don't need per-FSM separation.
inline bool logMisalignment(const QLoggingCategory &cat,
                             const QString &fsmName,
                             const QString &fromState,
                             const QString &event,
                             const QString &peerId,
                             const SnapClone::Core::AppContextProvider *ctx = nullptr)
{
    struct DefaultTag {};
    return logMisalignmentRateLimited<DefaultTag>(
        cat, fsmName, fromState, event, peerId, 5000, ctx);
}

} // namespace SnapClone::Defense::FSMValidator

// V32.28: contextSummary definition. Included AFTER the namespace
// close so all the templated/inline log helpers above just need a
// forward declaration of AppContextProvider while the helper body
// itself sees the full AppContext type. Any TU that includes
// FSMValidator.hpp picks up AppContext too — currently only
// UdpChannel.cpp does that.
#include "core/AppContext.hpp"

namespace SnapClone::Defense::FSMValidator {

inline QString contextSummary(const SnapClone::Core::AppContextProvider *ctx)
{
    if (!ctx) return QString();
    const auto &c = ctx->context();
    // Single bracketed summary, comma-separated keys, no spaces inside —
    // easy to grep ("ctx[appActive=0" finds backgrounded misalignments,
    // "metered=1" finds cellular ones, etc.).
    const char *connName = "unknown";
    switch (c.connectionType) {
    case SnapClone::Core::AppContext::ConnectionType::Ethernet:  connName = "ethernet"; break;
    case SnapClone::Core::AppContext::ConnectionType::Wifi:      connName = "wifi";     break;
    case SnapClone::Core::AppContext::ConnectionType::Cellular:  connName = "cellular"; break;
    case SnapClone::Core::AppContext::ConnectionType::Bluetooth: connName = "bluetooth";break;
    case SnapClone::Core::AppContext::ConnectionType::Other:     connName = "other";    break;
    case SnapClone::Core::AppContext::ConnectionType::None:      connName = "none";     break;
    case SnapClone::Core::AppContext::ConnectionType::Unknown:   break; // keep default
    }
    return QStringLiteral(" ctx[appActive=%1,screenOn=%2,page=%3,"
                          "conn=%4,metered=%5,powerSaver=%6,onAc=%7,"
                          "userAct=%8,ssidRecent=%9,reachFlap=%10]")
        .arg(c.appActive  ? 1 : 0)
        .arg(c.screenOn   ? 1 : 0)
        .arg(c.currentPage.isEmpty() ? QStringLiteral("unknown") : c.currentPage)
        .arg(QLatin1String(connName))
        .arg(c.metered    ? 1 : 0)
        .arg(c.powerSaver ? 1 : 0)
        .arg(c.onAc       ? 1 : 0)
        .arg(c.userJustActed(5000)               ? 1 : 0)
        .arg(c.ssidJustChanged(5000)              ? 1 : 0)
        .arg(c.reachabilityRecentlyFlapped(5000)  ? 1 : 0);
}

} // namespace SnapClone::Defense::FSMValidator
