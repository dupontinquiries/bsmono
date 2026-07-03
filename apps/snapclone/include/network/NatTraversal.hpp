#pragma once
// ─── NatTraversal.hpp ─────────────────────────────────────────────────────────
//
// STUN discovery + UDP hole punching for cross-network connectivity.
//
// Critical design: STUN is performed FROM the punch socket (not a separate
// ephemeral socket). This ensures the reflexive address we publish to the DHT
// matches the source address peers receive our punch packets from. Previously
// STUN used its own ephemeral port, so the published address didn't match
// the punch traffic — causing all hole punches to miss.
//
// Usage:
//   1. nat->start()            — binds punch socket, runs STUN
//   2. reflexiveAddressReady() — AppController publishes to DHT
//   3. nat->initiateHolePunch(peerId, peerReflexiveAddr)
//   4. holePunchReady()        — AppController calls connectToPeer
// ─────────────────────────────────────────────────────────────────────────────

#include "core/Types.hpp"
#include <functional>
#include <QObject>
#include <QUdpSocket>
#include <QTimer>
#include <QHostAddress>
#include <QVector>
class QTcpSocket; // forward decl — only used as pointer member
#include <QPair>
#include <optional>

namespace SnapClone::Core { class AppContextProvider; }

namespace SnapClone::Network {

// STUN servers — FOSS-preferred, with hardcoded IP fallbacks for broken DNS.
// IPs verified 2026-04. stun.jami.net shares infra with bootstrap.jami.net.
static constexpr struct { const char *host; quint16 port; } kStunServers[] = {
    // Jami/SFL — try hostname first, then raw IPs (bypasses broken system DNS)
    { "stun.jami.net",         3478 },
    { "158.69.23.138",         3478 }, // stun.jami.net IP-A (no DNS needed)
    { "144.217.10.53",         3478 }, // stun.jami.net IP-B (no DNS needed)
    { "stun.jami.net",           80 }, // port 80 for hotspot UDP-3478 blocks
    { "158.69.23.138",           80 }, // same, no DNS
    // Other FOSS servers
    { "stun.nextcloud.com",    3478 },
    { "stun.linphone.org",     3478 },
    { "stun.stunprotocol.org", 3478 },
    { "stun.framasoft.org",    3478 },
    { "stun.ekiga.net",        3478 },
};

// TURN relay fallback — only used for symmetric NAT where hole punch fails.
// openrelay.metered.ca is a free community relay; Jami also runs relay nodes.
static constexpr struct {
    const char *host; quint16 port;
    const char *user; const char *pass;
} kTurnServers[] = {
    { "turn.jami.net",        3478, "",                 ""                },
    { "openrelay.metered.ca", 80,   "openrelayproject", "openrelayproject" },
};

class NatTraversal : public QObject
{
    Q_OBJECT
public:
    explicit NatTraversal(QObject *parent = nullptr);
    ~NatTraversal() override;

    void start();
    void recordPeerActivity(); ///< restart CGNAT keepalive during peer connect
    void restartStunOnly(); ///< Retry STUN without closing punch socket
    void setSelfId(const Core::UserId &id) { m_selfId = id; }
    void stop();

    // Load STUN server list from QSettings bitmask.
    // Call before start(). If not called, uses all FOSS defaults.
    void loadStunServersFromSettings();

    void initiateHolePunch(const Core::UserId &peerId,
                           const Core::PeerAddress &peerReflexiveAddr);
    void stopHolePunch(const Core::UserId &peerId);

    Core::PeerAddress reflexiveAddress() const;
    QString turnRelayUrl(const Core::UserId &peerId) const;

    // The local UDP port the punch socket is bound to.
    // AppController uses this for diagnostics / logging.
    quint16 punchLocalPort() const { return m_punchSocket.localPort(); }

    /// Route inbound SCUD frames to UdpChannel via callback.
    /// Called once UdpChannel is ready so NatTraversal can forward to it.
    using UdpDatagramCallback = std::function<void(const QByteArray&,
                                                    const QString& /*senderIp*/,
                                                    quint16 /*senderPort*/)>;
    void setUdpDatagramCallback(UdpDatagramCallback cb) { m_udpCb = std::move(cb); }

    /// Write a datagram through the punch socket (so it uses the mapped port).
    void writeDatagram(const QByteArray &data, const QString &host, quint16 port);

    /// Send raw payload to peerAddr over the punch socket (post-hole-punch).
    void sendUdpData(const Core::PeerAddress &peerAddr, const QByteArray &data);

