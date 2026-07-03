// ─── DhtService.cpp ──────────────────────────────────────────────────────────
#include "network/DhtService.hpp"
#include "network/JamiAdapter.hpp"  // full DhtDiscovery definition (signals)

#include <QLoggingCategory>

namespace SnapClone::Network {

Q_LOGGING_CATEGORY(lcDhtSvc, "snapclone.dht.service")

DhtService::DhtService(QObject *parent) : QObject(parent) {}

DhtService *DhtService::instance()
{
    // App-lifetime singleton: heap-allocated and intentionally never deleted so
    // it outlives every UI surface and the DHT node itself, avoiding QObject
    // teardown-ordering hazards at process exit. Function-local-static init is
    // thread-safe (C++11). One node, one access point, for the whole app.
    static DhtService *s = new DhtService();
    return s;
}

void DhtService::registerDiscovery(DhtDiscovery *dht)
{
    if (m_dht == dht)
        return;  // idempotent

    if (m_dht != nullptr && dht != nullptr) {
        qCWarning(lcDhtSvc)
            << "registerDiscovery: a DHT node is already registered; refusing a"
               " second. Two live nodes would double DHT memory/battery cost.";
        return;
    }

    for (const auto &c : m_conns)
        QObject::disconnect(c);
    m_conns.clear();

    m_dht = dht;
    if (m_dht) {
#if defined(SNAPCLONE_OPENDHT_BACKEND) && !defined(SNAPCLONE_JAMI_BACKEND)
        // DhtDiscovery's full type (and its signals) only exist in this backend.
        // Forward the node's signals so UI surfaces tie to the service, not the
        // raw node. Signal-to-signal: emitting downstream is a no-op when nothing
        // is connected, so this is free until something subscribes.
        m_conns << connect(m_dht, &DhtDiscovery::connectionRequestReceived,
                           this, &DhtService::connectionRequestReceived);
        m_conns << connect(m_dht, &DhtDiscovery::peerPublicKeyDiscovered,
                           this, &DhtService::peerPublicKeyDiscovered);
        // If the node is destroyed without an explicit clear, null out so
        // discovery()/isAvailable() never dangle.
        m_conns << connect(m_dht, &QObject::destroyed, this, [this](QObject *) {
            m_dht = nullptr;
            emit availabilityChanged(false);
        });
#endif
    }
    emit availabilityChanged(m_dht != nullptr);
}

void DhtService::clearDiscovery(DhtDiscovery *dht)
{
    if (dht != nullptr && dht != m_dht)
        return;  // not the node we hold

    for (const auto &c : m_conns)
        QObject::disconnect(c);
    m_conns.clear();

    if (m_dht) {
        m_dht = nullptr;
        emit availabilityChanged(false);
    }
}

}  // namespace SnapClone::Network
