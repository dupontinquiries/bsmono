#include "network/QtWebSocketCompat.hpp"
#include "network/RelayTransport.hpp"
#include <QTimer>
#include <QSslConfiguration>
#include <QSslSocket>
#include <QLoggingCategory>
#include <QCryptographicHash>
#include <QSettings>

Q_LOGGING_CATEGORY(lcRelay, "snapclone.relay")

namespace SnapClone::Network {

RelayTransport::RelayTransport(QObject *parent)
    : ITransport(parent)
{
    m_connectTimer.setSingleShot(true);
    m_connectTimer.setInterval(20000); // 20s to pair with peer
    m_connectTimer.setTimerType(Qt::VeryCoarseTimer); // battery: batch wakeups
    connect(&m_connectTimer, &QTimer::timeout, this, [this]() {
        if (m_socket.state() != QAbstractSocket::ConnectedState) {
            qCWarning(lcRelay) << "Relay connection timeout";
            m_socket.abort();
            emit errorOccurred(QStringLiteral("Relay timeout"));
        }
    });
    connect(&m_socket, &QWebSocket::connected,
            this, &RelayTransport::onConnected);
    connect(&m_socket, &QWebSocket::disconnected,
            this, &RelayTransport::onDisconnected);
    connect(&m_socket, &QWebSocket::binaryMessageReceived,
            this, &RelayTransport::onBinaryMessageReceived);
    connect(&m_socket, SC_QWEBSOCKET_ERROR_SIGNAL,
            this, &RelayTransport::onError);
    m_pingTimer.setInterval(30000);
    m_pingTimer.setSingleShot(false);
    m_pingTimer.setTimerType(Qt::VeryCoarseTimer); // battery: 30s ping needn't be precise
    connect(&m_pingTimer, &QTimer::timeout, this, [this]() {
        if (m_socket.state() == QAbstractSocket::ConnectedState)
            m_socket.ping();
    });
}

RelayTransport::~RelayTransport()
{
    m_connectTimer.stop();
    m_socket.QObject::disconnect();
    m_socket.abort();
}

void RelayTransport::connectToRelay(const Core::UserId &selfId,
                                     const Core::UserId &peerId,
                                     const QString      &relayUrl)
{
    const QString token = sessionToken(selfId.value, peerId.value);
    // Note: tcp:// URLs (Magic Wormhole transit relay) are handled by
    // MagicWormholeRelayTransport, not this class. AppController selects
    // the correct transport class based on the URL scheme.

    const QString url = relayUrl + QStringLiteral("/relay/") + token;
    qCInfo(lcRelay) << "Connecting to relay" << url.left(40) << "...";

    // Accept self-signed TLS certs on the relay — payload is E2E-encrypted by
    // our ECDH layer so the relay cert is only needed for firewall traversal,
    // not for security. A wss:// relay URL with a self-signed cert still gets
    // you through HTTPS-only corporate firewalls.
    if (url.startsWith(QStringLiteral("wss://"), Qt::CaseInsensitive)) {
        QSslConfiguration cfg = QSslConfiguration::defaultConfiguration();
        cfg.setPeerVerifyMode(QSslSocket::VerifyNone);
        m_socket.setSslConfiguration(cfg);
    }

    m_connectTimer.start();
    m_socket.open(QUrl(url));
}

QString RelayTransport::sessionToken(const QString &id1, const QString &id2)
{
    // Sort so both peers compute the same token regardless of who goes first.
    const QString sorted = (id1 < id2) ? (id1 + id2) : (id2 + id1);
    return QString::fromLatin1(
        QCryptographicHash::hash(sorted.toUtf8(), QCryptographicHash::Sha256)
            .toHex().left(32)); // 128-bit token in hex
}

void RelayTransport::disconnect()
{
    m_socket.QObject::disconnect();
    m_socket.abort();
}

void RelayTransport::send(const QByteArray &payload)
{
    if (m_socket.state() != QAbstractSocket::ConnectedState) return;
    m_socket.sendBinaryMessage(payload);
}

bool RelayTransport::isConnected() const
{
    return m_socket.state() == QAbstractSocket::ConnectedState;
}

void RelayTransport::onConnected()
{
    m_connectTimer.stop();
    qCInfo(lcRelay) << "Relay connected — waiting for peer to pair";
    // Ping relay every 30s to prevent nginx/proxy idle timeout (default: 60s).
    m_pingTimer.start();
    emit connected();
}

void RelayTransport::onDisconnected()
{
    m_pingTimer.stop();
    qCInfo(lcRelay) << "Relay disconnected";
    emit disconnected();
}

void RelayTransport::onBinaryMessageReceived(const QByteArray &message)
{
    emit messageReceived(message);
}

void RelayTransport::onError(QAbstractSocket::SocketError)
{
    qCWarning(lcRelay) << "Relay error:" << m_socket.errorString();
    emit errorOccurred(m_socket.errorString());
}

} // namespace SnapClone::Network
