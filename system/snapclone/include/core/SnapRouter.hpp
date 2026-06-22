#pragma once

// ─── Core/SnapRouter.hpp ──────────────────────────────────────────────────────
//
// Wire protocol v3 — unified state sync (patch layer over v1/v2)
//
// All communication is expressed as two primitives:
//
//   ANNOUNCE  "Here is what I have / know about."
//   DATA      "Here are bytes (full or delta)."
//
// Every message carries flags instead of explicit types:
//
//   F_ANNOUNCE   metadata only (snapId, hash, sender, timestamp)
//   F_DATA       includes full payload bytes
//   F_DELTA      payload is a binary patch vs baseHash (future)
//   F_ACK        piggyback acknowledgment (highest contiguous seq received)
//   F_SYNC       state reconciliation — list of known hashes + since timestamp
//   F_HINT       predictive hint — receiver should pre-allocate / pre-fetch
//
// Key behaviours:
//
//   Predictive sending:
//     Sender checks if the peer likely has the blob (just connected / known hash).
//     If unlikely → send ANNOUNCE|DATA immediately (no round-trip).
//     If likely   → send ANNOUNCE only, wait for implicit ack in next message.
//
//   Piggybacked ACKs:
//     Every outgoing packet carries ack = highestContiguousSeqReceived.
//     No standalone ACK message needed.
//
//   Sync = same primitive:
//     ANNOUNCE|SYNC carries a SyncState payload (since + knownHashes).
//     Receiver replies with ANNOUNCE|SYNC listing their own state.
//     Missing snaps are inferred — no NEED/HAVE round-trip.
//
// Compatibility:
//   v1 and v2 remain fully supported.
//   v3 is used when both peers advertise it via the initial handshake.
//   Fallback to v2 is automatic.
//
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QHash>
#include <QSet>
#include "core/Types.hpp"
#include "core/EncodeTier.hpp"

namespace SnapClone::Network { class NetworkManager; }
namespace SnapClone::Network { class UdpChannel; }
namespace SnapClone::Storage { class StorageManager; }
namespace SnapClone::Crypto  { class ICryptoService; }

namespace SnapClone::Core {

class SnapRouter : public QObject
{
    Q_OBJECT

public:
    explicit SnapRouter(
        Network::NetworkManager *network,
        Storage::StorageManager *storage,
        Crypto::ICryptoService  *crypto,
        QObject                 *parent = nullptr);

    void routeOutbound(const Snap &plainSnap);
    void routeInbound(const QByteArray &encryptedPayload, const UserId &senderId);
    void setSelfUserId(const UserId &id);
    void setUdpChannel(Network::UdpChannel *ch); // implemented in SnapRouter.cpp

    /// Call when a peer (re)connects — triggers v3 ANNOUNCE|SYNC or v2 SYNC_REQ.
    void onPeerConnected(const UserId &peerId);

    /// Mark a peer as v3-capable (set from handshake negotiation).
    void setPeerVersion(const UserId &peerId, quint8 version);
    void clearPeerState(const UserId &id);

signals:
    void outboundDelivered(SnapClone::Core::SnapId id);
    void inboundReady(SnapClone::Core::Snap snap);
    void routingFailed(SnapClone::Core::SnapId id, QString reason);
    /// Emitted when snap is saved to outbox but peer is offline (no session key).
    /// AppController should trigger a reconnect; SYNC will deliver on connection.
    void snapQueuedForOfflinePeer(SnapClone::Core::SnapId id,
                                  SnapClone::Core::UserId recipientId);

private:
    /// Recently-emitted inbound snap IDs (oldest first).
    /// Used to suppress UI duplicates when peer re-sends a snap on
    /// reconnect (peer hadn't received our piggyback ACK yet).
    /// Storage handles persistent dedup via INSERT OR REPLACE; this
    /// cache prevents the extra inboundReady emission per session.
    /// Bounded to last 256 IDs; older entries fall off when full.
    QList<QString> m_recentInbound;
    static constexpr int kInboundDedupCacheMax = 256;
    [[nodiscard]] bool isRecentInbound(const SnapId &id) const;
    void rememberInbound(const SnapId &id);
    // ── v2 sub-types (legacy) ─────────────────────────────────────────────
    enum class MsgType : quint8 {
        Snap     = 0,
        Offer    = 1,
        Have     = 2,
        Need     = 3,
        Ack      = 4,
        SyncReq  = 5,
        SyncList = 6,
    };

