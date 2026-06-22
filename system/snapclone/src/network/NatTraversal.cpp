// ─── NatTraversal.cpp ─────────────────────────────────────────────────────────
//
// Correct cross-network traversal strategy:
//
//   Step 1 — STUN on the PUNCH socket
//     Bind m_punchSocket to port 9876 (ShareAddress so TCP can also use it).
//     Send a STUN binding request FROM m_punchSocket to Google STUN.
//     The STUN response gives us our TRUE reflexive address for that socket:
//     the IP:port the NAT assigned for UDP from port 9876.
//     This is the address we announce to the DHT as our punch endpoint.
//     It is NOT the same as the ephemeral STUN socket port used previously.
//
//   Step 2 — DHT announces punch endpoint
//     AppController publishes: publicIP (from ipify) + punch port (from STUN).
//     Peers look up our sc-ID and get both the TCP endpoint and punch port.
//
//   Step 3 — Simultaneous UDP hole punching
//     Both peers send UDP packets to each other's punch endpoints.
//     Most NATs (full-cone, restricted-cone, port-restricted) allow the reply
//     after seeing the outbound packet. This opens a bidirectional pinhole.
//
//   Step 4 — TCP connect after punch
//     After punching, TCP connect to peerPublicIP:9876.
//     For port-preserving NATs (most home routers), the TCP SYN now passes
//     because the NAT already has a mapping for that addr:port pair from UDP.
//     For non-port-preserving NATs we may need the punched UDP port for TCP too —
//     but that requires SO_REUSEPORT TCP which Qt doesn't expose. TURN is the
//     fallback for those (symmetric NAT / carrier-grade NAT).
//
//   Timing:
//     Simultaneous punch requires coordination. We use the DHT itself:
//     when we publish our punch endpoint we also read the peer's; both apps
//     start punching as soon as they have both endpoints. The 2-second window
//     (8 × 250ms packets) gives enough overlap even with a few seconds of
//     DHT propagation delay.
// ─────────────────────────────────────────────────────────────────────────────

#include <QTimer>
#include "network/NatTraversal.hpp"
#include "core/Timings.hpp"
#include "core/AppContext.hpp"
#include <QTcpSocket>  // needed for m_tcpStun (fwd-decl in header)
#include <QLoggingCategory>
#include <QNetworkDatagram>
#include <QRandomGenerator>
#include <QtEndian>  // qToBigEndian — was only resolving transitively, broke on some Qt builds
#include <QHostInfo>
#include <QSettings>
#include <optional>

Q_LOGGING_CATEGORY(lcNat, "snapclone.nat")

