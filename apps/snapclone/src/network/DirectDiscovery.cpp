#include "network/DirectDiscovery.hpp"
#include <QLoggingCategory>
#include <QNetworkInterface>
#include <QNetworkDatagram>
#include <QDataStream>
#include <QDateTime>
#include <QHostAddress>
#include <QRandomGenerator>

Q_LOGGING_CATEGORY(lcDirect, "snapclone.direct")

namespace SnapClone::Network {

// ─── Frame wire format ────────────────────────────────────────────────────────
// Announce frame (UDP multicast / broadcast):
//   [4]  magic    0x53434150 ("SCAP" = SnapClone Announce Peer)
//   [2]  id_len
//   [N]  sc-ID (UTF-8)
//   [2]  ws_port
//   [8]  timestamp (ms since epoch, big-endian)
//
// Peer-table frame (sent over established WebSocket session):
//   [4]  magic    0x5054424C ("PTBL")
//   [2]  count
//   for each entry:
//     [2]  id_len
//     [N]  sc-ID
//     [2]  host_len
//     [M]  host (UTF-8 IP string)
//     [2]  port
//     [8]  last_seen_ms
// ─────────────────────────────────────────────────────────────────────────────

static constexpr quint32 kAnnounceMagic = 0x53434150; // "SCAP"

DirectDiscovery::DirectDiscovery(QObject *parent)
    : QObject(parent)
{
    connect(&m_ip6Socket, &QUdpSocket::readyRead,
            this, &DirectDiscovery::onIpv6ReadyRead);

    m_lanProbeTimer.setSingleShot(true);
    connect(&m_lanProbeTimer, &QTimer::timeout,
            this, &DirectDiscovery::onLanProbeTimer);

    m_announceTimer.setInterval(45000); // re-announce every 45s
    m_announceTimer.setTimerType(Qt::VeryCoarseTimer); // battery: batch the 45s announce
    connect(&m_announceTimer, &QTimer::timeout, this, [this]() {
        sendIpv6Announce();
        probeLastKnownAddrs(); // also re-probe LAN for stale contacts
    });
}

DirectDiscovery::~DirectDiscovery() { stop(); }

void DirectDiscovery::start(const Core::UserId &selfId, quint16 wsPort,
                             const QList<KnownPeer> &knownPeers)
{
    m_selfId     = selfId;
    m_wsPort     = wsPort;
    m_knownPeers = knownPeers;

    // ── IPv6 link-local multicast ─────────────────────────────────────────
    // Bind to port kIp6Port on all interfaces. Join ff02::1 (all-nodes).
    m_ip6Socket.close();
    if (m_ip6Socket.bind(QHostAddress::AnyIPv6, kIp6Port,
                         QUdpSocket::ShareAddress | QUdpSocket::ReuseAddressHint)) {
        // Join all-nodes multicast on every up interface
        for (const QNetworkInterface &iface : QNetworkInterface::allInterfaces()) {
            if (!iface.flags().testFlag(QNetworkInterface::IsUp)) continue;
            if (iface.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
            m_ip6Socket.joinMulticastGroup(
                QHostAddress(QStringLiteral("ff02::1")), iface);
        }
        sendIpv6Announce();
        m_announceTimer.start();
        qCInfo(lcDirect) << "IPv6 link-local discovery started on port" << kIp6Port;
    } else {
        qCDebug(lcDirect) << "IPv6 not available:" << m_ip6Socket.errorString();
    }

    // ── LAN probe — try known addrs on current subnets shortly after start ─
    // Wait 2s for STUN/ipify to complete so we know our own address first.
    m_lanProbeTimer.start(2000);
}

void DirectDiscovery::stop()
{
    m_lanProbeTimer.stop();
    m_announceTimer.stop();
    m_ip6Socket.close();
}

void DirectDiscovery::updateKnownPeer(const Core::UserId      &id,
                                       const Core::PeerAddress &addr)
{
    for (auto &p : m_knownPeers) {
        if (p.id.value == id.value) {
            p.addr = addr;
            p.lastSeenMs = QDateTime::currentMSecsSinceEpoch();
            return;
        }
    }
    m_knownPeers.append({ id, addr, QDateTime::currentMSecsSinceEpoch() });
}

// ─── IPv6 link-local ──────────────────────────────────────────────────────────

void DirectDiscovery::sendIpv6Announce()
{
    if (m_ip6Socket.state() != QAbstractSocket::BoundState) return;
    const QByteArray idBytes = m_selfId.value.toUtf8();

    QByteArray frame;
    QDataStream ds(&frame, QIODeviceBase::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kAnnounceMagic
       << static_cast<quint16>(idBytes.size());
    ds.writeRawData(idBytes.constData(), idBytes.size());
    ds << m_wsPort
       << static_cast<qint64>(QDateTime::currentMSecsSinceEpoch());

    // Send to ff02::1 (all link-local nodes) on every interface
    for (const QNetworkInterface &iface : QNetworkInterface::allInterfaces()) {
        if (!iface.flags().testFlag(QNetworkInterface::IsUp)) continue;
        if (iface.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
        m_ip6Socket.writeDatagram(frame,
            QHostAddress(QStringLiteral("ff02::1")), kIp6Port);
    }
    qCDebug(lcDirect) << "IPv6 announce sent";
}

void DirectDiscovery::onIpv6ReadyRead()
{
    while (m_ip6Socket.hasPendingDatagrams()) {
        const QNetworkDatagram dg = m_ip6Socket.receiveDatagram(512);
        const QByteArray raw = dg.data();
        if (raw.size() < 8) continue;

        QDataStream ds(raw);
        ds.setByteOrder(QDataStream::BigEndian);
        quint32 magic; ds >> magic;
        if (magic != kAnnounceMagic) continue;

        quint16 idLen; ds >> idLen;
        // Fixed header consumed: 4 (magic) + 2 (idLen) = 6 bytes.
        // After ID we need 2 more bytes for wsPort.
        constexpr int kFixedHdr = 6, kAfterIdMin = 2, kMaxIdLen = 256;
        if (idLen > kMaxIdLen
            || static_cast<int>(idLen) > raw.size() - kFixedHdr - kAfterIdMin) continue;
        QByteArray idBytes(idLen, Qt::Uninitialized);
        ds.readRawData(idBytes.data(), idLen);
        const QString peerId = QString::fromUtf8(idBytes);
        if (peerId == m_selfId.value) continue; // our own announce

        quint16 wsPort; ds >> wsPort;
        // qint64 ts; ds >> ts; // timestamp (unused for now)

        // Use the IPv6 link-local source address
        // Strip IPv6 zone ID (e.g. "fe80::1%eth0" → "fe80::1").
        // Qt's URL parser doesn't handle zone IDs and causes connect failures.
        QString host = dg.senderAddress().toString();
        const int zoneIdx = static_cast<int>(host.indexOf(QLatin1Char('%')));
        if (zoneIdx != -1) host = host.left(zoneIdx);
        qCInfo(lcDirect) << "IPv6 peer discovered:" << peerId
                         << "@" << host << ":" << wsPort;
        emit peerDiscovered(Core::UserId{ peerId },
                            Core::PeerAddress{ host, wsPort });
    }
}

// ─── LAN probe ────────────────────────────────────────────────────────────────

void DirectDiscovery::onLanProbeTimer()
{
    probeLastKnownAddrs();
}

void DirectDiscovery::probeLastKnownAddrs()
{
    if (m_knownPeers.isEmpty()) return;

    // Build set of our current subnets
    QList<QPair<QHostAddress, int>> ourSubnets;
    for (const QNetworkInterface &ni : QNetworkInterface::allInterfaces()) {
        if (!ni.flags().testFlag(QNetworkInterface::IsUp)) continue;
        if (ni.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
        for (const QNetworkAddressEntry &e : ni.addressEntries()) {
            if (e.ip().protocol() == QAbstractSocket::IPv4Protocol
                && !e.ip().isLoopback())
                ourSubnets.append({ e.ip(), e.prefixLength() });
            if (e.ip().protocol() == QAbstractSocket::IPv6Protocol
                && e.ip().isLinkLocal())
                ourSubnets.append({ e.ip(), 64 });
        }
    }

    int emitted = 0;
    const qint64 now = QDateTime::currentMSecsSinceEpoch();

    for (const KnownPeer &peer : m_knownPeers) {
        if (peer.addr.host.isEmpty()) continue;
        // Skip if seen very recently (avoid hammering active sessions)
        if (now - peer.lastSeenMs < 30000) continue;

        const QHostAddress ha(peer.addr.host);

        // Is the cached addr on one of our current subnets?
        bool onOurSubnet = false;
        for (const auto &[ip, prefix] : ourSubnets) {
            if (ha.isInSubnet(ip, prefix)) { onOurSubnet = true; break; }
        }

        if (onOurSubnet) {
            // Direct LAN address — try immediately
            qCInfo(lcDirect) << "LAN probe: trying" << peer.id.value
                             << "@" << peer.addr.toString();
            emit peerDiscovered(peer.id, peer.addr);
            ++emitted;
        } else {
            // Peer is not on our current subnet — skip. Their cached addr
            // is on a different network and won't be routable. DHT handles
            // cross-network discovery; no point in probing gateway addrs.
            qCDebug(lcDirect) << "Peer" << peer.id.value
                              << "not on our subnet — skipping LAN probe.";
        }
    }
    qCDebug(lcDirect) << "LAN probe emitted" << emitted << "direct addr(s)";
}

// ─── Peer-table gossip ────────────────────────────────────────────────────────

void DirectDiscovery::onPeerConnected(const Core::UserId &peerId,
                                       NetworkManager    *net)
{
    // Build peer-table frame from our known peers (exclude the peer itself)
    QList<KnownPeer> toShare;
    for (const KnownPeer &p : m_knownPeers) {
        if (p.id.value == peerId.value) continue;
        if (p.addr.host.isEmpty()) continue;
        toShare.append(p);
        if (toShare.size() >= 20) break; // cap at 20 entries
    }
    if (toShare.isEmpty()) return;

    QByteArray frame;
    QDataStream ds(&frame, QIODeviceBase::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kPtblMagic << static_cast<quint16>(toShare.size());
    for (const KnownPeer &p : toShare) {
        const QByteArray idB   = p.id.value.toUtf8();
        const QByteArray hostB = p.addr.host.toUtf8();
        ds << static_cast<quint16>(idB.size());
        ds.writeRawData(idB.constData(), idB.size());
        ds << static_cast<quint16>(hostB.size());
        ds.writeRawData(hostB.constData(), hostB.size());
        ds << p.addr.port
           << static_cast<qint64>(p.lastSeenMs);
    }

    net->sendToPeer(peerId, frame);
    qCInfo(lcDirect) << "Sent peer table (" << toShare.size()
                     << "entries) to" << peerId.value;
}

// ─── Peer-table inbound parsing (called by AppController) ────────────────────
// AppController::handleInboundPtbl should call this when it detects kPtblMagic.

} // namespace SnapClone::Network
