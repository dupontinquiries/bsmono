#include "network/QtWebSocketCompat.hpp"
#include "network/NetworkDiagnostics.hpp"

#include <QUdpSocket>
#include <QTcpServer>
#include <QTcpSocket>
#include <QWebSocket>
#include <QNetworkAccessManager>
#include <QNetworkRequest>
#include <QNetworkReply>
#include <QTimer>
#include <QHostAddress>
#include <QLoggingCategory>
#include <QRandomGenerator>
#include <QNetworkInterface>

// ─── Network/NetworkDiagnostics.cpp ───────────────────────────────────────────
//
// Each check runs asynchronously and calls onCheckDone() exactly once.
//
// Race-condition fix:
//   Each check uses a shared std::shared_ptr<bool> `done` flag.  The first
//   callback to arrive sets *done = true and proceeds; any subsequent
//   callback (e.g. readyRead arriving just after the timeout fires) finds
//   *done == true and returns immediately without calling onCheckDone() again.
//   This prevents m_pending from going negative and diagnosisReady() from
//   being emitted multiple times.
//
// All QObjects created inside checks (sockets, timers, NAM) are parented
// to `this` so they are cleaned up if NetworkDiagnostics is destroyed mid-run.
// ─────────────────────────────────────────────────────────────────────────────

Q_LOGGING_CATEGORY(lcDiag, "snapclone.diagnostics")

