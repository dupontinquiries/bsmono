#pragma once
// ─── DhtService.hpp ──────────────────────────────────────────────────────────
//
// The single, app-wide access point to the one DHT node.
//
// # Why this exists
//
// There is exactly one DHT node in the app: AppController creates one
// DhtDiscovery in initialize() and tears it down once at shutdown. But that
// singleness is *implicit* — nothing prevents a future UI surface from spinning
// up its own node, and every consumer today reaches the DHT through an ad-hoc
// `qobject_cast<DhtDiscovery*>(controller backend)`, which means each one needs
// an AppController pointer threaded in and each subscribes to the raw node
// separately.
//
// DhtService makes the singleness explicit and enforced, and gives the UI ONE
// handle to tie to:
//
//   * AppController registers its DhtDiscovery here once (registerDiscovery).
//     The service does NOT create a node — it references the canonical one.
//   * A second registration while one is live is refused with a warning. Two
//     live nodes would double the DHT's memory and battery cost (routing table,
//     sockets, periodic re-announce traffic), so this guards against exactly the
//     regression we want to avoid as the UI grows.
//   * UI components connect to DhtService's forwarded signals instead of each
//     casting and subscribing to the raw node — one set of connections for the
//     whole app rather than N redundant ones.
//
// This is an access/observation layer only. It does not change the DHT's
// lifecycle or behaviour — DhtResourceManager still owns the battery-aware
// up/down decisions, and background reception is unaffected.

#include <QObject>
#include <QByteArray>
#include <QList>

#include "core/Types.hpp"  // SnapClone::Core::UserId

namespace SnapClone::Network {

class DhtDiscovery;

class DhtService : public QObject
{
    Q_OBJECT
public:
    /// The app-wide instance. Created on first use, lives for the whole process.
    static DhtService *instance();

    /// Register the canonical DHT node (called once by AppController). Passing a
    /// second, different node while one is registered is refused (see header).
    void registerDiscovery(DhtDiscovery *dht);

    /// Drop the registered node (called by AppController at shutdown, before the
    /// node is destroyed). A nullptr forces a clear; a non-null only clears if it
    /// matches the registered node.
    void clearDiscovery(DhtDiscovery *dht);

    /// The one node, or nullptr if none is currently registered.
    DhtDiscovery *discovery() const { return m_dht; }

    /// True while a node is registered.
    bool isAvailable() const { return m_dht != nullptr; }

signals:
    /// A node became available / unavailable.
    void availabilityChanged(bool available);

    /// Forwarded from the underlying DHT so UI surfaces subscribe HERE, once.
    void connectionRequestReceived(SnapClone::Core::UserId fromId, QByteArray encryptedBlob);
    void peerPublicKeyDiscovered(SnapClone::Core::UserId peerId, QByteArray pubKey);

private:
    explicit DhtService(QObject *parent = nullptr);
    DhtService(const DhtService &) = delete;
    DhtService &operator=(const DhtService &) = delete;

    DhtDiscovery                  *m_dht = nullptr;
    QList<QMetaObject::Connection> m_conns;  // forwarding connections, torn down on clear
};

}  // namespace SnapClone::Network
