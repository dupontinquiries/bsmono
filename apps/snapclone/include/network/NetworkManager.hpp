#pragma once

// ─── Network/NetworkManager.hpp ──────────────────────────────────────────────
//
// NetworkManager coordinates all distributed networking:
//
//   • Peer discovery via the pluggable IDiscovery interface.
//   • Inbound WebSocket server (QWebSocketServer) for accepting connections.
//   • Outbound WebSocket connections to discovered peers.
//   • Reliable delivery with per-peer pending queues and exponential back-off.
//   • Resolves actual LAN IP before advertising in gossip (not "0.0.0.0").
//
// Threading: runs entirely on the main event loop.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <climits>
class QWebSocketServer; // forward decl — only used as pointer member

#include <memory>
#include <unordered_map>
#include "core/Types.hpp"

namespace SnapClone::Core { class AppContextProvider; }

namespace SnapClone::Network {

// ─── Pluggable Discovery Interface ───────────────────────────────────────────

class IDiscovery : public QObject
{
    Q_OBJECT
public:
    explicit IDiscovery(QObject *parent = nullptr) : QObject(parent) {}
    ~IDiscovery() override = default;

    virtual void start()  = 0;
    virtual void stop()   = 0;
    virtual void announcePresence(const Core::UserId &self, const Core::PeerAddress &addr) = 0;
    virtual void lookupPeer(const Core::UserId &id) = 0;

    /// V32.45: optional backup-replica lookup. Default no-op; only DhtDiscovery
    /// implements it (queries a second DHT key in a different neighbourhood,
    /// populated as a short burst when a peer changes networks). AppController
    /// calls this only as a relookup escalation, after the primary path failed.
    virtual void lookupPeerReplica(const Core::UserId &/*id*/) {}
    /// Publish our STUN punch port so DHT peers can punch to correct address.
    virtual void setPunchPort(quint16 /*punchPort*/) {}
    /// Set the STUN reflexive host (may differ from ipify on CGNAT).
    virtual void setStunHost(const QString &/*stunHost*/) {}

signals:
    void peerDiscovered(SnapClone::Core::UserId id, SnapClone::Core::PeerAddress address);
    void networkChanged(); ///< Emitted when local network interfaces change.
    void peerLost(SnapClone::Core::UserId id);
};

// ─── Pluggable Transport Interface ───────────────────────────────────────────

/// Static descriptor of a transport's operational characteristics.
///
/// Future-proofing for community-led mesh transports (LoRa over USB-C, packet
/// radio, sneakernet, mesh wifi). The router can use this to:
///   • Choose smaller payload chunking when bandwidth is tight
///   • Defer non-essential traffic (presence beacons, address announcements)
///     when on a high-latency path
///   • Skip features that don't fit (e.g. don't send a 5MB video over LoRa)
///   • Bias snap-routing toward higher-bandwidth transports when available
///
/// Defaults model "fast internet" — the existing WebSocket/relay transports
/// don't need to override anything to keep their current behavior.
struct TransportCapabilities {
    /// Approximate sustained bandwidth in bits per second. INT_MAX means
    /// "effectively unlimited" — internet-class. Treat anything <100 kbit/s
    /// as "constrained" and adapt UX (compress more aggressively, show
    /// "low bandwidth" indicator, etc).
    int    bandwidthBps    = INT_MAX;

    /// Typical round-trip latency in milliseconds. <500 ms is "interactive";
    /// >2000 ms is "store-and-forward" and the UI should set expectations
    /// (e.g. "queued, may take a while" rather than spinner).
    int    typicalLatencyMs = 100;

    /// Maximum payload size accepted by the transport per send() call.
    /// 0 = unlimited (router takes care of chunking). LoRa: ~256 bytes.
    /// Sneakernet over QR codes: ~2 KiB.
    int    maxPayloadBytes  = 0;

    /// True if the transport can deliver bytes at any time without the peer
    /// being live-connected (mailbox / store-and-forward / DHT). False for
    /// circuit transports (WebSocket, direct TCP).
    bool   storeAndForward  = false;

    /// True if connecting incurs cost the user might care about — radio
    /// bands shared with neighbors, satellite minutes, cellular data on a
    /// metered plan. UI may want a "use cheap transport" checkbox.
    bool   meteredOrShared  = false;

    /// Free-form short identifier for logs/UI: "websocket", "lora-usb-c",
    /// "magic-wormhole", "jami". Used for "delivered via X" hints.
    QString backendName;
};

class ITransport : public QObject
{
    Q_OBJECT
public:
    explicit ITransport(QObject *parent = nullptr) : QObject(parent) {}
    ~ITransport() override = default;

