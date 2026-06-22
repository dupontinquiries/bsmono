#pragma once
// ─── RelayTransport.hpp ───────────────────────────────────────────────────────
// WebSocket relay transport for symmetric-NAT fallback.
// When hole punch fails, both peers connect outbound to a relay server.
// The relay pairs them by session token and forwards all frames.
// Session token = SHA-256(smaller_scId + larger_scId) — same on both ends.
// ─────────────────────────────────────────────────────────────────────────────
#include "network/NetworkManager.hpp"
#include <QWebSocket>
#include <QTimer>

namespace SnapClone::Network {

class RelayTransport : public ITransport
{
    Q_OBJECT
public:
    explicit RelayTransport(QObject *parent = nullptr);
    ~RelayTransport() override;

    // Connect to relay pairing with peerId using our own scId.
    void connectToRelay(const Core::UserId &selfId,
                        const Core::UserId &peerId,
                        const QString      &relayUrl);

    // ITransport interface
    void connectToPeer(const Core::PeerAddress &) override {} // unused — use connectToRelay
    void disconnect()  override;
    void send(const QByteArray &payload) override;
    [[nodiscard]] bool isConnected() const override;

private slots:
    void onConnected();
    void onDisconnected();
    void onBinaryMessageReceived(const QByteArray &message);
    void onError(QAbstractSocket::SocketError err);

private:
    static QString sessionToken(const QString &id1, const QString &id2);
    QWebSocket m_socket;
    QTimer     m_connectTimer;
    QTimer     m_pingTimer;
};

} // namespace SnapClone::Network
