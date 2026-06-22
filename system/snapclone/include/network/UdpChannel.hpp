#pragma once
// ─── UdpChannel.hpp ───────────────────────────────────────────────────────────
// Reliable-ish UDP data channel for sending snaps through NAT pinholes.
//
// Works in parallel with TCP WebSocket: if TCP can't connect (CGNAT), this
// channel routes data through the UDP pinhole opened by hole punch.
//
// Design:
//   • Separate QUdpSocket (not the punch socket) bound with ShareAddress.
//     Packets from the same local addr to the same remote addr reuse the NAT
//     pinhole opened by the punch socket — no extra port needed.
//   • Chunked transfer: payloads split into ≤48KB datagrams with sequence numbers.
//   • Per-chunk ACK: sender retransmits any chunk not ACKed within 1.5s.
//   • Keepalive: sends a tiny heartbeat every 20s to keep NAT mapping alive.
//   • CONNECT/CONNECT_ACK handshake: announces our sc-ID to the peer so they
//     can route incoming data correctly without a TCP handshake.
//
// Frame wire format (binary, all big-endian):
//   [4] magic    0x53 0x43 0x55 0x44  ("SCUD")
//   [1] type     0x01=CHUNK 0x02=ACK 0x03=KEEPALIVE 0x04=CONNECT 0x05=CONNECT_ACK
//   [2] session  random uint16, identifies one transfer
//   [4] seq      chunk index (0-based) or ack'd chunk index
//   [4] total    total chunk count for this session (0 for non-data frames)
//   [2] id_len   byte length of sender sc-ID string
//   [N] id       sender's sc-ID (e.g. "sc-09b33530")
//   [payload]    chunk data (CHUNK frames only)
// ─────────────────────────────────────────────────────────────────────────────
#pragma once
#include "core/Types.hpp"
#include <QObject>
#include <QTimer>
#include <QHash>
#include <QSet>
#include <QByteArray>
#include <QHostAddress>
#include <functional>

namespace SnapClone::Core { class AppContextProvider; }

namespace SnapClone::Network {

class UdpChannel : public QObject
{
    Q_OBJECT
public:
    explicit UdpChannel(QObject *parent = nullptr);
    ~UdpChannel() override;

    /// Set our identity and write function (uses punch socket, no own bind).
    using WriteFn = std::function<void(const QByteArray&, const QString&, quint16)>;
    void start(const Core::UserId &selfId, WriteFn writeFn);

    /// Called by NatTraversal when a SCUD frame arrives on the punch socket.
    void onDatagram(const QByteArray &data,
                    const QString    &senderIp,
                    quint16           senderPort);
    void stop();

    /// Open a channel to a remote peer after hole punch succeeds.
    /// Sends CONNECT frame and starts keepalive.
    void connectToPeer(const Core::UserId &peerId,
                       const Core::PeerAddress &peerAddr);

    /// Send a payload to a connected peer. Splits into chunks automatically.
    /// Returns false if peer not connected.
    bool sendPayload(const Core::UserId &peerId, const QByteArray &payload);

    bool isConnected(const Core::UserId &peerId) const;

    /// V32.28: optionally attach an AppContextProvider so the
    /// retransmit timer can scale its aggression with app state.
    /// When `context.prefersConservative()` and the user isn't
    /// actively waiting, retransmits skip every other tick (effective
    /// 3s instead of 1.5s) — saves cellular bytes while pocketed.
    /// userJustActed pushes back to full cadence.
    /// Nullptr-safe — channel uses fixed cadence when not attached.
    void setAppContext(Core::AppContextProvider *ctx) { m_appContext = ctx; }

signals:
    /// Emitted when a complete reassembled payload arrives from a peer.
    void payloadReceived(Core::UserId peerId, QByteArray payload);
    /// Emitted when the UDP channel to a peer becomes active (CONNECT_ACK received).
    void channelReady(Core::UserId peerId, Core::PeerAddress peerAddr);
    /// Emitted when no keepalive response received for 60s.
    void channelLost(Core::UserId peerId);
    /// Emitted when CONNECT handshake exceeds kMaxConnectAttempts with no reply.
    /// AppController should fall back to relay transport.
    void channelFailed(Core::UserId peerId);

private slots:
    void onRetransmitTimer();
    void onKeepaliveTimer();

private:
    static constexpr int    kMagic         = 0x53435544; // "SCUD"
    static constexpr int    kChunkSize     = 1100;       // ≤ IPv4 MTU (avoids fragmentation)
    static constexpr int    kWindowSize    = 16;         // chunks in flight before waiting for ACK
    static constexpr int    kAckTimeoutMs  = 1500;       // retransmit after 1.5s
    static constexpr int    kMaxRetransmit = 12;         // per-chunk retransmit limit
    static constexpr int    kKeepaliveMs   = 20000;      // keepalive every 20s
    static constexpr int    kDeadMs        = 60000;      // declare dead after 60s

    enum FrameType : quint8 {
        CHUNK       = 0x01,
        ACK         = 0x02,
        KEEPALIVE   = 0x03,
        CONNECT     = 0x04,
        CONNECT_ACK = 0x05,
    };