    virtual void connectToPeer(const Core::PeerAddress &address) = 0;
    virtual void disconnect()  = 0;
    virtual void send(const QByteArray &payload) = 0;
    [[nodiscard]] virtual bool isConnected() const = 0;

    /// Default implementation returns internet-class capabilities. Override
    /// in LoRa / packet-radio transports to declare actual constraints.
    /// Router consumes this to chunk, defer, or reroute as appropriate.
    [[nodiscard]] virtual TransportCapabilities capabilities() const {
        TransportCapabilities caps;
        caps.backendName = QStringLiteral("unspecified");
        return caps;
    }

signals:
    void connected();
    void disconnected();
    void messageReceived(QByteArray payload);
    void errorOccurred(QString message);
};

// ─── NetworkManager ──────────────────────────────────────────────────────────

class NetworkManager : public QObject
{
    Q_OBJECT

public:
    explicit NetworkManager(std::unique_ptr<IDiscovery> discovery, QObject *parent = nullptr);
    ~NetworkManager() override;

    void start(const Core::UserId &selfId, quint16 listenPort = 9876);
    // Returns the port the WebSocket server actually bound (may differ from
    // listenPort if that port was already in use).
    quint16 actualListenPort() const;
    void stop();

    [[nodiscard]] int  connectedPeerCount()  const;
    /// Returns true if we have an active outbound connection to this peer
    /// (i.e. we initiated the WebSocket, not them).
    [[nodiscard]] bool hasOutboundSession(const Core::UserId &id) const;
    // True if peer is connected via ANY transport (inbound or outbound).
    [[nodiscard]] bool hasAnyLiveSession(const Core::UserId &id) const;
    // True only when FULLY ESTABLISHED (not just connecting).
    [[nodiscard]] bool hasEstablishedSession(const Core::UserId &id) const;
    // After 60s of stable connection: close redundant parallel sessions,
    // keeping only the one with lowest latency (most recently active).
    void consolidateSessions(const Core::UserId &id);

public slots:
    void sendToPeer(SnapClone::Core::UserId recipientId, QByteArray payload);
    void connectToPeer(SnapClone::Core::UserId id, SnapClone::Core::PeerAddress address);
    void disconnectPeer(SnapClone::Core::UserId id);
    /// After the handshake reveals the real peer ID, move the live inbound
    /// transport from inboundId's session into realId's session so that
    /// sendToPeer(realId) can deliver over the already-connected socket.
    void promoteInboundSession(const Core::UserId &realId,
                                const Core::UserId &inboundId);
    /// Called when any discovery backend (gossip or Jami) finds a peer.
    /// Public so secondary backends can connect to it directly.
    void onPeerDiscovered(SnapClone::Core::UserId id, SnapClone::Core::PeerAddress address);

    // Suppress onPeerDiscovered-triggered connections for this peer for
    // durationMs. Call when AppController is managing the retry loop so
    // NetworkManager's discovery path doesn't race against it.
    void suppressDiscovery(const Core::UserId &id, int durationMs = 30000);
    // Extend the active transport connect-timeout for this peer by 8 seconds.
    // Call when hole punch completes so TCP has time to use the open NAT hole.
    void notifyPunchReady(const Core::UserId &peerId);
    void injectPayload(const Core::UserId &sender, const QByteArray &payload);

    // Set our own public IP so onPeerDiscovered can skip hairpin addresses.
    void setSelfPublicIp(const QString &ip);
    // Re-announce gossip presence when our LAN IP changes (network switch).
    void updateSelfAddress(const Core::UserId &selfId,
                           const Core::PeerAddress &newAddr);
    /// Adopt a relay transport as the session transport for peerId.
    void adoptRelayTransport(const Core::UserId &peerId, ITransport *relay);