namespace SnapClone::Network {

NetworkDiagnostics::NetworkDiagnostics(quint16 wsPort, quint16 gossipPort, QObject *parent)
    : QObject(parent)
    , m_wsPort(wsPort)
    , m_gossipPort(gossipPort)
{}

void NetworkDiagnostics::runAll()
{
    m_results.clear();
    m_pending = 5;

    checkUdpMulticast();
    checkTcpListen();
    checkLoopbackWebSocket();
    checkPublicAddress();
    checkUdpStun();
}

void NetworkDiagnostics::onCheckDone(DiagnosticResult result)
{
    m_results.append(result);
    emit checkCompleted(result);
    if (--m_pending == 0) {
        emit diagnosisReady(m_results);
    }
}

// ─── Check 1: UDP Multicast loopback ─────────────────────────────────────────
//
// Sends a probe datagram to the gossip multicast group and listens for it
// to arrive back on the same socket.  A timeout of 500 ms is generous for
// loopback traffic but avoids hanging on systems where the OS drops multicast.

void NetworkDiagnostics::checkUdpMulticast()
{
    DiagnosticResult base;
    base.name = QStringLiteral("UDP Multicast (gossip port %1)").arg(m_gossipPort);

    auto *sock = new QUdpSocket(this);
    const bool bound = sock->bind(QHostAddress::AnyIPv4, m_gossipPort,
                                  QUdpSocket::ShareAddress | QUdpSocket::ReuseAddressHint);
    if (!bound) {
        sock->deleteLater();
        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Fail;
        r.detail     = QStringLiteral("Could not bind UDP port %1: %2")
                           .arg(m_gossipPort).arg(sock->errorString());
        r.suggestion = QStringLiteral(
            "Another process may own this port, or a firewall rule is blocking it.\n"
            "  sudo firewall-cmd --add-port=%1/udp --permanent && sudo firewall-cmd --reload\n"
            "  sudo ufw allow %1/udp").arg(m_gossipPort);
        onCheckDone(r);
        return;
    }

    sock->joinMulticastGroup(QHostAddress(QStringLiteral("224.0.0.251")));
    sock->writeDatagram(QByteArrayLiteral("SNAPCLONE_DIAG"),
                        QHostAddress(QStringLiteral("224.0.0.251")), m_gossipPort);

    auto *timer = new QTimer(this);
    timer->setSingleShot(true);

    // Shared flag prevents both readyRead and timeout from completing the check.
    auto done = std::make_shared<bool>(false);

    connect(sock, &QUdpSocket::readyRead, this, [=, this]() {
        if (*done) return;
        *done = true;
        timer->stop();

        QByteArray buf(64, Qt::Uninitialized);
        sock->readDatagram(buf.data(), buf.size());
        sock->deleteLater();
        timer->deleteLater();

        DiagnosticResult r = base;
        r.status = DiagnosticResult::Status::Ok;
        r.detail = QStringLiteral("UDP multicast loopback succeeded on port %1.").arg(m_gossipPort);
        onCheckDone(r);
    });

    connect(timer, &QTimer::timeout, this, [=, this]() {
        if (*done) return;
        *done = true;
        sock->deleteLater();
        timer->deleteLater();

        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Warning;
        r.detail     = QStringLiteral("UDP multicast probe timed out after 500 ms. "
                                      "LAN peer discovery will not work.");
        r.suggestion = QStringLiteral(
            "Allow UDP multicast in your firewall:\n"
            "  sudo firewall-cmd --add-port=%1/udp --permanent\n"
            "  sudo firewall-cmd --add-protocol=igmp --permanent\n"
            "  sudo firewall-cmd --reload\n"
            "UFW:  sudo ufw allow %1/udp\n\n"
            "You can still add peers manually via 'Add Friend by Address'.").arg(m_gossipPort);
        onCheckDone(r);
    });

    timer->start(500);
}

// ─── Check 2: TCP listen on WebSocket port ────────────────────────────────────

void NetworkDiagnostics::checkTcpListen()
{
    // We cannot try to bind m_wsPort here because the app's own WebSocket
    // server already owns it — the bind would always fail with EADDRINUSE,
    // giving a false "port blocked" result.
    //
    // Instead: attempt a loopback TCP connection to our own server.
    // If it connects → our server is up and the port is reachable on loopback.
    // If it fails    → something is wrong (firewall blocking loopback, or the
    //                  server failed to start — check earlier log output).
    DiagnosticResult base;
    base.name = QStringLiteral("TCP Listen (WebSocket port %1)").arg(m_wsPort);

    auto *socket = new QTcpSocket(this);
    auto *timer  = new QTimer(this);
    timer->setSingleShot(true);

    auto done = std::make_shared<bool>(false);

    auto cleanup = [socket, timer]() {
        socket->deleteLater();
        timer->deleteLater();
    };

    connect(socket, &QTcpSocket::connected, this, [=, this]() {
        if (*done) return;
        *done = true;
        timer->stop();
        socket->disconnectFromHost();
        cleanup();

        DiagnosticResult r = base;
        r.status = DiagnosticResult::Status::Ok;
        r.detail = QStringLiteral("WebSocket server is listening on port %1 "
                                   "(loopback connection succeeded).").arg(m_wsPort);
        onCheckDone(r);
    });

    connect(socket, &QTcpSocket::errorOccurred,
            this, [=, this](QAbstractSocket::SocketError) {
        if (*done) return;
        *done = true;
        timer->stop();
        cleanup();

        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Fail;
        r.detail     = QStringLiteral("Cannot connect to local WebSocket server "
                                       "on port %1: %2").arg(m_wsPort).arg(socket->errorString());
        r.suggestion = QStringLiteral(
            "Port %1 is in use or blocked.\n"
            "Check:  ss -tlnp | grep %1\n"
            "Fix:    sudo firewall-cmd --add-port=%1/tcp --permanent "
            "&& sudo firewall-cmd --reload\n"
            "        sudo ufw allow %1/tcp").arg(m_wsPort);
        onCheckDone(r);
    });

    connect(timer, &QTimer::timeout, this, [=, this]() {
        if (*done) return;
        *done = true;
        socket->abort();
        cleanup();

        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Fail;
        r.detail     = QStringLiteral("Loopback connection to port %1 timed out — "
                                       "server may not have started.").arg(m_wsPort);
        r.suggestion = QStringLiteral(
            "Check application logs for WebSocket server errors.\n"
            "Also ensure port %1 is open: sudo firewall-cmd --add-port=%1/tcp --permanent "
            "&& sudo firewall-cmd --reload").arg(m_wsPort);
        onCheckDone(r);
    });

    timer->start(2000);
    socket->connectToHost(QHostAddress::LocalHost, m_wsPort);
}

// ─── Check 3: WebSocket loopback ─────────────────────────────────────────────
//
// Creates a temporary TCP listener on a random port and attempts a WebSocket
// connection to 127.0.0.1.  Tests whether the OS drops loopback TCP.

void NetworkDiagnostics::checkLoopbackWebSocket()
{
    DiagnosticResult base;
    base.name = QStringLiteral("WebSocket Loopback (127.0.0.1:%1)").arg(m_wsPort);

    auto *server = new QTcpServer(this);
    if (!server->listen(QHostAddress::LocalHost, 0)) {
        server->deleteLater();
        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Warning;
        r.detail     = QStringLiteral("Could not create loopback test server: %1")
                           .arg(server->errorString());
        r.suggestion = QStringLiteral("Loopback check skipped.");
        onCheckDone(r);
        return;
    }

    const quint16 tempPort = server->serverPort();
    auto *ws    = new QWebSocket(QString{}, QWebSocketProtocol::VersionLatest, this);
    auto *timer = new QTimer(this);
    timer->setSingleShot(true);

    auto done = std::make_shared<bool>(false);

    auto cleanup = [ws, server, timer]() {
        ws->deleteLater();
        server->deleteLater();
        timer->deleteLater();
    };

    connect(ws, &QWebSocket::connected, this, [=, this]() {
        if (*done) return;
        *done = true;
        timer->stop();
        ws->close();
        cleanup();

        DiagnosticResult r = base;
        r.status = DiagnosticResult::Status::Ok;
        r.detail = QStringLiteral("WebSocket loopback connection succeeded.");
        onCheckDone(r);
    });

    connect(ws, SC_QWEBSOCKET_ERROR_SIGNAL,
            this, [=, this](QAbstractSocket::SocketError) {
        if (*done) return;
        *done = true;
        timer->stop();
        cleanup();

        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Warning;
        r.detail     = QStringLiteral("WebSocket loopback failed: %1").arg(ws->errorString());
        r.suggestion = QStringLiteral(
            "A local firewall rule may be dropping loopback traffic.\n"
            "  sudo iptables -I INPUT -i lo -j ACCEPT");
        onCheckDone(r);
    });

    connect(timer, &QTimer::timeout, this, [=, this]() {
        if (*done) return;
        *done = true;
        ws->abort();
        cleanup();

        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Warning;
        r.detail     = QStringLiteral("WebSocket loopback timed out.");
        r.suggestion = QStringLiteral("Firewall may be blocking loopback TCP.");
        onCheckDone(r);
    });

    timer->start(1500);
    ws->open(QUrl(QStringLiteral("ws://127.0.0.1:%1").arg(tempPort)));
}

// ─── Check 4: Public / NAT address discovery ──────────────────────────────────
//
// Queries api.ipify.org via HTTPS to discover the external IP and reports
// all local non-loopback addresses regardless.

void NetworkDiagnostics::checkPublicAddress()
{
    DiagnosticResult base;
    base.name = QStringLiteral("Public / NAT Address");

    // Collect local addresses synchronously — never blocks.
    QStringList localAddrs;
    for (const QHostAddress &addr : QNetworkInterface::allAddresses()) {
        if (!addr.isLoopback() && addr.protocol() == QAbstractSocket::IPv4Protocol) {
            localAddrs.append(addr.toString());
        }
    }

    auto *nam   = new QNetworkAccessManager(this);
    auto *timer = new QTimer(this);
    timer->setSingleShot(true);

    auto *reply = nam->get(QNetworkRequest(QUrl(QStringLiteral("https://api.ipify.org"))));

    auto done = std::make_shared<bool>(false);

    auto cleanup = [reply, nam, timer]() {
        reply->deleteLater();
        nam->deleteLater();
        timer->deleteLater();
    };

    connect(reply, &QNetworkReply::finished, this, [=, this]() {
        if (*done) return;
        *done = true;
        timer->stop();
        cleanup();

        DiagnosticResult r = base;
        if (reply->error() == QNetworkReply::NoError) {
            const QString publicIp = QString::fromLatin1(reply->readAll().trimmed());
            r.status = DiagnosticResult::Status::Ok;
            r.detail = QStringLiteral("Public IP: %1\nLocal IPs: %2\n\n"
                                       "Share your Public IP with internet peers "
                                       "or your Local IP for LAN peers.")
                           .arg(publicIp, localAddrs.join(QStringLiteral(", ")));
        } else {
            r.status = DiagnosticResult::Status::Warning;
            r.detail = QStringLiteral("Could not determine public IP (%1).\nLocal IPs: %2")
                           .arg(reply->errorString(), localAddrs.join(QStringLiteral(", ")));
            r.suggestion = QStringLiteral(
                "Outbound HTTPS may be blocked or you have no internet access.\n"
                "LAN-only mode (gossip multicast) still works if UDP is open.");
        }
        onCheckDone(r);
    });

    connect(timer, &QTimer::timeout, this, [=, this]() {
        if (*done) return;
        *done = true;
        reply->abort();
        cleanup();

        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Warning;
        r.detail     = QStringLiteral("Public IP lookup timed out.\nLocal IPs: %1")
                           .arg(localAddrs.join(QStringLiteral(", ")));
        r.suggestion = QStringLiteral("No internet or HTTPS is blocked. LAN mode still works.");
        onCheckDone(r);
    });

    timer->start(4000);
}


// ─── Check 5: UDP STUN reachability ──────────────────────────────────────────
//
// Sends a STUN Binding Request from a temporary socket to Google's STUN server.
// If we get a response, outbound AND inbound UDP is working and we know our
// reflexive address. If it times out, our NAT/firewall is blocking UDP or the
// STUN server is unreachable.
// Uses a temporary socket (not the punch socket) to avoid interfering with
// active hole punching.

void NetworkDiagnostics::checkUdpStun()
{
    DiagnosticResult base;
    base.name = QStringLiteral("UDP / STUN Connectivity");

    auto *sock  = new QUdpSocket(this);
    auto *timer = new QTimer(this);
    timer->setSingleShot(true);
    auto done = std::make_shared<bool>(false);

    // Bind to any port — we just need to know if UDP replies arrive.
    sock->bind(QHostAddress::AnyIPv4, 0);

    // Minimal STUN Binding Request: magic cookie + 12-byte transaction ID
    // RFC 5389 §6
    QByteArray stunReq(20, 0);
    stunReq[0]  = 0x00; stunReq[1]  = 0x01; // Binding Request
    stunReq[2]  = 0x00; stunReq[3]  = 0x00; // Attribute length = 0
    stunReq[4]  = 0x21; stunReq[5]  = 0x12; // Magic cookie 0x2112A442
    stunReq[6]  = 0xA4; stunReq[7]  = 0x42;
    // Random transaction ID
    for (int i = 8; i < 20; ++i)
        stunReq[i] = static_cast<char>(QRandomGenerator::global()->generate() & 0xFF);

    // Send to Google STUN (hardcoded IP to avoid DNS dependency)
    sock->writeDatagram(stunReq, QHostAddress(QStringLiteral("74.125.197.127")), 19302);

    connect(sock, &QUdpSocket::readyRead, this, [=, this]() {
        if (*done) return;
        *done = true;
        timer->stop();
        sock->deleteLater();
        timer->deleteLater();

        // Parse the STUN response minimally — just check it's a Binding Response
        // and extract the XOR-MAPPED-ADDRESS if present.
        QString reflexive;
        while (sock->hasPendingDatagrams()) {
            QByteArray resp(static_cast<int>(sock->pendingDatagramSize()), Qt::Uninitialized);
            sock->readDatagram(resp.data(), resp.size());
            if (resp.size() >= 20) {
                const quint16 msgType = (static_cast<quint8>(resp[0]) << 8)
                                      |  static_cast<quint8>(resp[1]);
                if (msgType == 0x0101) { // Binding Success Response
                    // Try to parse XOR-MAPPED-ADDRESS (attr type 0x0020)
                    int offset = 20;
                    while (offset + 4 <= resp.size()) {
                        const quint16 aType = (static_cast<quint8>(resp[offset]) << 8)
                                            |  static_cast<quint8>(resp[offset+1]);
                        const quint16 aLen  = (static_cast<quint8>(resp[offset+2]) << 8)
                                            |  static_cast<quint8>(resp[offset+3]);
                        if ((aType == 0x0020 || aType == 0x0001) && aLen >= 8) {
                            const quint16 portXor = (static_cast<quint8>(resp[offset+6]) << 8)
                                                  |  static_cast<quint8>(resp[offset+7]);
                            const quint16 port = (aType == 0x0020)
                                ? (portXor ^ 0x2112)
                                : portXor;
                            const quint32 ipXor = (static_cast<quint8>(resp[offset+8])  << 24)
                                                | (static_cast<quint8>(resp[offset+9])  << 16)
                                                | (static_cast<quint8>(resp[offset+10]) <<  8)
                                                |  static_cast<quint8>(resp[offset+11]);
                            const quint32 ip = (aType == 0x0020)
                                ? (ipXor ^ 0x2112A442)
                                : ipXor;
                            reflexive = QStringLiteral("%1.%2.%3.%4:%5")
                                .arg((ip>>24)&0xFF).arg((ip>>16)&0xFF)
                                .arg((ip>>8)&0xFF).arg(ip&0xFF)
                                .arg(port);
                            break;
                        }
                        offset += 4 + ((aLen + 3) & ~3); // 4-byte align
                    }
                }
            }
        }

        DiagnosticResult r = base;
        r.status = DiagnosticResult::Status::Ok;
        r.detail = QStringLiteral("Outbound UDP works. STUN response received.");
        if (!reflexive.isEmpty())
            r.detail += QStringLiteral("\nYour public UDP address: %1").arg(reflexive);
        r.suggestion = QStringLiteral(
            "Inbound UDP should work if your router's firewall allows it.");
        onCheckDone(r);
    });

    connect(timer, &QTimer::timeout, this, [=, this]() {
        if (*done) return;
        *done = true;
        sock->deleteLater();
        timer->deleteLater();

        DiagnosticResult r = base;
        r.status     = DiagnosticResult::Status::Fail;
        r.detail     = QStringLiteral("STUN request to 74.125.197.127:19302 timed out.\n"
                                       "Outbound UDP may be blocked by your firewall or router.");
        r.suggestion = QStringLiteral(
            "Open UDP port 9876 on your router/firewall.\n"
            "Without UDP, hole punching cannot establish a direct connection.");
        onCheckDone(r);
    });

    timer->start(5000);
}

} // namespace SnapClone::Network