namespace SnapClone::Network {

// ─── Constants ────────────────────────────────────────────────────────────────

static constexpr quint32 kStunMagicCookie      = 0x2112A442;
static constexpr quint16 kStunBindingRequest   = 0x0001;
static constexpr quint16 kStunBindingResponse  = 0x0101;
static constexpr quint16 kAttrMappedAddress    = 0x0001;
static constexpr quint16 kAttrXorMappedAddress = 0x0020;
static constexpr int     kStunTimeoutMs        = Timings::kStunTimeoutMs;
static constexpr int     kPunchIntervalMs      = Timings::kPunchBurstIntervalMs;
static constexpr int     kPunchCount           = Timings::kPunchBurstCount;

// ─── Construction ─────────────────────────────────────────────────────────────

NatTraversal::NatTraversal(QObject *parent)
    : QObject(parent)
{
    connect(&m_stunTimer, &QTimer::timeout,
            this, &NatTraversal::onStunTimeout);
    m_stunTimer.setSingleShot(true);
    m_keepaliveTimer.setInterval(Timings::kCgnatKeepaliveMs);
    connect(&m_keepaliveTimer, &QTimer::timeout, this, [this]() {
        if (m_keepaliveHost.isNull() || m_keepalivePort == 0) return;
        const char zero = 0;
        m_punchSocket.writeDatagram(&zero, 1, m_keepaliveHost, m_keepalivePort);
        qCDebug(lcNat) << "CGNAT keepalive sent";
    });

    connect(&m_punchSocket, &QUdpSocket::readyRead,
            this, &NatTraversal::onPunchReadyRead);
}

NatTraversal::~NatTraversal() { stop(); }

void NatTraversal::stop()
{
    m_stunTimer.stop();
    m_punchTimer.stop();
    m_keepaliveTimer.stop();
    if (m_punchSocket.state() != QAbstractSocket::UnconnectedState)
        m_punchSocket.close();
    // Reset all STUN state so start() re-runs discovery from scratch.
    // Critical: without this, network-change → stop() → start() just
    // re-emits the stale old reflexive address without contacting STUN.
    m_stunDone          = false;
    m_stunVerified      = false;
    m_stunServerIndex   = 0;
    m_firstReflexivePort = 0;
    m_firstReflexiveHost.clear();
    m_reflexive         = {};
    m_punchAttempt.reset();
    m_keepaliveHost     = QHostAddress();
    m_keepalivePort     = 0;
    if (m_tcpStun) {
        // V32.19: clear member FIRST, then operate on local copy. abort()
        // emits errorOccurred synchronously; the errorOccurred slot does
        // `if (m_tcpStun) { ... m_tcpStun->deleteLater(); m_tcpStun = nullptr; }`
        // which nulls the member. The next line `m_tcpStun->deleteLater()`
        // would then segfault. By caching + clearing first, both the slot's
        // null-check and our cleanup see consistent state.
        auto *t = m_tcpStun;
        m_tcpStun = nullptr;
        t->disconnect(); // suppress any signals during/after abort
        t->abort();
        t->deleteLater();
    }
    m_tcpStunBuf.clear();
    m_symmetricCheckDone = false;
}

Core::PeerAddress NatTraversal::reflexiveAddress() const { return m_reflexive; }

// ─── Step 1: STUN on the punch socket ────────────────────────────────────────


void NatTraversal::loadStunServersFromSettings()
{
    // All known STUN servers with fallback ports.
    // jami.net also responds on port 80 — important for hotspots that block 3478.
    struct ServerDef { const char *host; quint16 port; int bit; };
    static const ServerDef kAll[] = {
        // Jami — hostname + hardcoded IPs + port 80/443
        { "stun.jami.net",         3478, 0 },
        { "stun.jami.net",           80, 0 },
        { "158.69.23.138",         3478, 0 }, // stun.jami.net IP-A
        { "144.217.10.53",         3478, 0 }, // stun.jami.net IP-B
        { "158.69.23.138",           80, 0 }, // port 80, no DNS
        // Google — almost universally reachable
        { "stun.l.google.com",    19302, 5 },
        { "74.125.197.127",       19302, 5 }, // stun.l.google.com hardcoded IP
        { "stun1.l.google.com",   19302, 5 },
        // Cloudflare — global anycast
        { "stun.cloudflare.com",   3478, 6 },
        { "162.159.128.100",       3478, 6 }, // Cloudflare hardcoded IP
        { "162.159.128.100",        443, 6 }, // Cloudflare port 443
        // Other FOSS
        { "stun.nextcloud.com",    3478, 1 },
        { "stun.linphone.org",     3478, 2 },
        { "stun.linphone.org",       80, 2 }, // port 80 fallback
        { "stun.ekiga.net",        3478, 3 },
        { "stun.framasoft.org",    3478, 4 },
    };

    QSettings s;
    // Default mask: ALL servers enabled (bits 0-6 = 0b1111111 = 127)
    const int mask = s.value(
        QStringLiteral("network/stunServers"),
        0b1111111).toInt();

    m_stunServers.clear();
    for (const auto &srv : kAll) {
        if (mask & (1 << srv.bit))
            m_stunServers.append({ QString::fromLatin1(srv.host), srv.port });
    }

    // Always ensure at least one server is available
    if (m_stunServers.isEmpty()) {
        m_stunServers.append({ QStringLiteral("stun.jami.net"),  3478 });
        m_stunServers.append({ QStringLiteral("stun.jami.net"),    80 });
    }

    qCInfo(lcNat) << "STUN server list loaded:" << m_stunServers.size() << "entries";
}

void NatTraversal::start()
{
    if (m_stunDone) {
        emit reflexiveAddressReady(m_reflexive);
        return;
    }

    // Bind the punch socket first. We do STUN FROM this socket so the
    // reflexive address we discover is the one that punch packets come from.
    // ShareAddress allows TCP to also bind port 9876 simultaneously.
    if (m_punchSocket.state() != QAbstractSocket::BoundState) {
        const bool bound9876 = m_punchSocket.bind(
            QHostAddress::AnyIPv4, 9876,
            QUdpSocket::ShareAddress | QUdpSocket::ReuseAddressHint);
        if (!bound9876) {
            // 9876 busy — try 9877, then let OS pick
            const bool bound9877 = m_punchSocket.bind(
                QHostAddress::AnyIPv4, 9877,
                QUdpSocket::ShareAddress | QUdpSocket::ReuseAddressHint);
            if (!bound9877)
                m_punchSocket.bind(QHostAddress::AnyIPv4, 0);
        }
        qCInfo(lcNat) << "Punch socket bound to local port"
                      << m_punchSocket.localPort();
    }

    if (m_stunServers.isEmpty())
        loadStunServersFromSettings();
    m_stunServerIndex = 0;
    tryNextStunServer();

    // Start TCP STUN in parallel after a short delay.
    // On networks where UDP STUN is blocked (e.g. iPhone hotspot, some
    // corporate WiFi), TCP STUN gives us the public IP for DHT announcement
    // much faster than waiting for all UDP servers to time out.
    // TCP result has port=0 (can't determine UDP punch port over TCP),
    // but the IP is still valid for DHT and lets the remote peer reach us.
    QTimer::singleShot(5000, this, [this]() {
        if (m_stunDone) return; // UDP STUN already succeeded — nothing to do
        startTcpStunParallel();
    });
}


/// Call when a peer connection attempt starts — restarts the CGNAT
/// keepalive so the NAT mapping stays open during the connection window.
void NatTraversal::recordPeerActivity()
{
    m_lastPeerActivityMs = QDateTime::currentMSecsSinceEpoch();
    if (!m_keepaliveTimer.isActive() && !m_keepaliveHost.isNull())
        m_keepaliveTimer.start();
}

void NatTraversal::restartStunOnly()
{
    // Retry STUN discovery WITHOUT closing the punch socket.
    // This preserves any active CGNAT mapping and hole-punch / SCUD handshake
    // that may be in progress. Only the STUN state is reset.
    m_stunTimer.stop();
    m_stunDone              = false;
    m_stunVerified          = false;
    m_stunServerIndex       = 0;
    m_consecutiveDnsFailures = 0;
    m_firstReflexivePort    = 0;
    m_firstReflexiveHost.clear();
    m_tcpStunBuf.clear(); // discard any partial STUN response from old socket
    // Abort pending TCP STUN socket — its readyRead may still fire via
    // queued events; clearing m_tcpStunBuf above prevents stale parse.
    if (m_tcpStun) {
        // V32.19: same cache-then-clear pattern as the reset() path above.
        // abort() emits errorOccurred synchronously into a slot that nulls
        // m_tcpStun; without caching first we'd segfault on deleteLater().
        auto *t = m_tcpStun;
        m_tcpStun = nullptr;
        t->disconnect();
        t->abort();
        t->deleteLater();
    }
    // Do NOT clear m_reflexive — keep old value until we get a new one.
    // Do NOT close the punch socket. Do NOT stop the punch timer.
    // Do NOT clear m_punchAttempt. Do NOT stop the keepalive timer.
    qCInfo(lcNat) << "STUN-only restart — socket preserved, punch state intact";
    if (m_punchSocket.state() != QAbstractSocket::BoundState) {
        // Socket somehow not bound — rebind before STUN
        m_punchSocket.bind(QHostAddress::AnyIPv4, 9876,
                           QUdpSocket::ShareAddress | QUdpSocket::ReuseAddressHint);
        qCInfo(lcNat) << "Punch socket re-bound to" << m_punchSocket.localPort();
    }
    if (m_stunServers.isEmpty())
        loadStunServersFromSettings();
    m_stunServerIndex = 0;
    tryNextStunServer();

    // Start TCP STUN in parallel after a short delay.
    // On networks where UDP STUN is blocked (e.g. iPhone hotspot, some
    // corporate WiFi), TCP STUN gives us the public IP for DHT announcement
    // much faster than waiting for all UDP servers to time out.
    // TCP result has port=0 (can't determine UDP punch port over TCP),
    // but the IP is still valid for DHT and lets the remote peer reach us.
    QTimer::singleShot(5000, this, [this]() {
        if (m_stunDone) return; // UDP STUN already succeeded — nothing to do
        startTcpStunParallel();
    });
}


void NatTraversal::startTcpStunParallel()
{
    // Run TCP STUN as a parallel path. On networks where UDP is blocked
    // (e.g. iPhone hotspot), this provides the public IP for DHT announcement
    // even without a UDP punch port. If UDP STUN succeeds later, it wins.
    if (m_tcpStun) return; // already running

    qCInfo(lcNat) << "Starting parallel TCP STUN (UDP may be blocked)";

    m_tcpStun = new QTcpSocket(this);
    connect(m_tcpStun, &QTcpSocket::connected, this, [this]() {
        QByteArray req(20, Qt::Uninitialized);
        auto *p16 = reinterpret_cast<quint16*>(req.data());
        auto *p32 = reinterpret_cast<quint32*>(req.data());
        p16[0] = qToBigEndian<quint16>(kStunBindingRequest);
        p16[1] = 0;
        p32[1] = qToBigEndian<quint32>(kStunMagicCookie);
        QRandomGenerator::global()->fillRange(
            reinterpret_cast<quint32*>(req.data() + 8), 3);
        m_tcpStun->write(req);
    });
    connect(m_tcpStun, &QTcpSocket::readyRead, this, [this]() {
        m_tcpStunBuf.append(m_tcpStun->readAll());
        if (m_tcpStunBuf.size() < 20) return;
        const quint16 msgType =
            (static_cast<quint8>(m_tcpStunBuf[0]) << 8) |
             static_cast<quint8>(m_tcpStunBuf[1]);
        const quint16 msgLen =
            (static_cast<quint8>(m_tcpStunBuf[2]) << 8) |
             static_cast<quint8>(m_tcpStunBuf[3]);
        if (m_tcpStunBuf.size() < 20 + msgLen) return;
        if (msgType == kStunBindingResponse) {
            m_tcpStun->close();
            QString host; quint16 port = 0; (void)port; // port unused — TCP gives no punch port
            int pos2 = 20;
            while (pos2 + 4 < m_tcpStunBuf.size()) {
                quint16 aType = (static_cast<quint8>(m_tcpStunBuf[pos2])<<8)
                              | static_cast<quint8>(m_tcpStunBuf[pos2+1]);
                quint16 aLen  = (static_cast<quint8>(m_tcpStunBuf[pos2+2])<<8)
                              | static_cast<quint8>(m_tcpStunBuf[pos2+3]);
                pos2 += 4;
                if ((aType == 0x0001 || aType == 0x0020) && aLen >= 8) {
                    const bool xored = (aType == 0x0020);
                    quint16 rawPort = (static_cast<quint8>(m_tcpStunBuf[pos2+2])<<8)
                                   |  static_cast<quint8>(m_tcpStunBuf[pos2+3]);
                    port = xored ? (rawPort ^ 0x2112u) : rawPort;
                    const quint32 rawIp =
                        (static_cast<quint32>(static_cast<quint8>(m_tcpStunBuf[pos2+4]))<<24)
                       |(static_cast<quint32>(static_cast<quint8>(m_tcpStunBuf[pos2+5]))<<16)
                       |(static_cast<quint32>(static_cast<quint8>(m_tcpStunBuf[pos2+6]))<<8)
                       | static_cast<quint32>(static_cast<quint8>(m_tcpStunBuf[pos2+7]));
                    const quint32 ip = xored ? (rawIp ^ 0x2112A442u) : rawIp;
                    host = QHostAddress(ip).toString();
                    if (xored) break;
                }
                pos2 += aLen + (aLen % 4 ? 4 - aLen % 4 : 0);
            }
            if (!host.isEmpty()) {
                if (!m_stunDone) {
                    qCInfo(lcNat) << "TCP STUN (parallel) reflexive:" << host << ":0"
                                  << "(no UDP punch port — UDP STUN may still succeed)";
                    m_reflexive = { host, 0 };
                    emit reflexiveAddressReady(m_reflexive);
                    // Do NOT set m_stunDone — let UDP STUN continue.
                } else {
                    qCInfo(lcNat) << "TCP STUN (parallel) completed after UDP succeeded"
                                  << "— ignoring (UDP result already in use)";
                }
            }
            // Reset so restartStunOnly() can create a fresh TCP STUN socket.
            if (m_tcpStun) { m_tcpStun->deleteLater(); m_tcpStun = nullptr; }
        }
    });
    connect(m_tcpStun, &QAbstractSocket::errorOccurred,
            this, [this](QAbstractSocket::SocketError) {
        if (m_tcpStun) {
            qCWarning(lcNat) << "TCP STUN (parallel) failed:" << m_tcpStun->errorString();
            m_tcpStun->deleteLater(); m_tcpStun = nullptr;
        }
    });
    QTimer::singleShot(8000, this, [this]() {
        if (m_tcpStun && !m_stunDone) {
            qCWarning(lcNat) << "TCP STUN (parallel) timeout";
            m_tcpStun->deleteLater(); m_tcpStun = nullptr;
        }
    });
    // Connect to Jami STUN by IP (no DNS needed — critical for hotspot DNS issues)
    m_tcpStun->connectToHost(QHostAddress(QStringLiteral("158.69.23.138")), 3478);
    m_tcpStunBuf.clear();
}

void NatTraversal::tryNextStunServer()
{
    if (m_stunServerIndex >= m_stunServers.size()) {
        // All UDP STUN exhausted — try STUN over TCP as last resort.
        // Useful on networks that block UDP entirely (some corporate WiFi).
        if (!m_tcpStun) {
            qCInfo(lcNat) << "UDP STUN exhausted — trying TCP STUN fallback";
            m_tcpStun = new QTcpSocket(this);
            connect(m_tcpStun, &QTcpSocket::connected, this, [this]() {
                // Build STUN Binding Request
                QByteArray req(20, Qt::Uninitialized);
                auto *p16 = reinterpret_cast<quint16*>(req.data());
                auto *p32 = reinterpret_cast<quint32*>(req.data());
                p16[0] = qToBigEndian<quint16>(kStunBindingRequest);
                p16[1] = 0;
                p32[1] = qToBigEndian<quint32>(kStunMagicCookie);
                QRandomGenerator::global()->fillRange(
                    reinterpret_cast<quint32*>(req.data() + 8), 3);
                m_tcpStun->write(req);
            });
            connect(m_tcpStun, &QTcpSocket::readyRead, this, [this]() {
                m_tcpStunBuf.append(m_tcpStun->readAll());
                if (m_tcpStunBuf.size() < 20) return;
                const quint16 msgType =
                    (static_cast<quint8>(m_tcpStunBuf[0]) << 8) |
                     static_cast<quint8>(m_tcpStunBuf[1]);
                const quint16 msgLen =
                    (static_cast<quint8>(m_tcpStunBuf[2]) << 8) |
                     static_cast<quint8>(m_tcpStunBuf[3]);
                if (m_tcpStunBuf.size() < 20 + msgLen) return;
                if (msgType == kStunBindingResponse) {
                    qCInfo(lcNat) << "TCP STUN response received";
                    m_tcpStun->close();
                    if (!m_stunDone) {
                        // Parse XOR-MAPPED-ADDRESS or MAPPED-ADDRESS
                        QString host; quint16 port = 0;
                        int pos2 = 20;
                        while (pos2 + 4 < m_tcpStunBuf.size()) {
                            quint16 aType = (static_cast<quint8>(m_tcpStunBuf[pos2])<<8)
                                          | static_cast<quint8>(m_tcpStunBuf[pos2+1]);
                            quint16 aLen  = (static_cast<quint8>(m_tcpStunBuf[pos2+2])<<8)
                                          | static_cast<quint8>(m_tcpStunBuf[pos2+3]);
                            pos2 += 4;
                            if ((aType == 0x0001 || aType == 0x0020) && aLen >= 8) {
                                // 0x0001 = MAPPED-ADDRESS, 0x0020 = XOR-MAPPED-ADDRESS
                                const bool xored = (aType == 0x0020);
                                quint16 rawPort = (static_cast<quint8>(m_tcpStunBuf[pos2+2])<<8)
                                               |  static_cast<quint8>(m_tcpStunBuf[pos2+3]);
                                port = xored ? (rawPort ^ 0x2112u) : rawPort;
                                const quint32 rawIp =
                                    (static_cast<quint32>(static_cast<quint8>(m_tcpStunBuf[pos2+4]))<<24)
                                   |(static_cast<quint32>(static_cast<quint8>(m_tcpStunBuf[pos2+5]))<<16)
                                   |(static_cast<quint32>(static_cast<quint8>(m_tcpStunBuf[pos2+6]))<<8)
                                   | static_cast<quint32>(static_cast<quint8>(m_tcpStunBuf[pos2+7]));
                                const quint32 ip = xored ? (rawIp ^ 0x2112A442u) : rawIp;
                                host = QHostAddress(ip).toString();
                                if (xored) break; // prefer XOR-MAPPED over MAPPED
                            }
                            pos2 += aLen + (aLen % 4 ? 4 - aLen % 4 : 0);
                        }
                        if (!host.isEmpty()) {
                            qCInfo(lcNat) << "TCP STUN reflexive:" << host << ":" << port;
                            m_stunDone = true;
                            m_reflexive = { host, 0 }; // port 0 = TCP only, no punch
                            if (!m_keepaliveHost.isNull()) m_keepaliveTimer.start();
                            if (m_tcpStun) { m_tcpStun->deleteLater(); m_tcpStun = nullptr; }
                            emit reflexiveAddressReady(m_reflexive);
                        }
                    }
                }
            });
            connect(m_tcpStun,
                    &QAbstractSocket::errorOccurred,
                    this, [this](QAbstractSocket::SocketError) {
                if (!m_tcpStun) return;
                qCWarning(lcNat) << "TCP STUN failed:"
                                 << m_tcpStun->errorString();
                // Don't set m_stunDone — caller may retry on network change.
                m_tcpStun->deleteLater(); m_tcpStun = nullptr;
                emit reflexiveAddressReady(Core::PeerAddress{});
            });
            // 5s timeout
            QTimer::singleShot(5000, this, [this]() {
                if (m_tcpStun && !m_stunDone) {
                    qCWarning(lcNat) << "TCP STUN timeout";
                    m_tcpStun->deleteLater(); m_tcpStun = nullptr;
                    emit reflexiveAddressReady(Core::PeerAddress{});
                }
            });
            // Use Jami STUN server by hardcoded IP (no DNS needed)
            m_tcpStun->connectToHost(QHostAddress(QStringLiteral("158.69.23.138")), 3478);
            m_tcpStunBuf.clear();
            return; // wait for TCP STUN result
        }
        qCWarning(lcNat) << "All STUN servers exhausted — emitting empty reflexive addr.";
        emit reflexiveAddressReady(Core::PeerAddress{});
        return;
    }
    const auto &srv = m_stunServers[m_stunServerIndex];
    qCInfo(lcNat) << "STUN: trying" << srv.first << ":" << srv.second
                  << "from local port" << m_punchSocket.localPort();

    const QString host = srv.first;
    const quint16 port = srv.second;
    QHostInfo::lookupHost(host, this, [this, port](const QHostInfo &info) {
        if (info.error() != QHostInfo::NoError || info.addresses().isEmpty()) {
            qCWarning(lcNat) << "STUN DNS failed:" << info.errorString();
            ++m_stunServerIndex;
            ++m_consecutiveDnsFailures;
            // If DNS keeps failing, skip remaining hostname entries and jump to
            // IP-literal servers — saves 4+ seconds per failed hostname lookup.
            if (m_consecutiveDnsFailures >= 3) {
                while (m_stunServerIndex < m_stunServers.size()) {
                    const QString &h = m_stunServers[m_stunServerIndex].first;
                    if (QHostAddress(h).isNull()) { // still a hostname
                        ++m_stunServerIndex;
                    } else {
                        break; // found an IP literal — try it
                    }
                }
            }
            tryNextStunServer();
            return;
        }
        m_consecutiveDnsFailures = 0; // DNS worked — reset skip counter
        m_stunTarget = info.addresses().first();
        m_stunTargetPort = port;
        sendStunBindingRequest();
        m_stunTimer.start(kStunTimeoutMs);
    });
}

void NatTraversal::sendStunBindingRequest()
{
    // Build STUN Binding Request, sent FROM m_punchSocket.
    QByteArray msg(20, '\0');
    auto w16 = [&](int o, quint16 v) {
        msg[o]   = static_cast<char>(v >> 8);
        msg[o+1] = static_cast<char>(v & 0xff);
    };
    auto w32 = [&](int o, quint32 v) {
        msg[o]   = static_cast<char>(v >> 24);
        msg[o+1] = static_cast<char>((v>>16) & 0xff);
        msg[o+2] = static_cast<char>((v>>8)  & 0xff);
        msg[o+3] = static_cast<char>(v & 0xff);
    };
    w16(0, kStunBindingRequest);
    w16(2, 0);
    w32(4, kStunMagicCookie);

    // Store transaction ID for response matching
    auto rng = QRandomGenerator::securelySeeded();
    for (int i = 8; i < 20; i += 4) {
        const quint32 r = rng.generate();
        msg[i]   = static_cast<char>(r >> 24);
        msg[i+1] = static_cast<char>((r>>16) & 0xff);
        msg[i+2] = static_cast<char>((r>>8)  & 0xff);
        msg[i+3] = static_cast<char>(r & 0xff);
    }
    m_stunTxId = msg.mid(8, 12); // save for response verification
    m_recentStunTxIds.append(m_stunTxId);
    if (m_recentStunTxIds.size() > 5) m_recentStunTxIds.removeFirst();

    // Send from m_punchSocket — this is the critical difference.
    m_punchSocket.writeDatagram(msg, m_stunTarget, m_stunTargetPort);
}

void NatTraversal::onPunchReadyRead()
{
    while (m_punchSocket.hasPendingDatagrams()) {
        const QNetworkDatagram dg = m_punchSocket.receiveDatagram(65535);
        const QByteArray data = dg.data();

        // Log every inbound UDP packet at debug level so we can tell
        // if punch packets are arriving at all (key diagnostic).
        qCDebug(lcNat) << "Punch socket rx" << data.size() << "bytes from"
                       << dg.senderAddress().toString() << ":" << dg.senderPort()
                       << "first4:" << data.left(4).toHex();

        // ── STUN response? ────────────────────────────────────────────────
        if (!m_stunDone && data.size() >= 20) {
            const quint16 msgType = (static_cast<quint8>(data[0]) << 8)
                                  |  static_cast<quint8>(data[1]);
            if (msgType == kStunBindingResponse) {
                // Verify transaction ID matches
                const QByteArray rxTxId = data.mid(8, 12);
                // Accept current TxId OR any recent one (covers responses arriving
                // after restartStunOnly() replaced m_stunTxId with a new value).
                const bool txIdMatch = (rxTxId == m_stunTxId)
                    || m_recentStunTxIds.contains(rxTxId);
                if (txIdMatch) {
                    m_stunTimer.stop();
                    m_keepaliveHost = m_stunTarget;
                    m_keepalivePort = m_stunTargetPort;
                    parseMappedAddress(data);
                    continue;
                }
            }
        }

        // ── SCUD frame (UdpChannel)? Forward via callback. ────────────────
        // UdpChannel shares the punch socket via the callback mechanism.
        // This avoids SO_REUSEPORT delivery ambiguity when two sockets
        // bind the same port — only the punch socket receives everything.
        if (data.size() >= 4
            && static_cast<quint8>(data[0]) == 0x53  // 'S'
            && static_cast<quint8>(data[1]) == 0x43  // 'C'
            && static_cast<quint8>(data[2]) == 0x55  // 'U'
            && static_cast<quint8>(data[3]) == 0x44) // 'D'
        {
            // Drop self-loopback SCUD frames before forwarding. UdpChannel
            // would treat each one as a CONNECT or DATA from a real peer
            // and reply, the kernel routes the reply to localhost, we
            // receive it, forward it, reply again — same self-feedback
            // loop as the punch path. Loopback source = us, always.
            //
            // BUILD-MARKER: SCUD-LOOPBACK-DROP-V2
            const QString senderIp = dg.senderAddress().toString();
            const bool isLoopbackV4 = senderIp == QLatin1String("127.0.0.1")
                                   || senderIp.startsWith(QLatin1String("127."));
            const bool isLoopbackV6 = senderIp == QLatin1String("::1")
                                   || senderIp == QLatin1String("::ffff:127.0.0.1")
                                   || senderIp.startsWith(QLatin1String("::ffff:127."));
            if (isLoopbackV4 || isLoopbackV6) {
                static qint64 s_lastScudWarnMs = 0;
                static int s_droppedSinceWarn = 0;
                ++s_droppedSinceWarn;
                const qint64 now = QDateTime::currentMSecsSinceEpoch();
                if (now - s_lastScudWarnMs > 5000) {
                    s_lastScudWarnMs = now;
                    qCWarning(lcNat) << "SCUD-LOOPBACK-DROP-V2: dropped"
                                     << s_droppedSinceWarn
                                     << "self-loopback SCUD frames in last 5s,"
                                     << "most recent from"
                                     << senderIp << ":" << dg.senderPort();
                    s_droppedSinceWarn = 0;
                }
                continue;
            }
            qCDebug(lcNat) << "SCUD frame from"
                          << dg.senderAddress().toString() << ":"
                          << dg.senderPort()
                          << "size" << data.size()
                          << "udpCb:" << (m_udpCb ? "set" : "NULL");
            if (m_udpCb) {
                m_udpCb(data,
                        dg.senderAddress().toString(),
                        static_cast<quint16>(dg.senderPort()));
            }
            continue;
        }

        // ── Punch packet from peer? ───────────────────────────────────────
        if (data.startsWith("SNAPCLONE_PUNCH_V1")) {
            const QString senderIp   = dg.senderAddress().toString();
            const quint16 senderPort = static_cast<quint16>(dg.senderPort());

            // Extract sender sc-ID from payload (after the newline)
            QString punchSenderId;
            const qsizetype nl = data.indexOf('\n');
            if (nl > 0 && nl + 1 < data.size())
                punchSenderId = QString::fromUtf8(data.mid(nl + 1));

            // ── Self-punch guard ───────────────────────────────────────────
            // Without this, two failure modes spam thousands of packets/sec:
            //
            // 1. Loopback: a peer address resolved to 127.0.0.1 (e.g. our
            //    own LAN address rebroadcast through DHT, or a misconfigured
            //    contact) → we punch localhost:9876 → kernel routes the
            //    datagram back to us → we receive it → we reply → kernel
            //    routes the reply back to us → infinite loop, no
            //    backoff, no end condition.
            //
            // 2. Same sc-ID: our own reflexive STUN address punched to us
            //    (rare but possible during NAT testing) — same loop, same
            //    fix.
            //
            // Drop silently after rate-limited counter logging.
            //
            // BUILD-MARKER: PUNCH-LOOPBACK-DROP-V2
            const bool isLoopbackV4 = senderIp == QLatin1String("127.0.0.1")
                                   || senderIp.startsWith(QLatin1String("127."));
            const bool isLoopbackV6 = senderIp == QLatin1String("::1")
                                   || senderIp == QLatin1String("::ffff:127.0.0.1")
                                   || senderIp.startsWith(QLatin1String("::ffff:127."));
            const bool isLoopback = isLoopbackV4 || isLoopbackV6;
            const bool isSelfId   = !punchSenderId.isEmpty()
                                 && punchSenderId == m_selfId.value;
            if (isLoopback || isSelfId) {
                static qint64 s_lastSelfWarnMs = 0;
                static int s_punchDroppedSinceWarn = 0;
                ++s_punchDroppedSinceWarn;
                const qint64 now = QDateTime::currentMSecsSinceEpoch();
                if (now - s_lastSelfWarnMs > 5000) {
                    s_lastSelfWarnMs = now;
                    qCWarning(lcNat) << "PUNCH-LOOPBACK-DROP-V2: dropped"
                                     << s_punchDroppedSinceWarn
                                     << "self-originated punches in last 5s,"
                                     << "most recent from"
                                     << senderIp << ":" << senderPort
                                     << "id=" << punchSenderId
                                     << "(loopback=" << isLoopback
                                     << "selfId=" << isSelfId << ")";
                    s_punchDroppedSinceWarn = 0;
                }
                continue;
            }

            qCDebug(lcNat) << "Received punch from" << senderIp << ":" << senderPort
                          << (punchSenderId.isEmpty() ? "" : " id=" + punchSenderId)
                          << "— pinhole open, sending reply";

            // Reply with our own sc-ID so the sender can confirm us
            QByteArray reply("SNAPCLONE_PUNCH_V1\n");
            reply.append(m_selfId.value.toUtf8());
            m_punchSocket.writeDatagram(reply, dg.senderAddress(), senderPort);

            if (m_punchAttempt) {
                // Update addr with actual source in case NAT remapped port
                m_punchAttempt->addr = { senderIp, senderPort };
                m_punchTimer.stop();
                const auto attempt = *m_punchAttempt;
                m_punchAttempt.reset();
                qCInfo(lcNat) << "Hole punch confirmed for" << attempt.peerId.value;
                registerPunchedPeer(attempt.peerId, attempt.addr);
                // Report the peer's actual CGNAT UDP addr back to AppController.
                // AppController will relay this to the peer via DHT mailbox (peer-assisted STUN).
                emit punchSeen(attempt.peerId, attempt.addr);
                emit holePunchReady(attempt.peerId, attempt.addr);
            }                 else {
                // Our punch volleys are done but peer punch just arrived.
                // If sc-ID is missing from payload, resolve by sender IP.
                if (punchSenderId.isEmpty()) {
                    const QString addrKey = senderIp + QLatin1Char(':') + QString::number(senderPort);
                    if (m_udpPeers.contains(addrKey))
                        punchSenderId = m_udpPeers[addrKey].value;
                    if (punchSenderId.isEmpty()) {
                        for (auto it = m_udpPeers.begin(); it != m_udpPeers.end(); ++it) {
                            if (it.key().startsWith(senderIp + QLatin1Char(':'))) {
                                punchSenderId = it.value().value; break;
                            }
                        }
                    }
                }
                if (!punchSenderId.isEmpty()) {
                    const Core::UserId senderId{ punchSenderId };
                    const Core::PeerAddress confirmedAddr{ senderIp, senderPort };
                    registerPunchedPeer(senderId, confirmedAddr);
                    // If maintenance punch is running for THIS peer but to a different
                    // addr, update it so we now punch to the actual CGNAT source.
                    if (m_punchAttempt
                        && m_punchAttempt->peerId.value == punchSenderId
                        && (m_punchAttempt->addr.host != senderIp
                            || m_punchAttempt->addr.port != senderPort)) {
                        qCInfo(lcNat) << "Updating punch target to actual CGNAT source:"
                                      << senderIp << ":" << senderPort;
                        m_punchAttempt->addr = confirmedAddr;
                        m_punchAttempt->readyEmitted = false;
                    }
                    // Guard: only emit holePunchReady ONCE per peer per session.
                    // Maintenance punches (every 8s) and burst re-punches would
                    // otherwise re-fire holePunchReady on every packet, causing
                    // UdpChannel to call connectToPeer hundreds of times.
                    if (!m_readyEmitted.contains(punchSenderId)) {
                        m_readyEmitted.insert(punchSenderId);
                        qCInfo(lcNat) << "Late punch from" << punchSenderId
                                      << "— confirming pinhole" << senderIp << ":" << senderPort;
                        emit punchSeen(senderId, confirmedAddr);
                        emit holePunchReady(senderId, confirmedAddr);
                    } else {
                        qCDebug(lcNat) << "Passive punch from" << punchSenderId
                                       << "(already ready) — pinhole maintained.";
                    }
                } else {
                    qCInfo(lcNat) << "Late punch from unknown sender"
                                  << senderIp << ":" << senderPort << "— ignoring.";
                }
            } // end late-punch else
        }
    }
}

void NatTraversal::parseMappedAddress(const QByteArray &data)
{
    const int msgLen = (static_cast<quint8>(data[2]) << 8)
                     |  static_cast<quint8>(data[3]);

    QString mappedHost;
    quint16 mappedPort = 0;
    bool    gotXor     = false;

    int pos = 20;
    while (pos + 4 <= 20 + msgLen && pos + 4 <= data.size()) {
        const quint16 attrType = (static_cast<quint8>(data[pos])   << 8)
                               |  static_cast<quint8>(data[pos+1]);
        const quint16 attrLen  = (static_cast<quint8>(data[pos+2]) << 8)
                               |  static_cast<quint8>(data[pos+3]);
        pos += 4;

        const bool isMapped    = (attrType == kAttrMappedAddress);
        const bool isXorMapped = (attrType == kAttrXorMappedAddress);

        if ((isMapped || isXorMapped)
            && attrLen >= 8
            && pos + static_cast<int>(attrLen) <= data.size()) {

            if (static_cast<quint8>(data[pos+1]) != 0x01) { // IPv4 only
                pos += attrLen;
                pos = (pos + 3) & ~3;
                continue;
            }

            quint16 port = (static_cast<quint8>(data[pos+2]) << 8)
                         |  static_cast<quint8>(data[pos+3]);
            quint32 addr = (static_cast<quint8>(data[pos+4]) << 24)
                         | (static_cast<quint8>(data[pos+5]) << 16)
                         | (static_cast<quint8>(data[pos+6]) <<  8)
                         |  static_cast<quint8>(data[pos+7]);

            if (isXorMapped) {
                port ^= static_cast<quint16>(kStunMagicCookie >> 16);
                addr ^= kStunMagicCookie;
                gotXor = true;
            }

            if (gotXor || mappedHost.isEmpty()) {
                mappedPort = port;
                mappedHost = QHostAddress(addr).toString();
            }
        }
        pos += static_cast<int>(attrLen);
        pos = (pos + 3) & ~3;
    }

    if (mappedHost.isEmpty() || mappedPort == 0) {
        qCWarning(lcNat) << "STUN response: no usable MAPPED-ADDRESS";
        ++m_stunServerIndex;
        tryNextStunServer();
        return;
    }

    // First STUN result received.
    if (!m_stunDone) {
        // Do a second STUN query to check port stability (port-preserving NAT).
        // If the port matches on the second query, we have a stable mapping.
        m_firstReflexivePort = mappedPort;
        m_firstReflexiveHost = mappedHost;
        if (!m_stunVerified) {
            qCInfo(lcNat) << "STUN first result:" << mappedHost << ":" << mappedPort
                          << "— verifying with second query...";
            // Re-send to same server to check if port changes
            auto rng2 = QRandomGenerator::securelySeeded();
            QByteArray msg2(20, '\0');
            auto w16b = [&](int o, quint16 v) {
                msg2[o] = static_cast<char>(v >> 8); msg2[o+1] = static_cast<char>(v & 0xff);
            };
            auto w32b = [&](int o, quint32 v) {
                msg2[o]   = static_cast<char>(v >> 24);
                msg2[o+1] = static_cast<char>((v>>16) & 0xff);
                msg2[o+2] = static_cast<char>((v>>8)  & 0xff);
                msg2[o+3] = static_cast<char>(v & 0xff);
            };
            w16b(0, kStunBindingRequest); w16b(2, 0); w32b(4, kStunMagicCookie);
            for (int i = 8; i < 20; i += 4) {
                const quint32 r = rng2.generate();
                msg2[i] = static_cast<char>(r>>24); msg2[i+1] = static_cast<char>((r>>16)&0xff);
                msg2[i+2] = static_cast<char>((r>>8)&0xff); msg2[i+3] = static_cast<char>(r&0xff);
            }
            m_stunTxId = msg2.mid(8, 12);
    m_recentStunTxIds.append(m_stunTxId);
    if (m_recentStunTxIds.size() > 5) m_recentStunTxIds.removeFirst();
            m_stunVerified = true; // prevents infinite re-verification
            m_punchSocket.writeDatagram(msg2, m_stunTarget, m_stunTargetPort);
            m_stunTimer.start(kStunTimeoutMs);
            return; // wait for second response
        }
    }

    // Second result — check port stability
    const bool portPreserving = (mappedPort == m_firstReflexivePort);
    if (!portPreserving) {
        qCInfo(lcNat) << "NAT is NOT port-preserving (got" << mappedPort
                      << "vs first" << m_firstReflexivePort << ")";
        mappedPort = m_firstReflexivePort;
    } else if (!m_symmetricCheckDone) {
        // Same-server check passed. Send to a DIFFERENT STUN server to
        // detect symmetric NAT (which appears port-preserving to the same
        // destination but allocates a new port for each new destination).
        m_symmetricCheckDone = true;
        qCInfo(lcNat) << "Same-server port-preserving — cross-checking with alt STUN server";
        // Use a Google STUN server as alt — different provider from our primary servers,
        // much more likely to be reachable when our primary servers are blocked.
        const QHostAddress altStun(QStringLiteral("74.125.197.127")); // stun.l.google.com alt IP
        const quint16 altPort = 19302;
        QByteArray req(20, '\0');
        const quint16 bReq = qToBigEndian<quint16>(kStunBindingRequest);
        const quint32 bMag = qToBigEndian<quint32>(kStunMagicCookie);
        memcpy(req.data(),     &bReq, 2);
        memcpy(req.data() + 4, &bMag, 4);
        QRandomGenerator::global()->fillRange(
            reinterpret_cast<quint32 *>(req.data() + 8), 3);
        m_stunTxId = req.mid(8, 12);
    m_recentStunTxIds.append(m_stunTxId);
    if (m_recentStunTxIds.size() > 5) m_recentStunTxIds.removeFirst();
        // Store for comparison when alt-server responds
        m_firstReflexivePort = mappedPort;
        m_firstReflexiveHost = mappedHost;
        m_punchSocket.writeDatagram(req, altStun, altPort);
        m_stunTimer.start(kStunTimeoutMs); // if alt-server times out, assume symmetric
        return; // wait for alt-server response
    } else {
        // Alt-server response — compare port with first-server result
        const bool isConeNat = (mappedPort == m_firstReflexivePort);
        mappedPort = m_firstReflexivePort; // always publish first-server port
        mappedHost = m_firstReflexiveHost;
        if (!isConeNat) {
            qCWarning(lcNat) << "Symmetric NAT detected — alt server gave port"
                             << mappedPort << "vs" << m_firstReflexivePort
                             << "— relay required for all cross-network peers.";
            m_portPreserving = false;
            m_stunDone = true;
            m_reflexive = { mappedHost, mappedPort };
            if (!m_keepaliveHost.isNull()) m_keepaliveTimer.start();
            emit symmetricNatDetected();
            emit reflexiveAddressReady(m_reflexive);
            return;
        }
        qCInfo(lcNat) << "NAT is port-preserving — stable punch port:"
                      << mappedPort << "(confirmed by 2 STUN servers)";
        if (!m_keepaliveHost.isNull()) m_keepaliveTimer.start();
    }

    m_stunDone  = true;
    m_reflexive = { mappedHost, mappedPort };
    m_portPreserving = portPreserving;

    qCInfo(lcNat) << "STUN reflexive (punch socket):" << mappedHost
                  << ":" << mappedPort
                  << "(local port" << m_punchSocket.localPort()
                  << "port-preserving:" << portPreserving << ")";

    emit reflexiveAddressReady(m_reflexive);
}

void NatTraversal::onStunTimeout()
{
    if (m_stunVerified && !m_stunDone) {
        // Verification query timed out — accept first result as-is
        qCInfo(lcNat) << "STUN verification timeout — using first result";
        if (!m_keepaliveHost.isNull()) m_keepaliveTimer.start();
        m_stunDone   = true;
        m_reflexive  = { m_firstReflexiveHost, m_firstReflexivePort };
        m_portPreserving = false; // unknown, assume not preserving
        qCInfo(lcNat) << "STUN reflexive (unverified):" << m_reflexive.host
                      << ":" << m_reflexive.port;
        emit reflexiveAddressReady(m_reflexive);
        return;
    }
    qCWarning(lcNat) << "STUN timeout, trying next server";
    ++m_stunServerIndex;
    tryNextStunServer();
}

// ─── Step 3: UDP hole punching ────────────────────────────────────────────────

void NatTraversal::initiateHolePunch(const Core::UserId &peerId,
                                      const Core::PeerAddress &peerAddr)
{
    if (peerAddr.host.isEmpty() || peerAddr.port == 0) {
        qCWarning(lcNat) << "Hole punch: invalid peer address for" << peerId.value;
        emit holePunchFailed(peerId);
        return;
    }

    // ── Self-target guard ──────────────────────────────────────────────────
    // Never punch ourselves — refusing here is the second line of defence
    // against the loopback feedback loop (the receive-side guard handles
    // the rest). Both paths together make the loop literally impossible.
    //
    // We treat 127.0.0.1 / ::1 / "localhost" as self-loopback. We also
    // refuse if the peerId equals our own — even on a real LAN address
    // that's still us.
    const bool isLoopback =
        peerAddr.host == QLatin1String("127.0.0.1") ||
        peerAddr.host == QLatin1String("::1")       ||
        peerAddr.host.compare(QLatin1String("localhost"), Qt::CaseInsensitive) == 0;
    const bool isSelfId = peerId.value == m_selfId.value;
    // V32.34+: also refuse to punch our OWN reflexive (public) address.
    // When two peers sit behind the SAME CGNAT they report the identical
    // reflexive IP, so a peer's connreq can hand us our own public address
    // as the punch target. We'd then maintenance-punch 69.156.66.68:9876
    // every 20s; the packet hairpins back to us as 127.0.0.1 and the
    // receive-side guard drops it with a PUNCH-LOOPBACK-DROP-V2 warning —
    // forever. Refusing here stops the self-punch loop at the source (no
    // wasted packet, no recurring warning). Host-only compare: the port may
    // legitimately differ, but our own public *host* is never a valid punch
    // target. Guarded on m_reflexive being known + non-empty.
    const bool isSelfReflexive =
        !m_reflexive.host.isEmpty()
        && peerAddr.host == m_reflexive.host;
    if (isLoopback || isSelfId || isSelfReflexive) {
        qCWarning(lcNat) << "Refusing to punch self:" << peerAddr.toString()
                         << "id=" << peerId.value
                         << "(loopback=" << isLoopback
                         << "selfId=" << isSelfId
                         << "selfReflexive=" << isSelfReflexive << ")";
        emit holePunchFailed(peerId);
        return;
    }

    // If already punching to this peer at the SAME address, skip.
    // But if the address changed (e.g. peerPunchAddr arrived after WS addr),
    // cancel the old punch and start one to the new address.
    if (m_punchAttempt && m_punchAttempt->peerId.value == peerId.value) {
        if (m_punchAttempt->addr.host == peerAddr.host &&
            m_punchAttempt->addr.port == peerAddr.port) {
            qCDebug(lcNat) << "Hole punch already in progress for" << peerId.value;
            return;
        }
        // New address for same peer — cancel old punch and restart
        qCInfo(lcNat) << "Hole punch: switching target for" << peerId.value
                      << "from" << m_punchAttempt->addr.toString()
                      << "to" << peerAddr.toString();
        m_punchTimer.stop();
        m_punchAttempt.reset();
    }

    // Clear the passive-ready guard so new active punch can re-emit holePunchReady.
    m_readyEmitted.remove(peerId.value);

    qCInfo(lcNat) << "UDP hole punch →" << peerId.value
                  << "@" << peerAddr.toString()
                  << "from local port" << m_punchSocket.localPort();

    m_punchAttempt = PunchAttempt{ peerId, peerAddr, 0 };

    const QHostAddress peerHost(peerAddr.host);
    const quint16      peerPort = peerAddr.port;

    const QByteArray selfIdBytes = m_selfId.value.toUtf8();
    QByteArray punchPayload("SNAPCLONE_PUNCH_V1\n");
    punchPayload.append(selfIdBytes);

    qCDebug(lcNat) << "Punch payload bytes:" << punchPayload.size()
                  << "selfId:" << m_selfId.value
                  << "hex:" << punchPayload.toHex();

    // V32.21: derive an effective burst count from app context. The default
    // is the historical fixed `kPunchCount` (60 packets × 150ms = 9s burst).
    // Two adjustments:
    //   • If we know we're idle on a metered/cellular/power-saver path AND
    //     the user did NOT just act, halve the burst — keeps the mapping
    //     open with much less cellular data while the user is pocketed.
    //   • If the user just acted (shutter tap, snap open, send), keep the
    //     full burst regardless — they're expecting snappy delivery and
    //     we want the punch to converge quickly.
    // Captured by value so the lambda has a stable count even if context
    // flips mid-burst (avoid weirdness from re-deriving each tick).
    int effectiveCount = kPunchCount;
    if (m_appContext) {
        const auto &ctx = m_appContext->context();
        const bool conserving = ctx.prefersConservative();
        const bool userActive = ctx.userJustActed(5000);
        if (conserving && !userActive) {
            effectiveCount = kPunchCount / 2; // 30 instead of 60 → ~4.5s burst
            qCInfo(lcNat) << "Punch volley: context conservative + user idle"
                          << "→ halved burst (" << effectiveCount << "/"
                          << kPunchCount << ")";
        }
    }

    auto punch = [this, peerHost, peerPort, punchPayload, effectiveCount]() {
        if (!m_punchAttempt) return;

        m_punchSocket.writeDatagram(punchPayload, peerHost, peerPort);
        ++m_punchAttempt->sentCount;

        qCDebug(lcNat) << "Punch" << m_punchAttempt->sentCount
                       << "/" << effectiveCount
                       << "→" << peerHost.toString() << ":" << peerPort;

        if (m_punchAttempt->sentCount >= effectiveCount) {
            // Maintenance punches keep the CGNAT mapping open.
            // AT&T CGNAT expires in ~30s; 20s is safe with 10s headroom.
            // Was 8s — increased to save battery (6 fewer UDP packets/min).
            m_punchTimer.setInterval(20000);
            m_punchTimer.setTimerType(Qt::VeryCoarseTimer); // battery: batch the keep-open punches
            if (!m_punchAttempt->readyEmitted) {
                m_punchAttempt->readyEmitted = true;
                qCInfo(lcNat) << "Hole punch volleys complete for"
                              << m_punchAttempt->peerId.value
                              << "— signalling ready, starting maintenance punches every 20s";
                emit holePunchReady(m_punchAttempt->peerId, m_punchAttempt->addr);
            } else {
                qCDebug(lcNat) << "Maintenance punch" << m_punchAttempt->sentCount
                               << "→" << m_punchAttempt->addr.host
                               << ":" << m_punchAttempt->addr.port;
            }
        }
    };

    punch(); // first packet immediately

    disconnect(&m_punchTimer, nullptr, nullptr, nullptr);
    connect(&m_punchTimer, &QTimer::timeout, this, punch);
    m_punchTimer.start(kPunchIntervalMs);
}

void NatTraversal::stopHolePunch(const Core::UserId &peerId)
{
    if (m_punchAttempt && m_punchAttempt->peerId.value == peerId.value) {
        m_punchTimer.stop();
        m_punchAttempt.reset();
        qCInfo(lcNat) << "Maintenance punch stopped for" << peerId.value
                      << "— UDP channel confirmed";
    }
}


// ─── TURN relay URL ───────────────────────────────────────────────────────────

QString NatTraversal::turnRelayUrl(const Core::UserId &) const
{
    if (sizeof(kTurnServers) / sizeof(*kTurnServers) == 0) return {};
    const auto &t = kTurnServers[0];
    return QStringLiteral("turn:%1:%2?transport=tcp")
        .arg(QString::fromLatin1(t.host))
        .arg(t.port);
}



void NatTraversal::writeDatagram(const QByteArray &data,
                                  const QString    &host,
                                  quint16           port)
{
    m_punchSocket.writeDatagram(data, QHostAddress(host), port);
}

void NatTraversal::registerPunchedPeer(const Core::UserId     &peerId,
                                        const Core::PeerAddress &peerAddr)
{
    const QString key = peerAddr.host + QLatin1Char(':')
                      + QString::number(peerAddr.port);
    m_udpPeers[key] = peerId;
    // Prune oldest entries if map grows too large (peer cycled many CGNAT ports)
    constexpr int kMaxUdpPeers = 64;
    if (m_udpPeers.size() > kMaxUdpPeers) {
        auto it = m_udpPeers.begin();
        m_udpPeers.erase(it); // remove oldest (first inserted)
    }
    qCDebug(lcNat) << "Registered UDP peer" << peerId.value << "at" << key;
}

void NatTraversal::sendUdpData(const Core::PeerAddress &peerAddr,
                                const QByteArray        &payload)
{
    if (m_punchSocket.state() != QAbstractSocket::BoundState) {
        qCWarning(lcNat) << "sendUdpData: punch socket not bound";
        return;
    }
    // Frame: magic(17) + idLen(2) + selfId(N) + payload
    // We don't have selfId here — caller must pre-frame or we embed it empty.
    // Use a simple framing: magic + 0x00 0x00 (empty ID) + payload.
    // The receiver uses the registered m_udpPeers map to identify the sender.
    QByteArray frame;
    frame.reserve(17 + 2 + payload.size());
    frame.append(QByteArrayLiteral("SNAPCLONE_DATA_V1"));
    frame.append(static_cast<char>(0));
    frame.append(static_cast<char>(0));
    frame.append(payload);
    m_punchSocket.writeDatagram(frame,
        QHostAddress(peerAddr.host),
        peerAddr.port);
}

} // namespace SnapClone::Network