    /// V32.27: optionally attach an AppContextProvider so the LAN-
    /// isolation threshold can be context-aware (drops to 1 attempt
    /// on cellular networks where LAN literally cannot work — see
    /// `effectiveIsolationThreshold` in the cpp). Non-owning; safe
    /// to call with nullptr to disable adaptation.
    void setAppContext(Core::AppContextProvider *ctx) { m_appContext = ctx; }

signals:
    void payloadReceived(SnapClone::Core::UserId senderId, QByteArray payload);
    void peerConnected(SnapClone::Core::UserId id);
    void peerDisconnected(SnapClone::Core::UserId id);
    /// V32.35+: forwards a peer address learned from discovery (gossip/DHT) so
    /// DirectDiscovery can populate its LAN-probe table. Emitted for EVERY
    /// discovered address (before connection rate-limiting), because even a
    /// rate-limited address is still a valid LAN candidate worth remembering.
    void lanPeerAddressLearned(SnapClone::Core::UserId id,
                               SnapClone::Core::PeerAddress address);
    // Emitted when a peer disconnects and we have no outbound address to retry.
    // AppController should attempt a DHT lookup to find their new endpoint.
    void peerNeedsRelookup(SnapClone::Core::UserId id);
    /// Emitted when TCP send fails — AppController can try UDP punch channel.
    void udpSendNeeded(SnapClone::Core::UserId peerId, QByteArray payload);
    /// Emitted when sendToPeer has no active session and must queue. Asks the
    /// controller to ACTIVELY establish a connection to this peer (LAN-cached
    /// address, DHT lookup, punch) rather than waiting passively for the
    /// discovery layer to happen to open a session. Without this, a snap routed
    /// to a known-but-unconnected peer queues forever if gossip discovery never
    /// fires on this network (observed: same-WiFi peers, correct addresses, but
    /// JOINs apparently dropped by the AP → no peerDiscovered → no connect).
    void connectionNeeded(SnapClone::Core::UserId peerId);
    void networkError(QString message);

private slots:
    void onInboundConnection();

private:
    void onTransportDisconnected(const Core::UserId &id, quint64 epoch);

    struct PeerSession {
        Core::PeerAddress           address;
        std::unique_ptr<ITransport> transport;
        bool                        connected  = false;
        bool                        connecting = false;
        QList<QByteArray>           pendingQueue;  // outbound messages buffered before connect
        QList<Core::PeerAddress>    pendingAddrs;  // addrs to try after current attempt fails
        int                         retryCount = 0;
        // Monotonically increasing epoch — captured by every async lambda.
        // Stale callbacks self-discard if the epoch no longer matches.
        quint64                     epoch      = 0;
        // When the most recent connect attempt began (epoch ms). 0 if never
        // tried. Used by NetworkStats to record connect latency on success.
        qint64                      connectStartedMs = 0;
    };

    PeerSession &ensureSession(const Core::UserId &id);

    std::unique_ptr<IDiscovery>                                    m_discovery;
    QWebSocketServer                                               *m_server = nullptr;
    std::unordered_map<std::string, std::unique_ptr<PeerSession>>  m_sessions;
    Core::UserId                                                   m_selfId;

    // ── LAN isolation tracker ──────────────────────────────────────────────
    // Cafe/airport/hotel/public-wifi APs often have CLIENT ISOLATION enabled
    // — multicast (beacons) leaks through but unicast between clients does
    // not. Result: we hear the peer's LAN beacon, we try to connect to their
    // RFC1918 IP, the WebSocket waits 12s for SYN-ACK that never comes,
    // and we waste 12s before falling back to the Magic Wormhole relay.
    //
    // We track per-peer failed LAN attempts. After kIsolatedLanThreshold
    // consecutive failures to a given peer's RFC1918 IP this session, we
    // mark that peer's LAN as isolated and reject further LAN attempts
    // for them immediately — pushing the AppController straight to the
    // relay path. The flag clears on any successful LAN connection.
    //
    // Why per-peer (not global): peer A might be on the same isolated AP
    // as us but peer B might be on a different network entirely (visible
    // only via relay), so a global "wifi is isolated" flag would falsely
    // skip B's relay-routed RFC1918 IP. Tracking per peer is conservative.
    static constexpr int kIsolatedLanThreshold = 3;
    std::unordered_map<std::string, int> m_lanFailureCount; // per-peer count
    std::unordered_map<std::string, qint64> m_lanFailureLastMs; // per-peer last-fail time
    // Session-global counter-evidence: once ANY RFC1918 address connects, AP
    // client isolation is provably OFF, so the per-peer LAN-isolation skip must
    // not fire for anyone. A stale per-peer count is also decayed after a while
    // (a peer that wasn't ready, or just moved onto this LAN, shouldn't be
    // permanently relegated to relay).
    bool m_lanProvenThisSession = false;
    static constexpr qint64 kLanFailureDecayMs = 45000; // forget LAN failures after 45 s

    // V32.27: non-owning pointer to app-state provider. Used in
    // connectToPeer's LAN-isolation gate to lower the threshold to
    // 1 attempt when the OS reports we're on cellular (LAN can't
    // possibly work, no reason to give it two attempts). Nullptr-
    // safe: the gate falls back to kIsolatedLanThreshold when not
    // attached.
    Core::AppContextProvider *m_appContext = nullptr;

    // One attempt per address — AppController address queue handles cycling.
    // Keeping retries at 1 means each address gets a single 3s attempt
    // then peerNeedsRelookup fires immediately so the next addr is tried.
    static constexpr int kMaxRetries  = 2;   // 3 total attempts before DHT relookup
    static constexpr int kRetryBaseMs = 200; // first retry fast, then exponential backoff
};

} // namespace SnapClone::Network
