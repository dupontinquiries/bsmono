#include "network/QtWebSocketCompat.hpp"
#include "network/WebSocketTransport.hpp"
#include <QSslConfiguration>
#include <QSslSocket>

#include <QLoggingCategory>
#include <QWebSocket>
#include <QTimer>

Q_LOGGING_CATEGORY(lcWs, "snapclone.websocket")

namespace SnapClone::Network {

WebSocketTransport::WebSocketTransport(QObject *parent)
    : ITransport(parent)
{
    m_connectTimer.setSingleShot(true);
    m_connectTimer.setInterval(12000); // 12s: allows 9s punch window to complete
    // Battery: VeryCoarseTimer lets the kernel align these wakeups to ~1s
    // boundaries so they coalesce with other coarse timers. A connect timeout
    // firing at 12-13s instead of exactly 12s only grants more time, and a 30s
    // keepalive landing at 30-31s is well inside any sane pong deadline.
    m_connectTimer.setTimerType(Qt::VeryCoarseTimer);
    m_keepaliveTimer.setTimerType(Qt::VeryCoarseTimer); // interval set per-connect below
    // When timer fires and socket is not yet connected, abort it.
    // Because m_connectTimer is a member, it is destroyed with the transport
    // and can never fire after the transport is gone — no dangling pointer.
    connect(&m_connectTimer, &QTimer::timeout, this, [this]() {
        if (m_socket.state() != QAbstractSocket::ConnectedState) {
            qCWarning(lcWs) << "Connection timeout — aborting";
            m_socket.abort();
        }
    });
    connectSocketSignals();
}

WebSocketTransport::~WebSocketTransport()
{
    // Stop our own timers first — prevents timeout firing during teardown.
    m_connectTimer.stop();
    m_keepaliveTimer.stop();
    // Disconnect ALL signals FROM m_socket so no slots fire during destruction.
    // QObject::disconnect() on the socket itself prevents re-entrant calls
    // to our slots (onDisconnected, onError) while we're being destroyed.
    m_socket.QObject::disconnect();
    // abort() immediately kills the TCP connection without scheduling
    // the WebSocket close handshake timer that causes QSingleShotTimer crashes.
    if (m_adopted) {
        m_adopted->QObject::disconnect();
        m_adopted->abort();
    } else {
        m_socket.abort();
    }
}

void WebSocketTransport::connectSocketSignals()
{
    connect(&m_socket, &QWebSocket::connected,
            this, &WebSocketTransport::onConnected);
    connect(&m_socket, &QWebSocket::disconnected,
            this, &WebSocketTransport::onDisconnected);
    connect(&m_socket, &QWebSocket::binaryMessageReceived,
            this, &WebSocketTransport::onBinaryMessageReceived);
    connect(&m_socket, SC_QWEBSOCKET_ERROR_SIGNAL,
            this, &WebSocketTransport::onError);
    connect(&m_socket, &QWebSocket::pong,
            this, &WebSocketTransport::onPong);
}

void WebSocketTransport::adoptSocket(QWebSocket *socket)
{
    // Takes ownership of `socket` from QWebSocketServer::nextPendingConnection().
    //
    // Ownership model:
    //   m_adopted (unique_ptr) is the sole owner — it deletes the socket on reset/destroy.
    //   We must NOT also call socket->setParent(this) because that would give Qt's
    //   parent-child tree a second claim on the same object, causing a double-delete
    //   when either the unique_ptr or the parent is destroyed.
    //
    // The socket's previous parent was the QWebSocketServer; we clear it by passing
    // nullptr here so Qt does not delete it when the server is later destroyed.
    socket->setParent(nullptr);
    m_adopted.reset(socket); // unique_ptr now holds the only owning reference

    connect(socket, &QWebSocket::disconnected,
            this, &WebSocketTransport::onDisconnected);
    connect(socket, &QWebSocket::binaryMessageReceived,
            this, &WebSocketTransport::onBinaryMessageReceived);
    connect(socket, SC_QWEBSOCKET_ERROR_SIGNAL,
            this, &WebSocketTransport::onError);
    connect(socket, &QWebSocket::pong,
            this, &WebSocketTransport::onPong);

    qCDebug(lcWs) << "Adopted inbound WebSocket from" << socket->peerAddress().toString();
    m_unansweredPings = 0;
    // Start keepalive — same lifecycle as outbound sockets (see onConnected
    // for rationale). Disconnect any prior lambda first; UniqueConnection
    // doesn't dedupe lambdas. Includes the same 3-pong-miss force-disconnect
    // as outbound.
    QObject::disconnect(&m_keepaliveTimer, &QTimer::timeout, nullptr, nullptr);
    m_keepaliveTimer.setInterval(30000);
    connect(&m_keepaliveTimer, &QTimer::timeout, this, [this]() {
        QWebSocket *active = m_adopted ? m_adopted.get() : &m_socket;
        if (!active || active->state() != QAbstractSocket::ConnectedState)
            return;
        ++m_unansweredPings;
        if (m_unansweredPings > kMaxUnansweredPings) {
            qCWarning(lcWs) << "Keepalive (adopted): peer unresponsive after"
                            << m_unansweredPings * 30 << "s — forcing reconnect";
            active->abort();
            m_unansweredPings = 0;
            return;
        }
        active->ping(QByteArrayLiteral("kp"));
    });
    m_keepaliveTimer.start();
    emit connected(); // socket is already in the connected state
}

// ─── ITransport interface ─────────────────────────────────────────────────────

void WebSocketTransport::extendConnectTimeout(int msec)
{
    if (m_connectTimer.isActive()) {
        // Add msec to remaining time (clamp to remaining + msec, not absolute).
        const int remaining = m_connectTimer.remainingTime();
        m_connectTimer.setInterval(qMax(1, remaining) + msec);
        m_connectTimer.start(); // restart with new interval
    }
}

void WebSocketTransport::connectToPeer(const Core::PeerAddress &address)
{
    const QString scheme = address.tls ? QStringLiteral("wss") : QStringLiteral("ws");
    const QUrl url(QStringLiteral("%1://%2:%3/snap").arg(scheme, address.host).arg(address.port));
    qCDebug(lcWs) << "Connecting to" << url.toString();

    if (address.tls) {
        // Accept self-signed certs on the relay — the payload is already
        // end-to-end encrypted by our ECDH layer so cert pinning is not
        // required for security; we just need the TLS transport to get
        // through HTTPS-only firewalls.
        QSslConfiguration cfg = QSslConfiguration::defaultConfiguration();
        cfg.setPeerVerifyMode(QSslSocket::VerifyNone);
        m_socket.setSslConfiguration(cfg);
    }

    m_connectTimer.start();
    m_socket.open(url);
}

void WebSocketTransport::disconnect()
{
    // Use abort() not close() — close() schedules a QSingleShotTimer for the
    // WebSocket closing handshake. If the transport is destroyed before that
    // timer fires, the timer accesses the deleted socket → segfault.
    // abort() terminates immediately with no deferred timers.
    if (m_adopted) {
        m_adopted->QObject::disconnect();
        m_adopted->abort();
    }
    m_socket.QObject::disconnect();
    m_socket.abort();
}

void WebSocketTransport::send(const QByteArray &payload)
{
    if (!isConnected()) {
        qCWarning(lcWs) << "send() on disconnected socket — discarding.";
        return;
    }
    if (payload.size() > kMaxFrameBytes) {
        qCWarning(lcWs) << "Payload" << payload.size() << "bytes exceeds limit; refusing.";
        return;
    }
    // Send on whichever socket is active.
    QWebSocket *active = (m_adopted && m_adopted->isValid()) ? m_adopted.get() : &m_socket;
    const qint64 result = active->sendBinaryMessage(payload);
    if (result == -1) {
        qCWarning(lcWs) << "sendBinaryMessage failed:" << active->errorString();
        emit errorOccurred(active->errorString());
    }
}

bool WebSocketTransport::isConnected() const
{
    if (m_adopted && m_adopted->state() == QAbstractSocket::ConnectedState) return true;
    return m_socket.state() == QAbstractSocket::ConnectedState;
}

// ─── Private slots ────────────────────────────────────────────────────────────

void WebSocketTransport::onConnected()
{
    m_connectTimer.stop();
    m_unansweredPings = 0;
    qCDebug(lcWs) << "WebSocket connected.";
    // Start keepalive: 30s WS pings on the active socket. The QWebSocket
    // ping() method sends a control frame and emits pong() when the peer
    // replies. Idle Bell home-router NAT entries die after ~3-5 min; phones
    // on cellular can drop sockets even faster (1-2 min on some carriers).
    // 30 s is well under all of these.
    //
    // We ALSO track pongs: if 3 consecutive pings (90 s) get no answer, we
    // force-disconnect instead of waiting up to 3 minutes for TCP's own
    // keepalive timeout. This lets the reconnect path try alt addresses
    // (relay, other LAN IP) sooner. Pong handler resets the counter.
    //
    // Disconnect any prior lambda before reconnecting — Qt::UniqueConnection
    // only deduplicates traditional method-pointer connects, not lambda
    // connects.
    QObject::disconnect(&m_keepaliveTimer, &QTimer::timeout, nullptr, nullptr);
    m_keepaliveTimer.setInterval(30000);
    connect(&m_keepaliveTimer, &QTimer::timeout, this, [this]() {
        QWebSocket *active = m_adopted ? m_adopted.get() : &m_socket;
        if (!active || active->state() != QAbstractSocket::ConnectedState)
            return;
        ++m_unansweredPings;
        if (m_unansweredPings > kMaxUnansweredPings) {
            qCWarning(lcWs) << "Keepalive: peer unresponsive after"
                            << m_unansweredPings * 30 << "s — forcing reconnect";
            // abort() rather than close() — close() does a graceful WS
            // close handshake which itself times out; abort drops the TCP
            // socket immediately, triggering our disconnected signal which
            // the network layer treats as a normal disconnect and starts
            // the reconnect flow.
            active->abort();
            m_unansweredPings = 0;  // reset for next session
            return;
        }
        active->ping(QByteArrayLiteral("kp"));
    });
    m_keepaliveTimer.start();
    emit connected();
}

void WebSocketTransport::onPong(quint64 elapsedTime, const QByteArray &payload)
{
    Q_UNUSED(elapsedTime);
    Q_UNUSED(payload);
    // Peer is alive — reset the unanswered counter.
    m_unansweredPings = 0;
}

void WebSocketTransport::onDisconnected()
{
    m_keepaliveTimer.stop();
    m_unansweredPings = 0;
    qCDebug(lcWs) << "WebSocket disconnected.";
    emit disconnected();
}

void WebSocketTransport::onBinaryMessageReceived(const QByteArray &message)
{
    if (message.size() > kMaxFrameBytes) {
        qCWarning(lcWs) << "Oversized frame" << message.size() << "bytes; discarding.";
        return;
    }
    emit messageReceived(message);
}

void WebSocketTransport::onError(QAbstractSocket::SocketError socketError)
{
    const QWebSocket *src = (m_adopted && m_adopted->isValid()) ? m_adopted.get() : &m_socket;
    const QString     msg = src->errorString();

    // RemoteHostClosedError with empty message = normal TCP close by peer.
    // Don't spam the UI with an empty error string for this routine event.
    if (socketError == QAbstractSocket::RemoteHostClosedError && msg.isEmpty()) {
        qCDebug(lcWs) << "Remote host closed connection (normal).";
        return; // disconnected signal handles the peer-lost logic
    }

    // Add an actionable hint for the most common error: the peer's firewall
    // is dropping TCP on port 9876.  "Connection timed out" (not "refused")
    // means packets are silently dropped, not actively rejected.
    QString hint;
    if (socketError == QAbstractSocket::SocketTimeoutError) {
        hint = QStringLiteral(
            " (Connection timed out — peer firewall may be blocking port 9876/tcp."
            " Run Network Diagnostics 🔍 on both devices.)");
    } else if (socketError == QAbstractSocket::NetworkError) {
        // "Network unreachable" = no route to host — peer is offline or
        // on a different network. Different from a firewall block.
        hint = QStringLiteral(
            " (Network unreachable — peer may be offline or on a different network."
            " Address will be retried via DHT when peer comes back online.)");
    } else if (socketError == QAbstractSocket::ConnectionRefusedError) {
        hint = QStringLiteral(
            " (Connection refused — peer is online but port 9876/tcp is closed."
            " Ask peer to open that port.)");
    }

    qCWarning(lcWs) << "WebSocket error" << socketError << msg;
    emit errorOccurred(msg + hint);
}

} // namespace SnapClone::Network
