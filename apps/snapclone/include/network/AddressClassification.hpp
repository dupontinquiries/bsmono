#pragma once
// ─── AddressClassification.hpp ────────────────────────────────────────────────
// Pure-functional address classification utilities.
//
// Why a separate header: this logic was scattered across 32 inline sites
// (AppController.cpp, AppController_commands.cpp, AppController_connect.cpp,
// NetworkManager.cpp, etc.) — each spelling RFC1918 subnets out long-hand,
// each subtly different (some missed CGNAT, some missed link-local, some
// did the loopback check inline, some didn't).
//
// Functional, no-state design:
//   • Every function is `inline constexpr`-friendly (where Qt types allow).
//   • Takes a QString or QHostAddress, returns bool/enum.
//   • No side effects, no logging, no allocations beyond what Qt's hash
//     calls would do anyway.
//
// Coverage:
//   • RFC1918 private: 10/8, 172.16/12, 192.168/16
//   • RFC6598 CGNAT shared: 100.64/10
//   • RFC3927 link-local IPv4: 169.254/16
//   • RFC4193 unique local IPv6: fc00::/7
//   • RFC4291 link-local IPv6: fe80::/10
//   • Loopback: 127/8, ::1, ::ffff:127/104
//   • IPv4-mapped IPv6 prefix: ::ffff:0:0/96 (normalised before classification)
//
// Usage pattern (read like English):
//   if (AddressClass::isLoopback(addr.host))       return;
//   if (AddressClass::isPrivate(addr.host))        // skip STUN/hole-punch
//   if (AddressClass::isCarrierGradeNAT(addr.host))// hairpin issues likely
//
// ─────────────────────────────────────────────────────────────────────────────
#include <QHostAddress>
#include <QString>