    /// Register a punched peer so UDP data from their addr is attributed correctly.
    void registerPunchedPeer(const Core::UserId &peerId,
                              const Core::PeerAddress &peerAddr);

    /// V32.21: optionally attach an AppContextProvider so the punch loop
    /// can adapt its aggression to current app state (idle+cellular →
    /// shorter burst; userJustActed → full burst). Pointer is non-owning;
    /// safe to call with nullptr to disable adaptation. If not called, the
    /// punch loop runs with fixed defaults (current behaviour).
    void setAppContext(Core::AppContextProvider *ctx) { m_appContext = ctx; }

signals:
    void reflexiveAddressReady(Core::PeerAddress addr);
    void holePunchReady(Core::UserId peerId, Core::PeerAddress peerAddr);
    void holePunchFailed(Core::UserId peerId);
    /// Emitted each time a punch packet arrives from a peer.
    /// seenAddr is the actual source addr as seen by our socket — the peer's
    /// real CGNAT UDP address. Used for peer-assisted STUN: we report this
    /// back to the peer via DHT mailbox so they learn their own reflexive addr.
    void punchSeen(Core::UserId peerId, Core::PeerAddress seenAddr);
    /// Emitted when cross-server STUN confirms symmetric NAT —
    /// hole punch impossible, relay required for all peers.
    void symmetricNatDetected();
    /// Fired when a UDP data payload arrives from a peer after hole punch.
    void udpDataReceived(Core::UserId peerId, QByteArray payload);

private slots:
    void onStunTimeout();
    void onPunchReadyRead();

private:
    void tryNextStunServer();
    void startTcpStunParallel(); // parallel TCP STUN for UDP-blocked networks
    void sendStunBindingRequest();
    void parseMappedAddress(const QByteArray &response);

    // Punch socket — used for BOTH STUN and hole punch traffic.
    // Binding to 9876 with ShareAddress lets TCP also bind that port.
    QUdpSocket  m_punchSocket;

    Core::UserId m_selfId;
    // V32.21: non-owning pointer to the app-state provider. Used in
    // initiateHolePunch() to scale punch volley aggression. Nullptr-safe;
    // the punch path falls back to fixed defaults if not set.
    Core::AppContextProvider *m_appContext = nullptr;
    // STUN state
    QTimer      m_stunTimer;
    // TCP STUN fallback — used when all UDP STUN servers fail
    QTcpSocket *m_tcpStun = nullptr;
    QByteArray  m_tcpStunBuf;
    // Active STUN server list — built from settings at start()
    QVector<QPair<QString, quint16>> m_stunServers;
    int         m_stunServerIndex        = 0;
    int         m_consecutiveDnsFailures = 0;
    bool        m_stunDone        = false;
    qint64      m_lastPeerActivityMs = 0;
    QHostAddress m_stunTarget;
    quint16      m_stunTargetPort  = 0;
    QByteArray         m_stunTxId;         // current 12-byte transaction ID
    QList<QByteArray>  m_recentStunTxIds;  // recent TxIds — accepts late responses after restart

    // Punch state
    QTimer      m_punchTimer;
    struct PunchAttempt {
        Core::UserId      peerId;
        Core::PeerAddress addr;
        int               sentCount    = 0;
        bool              readyEmitted = false; // holePunchReady fires only once
    };
    std::optional<PunchAttempt> m_punchAttempt;
    // Maps sender IP:port → peer UserId for routing UDP data packets.
    QHash<QString, Core::UserId> m_udpPeers;
    QSet<QString>               m_readyEmitted; // dedup: only emit holePunchReady once per peer per session
    UdpDatagramCallback          m_udpCb;   // forwards SCUD frames to UdpChannel

    Core::PeerAddress m_reflexive;      // our discovered reflexive addr
    // CGNAT keepalive: periodic UDP to STUN server keeps punch port mapping alive.
    QTimer       m_keepaliveTimer;
    QHostAddress m_keepaliveHost;
    quint16      m_keepalivePort = 0;

    // Port stability verification
    QString  m_firstReflexiveHost;
    bool     m_symmetricCheckDone = false; // true after cross-server NAT type check
    quint16  m_firstReflexivePort = 0;
    bool     m_stunVerified   = false;  // second STUN query sent
    bool     m_portPreserving = false;  // NAT preserves ports
};

} // namespace SnapClone::Network