    // ── v3 flags ──────────────────────────────────────────────────────────
    enum V3Flag : quint8 {
        F_ANNOUNCE = 1 << 0,
        F_DATA     = 1 << 1,
        F_DELTA    = 1 << 2,
        F_ACK      = 1 << 3,
        F_SYNC     = 1 << 4,
        F_HINT     = 1 << 5,
    };

    // ── v2 handlers (unchanged) ───────────────────────────────────────────
    void handleV2Message(const QByteArray &plain, MsgType type,
                         const UserId &senderId);
    void handleInboundSnap(const QByteArray &plain, const UserId &senderId);
    void handleHaveOrNeed(const QString &snapId, bool recipientHas,
                          const UserId &recipient);

    // ── v3 handlers ───────────────────────────────────────────────────────
    void handleV3Message(const QByteArray &plain, const UserId &senderId);
    void handleV3Announce(const QByteArray &plain, quint8 flags,
                          quint32 peerSeq, quint32 peerAck,
                          const UserId &senderId);
    void handleV3Sync(QDataStream &ds, const UserId &senderId); // Bug 6 fix

    // ── v3/v4/v5 senders ──────────────────────────────────────────────────
    // videoDurationMs added in v4 (carries playback duration for video snaps);
    // tier added in v5 (identifies media-quality tier for tiered delivery).
    // Older receivers ignore unknown fields via version-gated reads.
    void sendV3(const UserId &peerId, quint8 flags,
             const QString &snapId, const QString &contentHash,
             const QByteArray &payload = {},
             quint8 mediaType = 0, quint16 durationSecs = 5,
             qint32 videoDurationMs = 0,
             Core::Media::EncodeTier tier = Core::Media::EncodeTier::Standard);
    void sendV3Sync(const UserId &peerId);

    /// True if this peer has negotiated v3.
    bool isV3Peer(const UserId &id) const;

    /// Decide whether to send DATA immediately or just ANNOUNCE.
    /// Heuristic: send data if peer just connected or hash is not in our
    /// record of their known hashes.
    bool shouldSendDataImmediately(const UserId &peerId,
                                    const QString &contentHash) const;

    // ── Shared helpers ────────────────────────────────────────────────────
    bool encryptAndSend(const UserId &peerId, const QByteArray &plain);

    Network::NetworkManager *m_network;
    Storage::StorageManager *m_storage;
    Crypto::ICryptoService  *m_crypto;
    Network::UdpChannel     *m_udpChannel = nullptr; // optional, for UDP snap delivery
    UserId                   m_selfUserId;

    // ── v2 state ──────────────────────────────────────────────────────────
    struct PendingOffer {
        UserId     recipient;
        QString    contentHash;
        QByteArray mediaBlob;
    };
    QHash<QString, PendingOffer> m_pendingOffers; // snapId → offer

    // ── v3 state ──────────────────────────────────────────────────────────
    QSet<QString>               m_v3Peers;         // peers that speak v3
    QHash<QString, quint32>     m_peerSeq;          // last seq we sent per peer
    // Bug 5 fix: pending v3 snaps waiting for piggybacked ACK before marking delivered
    QHash<QString, QMap<quint32, QString>> m_v3PendingAck; // peerId → {seq → snapId}
    QSet<QString>               m_syncRepliedTo;    // peers we've already replied with sync this session
    QHash<QString, quint32>     m_peerAckRecv;      // highest ack received from peer
    QHash<QString, QSet<QString>> m_peerKnownHashes; // hashes peer has confirmed
    // V32.34+: per-peer SYNC dedup window. Phone A logs showed identical
    // SYNC requests arriving within ~5ms, each triggering a fresh re-send
    // of every undelivered snap. Likely caused by some upstream path
    // firing the SYNC twice (TCP retransmit + relay echo, or duplicate
    // socket-ready signals). The dedup is a per-peer "last SYNC handled
    // at" timestamp — if a new SYNC arrives within kSyncDedupMs of the
    // previous one, we drop it silently. Separate from path-convergence
    // work because this is local-buffer dedup, not transport selection.
    QHash<QString, qint64>      m_lastSyncHandledMs; // peerId → epoch ms of last accepted SYNC
};

} // namespace SnapClone::Core