namespace SnapClone::Network::AddressClass {

/// Normalise to a comparable form: strip IPv4-mapped-IPv6 prefix and trim.
/// Returns an empty string for unparseable input so callers can short-circuit.
[[nodiscard]] inline QHostAddress normalise(const QString &raw)
{
    QHostAddress a(raw.trimmed());
    if (a.protocol() == QAbstractSocket::IPv6Protocol) {
        bool v4Mapped = false;
        const quint32 v4 = a.toIPv4Address(&v4Mapped);
        if (v4Mapped) return QHostAddress(v4);
    }
    return a;
}

// V32.32: ── cached subnet anchor addresses ───────────────────────────────────
//
// QHostAddress construction from a string parses a full IP literal each call.
// In hot paths (gossip peer classification per JOIN payload member; DHT
// lookup result filtering during a spray) the public isPrivate() was
// constructing 8+ of these per invocation — once for each subnet check
// inside each sub-function. Pre-constructed statics make this O(1) lookup.
//
// All use a function-local static so the construction happens at first call,
// after Qt has fully initialised, and is thread-safe by C++11 guarantee.
inline const QHostAddress &kSubnet10()       { static const QHostAddress a(QStringLiteral("10.0.0.0"));    return a; }
inline const QHostAddress &kSubnet172_16()   { static const QHostAddress a(QStringLiteral("172.16.0.0"));  return a; }
inline const QHostAddress &kSubnet192_168()  { static const QHostAddress a(QStringLiteral("192.168.0.0")); return a; }
inline const QHostAddress &kSubnet100_64()   { static const QHostAddress a(QStringLiteral("100.64.0.0"));  return a; }
inline const QHostAddress &kSubnet169_254()  { static const QHostAddress a(QStringLiteral("169.254.0.0")); return a; }
inline const QHostAddress &kSubnetFe80()     { static const QHostAddress a(QStringLiteral("fe80::"));      return a; }
inline const QHostAddress &kSubnetFc00()     { static const QHostAddress a(QStringLiteral("fc00::"));      return a; }

// V32.32: ── normalised-input variants ────────────────────────────────────────
//
// The public string-input functions below construct a QHostAddress via
// normalise() then defer to these. When several checks must run against
// the same input (typical of isPrivate which checks 5 categories), the
// caller can call normalise() once and dispatch to all variants with
// just a single QHostAddress live.
//
// These are noexcept and have no string-parsing cost.

[[nodiscard]] inline bool isLoopbackNormalised(const QHostAddress &a)
{
    return !a.isNull() && a.isLoopback();
}

[[nodiscard]] inline bool isLinkLocalNormalised(const QHostAddress &a)
{
    if (a.isNull()) return false;
    return a.isInSubnet(kSubnet169_254(), 16)
        || a.isInSubnet(kSubnetFe80(),    10);
}

[[nodiscard]] inline bool isRfc1918Normalised(const QHostAddress &a)
{
    if (a.isNull()) return false;
    return a.isInSubnet(kSubnet10(),      8)
        || a.isInSubnet(kSubnet172_16(),  12)
        || a.isInSubnet(kSubnet192_168(), 16);
}

[[nodiscard]] inline bool isCarrierGradeNATNormalised(const QHostAddress &a)
{
    if (a.isNull()) return false;
    return a.isInSubnet(kSubnet100_64(), 10);
}

[[nodiscard]] inline bool isUniqueLocalV6Normalised(const QHostAddress &a)
{
    if (a.isNull() || a.protocol() != QAbstractSocket::IPv6Protocol) return false;
    return a.isInSubnet(kSubnetFc00(), 7);
}

[[nodiscard]] inline bool isLoopback(const QString &raw)
{
    return isLoopbackNormalised(normalise(raw));
}

/// RFC3927 link-local IPv4 (169.254/16) + RFC4291 link-local IPv6 (fe80::/10).
[[nodiscard]] inline bool isLinkLocal(const QString &raw)
{
    const auto a = normalise(raw);
    if (a.isNull()) return false;
    return isLinkLocalNormalised(a);
}

/// RFC1918 (10/8, 172.16/12, 192.168/16). Does NOT include CGNAT or
/// link-local — call those separately if you want them.
[[nodiscard]] inline bool isRfc1918(const QString &raw)
{
    const auto a = normalise(raw);
    if (a.isNull()) return false;
    return isRfc1918Normalised(a);
}

/// RFC6598 carrier-grade NAT range (100.64/10). Devices on Bell, T-Mobile
/// 5G, public WiFi often live here. NAT hairpin frequently fails — peers
/// with these addresses usually need a relay.
[[nodiscard]] inline bool isCarrierGradeNAT(const QString &raw)
{
    const auto a = normalise(raw);
    if (a.isNull()) return false;
    return isCarrierGradeNATNormalised(a);
}

/// RFC4193 unique local IPv6 (fc00::/7).
[[nodiscard]] inline bool isUniqueLocalV6(const QString &raw)
{
    QHostAddress a(raw.trimmed());
    if (a.isNull() || a.protocol() != QAbstractSocket::IPv6Protocol) return false;
    return isUniqueLocalV6Normalised(a);
}

/// "Private" = LAN-local, not reachable from the public internet.
/// Covers RFC1918, CGNAT, link-local, unique-local v6, and loopback.
///
/// V32.32: optimised to call `normalise()` ONCE and dispatch each
/// subnet check against the cached normalised address. Previously
/// each of the 5 sub-calls constructed a fresh QHostAddress AND
/// re-parsed the 3-4 RFC1918 subnet strings. In hot paths (DHT
/// result rejection during a spray, gossip-peer-classification on
/// every JOIN payload member) this added up.
[[nodiscard]] inline bool isPrivate(const QString &raw)
{
    const auto a = normalise(raw);
    if (a.isNull()) return false;
    return isLoopbackNormalised(a)
        || isRfc1918Normalised(a)
        || isCarrierGradeNATNormalised(a)
        || isLinkLocalNormalised(a)
        || isUniqueLocalV6Normalised(a);
}

/// Routable on the public internet (the inverse of isPrivate, plus a
/// well-formed check). Empty/invalid inputs return false.
///
/// V32.32: optimised to call `normalise()` once and dispatch via the
/// normalised variants — was calling `isPrivate(raw)` which re-parsed
/// the input string.
[[nodiscard]] inline bool isPublic(const QString &raw)
{
    const auto a = normalise(raw);
    if (a.isNull()) return false;
    if (a.isMulticast() || a.isBroadcast()) return false;
    return !(isLoopbackNormalised(a)
          || isRfc1918Normalised(a)
          || isCarrierGradeNATNormalised(a)
          || isLinkLocalNormalised(a)
          || isUniqueLocalV6Normalised(a));
}

/// One of the user's own machine's addresses (loopback OR equal to one
/// of our locally-bound addresses). Pass the list of bound addrs in.
[[nodiscard]] inline bool isOwnAddress(const QString &raw,
                                        const QList<QHostAddress> &ownAddrs)
{
    const auto a = normalise(raw);
    if (a.isNull()) return false;
    if (isLoopbackNormalised(a)) return true;
    for (const auto &own : ownAddrs)
        if (own == a) return true;
    return false;
}

/// Classification enum useful for routing decisions. Returns the most
/// specific match. Caller uses this in switch statements rather than
/// chaining bool predicates.
enum class Class : quint8 {
    Invalid       = 0,
    Loopback      = 1,
    LinkLocal     = 2,
    Rfc1918       = 3,
    CarrierGrade  = 4,  // RFC6598 CGNAT — frequently NAT-hairpin-broken
    UniqueLocalV6 = 5,
    Public        = 6,
    Multicast     = 7,
    Broadcast     = 8,
};

/// V32.32: optimised to call `normalise()` once and dispatch via the
/// normalised variants. The previous form re-parsed the input string
/// for every category check (potentially 5 re-parses per call). Result
/// ordering is unchanged.
[[nodiscard]] inline Class classify(const QString &raw)
{
    const auto a = normalise(raw);
    if (a.isNull())               return Class::Invalid;
    if (a.isLoopback())           return Class::Loopback;
    if (a.isMulticast())          return Class::Multicast;
    if (a.isBroadcast())          return Class::Broadcast;
    if (isLinkLocalNormalised(a)) return Class::LinkLocal;
    if (isCarrierGradeNATNormalised(a)) return Class::CarrierGrade;
    if (isRfc1918Normalised(a))   return Class::Rfc1918;
    if (isUniqueLocalV6Normalised(a)) return Class::UniqueLocalV6;
    return Class::Public;
}

/// Human-readable name for the class — useful in diagnostic logs.
[[nodiscard]] inline const char *className(Class c)
{
    switch (c) {
        case Class::Invalid:       return "invalid";
        case Class::Loopback:      return "loopback";
        case Class::LinkLocal:     return "link-local";
        case Class::Rfc1918:       return "rfc1918";
        case Class::CarrierGrade:  return "cgnat";
        case Class::UniqueLocalV6: return "unique-local-v6";
        case Class::Public:        return "public";
        case Class::Multicast:     return "multicast";
        case Class::Broadcast:     return "broadcast";
    }
    return "?";
}

/// Two addresses on the same /24 network. Used for "same-WiFi" detection
/// — consumer routers virtually always default to /24, so matching the
/// first three octets is a reliable proxy for "same broadcast domain"
/// without needing to query the netmask of each interface.
///
/// Returns false for empty/invalid input, IPv6 (no equivalent simple
/// netmask comparison), or mismatch.
[[nodiscard]] inline bool isSameLan24(const QString &a, const QString &b)
{
    if (a.isEmpty() || b.isEmpty()) return false;
    const QHostAddress ha(a), hb(b);
    if (ha.protocol() != QAbstractSocket::IPv4Protocol) return false;
    if (hb.protocol() != QAbstractSocket::IPv4Protocol) return false;
    return (ha.toIPv4Address() & 0xFFFFFF00u)
        == (hb.toIPv4Address() & 0xFFFFFF00u);
}

/// V32.35+: rank an IPv4 address by how good a SELF-ANNOUNCE LAN address it is
/// — i.e. how likely a peer on the same WiFi can reach us at it. Higher = better.
///
/// Why this exists: announcePresence() used to advertise the FIRST non-loopback
/// IPv4 from QNetworkInterface::allAddresses(). On the FP5 that enumerates the
/// USB-tether interface (usb0 = 172.16.42.1/16) BEFORE wlan0 (192.168.2.x), so
/// the phone announced its USB IP — unreachable by another phone on the same
/// WiFi, so LAN-direct never connected and snaps fell back to (failing) public
/// NAT traversal. Ranking lets us pick the real WiFi address regardless of
/// enumeration order.
///
/// Ranking rationale (consumer-network priors):
///   192.168.0.0/16  → 100  (overwhelmingly the home-WiFi range)
///   10.0.0.0/8      →  80  (common home/office LAN)
///   172.16.0.0/12   →  40  (ALSO the FP5 USB-tether range — could be either;
///                            ranked below 192.168/10 so a real WiFi addr wins,
///                            but above link-local so it's used if it's all we
///                            have)
///   link-local / CGNAT / loopback / public / IPv6 → low/zero (not a LAN-direct
///   address we want to advertise to same-subnet peers)
[[nodiscard]] inline int lanAdvertiseRank(const QString &raw)
{
    const auto a = normalise(raw);
    if (a.isNull()) return 0;
    if (a.protocol() != QAbstractSocket::IPv4Protocol) return 0; // IPv4 only here
    if (a.isLoopback()) return 0;
    if (a.isInSubnet(kSubnet192_168(), 16)) return 100;
    if (a.isInSubnet(kSubnet10(),       8)) return 80;
    if (a.isInSubnet(kSubnet172_16(),  12)) return 40;  // incl FP5 usb0 tether
    if (a.isInSubnet(kSubnet169_254(), 16)) return 10;  // link-local fallback
    return 0; // CGNAT / public / anything else: not a good LAN-direct address
}

/// V32.35+: pick the best self-announce LAN address from a candidate list
/// (typically QNetworkInterface::allAddresses() stringified). Returns the
/// highest-ranked; ties broken by first-seen (stable). Empty string if none
/// score above zero — caller should then fall back (e.g. to the datagram
/// source-IP learning on the receive side, or 127.0.0.1 as a last resort).
[[nodiscard]] inline QString pickBestLanAddress(const QStringList &candidates)
{
    QString best;
    int bestRank = 0;
    for (const QString &c : candidates) {
        const int r = lanAdvertiseRank(c);
        if (r > bestRank) { bestRank = r; best = c; }
    }
    return best;
}

} // namespace SnapClone::Network::AddressClass
