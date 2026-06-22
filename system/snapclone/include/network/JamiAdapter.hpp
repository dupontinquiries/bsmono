#pragma once

// ─── Network/JamiAdapter.hpp ──────────────────────────────────────────────────
//
// Jami DHT backend — internet-scale peer discovery and NAT traversal.
//
// Jami uses OpenDHT for decentralised peer lookup and ICE/TURN (via dhtnet)
// for hole-punching through NAT.  The combination works across the internet
// without any central server, complementing the LAN-only gossip backend.
//
// Build:
//   cmake -DSNAPCLONE_JAMI_BACKEND=ON ...
//
//   Ubuntu/Debian:  sudo apt install libjami-dev
//   Alpine/postmarketOS: apk add jami-daemon-dev
//
//   Then install the CMake flag and rebuild:
//   cmake -B build -DSNAPCLONE_JAMI_BACKEND=ON && cmake --build build
//
// Dual backend:
//   AppController wires BOTH GossipDiscovery (LAN) and JamiDiscovery (internet).
//   GossipDiscovery is the primary backend passed to NetworkManager.
//   JamiDiscovery is attached as a secondary: its peerDiscovered signal connects
//   to NetworkManager::onPeerDiscovered directly.
//
// Data flow with Jami:
//   1. JamiDiscovery::lookupPeer(id)  → DHT name lookup
//   2. peerDiscovered(id, "jami://ID") → NetworkManager::connectToPeer
//   3. NetworkManager calls JamiTransport::connectToPeer
//   4. JamiTransport::send(payload)   → sendMessage() → Jami network
//   5. Remote side: ConversationSignal::MessageReceived → messageReceived()
//   6. NetworkManager::payloadReceived → SnapRouter → InboxWidget
// ─────────────────────────────────────────────────────────────────────────────

#include "network/NetworkManager.hpp"
#include <optional>

namespace SnapClone::Network {

struct JamiAccountInfo {
    QString accountId;    ///< libjami internal UUID
    QString jamiId;       ///< 40-char hex DHT public key
    QString displayName;
    bool    registered = false;
};

// ─── Discovery via Jami DHT ───────────────────────────────────────────────────

class JamiDiscovery : public IDiscovery
{
    Q_OBJECT

public:
    explicit JamiDiscovery(QObject *parent = nullptr);
    ~JamiDiscovery() override;

    void start()  override;
    void stop()   override;
    void announcePresence(const Core::UserId &self,
                          const Core::PeerAddress &addr) override;
    void lookupPeer(const Core::UserId &id) override;

    [[nodiscard]] std::optional<JamiAccountInfo> accountInfo() const;
    [[nodiscard]] static bool isAvailable();

signals:
    void jamiAccountReady(SnapClone::Network::JamiAccountInfo info);
    void jamiInboundChannel(QString peerId, QString conversationId);
    /// Raw snap payload received via Jami conversation — forwarded to NetworkManager.
    void jamiPayloadReceived(SnapClone::Core::UserId senderId,
                             QByteArray payload,
                             QString conversationId);

private slots:
    void onJamiAccountStateChanged(const QString &accountId, const QString &state);
    void onJamiNameLookupResult(const QString &accountId, int status,
                                const QString &name, const QString &jamiId);
    void onConversationRequestReceived(const QString &accountId,
                                       const QString &conversationId);
    void onJamiMessageReceived(const QString &accountId,
                               const QString &conversationId,
                               const QString &fromJamiId,
                               const QByteArray &payload);

private:
    void createAccount();
    void registerCallbacks();

    struct Impl;
    std::unique_ptr<Impl> d;
};

// ─── Transport via Jami Conversation ─────────────────────────────────────────

class JamiTransport : public ITransport
{
    Q_OBJECT

public:
    explicit JamiTransport(const QString &accountId,
                           const QString &conversationId,
                           QObject *parent = nullptr);
    ~JamiTransport() override;

