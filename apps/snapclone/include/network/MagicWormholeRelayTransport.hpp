#pragma once
// MagicWormholeRelayTransport — TCP relay using the Magic Wormhole transit protocol.
// Protocol: connect TCP, send "please relay TOKEN\n", wait for "ok\n",
// then bidirectional raw forwarding. Payload is end-to-end encrypted by
// the ECDH layer — the relay sees only ciphertext.
//
// Compatible with transit.magic-wormhole.io:4001 and any
// magic-wormhole-transit-relay server.

#include "network/NetworkManager.hpp"
#include "core/Types.hpp"
#include <QTcpSocket>
#include <QTimer>
#include <QByteArray>

namespace SnapClone::Network {

class MagicWormholeRelayTransport : public ITransport
{
    Q_OBJECT
public:
    explicit MagicWormholeRelayTransport(QObject *parent = nullptr);
    ~MagicWormholeRelayTransport() override;

    void connectToRelay(const Core::UserId &selfId,
                        const Core::UserId &peerId,
                        const QString      &tcpUrl); // "tcp://host:port"

    // ITransport interface
    void connectToPeer(const Core::PeerAddress &) override {} // not used
    void disconnect()  override;
    void send(const QByteArray &payload) override;
    [[nodiscard]] bool isConnected() const override;

    // Compute the shared session token (same as RelayTransport)
    static QString sessionToken(const QString &id1, const QString &id2);

private slots:
    void onConnected();
    void onReadyRead();
    void onError(QAbstractSocket::SocketError);
    void onDisconnected();

private:
    QTcpSocket   m_socket;
    QTimer       m_connectTimer;
    QByteArray   m_readBuf;
    bool         m_handshakeDone  = false;

    // Reconnect state — repopulated by connectToRelay() for use in onDisconnected()
    Core::UserId m_selfId;
    Core::UserId m_peerId;
    QString      m_relayUrl;
    int          m_reconnectCount = 0;
    static constexpr int kMaxReconnects = 3;

    // Framing: 4-byte big-endian length prefix before each payload
    void sendFramed(const QByteArray &data);
    void processIncomingFrames();
};

} // namespace SnapClone::Network
