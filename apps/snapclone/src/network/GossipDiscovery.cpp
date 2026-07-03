#include "network/GossipDiscovery.hpp"
#include "network/AddressClassification.hpp"  // V32.35+: isPrivate for LAN-direct fix

#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonArray>
#include <QNetworkInterface>
#include <QRandomGenerator>
#include <QRegularExpression>
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcGossip, "snapclone.gossip")

// Maximum UDP datagram size accepted from the network.
// Enforced at both read time (to avoid over-allocation) and in
// processDatagram (defence-in-depth for any other call path).
static constexpr qint64 kMaxGossipDatagramBytes = 4096;

namespace SnapClone::Network {

// ─── Construction ─────────────────────────────────────────────────────────────

GossipDiscovery::GossipDiscovery(quint16 listenPort, QObject *parent)
    : IDiscovery(parent)
    , m_listenPort(listenPort)
{
    connect(&m_gossipTimer, &QTimer::timeout, this, &GossipDiscovery::onGossipTick);
    connect(&m_pruneTimer,  &QTimer::timeout, this, &GossipDiscovery::pruneDeadPeers);
    connect(&m_socket, &QUdpSocket::readyRead, this, &GossipDiscovery::onDatagramReady);
}

// ─── IDiscovery interface ─────────────────────────────────────────────────────

void GossipDiscovery::start()
{
    const bool bound = m_socket.bind(
        QHostAddress::AnyIPv4,
        m_listenPort,
        QUdpSocket::ShareAddress | QUdpSocket::ReuseAddressHint);

    if (!bound) {
        qCWarning(lcGossip) << "Failed to bind UDP socket on port" << m_listenPort
                            << ":" << m_socket.errorString();
        return;
    }

    // Join the multicast group on every non-loopback IPv4 interface.
    // Without specifying the interface, Qt picks the default system route
    // which may be wrong on machines with multiple NICs (eth0 + wlan0).
    // Joining on all interfaces costs nothing extra and ensures discovery
    // works regardless of which interface the peer uses.
    int joinCount = 0;
    const auto ifaces = QNetworkInterface::allInterfaces();
    for (const QNetworkInterface &iface : ifaces) {
        if (!iface.isValid()) continue;
        if (iface.flags() & QNetworkInterface::IsLoopBack) continue;
        if (!(iface.flags() & QNetworkInterface::IsUp)) continue;
        if (!(iface.flags() & QNetworkInterface::IsRunning)) continue;

        // Check the interface has at least one IPv4 address.
        bool hasIPv4 = false;
        for (const QNetworkAddressEntry &entry : iface.addressEntries()) {
            if (entry.ip().protocol() == QAbstractSocket::IPv4Protocol) {
                hasIPv4 = true;
                break;
            }
        }
        if (!hasIPv4) continue;

        const bool ok = m_socket.joinMulticastGroup(
            QHostAddress(QStringLiteral("224.0.0.251")), iface);
        if (ok) {
            qCDebug(lcGossip) << "Joined multicast on" << iface.name();
            ++joinCount;
        } else {
            qCDebug(lcGossip) << "Multicast join failed on" << iface.name()
                              << ":" << m_socket.errorString();
        }
    }

    if (joinCount == 0) {
        // Fallback: join without specifying interface (Qt picks the default).
        m_socket.joinMulticastGroup(QHostAddress(QStringLiteral("224.0.0.251")));
        qCWarning(lcGossip) << "No suitable interface for multicast join; using default route.";
    }

    m_gossipTimer.start(kGossipIntervalMs);
    m_pruneTimer.start(kPruneIntervalMs);

    qCInfo(lcGossip) << "GossipDiscovery started on port" << m_listenPort
                     << "(joined multicast on" << joinCount << "interface(s))";

    // If announcePresence() was called before start(), send the deferred JOIN now.
    if (!m_selfId.isNull()) {
        const QByteArray joinMsg = buildGossipPayload(QStringLiteral("JOIN"));
        // Multicast (preferred)
        const qint64 sent = m_socket.writeDatagram(joinMsg,
                               QHostAddress(QStringLiteral("224.0.0.251")),
                               m_listenPort);
        // ALSO broadcast — on networks that filter multicast (Bell wifi etc),
        // this is what actually reaches peers. We do this on EVERY start
        // (not just when isolated) because the first JOIN is the most
        // important — peers waiting to discover us see it immediately.
        for (const QNetworkInterface &iface : QNetworkInterface::allInterfaces()) {
            if (!iface.flags().testFlag(QNetworkInterface::IsUp)) continue;
            if (iface.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
            if (!iface.flags().testFlag(QNetworkInterface::IsRunning)) continue;
            for (const QNetworkAddressEntry &e : iface.addressEntries()) {
                if (e.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
                const QHostAddress bcast = e.broadcast();
                if (bcast.isNull()) continue;
                m_socket.writeDatagram(joinMsg, bcast, m_listenPort);
            }
        }
        m_socket.writeDatagram(joinMsg, QHostAddress::Broadcast, m_listenPort);
        qCInfo(lcGossip) << "Sent deferred JOIN for" << m_selfId.value
                         << "at" << m_selfAddress.toString()
                         << "bytes:" << sent
                         << "(multicast + broadcast)";
    }
}

void GossipDiscovery::stop()
{
    m_gossipTimer.stop();
    m_pruneTimer.stop();
    m_socket.close();
}

void GossipDiscovery::announcePresence(const Core::UserId &self, const Core::PeerAddress &addr)
{
    m_selfId = self;

    // Resolve "0.0.0.0" to a real LAN IP — peers need a routable address.
    if (addr.host == QLatin1String("0.0.0.0") || addr.host.isEmpty()) {
        // V32.35+: pick the BEST LAN address, not just the first non-loopback.
        // On the FP5, allAddresses() enumerates usb0 (172.16.42.1, the USB
        // tether) before wlan0 (192.168.2.x), so "first" advertised the
        // unreachable tether IP and two same-WiFi phones never went LAN-direct.
        // AddressClass::pickBestLanAddress ranks 192.168/10 above the 172.16
        // tether range. (The receive side also now learns the datagram source
        // IP, so even a wrong announce is corrected on arrival — but announcing
        // the right address up front is better.)
        QStringList candidates;
        for (const QHostAddress &a : QNetworkInterface::allAddresses()) {
            if (!a.isLoopback() && a.protocol() == QAbstractSocket::IPv4Protocol)
                candidates << a.toString();
        }
        const QString lanIp = AddressClass::pickBestLanAddress(candidates);
        m_selfAddress = Core::PeerAddress{
            lanIp.isEmpty()
                ? (candidates.isEmpty() ? QStringLiteral("127.0.0.1")
                                        : candidates.first())
                : lanIp,
            addr.port
        };
        qCInfo(lcGossip) << "announcePresence: picked LAN addr" << m_selfAddress.host
                         << "from candidates" << candidates;
    } else {
        m_selfAddress = addr;
    }

    if (!m_socket.isValid() || m_socket.state() == QAbstractSocket::UnconnectedState) {
        qCDebug(lcGossip) << "announcePresence called before start() — JOIN deferred.";
        return;
    }

    const QByteArray joinMsg = buildGossipPayload(QStringLiteral("JOIN"));
    // Multicast
    m_socket.writeDatagram(joinMsg,
                           QHostAddress(QStringLiteral("224.0.0.251")),
                           m_listenPort);
    // Broadcast fallback — see start() rationale: many consumer wifi setups
    // drop multicast (Bell, hotel APs, etc) but pass broadcast reliably.
    for (const QNetworkInterface &iface : QNetworkInterface::allInterfaces()) {
        if (!iface.flags().testFlag(QNetworkInterface::IsUp)) continue;
        if (iface.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
        if (!iface.flags().testFlag(QNetworkInterface::IsRunning)) continue;
        for (const QNetworkAddressEntry &e : iface.addressEntries()) {
            if (e.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
            const QHostAddress bcast = e.broadcast();
            if (bcast.isNull()) continue;
            m_socket.writeDatagram(joinMsg, bcast, m_listenPort);
        }
    }
    m_socket.writeDatagram(joinMsg, QHostAddress::Broadcast, m_listenPort);
    qCInfo(lcGossip) << "Announced presence:" << self.value
                     << "at" << m_selfAddress.toString()
                     << "(multicast + broadcast)";
}

void GossipDiscovery::lookupPeer(const Core::UserId &id)
{
    if (m_view.contains(id.value)) {
        const auto &record = m_view[id.value];
        emit peerDiscovered(record.id, record.address);
    }
}

// ─── Gossip tick ──────────────────────────────────────────────────────────────

void GossipDiscovery::onGossipTick()
{
    if (m_selfId.isNull()) return;

    // No backoff when isolated. PREV BEHAVIOUR: after 5 empty ticks (~15 s)
    // we'd skip 9 of every 10 ticks, sending a heartbeat only every 30 s.
    // The intent was battery: "don't waste multicast nobody receives".
    //
    // Reality on consumer WiFi (Bell home routers, hotel APs, public WiFi):
    //   • IGMP-snooping/proxying is often misconfigured; multicast packets
    //     drop intermittently.
    //   • Two devices powering on within a short window of each other can
    //     each settle into the slow cadence and miss each other's JOIN
    //     for up to 30 s — feels like "connection is shotty".
    //   • A 200-byte UDP frame every 3 s is ~67 B/s — negligible compared
    //     to anything else the radio is doing.
    //
    // Keep heartbeats on every tick. We still suppress unicast-gateway
    // probes after a few empty cycles (those are noisier in logs) but
    // multicast goes out reliably.
    if (m_view.isEmpty()) {
        ++m_emptyTicks;
    } else {
        m_emptyTicks = 0;
    }
    // Broadcast a JOIN heartbeat.
    // This is critical for initial discovery: both devices start with
    // empty views, the initial JOIN may be dropped (UDP unreliable),
    // and without a heartbeat neither device ever discovers the other.
    if (m_view.isEmpty()) {
        // Note: m_emptyTicks already incremented above — don't increment again.
        const QByteArray join = buildGossipPayload(QStringLiteral("JOIN"));
        // 1. Multicast — preferred path on well-configured networks.
        m_socket.writeDatagram(
            join, QHostAddress(QStringLiteral("224.0.0.251")), m_listenPort);

        // 2. UDP broadcast fallback — subnet-directed broadcast on every
        //    interface AND the global limited broadcast 255.255.255.255.
        //    Consumer wifi (Bell, Comcast, ATT, hotel APs) commonly drops
        //    multicast due to misconfigured IGMP-snooping but passes
        //    broadcast reliably (used by DHCP, ARP, NetBIOS).
        //
        //    Backoff applies: this runs every tick when m_emptyTicks==1,
        //    then every 2/4/8 ticks to limit traffic when isolated for
        //    long periods.
        const int backoffTicks = 1 << qMin(m_emptyTicks - 1, 3); // 1,2,4,8
        if (m_emptyTicks == 1 || (m_emptyTicks % backoffTicks) == 0) {
            // Enable broadcast — required to send to 255.255.255.255
            // and subnet-broadcast on some kernels.
            m_socket.setSocketOption(QAbstractSocket::MulticastTtlOption, 1);

            int subnetCount = 0;
            for (const QNetworkInterface &iface : QNetworkInterface::allInterfaces()) {
                if (!iface.flags().testFlag(QNetworkInterface::IsUp)) continue;
                if (iface.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
                if (!iface.flags().testFlag(QNetworkInterface::IsRunning)) continue;
                for (const QNetworkAddressEntry &e : iface.addressEntries()) {
                    if (e.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
                    if (e.ip().isLoopback()) continue;
                    const QHostAddress bcast = e.broadcast();
                    if (bcast.isNull()) continue;
                    m_socket.writeDatagram(join, bcast, m_listenPort);
                    ++subnetCount;
                    qCDebug(lcGossip) << "Gossip subnet broadcast JOIN →"
                                      << bcast.toString();
                }
            }
            // Limited broadcast — last-resort.
            m_socket.writeDatagram(join, QHostAddress::Broadcast, m_listenPort);
            if (subnetCount > 0 || m_emptyTicks == 1) {
                qCInfo(lcGossip) << "Gossip JOIN — multicast + broadcast on"
                                 << subnetCount << "subnet(s)";
            }
        }
        return;
    }
    m_emptyTicks = 0; // reset backoff when we have peers
    qCDebug(lcGossip) << "Gossip tick — view has" << m_view.size() << "peer(s)";

    const QStringList keys  = m_view.keys();
    const int fanout        = std::min(kFanout, static_cast<int>(keys.size()));

    // Shuffle for random peer selection without std::shuffle complexity.
    QStringList shuffled = keys;
    const qsizetype n   = shuffled.size();
    for (qsizetype i = n - 1; i > 0; --i) {
        const qsizetype j = QRandomGenerator::global()->bounded(static_cast<quint32>(i + 1));
        shuffled.swapItemsAt(i, j);
    }

    const QByteArray payload = buildGossipPayload(QStringLiteral("PING"));
    for (int i = 0; i < fanout; ++i) {
        const auto &peer = m_view[shuffled[i]];
        m_socket.writeDatagram(payload,
                               QHostAddress(peer.address.host),
                               m_listenPort);
    }
}

// ─── Prune stale peers ────────────────────────────────────────────────────────

void GossipDiscovery::pruneDeadPeers()
{
    const QDateTime cutoff =
        QDateTime::currentDateTimeUtc().addMSecs(-kPruneIntervalMs * kPruneMultiplier);

    QStringList toRemove;
    for (auto it = m_view.begin(); it != m_view.end(); ++it) {
        if (it.value().lastSeen < cutoff) {
            toRemove.append(it.key());
        }
    }
    for (const QString &key : toRemove) {
        qCInfo(lcGossip) << "Pruning stale peer:" << key;
        emit peerLost(m_view.take(key).id);
    }
}

// ─── Inbound datagrams ────────────────────────────────────────────────────────

void GossipDiscovery::onDatagramReady()
{
    while (m_socket.hasPendingDatagrams()) {
        QHostAddress senderAddr;
        // pendingDatagramSize() can be -1 on error or 0 for empty.
        // Cap before allocating to avoid OOM on malformed datagrams.
        const qint64 pending = m_socket.pendingDatagramSize();
        if (pending <= 0 || pending > kMaxGossipDatagramBytes) {
            m_socket.readDatagram(nullptr, 0, &senderAddr); // drain and discard
            continue;
        }
        QByteArray data(static_cast<int>(pending), Qt::Uninitialized);
        m_socket.readDatagram(data.data(), data.size(), &senderAddr);
        qCDebug(lcGossip) << "Gossip packet received from" << senderAddr.toString()
                         << "size" << pending;
        processDatagram(data, senderAddr.toString());
    }
}

void GossipDiscovery::processDatagram(const QByteArray &data, const QString &senderHost)
{
    // Reject oversized datagrams before parsing — a crafted large JSON
    // could cause excessive allocations inside QJsonDocument::fromJson.
    if (data.size() > static_cast<int>(kMaxGossipDatagramBytes)) {
        qCDebug(lcGossip) << "Oversized datagram" << data.size() << "bytes from"
                          << senderHost << "— discarding.";
        return;
    }
    const QJsonDocument doc = QJsonDocument::fromJson(data);
    if (!doc.isObject()) return;

    const QJsonObject obj  = doc.object();
    const QString     id   = obj[QStringLiteral("id")].toString();
    const QString     host = obj[QStringLiteral("host")].toString();
    // Port must be a valid 1-65535 integer. QJsonValue::toInt() returns 0 on
    // type mismatch (e.g. peer sent a string) or out-of-range — explicit
    // bounds check prevents accepting a peer claiming port 0 or 100000.
    const int portRaw = obj[QStringLiteral("port")].toInt();
    if (portRaw <= 0 || portRaw > 65535) {
        qCDebug(lcGossip) << "Gossip: invalid port" << portRaw << "from"
                          << senderHost << "— discarding.";
        return;
    }
    const quint16 port = static_cast<quint16>(portRaw);

    // Validate field lengths — protect against injection and DoS.
    constexpr int kMaxFieldLen = 256;
    if (id.isEmpty() || id.size() > kMaxFieldLen) return;
    if (host.size() > kMaxFieldLen) return;
    if (id == m_selfId.value) return;

    // Reject bare UUIDs (e.g. "d8d83a2f-0b49-46dc-8bf5-56853946ebea").
    // These come from stale builds that used QUuid::createUuid() as the peer ID.
    // They have no stable identity and will never complete a useful handshake.
    // A valid peer ID looks like "username@ip" (contains '@' but no UUID hyphens pattern).
    // UUID format: 8-4-4-4-12 hex chars separated by hyphens.
    static const QRegularExpression kUuidRx(
        QStringLiteral("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"),
        QRegularExpression::CaseInsensitiveOption);
    if (kUuidRx.match(id).hasMatch()) {
        qCDebug(lcGossip) << "Ignoring UUID peer ID (stale build):" << id;
        return;
    }

    // Also reject peers advertising the WebSocket port on the wrong protocol.
    // Port 9877 is gossip UDP — a peer advertising WebSocket on 9877 is misconfigured.
    if (port == 0) return;

    // V32.35+ LAN-DIRECT FIX: choose the address two same-subnet peers can
    // actually reach each other on. The datagram's SOURCE IP (senderHost) is
    // provably reachable on this subnet — it's where the JOIN literally came
    // from. The payload's self-reported `host` may be a stale, public, or
    // gateway address (we've seen two phones on one WiFi each advertise their
    // shared public NAT IP 69.x, which is NOT on the local subnet, so
    // DirectDiscovery's subnet check skipped the LAN probe and the phones never
    // went direct despite being on the same router — they fell back to public
    // NAT traversal, which fails behind CGNAT).
    //
    // Rule: if the datagram came from a PRIVATE/LAN source IP, that's the
    // authoritative LAN address — prefer it over whatever the payload claims.
    // Only fall back to the payload host when the source is public (a relayed/
    // forwarded JOIN) or the peer explicitly advertised 0.0.0.0. Cross-subnet
    // discovery is DHT's job, not the LAN probe's.
    const bool senderIsLan =
        SnapClone::Network::AddressClass::isPrivate(senderHost);
    const QString lanHost =
        (host.isEmpty() || host == QLatin1String("0.0.0.0"))
            ? senderHost
            : (senderIsLan ? senderHost : host);
    const Core::PeerAddress addr{ lanHost, port };

    const bool isNew = !m_view.contains(id);

    if (isNew && static_cast<int>(m_view.size()) >= kMaxViewSize) {
        qCDebug(lcGossip) << "View full; skipping new peer" << id;
    } else {
        m_view[id] = PeerRecord{
            Core::UserId{ id },
            addr,
            QDateTime::currentDateTimeUtc()
        };

        if (isNew) {
            qCInfo(lcGossip) << "New peer discovered:" << id << "@" << addr.toString();
            emit peerDiscovered(Core::UserId{ id }, addr);
            // Send reciprocal JOIN: multicast AND unicast directly to sender.
            // Unicast ensures delivery when hotspot blocks multicast.
            if (!m_selfId.isNull()) {
                const QByteArray join = buildGossipPayload(QStringLiteral("JOIN"));
                m_socket.writeDatagram(
                    join,
                    QHostAddress(QStringLiteral("224.0.0.251")),
                    m_listenPort);
                // Also unicast directly to the sender
                m_socket.writeDatagram(
                    join, QHostAddress(addr.host), m_listenPort);
                qCDebug(lcGossip) << "Sent reciprocal JOIN (multicast+unicast) to"
                                  << addr.toString();
            }
        }
    }

    // Merge piggy-backed peer list.
    const QJsonArray peers = obj[QStringLiteral("peers")].toArray();
    for (const QJsonValue &v : peers) {
        const QJsonObject p    = v.toObject();
        const QString     pid  = p[QStringLiteral("id")].toString();
        const QString     phost= p[QStringLiteral("host")].toString();
        // Range-check before narrowing — toInt() returns 0 on bad type and
        // can silently clamp on out-of-range. We need a real port.
        const int pportRaw = p[QStringLiteral("port")].toInt();
        if (pportRaw <= 0 || pportRaw > 65535) continue;
        const quint16 pport = static_cast<quint16>(pportRaw);

        if (pid.isEmpty()
            || pid.size() > kMaxFieldLen
            || phost.size() > kMaxFieldLen
            || pid == m_selfId.value
            || kUuidRx.match(pid).hasMatch()  // reject stale-build UUID IDs
            || m_view.contains(pid)
            || static_cast<int>(m_view.size()) >= kMaxViewSize) {
            continue;
        }

        m_view[pid] = PeerRecord{
            Core::UserId{ pid },
            Core::PeerAddress{ phost, pport },
            QDateTime::currentDateTimeUtc()
        };
        qCDebug(lcGossip) << "Gossip-merged peer:" << pid;
        emit peerDiscovered(Core::UserId{ pid }, Core::PeerAddress{ phost, pport });
    }
}

// ─── Payload builder ──────────────────────────────────────────────────────────

QByteArray GossipDiscovery::buildGossipPayload(const QString &type) const
{
    QJsonObject obj;
    obj[QStringLiteral("type")] = type;
    obj[QStringLiteral("id")]   = m_selfId.value;
    obj[QStringLiteral("host")] = m_selfAddress.host;
    obj[QStringLiteral("port")] = m_selfAddress.port;

    QJsonArray peerArray;
    const QList<PeerRecord> records = m_view.values();
    const int shareCount = std::min(kFanout * 2, static_cast<int>(records.size()));
    for (int i = 0; i < shareCount; ++i) {
        QJsonObject p;
        p[QStringLiteral("id")]   = records[i].id.value;
        p[QStringLiteral("host")] = records[i].address.host;
        p[QStringLiteral("port")] = records[i].address.port;
        peerArray.append(p);
    }
    obj[QStringLiteral("peers")] = peerArray;

    return QJsonDocument(obj).toJson(QJsonDocument::Compact);
}

} // namespace SnapClone::Network