    void connectToPeer(const Core::PeerAddress &address) override;
    void disconnect()  override;
    void send(const QByteArray &payload) override;
    [[nodiscard]] bool isConnected() const override;

private slots:
    void onJamiCallStateChanged(const QString &callId, const QString &state);
    void onJamiDataReceived(const QString &callId, const QByteArray &data);

private:
    QString m_accountId;
    QString m_callId;    // reused as conversationId in modern API

    struct Impl;
    std::unique_ptr<Impl> d;
};

} // namespace SnapClone::Network

Q_DECLARE_METATYPE(SnapClone::Network::JamiAccountInfo)

// ─── OpenDHT direct backend ──────────────────────────────────────────────────
// Used when SNAPCLONE_OPENDHT_BACKEND=ON (postmarketOS / no libjami-dev).
// Implements IDiscovery using opendht::DhtRunner directly.
// Peers publish their WebSocket endpoint to the DHT and look up others by userId.
// No new transport is needed — discovery only; snaps travel over WebSocket as normal.

#if defined(SNAPCLONE_OPENDHT_BACKEND) && !defined(SNAPCLONE_JAMI_BACKEND)

namespace SnapClone::Core { class AppContextProvider; }

namespace SnapClone::Network {

class DhtDiscovery : public IDiscovery
{
    Q_OBJECT

public:
    explicit DhtDiscovery(QObject *parent = nullptr);
    ~DhtDiscovery() override;

    void start()  override;
    void stop()   override;
    void setReannounceInterval(int ms); ///< DhtResourceManager calls this
    void announcePresence(const Core::UserId &self,
                          const Core::PeerAddress &addr) override;
    void lookupPeer(const Core::UserId &id) override;
    void lookupPeerReplica(const Core::UserId &id) override;  // V32.45: backup key

    // ── Two-step DHT connection requests ─────────────────────────────
    // blob = sc-from_id + "\n" + nonce(12) + ciphertext — DHT stores opaque bytes.
    void publishConnectionRequest(const Core::UserId &target,
                                  const QByteArray &encryptedBlob);
    void checkConnectionRequests(const Core::UserId &self);
    // Call once after identity key generation so our public key
    // is embedded in every DHT announce — enables pre-connect encryption.
    void setSelfPublicKey(const QByteArray &pubKey);

    /// V32.28: optional AppContext attachment. When attached, the
    /// 30s bootstrap retry stretches to 60s if reachability has
    /// recently flapped — bootstrapping while the network is half-
    /// broken is wasted work. Nullptr-safe (fixed 30s if no context).
    void setAppContext(Core::AppContextProvider *ctx);

private:
    void announceAddress(const Core::UserId &self,
                          const QString &host, quint16 wsPort,
                          quint16 punchPort = 0);
    void flushPendingAnnounce(const Core::UserId &self);
    // V32.45: shared lookup body used by both lookupPeer (primary key) and
    // lookupPeerReplica (the "/r2" backup key). `replica` only affects logging.
    void lookupAtKey(const Core::UserId &id, const QByteArray &keyStr, bool replica);
    void setPunchPort(quint16 punchPort) override;
    void setStunHost(const QString &stunHost) override;

signals:
    // Carries raw encrypted blob — AppController decrypts with ICryptoService.
    // Blob format: sc-from_id + "\n" + nonce(12 bytes) + ciphertext
    void connectionRequestReceived(SnapClone::Core::UserId fromId,
                                   QByteArray encryptedBlob);
    // Emitted when a DHT lookup result includes a peer's X25519 public key.
    // AppController registers it so encryptFor works before first connect.
    void peerPublicKeyDiscovered(SnapClone::Core::UserId peerId,
                                 QByteArray pubKey);

private:
    struct Impl;
    std::unique_ptr<Impl> d;
};

} // namespace SnapClone::Network

#endif // SNAPCLONE_OPENDHT_BACKEND
