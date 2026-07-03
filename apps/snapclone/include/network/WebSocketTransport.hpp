#pragma once
#include "network/NetworkManager.hpp"
#include <QWebSocket>
#include <QTimer>
#include <memory>

namespace SnapClone::Network {

class WebSocketTransport : public ITransport
{
    Q_OBJECT
public:
    explicit WebSocketTransport(QObject *parent = nullptr);
    ~WebSocketTransport() override;
    void connectToPeer(const Core::PeerAddress &address) override;
    // Extend the pending connect timeout by msec (additive).
    // Call when a hole punch completes so TCP has time to traverse the hole.
    void extendConnectTimeout(int msec);
    void adoptSocket(QWebSocket *socket);
    void disconnect()  override;
    void send(const QByteArray &payload) override;
    [[nodiscard]] bool isConnected() const override;

private slots:
    void onConnected();
    void onDisconnected();
    void onBinaryMessageReceived(const QByteArray &message);
    void onError(QAbstractSocket::SocketError socketError);
    /// Pong from peer — resets unanswered-ping counter.
    void onPong(quint64 elapsedTime, const QByteArray &payload);

private:
    void connectSocketSignals();
    QWebSocket                  m_socket;
    std::unique_ptr<QWebSocket> m_adopted;
    QTimer                      m_connectTimer; // destroyed with transport — safe
    // Keepalive ping every 30s while connected. Consumer-grade NAT/firewall
    // hardware silently drops idle TCP after ~5 min; without periodic
    // traffic, the socket appears connected on both sides until one tries
    // to send and gets a TIMEOUT after a multi-minute wait. WS ping frames
    // are 6 bytes per direction — completely free.
    //
    // We also track pong replies: if 3 consecutive pings (90 s) get no
    // response, force a disconnect rather than waiting up to 3 minutes for
    // the underlying TCP keepalive to time out. The reconnect path can
    // then try other addresses (relay, alt LAN IP, etc.) sooner — that's
    // the "situational adaptiveness" the user asked for.
    QTimer                      m_keepaliveTimer;
    int                         m_unansweredPings = 0;
    static constexpr int        kMaxUnansweredPings = 3;
    // 2 MB: enough for any snap payload (JPEG ~500KB + crypto overhead).
    // 50 MB was the original limit — 100× too large; a rogue peer could
    // force 50 MB to be buffered before rejection.
    static constexpr qsizetype kMaxFrameBytes = 2 * 1024 * 1024;
};

} // namespace SnapClone::Network
