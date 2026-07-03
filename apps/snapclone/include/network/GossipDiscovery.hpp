#pragma once
#include "core/Timings.hpp"

// ─── Network/GossipDiscovery.hpp ─────────────────────────────────────────────
//
// Gossip-protocol-based peer discovery.
//
// Algorithm overview (Epidemic/SWIM-lite):
//   1. On startup, every node sends a JOIN broadcast on the local subnet UDP
//      multicast group (224.0.0.251:9877).
//   2. Each node maintains a partial view (PeerView) — a random subset of
//      known live peers, size O(log N).
//   3. Every kGossipIntervalMs milliseconds a node picks kFanout random peers
//      from its view and pushes its own record + a random subset of its view.
//   4. A peer that is not seen for kFailureTimeoutMs is presumed dead and
//      removed from the view; a peerLost() signal is emitted.
//
// Security note: gossip messages are signed with the sender's identity key
// (delegated to CryptoService) so nodes cannot impersonate each other.
//
// This implementation is intentionally UDP-only and requires no central
// coordinator — it is purely distributed.
// ─────────────────────────────────────────────────────────────────────────────

#include "network/NetworkManager.hpp"
#include <QTimer>
#include <QUdpSocket>
#include <QDateTime>
#include <QHash>

namespace SnapClone::Network {

class GossipDiscovery : public IDiscovery
{
    Q_OBJECT

public:
    explicit GossipDiscovery(quint16 listenPort = 9877, QObject *parent = nullptr);

    void start()  override;
    void stop()   override;
    void announcePresence(const Core::UserId &self, const Core::PeerAddress &addr) override;
    void lookupPeer(const Core::UserId &id) override;

private slots:
    void onGossipTick();
    void onDatagramReady();

private:
    // ── Wire format ──────────────────────────────────────────────────────
    // Gossip messages are compact JSON:
    //   { "type": "JOIN"|"PING"|"VIEW",
    //     "id": "<userId>",
    //     "host": "<ip>",
    //     "port": <n>,
    //     "peers": [ {"id":…,"host":…,"port":…}, … ] }
    // In production this would be protobuf for efficiency.

    struct PeerRecord {
        Core::UserId    id;
        Core::PeerAddress address;
        QDateTime       lastSeen;
    };

    void processDatagram(const QByteArray &data, const QString &senderHost);
    void gossipToRandomPeers();
    void pruneDeadPeers();

    QByteArray buildGossipPayload(const QString &type) const;

    QUdpSocket              m_socket;
    QTimer                  m_gossipTimer;
    QTimer                  m_pruneTimer;
    QHash<QString, PeerRecord> m_view;      // keyed by UserId.value
    int                        m_emptyTicks = 0; // backoff counter when view is empty
    Core::UserId            m_selfId;
    Core::PeerAddress       m_selfAddress;
    quint16                 m_listenPort;

    static constexpr int  kGossipIntervalMs  = Timings::kGossipTickMs;
    static constexpr int  kPruneIntervalMs   = Timings::kGossipPruneMs;
    // Peers are pruned after kPruneIntervalMs × kPruneMultiplier of silence.
    // 8s × 4 = 32s gives enough time for temporary network glitches to recover.
    static constexpr int  kPruneMultiplier   = 4;
    static constexpr int  kFailureTimeoutMs  = Timings::kGossipFailureMs;
    static constexpr int  kFanout           = 3;
    static constexpr int  kMaxViewSize       = 20;
};

} // namespace SnapClone::Network