    // ── Explicit per-peer phase (V30 — FSM defense-in-depth) ───────────────
    // Adds an explicit state machine on top of the implicit `connected`
    // boolean. Used by the FSM validator to flag misaligned frame types
    // (e.g. ACK before any CONNECT, CHUNK from a closed peer).
    //
    // Legal transitions:
    //   Idle        → Connecting    (we sent CONNECT)
    //   Idle        → Established   (received CONNECT — inbound case)
    //   Connecting  → Established   (received CONNECT_ACK or CONNECT)
    //   Established → Established   (data flow — CHUNK, ACK, KEEPALIVE)
    //   any         → Idle          (dead-peer prune, explicit reset)
    enum class Phase : quint8 {
        Idle        = 0,  ///< Never seen this peer or peer pruned for inactivity
        Connecting  = 1,  ///< We sent CONNECT, awaiting CONNECT_ACK
        Established = 2,  ///< Handshake complete; data flows allowed
    };

    /// FSM legality check for inbound frames. Returns true if a frame of
    /// `type` is plausible given the peer's current `phase`. Caller drops
    /// + logs on false return.
    [[nodiscard]] static constexpr bool isLegalInbound(Phase phase, FrameType type) noexcept
    {
        switch (phase) {
        case Phase::Idle:
            // From Idle, only CONNECT can legitimately arrive (a peer is
            // greeting us). All other frame types imply we lost state but
            // they didn't — drop and force re-handshake.
            return type == CONNECT;
        case Phase::Connecting:
            // We sent CONNECT and are waiting. Legal responses: CONNECT_ACK
            // (peer ack'd us), or CONNECT (peer also initiated — simultaneous
            // open; we'll converge). KEEPALIVE accepted as graceful tolerance.
            return type == CONNECT_ACK || type == CONNECT
                || type == KEEPALIVE;
        case Phase::Established:
            // All five types legal — full data flow.
            return true;
        }
        return false;
    }

    [[nodiscard]] static constexpr const char *phaseName(Phase p) noexcept
    {
        switch (p) {
            case Phase::Idle:        return "Idle";
            case Phase::Connecting:  return "Connecting";
            case Phase::Established: return "Established";
        }
        return "?";
    }

    [[nodiscard]] static constexpr const char *frameTypeName(FrameType t) noexcept
    {
        switch (t) {
            case CHUNK:       return "CHUNK";
            case ACK:         return "ACK";
            case KEEPALIVE:   return "KEEPALIVE";
            case CONNECT:     return "CONNECT";
            case CONNECT_ACK: return "CONNECT_ACK";
        }
        return "?";
    }

    struct PeerState {
        Core::PeerAddress addr;
        bool              connected = false;
        Phase             phase     = Phase::Idle;  ///< FSM state (V30+)
        qint64            lastSeen  = 0;   // msec since epoch
        int               connectAttempts = 0; // retransmit count for CONNECT handshake

        // Outbound transfer in progress
        QByteArray        sendPayload;
        quint16           sendSession  = 0;
        int               sendTotal    = 0;
        int               sendNext     = 0;      // next chunk to send
        QSet<int>         ackedChunks;
        QHash<int,int>    chunkRetransmits; // per-chunk retransmit count
        int               windowBase   = 0; // lowest unACKed chunk

        // Inbound reassembly
        quint16           recvSession     = 0;
        int               recvTotal       = 0;
        qint64            recvLastChunkMs = 0; // timestamp of last chunk received
        QHash<int,QByteArray> recvChunks;
    };

    QByteArray buildFrame(FrameType type, quint16 session,
                          quint32 seq, quint32 total,
                          const QByteArray &payload = {}) const;
    void sendFrame(const PeerState &peer, FrameType type,
                   quint16 session, quint32 seq, quint32 total,
                   const QByteArray &payload = {});
    void sendNextChunk(const QString &peerId, PeerState &peer);
    void handleChunk(const QString &senderId, PeerState &peer,
                     quint16 session, quint32 seq, quint32 total,
                     const QByteArray &payload);
    void handleAck  (const QString &peerId, PeerState &peer,
                     quint16 session, quint32 seq);

    Core::UserId                m_selfId;
    WriteFn                     m_writeFn;
    QTimer                      m_retransmitTimer;
    QTimer                      m_keepaliveTimer;
    QHash<QString, PeerState>   m_peers; // keyed by peerId.value
    // Reverse lookup: "ip:port" → peerId for routing inbound packets
    QHash<QString, QString>     m_addrToId;

    // V32.28: non-owning pointer to app-state provider. Used in
    // onRetransmitTimer to decide whether to skip a tick (conservative
    // mode + user idle) — effectively halves retransmit cadence to
    // save cellular bytes when the user isn't watching. Nullptr-safe.
    Core::AppContextProvider   *m_appContext = nullptr;
    // Tick counter for the skip-every-other strategy. Incremented at
    // every onRetransmitTimer entry; conservative+idle path only acts
    // on even ticks.
    quint32                     m_retransmitTickCount = 0;
};

} // namespace SnapClone::Network
