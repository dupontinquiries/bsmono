#include "network/MagicWormholeRelayTransport.hpp"
#include "core/Timings.hpp"
#include <QCryptographicHash>
#include <QHostAddress>
#include <QUrl>
#include <QLoggingCategory>

Q_LOGGING_CATEGORY(lcMwRelay, "snapclone.mwrelay")

namespace SnapClone::Network {

MagicWormholeRelayTransport::MagicWormholeRelayTransport(QObject *parent)
    : ITransport(parent)
{
    m_connectTimer.setSingleShot(true);
    m_connectTimer.setInterval(Timings::kRelayConnectMs);
    connect(&m_connectTimer, &QTimer::timeout, this, [this]() {
        qCWarning(lcMwRelay) << "Connection timeout";
        m_socket.abort();
        emit errorOccurred(QStringLiteral("Magic Wormhole relay: connection timeout"));
    });

    connect(&m_socket, &QTcpSocket::connected,      this, &MagicWormholeRelayTransport::onConnected);
    connect(&m_socket, &QTcpSocket::readyRead,       this, &MagicWormholeRelayTransport::onReadyRead);
    connect(&m_socket, &QTcpSocket::disconnected,    this, &MagicWormholeRelayTransport::onDisconnected);
    connect(&m_socket, &QAbstractSocket::errorOccurred,
            this, &MagicWormholeRelayTransport::onError);
}

MagicWormholeRelayTransport::~MagicWormholeRelayTransport()
{
    m_socket.abort();
}

QString MagicWormholeRelayTransport::sessionToken(const QString &id1, const QString &id2)
{
    // Deterministic sort so both peers always derive the same token.
    // Magic Wormhole transit relay requires exactly 64 lowercase hex chars (256-bit).
    const QString sorted = (id1 < id2) ? (id1 + id2) : (id2 + id1);
    return QString::fromLatin1(
        QCryptographicHash::hash(sorted.toUtf8(), QCryptographicHash::Sha256)
            .toHex()); // full 64-char hex — relay rejects shorter tokens
}

void MagicWormholeRelayTransport::connectToRelay(const Core::UserId &selfId,
                                                  const Core::UserId &peerId,
                                                  const QString      &tcpUrl)
{
    // Save connection params for use in onDisconnected() reconnect path.
    m_selfId         = selfId;
    m_peerId         = peerId;
    m_relayUrl       = tcpUrl;
    m_reconnectCount = 0; // fresh explicit connect resets backoff counter

    // Parse tcp://host:port
    QUrl url(tcpUrl);
    const QString host = url.host();
    const quint16 port = static_cast<quint16>(url.port(4001));
    if (host.isEmpty()) {
        qCWarning(lcMwRelay) << "Invalid relay URL:" << tcpUrl;
        emit errorOccurred(QStringLiteral("Invalid relay URL: ") + tcpUrl);
        return;
    }

    const QString token = sessionToken(selfId.value, peerId.value);
    qCInfo(lcMwRelay) << "Connecting to Magic Wormhole relay"
                      << host << ":" << port
                      << "token:" << token.left(12) << "...";

    m_handshakeDone = false;
    m_readBuf.clear();
    m_connectTimer.start();

    // Store token for use in onConnected
    m_socket.setProperty("mw_token", token);
    m_socket.connectToHost(host, port);
}

void MagicWormholeRelayTransport::onConnected()
{
    m_connectTimer.stop();
    const QString token = m_socket.property("mw_token").toString();
    // Magic Wormhole transit handshake: "please relay TOKEN\n"
    const QByteArray handshake =
        QByteArrayLiteral("please relay ") + token.toLatin1() + '\n';
    m_socket.write(handshake);
    qCInfo(lcMwRelay) << "Sent relay handshake, waiting for ok...";
}

void MagicWormholeRelayTransport::onReadyRead()
{
    m_readBuf.append(m_socket.readAll());
    // Cap total read buffer to prevent memory exhaustion from malformed relay servers.
    static constexpr int kMaxBufBytes = 16 * 1024 * 1024; // 16 MB
    if (m_readBuf.size() > kMaxBufBytes) {
        qCWarning(lcMwRelay) << "Read buffer overflow (" << m_readBuf.size()
                             << "bytes) — aborting relay connection";
        m_socket.abort();
        return;
    }

    if (!m_handshakeDone) {
        // Wait for "ok\n" from relay
        const int nl = static_cast<int>(m_readBuf.indexOf('\n'));
        if (nl < 0) return; // incomplete
        const QByteArray response = m_readBuf.left(nl).trimmed();
        m_readBuf.remove(0, nl + 1);

        if (response != "ok") {
            qCWarning(lcMwRelay) << "Relay rejected handshake:" << response;
            m_socket.abort();
            emit errorOccurred(QStringLiteral("Relay rejected: ") +
                               QString::fromLatin1(response));
            return;
        }
        m_handshakeDone = true;
        qCInfo(lcMwRelay) << "Magic Wormhole relay paired — connection ready";
        emit connected();
    }

    // Process any data that arrived after the handshake
    processIncomingFrames();
}

void MagicWormholeRelayTransport::processIncomingFrames()
{
    // Framing: 4-byte big-endian length + payload (same as our WebSocket transport)
    while (m_readBuf.size() >= 4) {
        const quint32 len =
            (static_cast<quint8>(m_readBuf[0]) << 24) |
            (static_cast<quint8>(m_readBuf[1]) << 16) |
            (static_cast<quint8>(m_readBuf[2]) <<  8) |
             static_cast<quint8>(m_readBuf[3]);

        if (len > 64 * 1024 * 1024) { // 64 MB sanity check
            qCWarning(lcMwRelay) << "Frame too large:" << len << "bytes — aborting";
            m_socket.abort();
            return;
        }
        if (m_readBuf.size() < 4 + static_cast<int>(len)) break; // incomplete

        const QByteArray payload = m_readBuf.mid(4, static_cast<int>(len));
        m_readBuf.remove(0, 4 + static_cast<int>(len));
        emit messageReceived(payload);
    }
}

void MagicWormholeRelayTransport::sendFramed(const QByteArray &data)
{
    const quint32 len = static_cast<quint32>(data.size());
    QByteArray frame(4 + data.size(), Qt::Uninitialized);
    frame[0] = static_cast<char>((len >> 24) & 0xFF);
    frame[1] = static_cast<char>((len >> 16) & 0xFF);
    frame[2] = static_cast<char>((len >>  8) & 0xFF);
    frame[3] = static_cast<char>( len        & 0xFF);
    frame.replace(4, data.size(), data);
    m_socket.write(frame);
}

void MagicWormholeRelayTransport::send(const QByteArray &payload)
{
    if (!m_handshakeDone) {
        qCWarning(lcMwRelay) << "send() called before handshake complete";
        return;
    }
    sendFramed(payload);
}

void MagicWormholeRelayTransport::disconnect()
{
    m_connectTimer.stop();
    m_socket.abort();
}

bool MagicWormholeRelayTransport::isConnected() const
{
    return m_handshakeDone && m_socket.state() == QAbstractSocket::ConnectedState;
}

void MagicWormholeRelayTransport::onDisconnected()
{
    m_connectTimer.stop();
    const bool wasConnected = m_handshakeDone;
    m_handshakeDone = false;
    m_readBuf.clear();

    if (wasConnected && m_reconnectCount < kMaxReconnects && !m_relayUrl.isEmpty()) {
        // Relay server dropped us (idle timeout, server restart, etc.)
        // Reconnect with exponential backoff: 2s → 4s → 8s.
        const int delayMs = (1 << m_reconnectCount) * 2000;
        ++m_reconnectCount;
        qCInfo(lcMwRelay) << "Relay disconnected — reconnect attempt"
                          << m_reconnectCount << "of" << kMaxReconnects
                          << "in" << (delayMs/1000) << "s";
        QTimer::singleShot(delayMs, this, [this]() {
            if (m_socket.state() == QAbstractSocket::UnconnectedState
                    && !m_relayUrl.isEmpty())
                connectToRelay(m_selfId, m_peerId, m_relayUrl);
        });
    } else {
        m_reconnectCount = 0;
        if (wasConnected) emit disconnected();
    }
}

void MagicWormholeRelayTransport::onError(QAbstractSocket::SocketError)
{
    qCWarning(lcMwRelay) << "TCP error:" << m_socket.errorString();
    m_connectTimer.stop();
    emit errorOccurred(m_socket.errorString());
}

} // namespace SnapClone::Network
