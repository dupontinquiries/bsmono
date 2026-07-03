#include "network/PollinatorDiscovery.hpp"
#include <QUdpSocket>
#include <QTimer>
#include <QNetworkDatagram>
#include <QNetworkInterface>
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcPollinator, "snapclone.pollinator")

namespace SnapClone::Network {

static constexpr const char *kMulticastAddr = "224.0.0.251";
static constexpr quint16     kMulticastPort = 9879;
static constexpr int         kBeaconIntervalMs = 2000;
static constexpr int         kMaxDisplayNameLen = 64;

struct PollinatorDiscovery::Impl {
    QUdpSocket  socket;
    QTimer      timer;
    QString     selfId;
    QString     selfDisplayName;
    bool        active = false;
};

PollinatorDiscovery::PollinatorDiscovery(QObject *parent)
    : QObject(parent), d(std::make_unique<Impl>())
{
    connect(&d->timer, &QTimer::timeout, this, &PollinatorDiscovery::sendBeacon);
    d->timer.setInterval(kBeaconIntervalMs);

    connect(&d->socket, &QUdpSocket::readyRead,
            this, &PollinatorDiscovery::onReadyRead);
}

PollinatorDiscovery::~PollinatorDiscovery()
{
    stop();
}

void PollinatorDiscovery::setSelfId(const QString &id, const QString &displayName)
{
    d->selfId          = id;
    d->selfDisplayName = displayName.left(kMaxDisplayNameLen);
}

void PollinatorDiscovery::start()
{
    if (d->active) return;
    if (d->selfId.isEmpty()) {
        qCWarning(lcPollinator) << "Cannot start — selfId not set";
        return;
    }

    // Bind to multicast port (reuse address so multiple apps can listen)
    if (!d->socket.bind(QHostAddress::AnyIPv4, kMulticastPort,
                        QUdpSocket::ShareAddress | QUdpSocket::ReuseAddressHint)) {
        qCWarning(lcPollinator) << "Failed to bind multicast socket:" << d->socket.errorString();
        return;
    }

    if (!d->socket.joinMulticastGroup(QHostAddress(QLatin1String(kMulticastAddr)))) {
        qCWarning(lcPollinator) << "Failed to join multicast group:" << d->socket.errorString();
        d->socket.close();
        return;
    }

    // Enable broadcast — we send the same beacon to UDP broadcast addresses
    // in addition to multicast. On consumer wifi (Bell, Comcast, ATT routers,
    // hotel APs) IGMP-snooping is often misconfigured and multicast packets
    // are dropped intermittently or entirely. Broadcast (255.255.255.255 and
    // subnet-directed broadcast like 192.168.2.255) uses a different routing
    // mechanism that's much more reliable on consumer hardware.
    d->socket.setSocketOption(QAbstractSocket::MulticastTtlOption, 1);

    d->active = true;
    qCInfo(lcPollinator) << "Beacon started — broadcasting" << d->selfId
                          << "on" << kMulticastAddr << ":" << kMulticastPort;

    sendBeacon(); // immediate first broadcast
    d->timer.start();
}

void PollinatorDiscovery::stop()
{
    if (!d->active) return;
    d->active = false;
    d->timer.stop();
    d->socket.leaveMulticastGroup(QHostAddress(QLatin1String(kMulticastAddr)));
    d->socket.close();
    qCInfo(lcPollinator) << "Beacon stopped";
}

void PollinatorDiscovery::sendBeacon()
{
    if (!d->active || d->selfId.isEmpty()) return;

    // "SNAPCLONE_BEACON\nsc-ID\ndisplayName\n"
    const QByteArray payload = QStringLiteral("SNAPCLONE_BEACON\n%1\n%2\n")
        .arg(d->selfId, d->selfDisplayName).toUtf8();

    // 1. Multicast — preferred path, works on Linux desktop and well-configured
    //    networks. Single packet that all subscribers receive.
    d->socket.writeDatagram(payload,
        QHostAddress(QLatin1String(kMulticastAddr)), kMulticastPort);

    // 2. UDP broadcast fallback — every interface's subnet-directed broadcast
    //    AND the global limited broadcast 255.255.255.255. Consumer wifi
    //    (Bell, etc.) tends to drop multicast due to IGMP-snooping misconfig
    //    but pass broadcast reliably. Most desktop firewalls also pass
    //    broadcast more readily than multicast since it's the older
    //    mechanism (think DHCP, NetBIOS).
    //
    //    Both addresses get the same payload — receivers filter duplicates
    //    by sc-ID. The cost is ~1 extra packet per interface per 2 seconds.
    int broadcastCount = 0;
    for (const QNetworkInterface &iface : QNetworkInterface::allInterfaces()) {
        if (!iface.flags().testFlag(QNetworkInterface::IsUp)) continue;
        if (iface.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
        if (!iface.flags().testFlag(QNetworkInterface::IsRunning)) continue;
        for (const QNetworkAddressEntry &entry : iface.addressEntries()) {
            if (entry.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
            if (entry.ip().isLoopback()) continue;
            const QHostAddress bcast = entry.broadcast();
            if (bcast.isNull()) continue;
            d->socket.writeDatagram(payload, bcast, kMulticastPort);
            ++broadcastCount;
        }
    }
    // Limited broadcast — last-resort. Some kernels reject this if no
    // broadcast permission has been granted on the socket, so this is
    // best-effort. Errors are silent.
    d->socket.writeDatagram(payload,
        QHostAddress::Broadcast, kMulticastPort);

    qCDebug(lcPollinator) << "Beacon sent" << d->selfId
                          << "(multicast + broadcast on" << broadcastCount
                          << "ifaces, " << payload.size() << "bytes)";
}

void PollinatorDiscovery::onReadyRead()
{
    while (d->socket.hasPendingDatagrams()) {
        const QNetworkDatagram dg = d->socket.receiveDatagram(512);
        if (!dg.isValid()) continue;

        const QString raw = QString::fromUtf8(dg.data());
        const QStringList parts = raw.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
        if (parts.size() < 2 || parts[0] != QLatin1String("SNAPCLONE_BEACON"))
            continue;

        const QString peerId   = parts[1];
        const QString peerName = parts.size() >= 3 ? parts[2] : peerId;

        // Ignore our own beacon
        if (peerId == d->selfId) continue;
        if (!peerId.startsWith(QLatin1String("sc-"))) continue;

                const QString senderAddr = dg.senderAddress().toString();
        qCInfo(lcPollinator) << "Peer beaconed:" << peerId
                              << "@ LAN addr" << senderAddr;
        emit peerBeaconed(Core::UserId{ peerId }, peerName, senderAddr);
    }
}

} // namespace SnapClone::Network
