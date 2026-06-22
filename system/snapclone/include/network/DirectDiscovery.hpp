#pragma once
// ─── DirectDiscovery.hpp ──────────────────────────────────────────────────────
// Zero-infrastructure peer discovery that runs in parallel with DHT.
// Uses multiple strategies, fastest one wins:
//
//   1. Last-known-IP cache — direct TCP connect to last seen address.
//      Works instantly if peer's IP hasn't changed (same network, stable DHCP).
//
//   2. LAN probe — tries cached IPs on current subnets immediately at startup.
//      Finds peers on the same network (home, office) without mDNS multicast.
//
//   3. IPv6 link-local — announces and discovers via ff02::1 multicast.
//      Works even when IPv4 is broken. Separate from gossip on port 9877.
//
//   4. Peer table gossip — after connecting to any peer, exchange known-peer
//      tables. Friends-of-friends discovery without any central infrastructure.
//      Format: PTBL frame containing up to 20 {scId, host, port, lastSeen} entries.
//
// All strategies are additive — they emit peerDiscovered signals that the
// AppController feeds into the existing address queue alongside DHT results.
// ─────────────────────────────────────────────────────────────────────────────
#pragma once
#include "core/Types.hpp"
#include "network/NetworkManager.hpp"
#include <QObject>
#include <QUdpSocket>
#include <QTimer>
#include <QDateTime>

namespace SnapClone::Network {

struct KnownPeer {
    Core::UserId      id;
    Core::PeerAddress addr;
    qint64            lastSeenMs = 0; // msec since epoch
};

class DirectDiscovery : public QObject
{
    Q_OBJECT
public:
    explicit DirectDiscovery(QObject *parent = nullptr);
    ~DirectDiscovery() override;

    /// Start all discovery strategies.
    /// selfId: our sc-ID. wsPort: our WS listen port. knownPeers: contacts list.
    void start(const Core::UserId &selfId, quint16 wsPort,
               const QList<KnownPeer> &knownPeers);
    void stop();

    /// Call when we connect to a peer — triggers peer-table exchange.
    void onPeerConnected(const Core::UserId &peerId,
                         NetworkManager    *net);

    /// Update our known-peer table (called when storage has new addrs).
    void updateKnownPeer(const Core::UserId &id,
                         const Core::PeerAddress &addr);

signals:
    /// Peer found — feed into AppController address queue (same as DHT).
    void peerDiscovered(Core::UserId peerId, Core::PeerAddress addr);

private slots:
    void onIpv6ReadyRead();
    void onLanProbeTimer();

private:
    void sendIpv6Announce();
    void probeLastKnownAddrs();
    void tryDirectConnect(const KnownPeer &peer);

    // Peer-table exchange frame magic
    static constexpr quint32 kPtblMagic = 0x5054424C; // "PTBL"

    Core::UserId         m_selfId;
    quint16              m_wsPort = 9876;
    QList<KnownPeer>     m_knownPeers;

    // IPv6 link-local multicast socket (separate from IPv4 gossip)
    QUdpSocket           m_ip6Socket;
    static constexpr quint16 kIp6Port = 9878; // distinct from gossip port 9877

    // LAN probe timer — fires shortly after start to attempt direct connects
    QTimer               m_lanProbeTimer;
    // Re-announce timer — periodic IPv6 announce + LAN probe refresh
    QTimer               m_announceTimer;
};

} // namespace SnapClone::Network
