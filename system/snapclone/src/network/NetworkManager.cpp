#include "network/NetworkManager.hpp"
#include "network/WebSocketTransport.hpp"
#include "network/AddressClassification.hpp"
#include "network/NetworkStats.hpp"
#include "core/AppContext.hpp"  // V32.27: context-aware LAN-isolation threshold
#include "core/VoiceWirePacket.hpp"  // identify ephemeral voice frames (never queued)

#include <QLoggingCategory>
#include <QDateTime>
#include <QTimer>
#include <QWebSocketServer>
#include <QNetworkInterface>
#include <QRegularExpression>

// ─── Network/NetworkManager.cpp ───────────────────────────────────────────────
//
// Signal flow (see also: docs/SignalRegistry.md):
//
//   Inbound path:
//     QWebSocketServer::newConnection
//       → onInboundConnection()            [accepts socket, creates session]
//       → WebSocketTransport::messageReceived
//       → payloadReceived()                [consumed by SnapRouter]
//
//   Discovery path:
//     IDiscovery::peerDiscovered
//       → onPeerDiscovered()               [creates outbound session]
//       → connectToPeer()
//       → ITransport::connected
//       → peerConnected()                  [consumed by AppController → UI]
//
//   Disconnect / retry path:
//     ITransport::disconnected
//       → onTransportDisconnected(id)      [explicit id — NO sender() usage]
//       → peerDisconnected()               [consumed by AppController → UI]
//       → QTimer::singleShot(backoff)
//       → connectToPeer()                  [retry loop]
//
// ─────────────────────────────────────────────────────────────────────────────

Q_LOGGING_CATEGORY(lcNet, "snapclone.network")

namespace SnapClone::Network {

// ─── Construction ─────────────────────────────────────────────────────────────

NetworkManager::NetworkManager(
    std::unique_ptr<IDiscovery> discovery,
    QObject *parent)
    : QObject(parent)
    , m_discovery(std::move(discovery))
    , m_server(new QWebSocketServer(
          QStringLiteral("SnapClone"),
          QWebSocketServer::NonSecureMode,
          this))  // QWebSocketServer owned by QObject parent tree
{
    // Discovery is re-parented to us so its lifetime is managed by Qt.
    // This is safe here because m_discovery was just moved-from a unique_ptr
    // and its parent was nullptr.
    m_discovery->setParent(this);

    connect(m_discovery.get(), &IDiscovery::peerDiscovered,
            this, &NetworkManager::onPeerDiscovered);

    connect(m_discovery.get(), &IDiscovery::peerLost,
            this, [this](const Core::UserId &id) {
                // Only emit peerDisconnected if there is no live WebSocket
                // session for this peer. Gossip pruning is conservative
                // (it fires if we miss a few heartbeats) but the actual
                // transport may still be connected — e.g. an inbound session
                // that was promoted to the real user ID. Emitting
                // peerDisconnected in that case would wrongly mark the peer
                // offline in the friend list.
                const auto it = m_sessions.find(id.value.toStdString());
                const bool sessionAlive = (it != m_sessions.end())
                    && it->second && it->second->connected;
                if (sessionAlive) {
                    qCDebug(lcNet) << "Discovery pruned" << id.value
                                   << "but session is still live — suppressing peerDisconnected.";
                    return;
                }
                qCInfo(lcNet) << "Discovery: peer lost" << id.value;
                emit peerDisconnected(id);
            });

    // Accept inbound WebSocket connections from remote peers.
    connect(m_server, &QWebSocketServer::newConnection,
            this, &NetworkManager::onInboundConnection);
}

NetworkManager::~NetworkManager()
{
    stop();
}

// ─── Lifecycle ────────────────────────────────────────────────────────────────

void NetworkManager::start(const Core::UserId &selfId, quint16 listenPort)
{
    m_selfId = selfId;

    // Start the inbound server. Try the configured port first, then two
    // fallbacks in case a crashed previous instance still holds the port.
    const quint16 fallbacks[] = { listenPort,
                                  quint16(listenPort + 1),
                                  quint16(listenPort + 2) };
    bool bound = false;
    for (quint16 port : fallbacks) {
        if (m_server->listen(QHostAddress::AnyIPv4, port)) {
            if (port != listenPort)
                qCWarning(lcNet) << "Port" << listenPort
                                 << "in use — WebSocket server on port" << port;
            else
                qCInfo(lcNet) << "WebSocket server listening on port" << port;
            listenPort = port; // use actual bound port for announcements
            bound = true;
            break;
        }
    }
    if (!bound)
        qCWarning(lcNet) << "WebSocket server failed to bind any port"
                         << "– inbound connections will not work";

    // Resolve our actual LAN address to advertise in gossip.
    // Advertising "0.0.0.0" would give remote peers an unreachable address.
    //
    // V32.36+ (device logs): MUST use AddressClass::pickBestLanAddress, not a
    // naive "first non-loopback IPv4" loop. On the FP5, allAddresses()
    // enumerates usb0 (172.16.42.1, the USB-tether interface) BEFORE wlan0
    // (192.168.2.x), so the first-match loop advertised the unreachable tether
    // IP. Both phones then announced ws://172.16.42.1:9876, each peer tried to
    // reach the other at an address that wasn't on the WiFi subnet, no session
    // ever opened ("No active session — queuing"), and every snap queued
    // forever without an ACK. The picker ranks 192.168/10 above the 172.16
    // tether range so wlan0 wins. (GossipDiscovery::announcePresence also picks
    // the best address, but only when handed "0.0.0.0"; passing it a concrete
    // wrong IP skipped that correction — so we fix it at the source here.)
    QStringList lanCandidates;
    for (const QHostAddress &addr : QNetworkInterface::allAddresses()) {
        if (!addr.isLoopback() && addr.protocol() == QAbstractSocket::IPv4Protocol)
            lanCandidates << addr.toString();
    }
    QString lanIp = AddressClass::pickBestLanAddress(lanCandidates);
    if (lanIp.isEmpty()) {
        // No ranked LAN address — fall back to first candidate, then loopback.
        lanIp = lanCandidates.isEmpty() ? QStringLiteral("127.0.0.1")
                                        : lanCandidates.first();
        qCWarning(lcNet) << "No ranked LAN address; advertising" << lanIp
                         << "from candidates" << lanCandidates;
    } else {
        qCInfo(lcNet) << "Advertising LAN address" << lanIp
                      << "(picked from" << lanCandidates << ")";
    }

    const Core::PeerAddress selfAddr{ lanIp, listenPort };
    m_discovery->start();
    m_discovery->announcePresence(selfId, selfAddr);

    qCInfo(lcNet) << "NetworkManager started — self:" << selfId.value
                  << "server: ws://" + lanIp + ":" + QString::number(listenPort) + "/snap"
                  << "(open port" << listenPort << "tcp on this device for inbound connections)";
}

void NetworkManager::stop()
{
    // Stop discovery first so no new peer notifications arrive during teardown.
    m_discovery->stop();

    // Close the inbound server — no new connections after this.
    m_server->close();

    // Close all transports — disconnect signals first so no callbacks fire
    // into the half-destroyed session map, then release via deleteLater()
    // so transport destructors run after the current call stack unwinds.
    // Synchronous destruction here caused crashes in Qt's event dispatch
    // when destructors internally called QMetaObject::activate.
    for (auto &[key, session] : m_sessions) {
        if (session && session->transport) {
            session->transport->QObject::disconnect(); // detach all Qt signals
            session->transport->disconnect();           // close the socket
            session->transport.release()->deleteLater(); // defer destructor
        }
    }
    m_sessions.clear();

    qCDebug(lcNet) << "NetworkManager stopped — all sessions closed.";
}

// ─── Public slots ─────────────────────────────────────────────────────────────

void NetworkManager::sendToPeer(Core::UserId recipientId, QByteArray payload)
{
    const std::string key = recipientId.value.toStdString();
    auto it = m_sessions.find(key);

    if (it == m_sessions.end() || !it->second->connected) {
        // Try UDP fallback before queuing — fires udpSendNeeded signal
        // so AppController can route via the punch socket if available.
        emit udpSendNeeded(recipientId, payload);

        // Real-time voice is ephemeral. A voice frame that can't go out live
        // (or over the UDP punch path above) is worthless a few ms later, so we
        // never queue it. Queuing voice would (a) spam this path every ~20 ms
        // while the peer is offline and (b) evict pending SNAPS from the shared
        // delivery queue below. We still nudge a connection (debounced) so the
        // call can establish if the peer appears; until then frames just drop.
        if (Core::VoiceWire::isVoicePacket(payload)) {
            auto &voiceSession = ensureSession(recipientId);
            if (!voiceSession.connecting)
                emit connectionNeeded(recipientId);
            return;
        }

        auto &session = ensureSession(recipientId);
        // Cap the queue: drop oldest entry if we exceed the limit.
        // Without this, an unreachable peer accumulates an unbounded list
        // of ~300KB snap payloads in RAM. 20 entries × 300KB = 6MB cap.
        constexpr int kMaxQueueDepth = 20;
        if (session.pendingQueue.size() >= kMaxQueueDepth) {
            qCWarning(lcNet) << "Queue full for" << recipientId.value
                             << "— dropping oldest pending message.";
            session.pendingQueue.removeFirst();
        }
        qCDebug(lcNet) << "No active session for" << recipientId.value << "— queuing.";
        session.pendingQueue.append(std::move(payload));
        // V32.36+ (device logs): actively ASK to connect. Previously sendToPeer
        // only queued + fired udpSendNeeded, relying entirely on the discovery
        // layer (gossip peerDiscovered → onPeerDiscovered → connectToPeer) to
        // open the session. When gossip discovery doesn't fire on a network
        // (same-WiFi peers with correct 192.168 addresses still queued forever,
        // JOINs apparently dropped by the AP), nothing ever initiated the
        // connection and every snap sat in the queue. Emitting connectionNeeded
        // lets AppController kick off connectByDhtId (which also tries the cached
        // LAN address + punch), so SENDING a snap drives the connection instead
        // of waiting. Debounced in AppController to avoid a storm per queued snap.
        if (!session.connecting) {
            emit connectionNeeded(recipientId);
        }
        return;
    }
    it->second->transport->send(payload);
}

void NetworkManager::connectToPeer(Core::UserId id, Core::PeerAddress address)
{
    const std::string key = id.value.toStdString();

    // ── Address classification ─────────────────────────────────────────────
    // Loopback / link-local / multicast / broadcast → reject outright.
    // CGNAT (RFC6598 100.64/10) → log a hint that NAT hairpin may fail; we
    //   still try, but the diagnostics layer can prefer a relay sooner.
    // RFC1918 / public → normal path.
    const auto klass = Network::AddressClass::classify(address.host);
    using AC = Network::AddressClass::Class;
    if (klass == AC::Loopback || klass == AC::Invalid
     || klass == AC::Multicast || klass == AC::Broadcast
     || klass == AC::LinkLocal) {
        qCWarning(lcNet) << "connectToPeer: refusing"
                         << Network::AddressClass::className(klass) << "target"
                         << address.toString() << "for" << id.value
                         << "— upstream source bug, please investigate";
        return;
    }
    if (id.value == m_selfId.value) {
        qCWarning(lcNet) << "connectToPeer: refusing self-id target"
                         << id.value << "@" << address.toString()
                         << "— upstream source bug, please investigate";
        return;
    }
    if (klass == AC::CarrierGrade) {
        // Information only — don't refuse. Two CGNAT peers can still reach
        // each other if they share an ISP that does hairpin, or via a
        // relay. We just want the diagnostic visibility.
        qCInfo(lcNet) << "connectToPeer: target" << address.toString()
                      << "is in CGNAT range (100.64/10) — hole-punch may"
                      << "fail, relay fallback likely needed";
    }

    // ── LAN isolation guard ───────────────────────────────────────────────
    // If we've already failed this peer's RFC1918 address kIsolatedLanThreshold
    // times in this session, the AP almost certainly has client-isolation
    // enabled. Skip immediately so AppController falls back to relay fast
    // instead of wasting another 12s on a doomed handshake.
    //
    // Distinction from refuse-link-local above: link-local NEVER works (we
    // can't route off-link). LAN failures are session-specific: client
    // isolation could be off later (different AP, peer reconnects from a
    // different network). So this is a session-scoped soft block, not a
    // hard refusal.
    if (klass == AC::Rfc1918) {
        // V32.27: derive an effective threshold from app context.
        // Default: kIsolatedLanThreshold (2). On cellular: 1 — LAN
        // literally cannot work over a cellular interface, so giving
        // it a second attempt just wastes time before we fall back to
        // relay/CGNAT path. On WiFi/Ethernet/unknown: keep the
        // permissive default since transient LAN failures (single
        // dropped packet, brief AP hiccup) shouldn't lock us out for
        // the session.
        int effectiveThreshold = kIsolatedLanThreshold;
        if (m_appContext) {
            const auto &ctx = m_appContext->context();
            if (ctx.connectionType == Core::AppContext::ConnectionType::Cellular)
                effectiveThreshold = 1;
        }

        // Decay: a stale failure count shouldn't keep blocking a now-valid LAN
        // address. The peer may not have been ready during the earlier attempts,
        // or it just moved onto this LAN (its DHT record now carries an RFC1918
        // address it didn't before). If the last LAN failure was a while ago,
        // forget it and give LAN another shot.
        const auto tit = m_lanFailureLastMs.find(key);
        if (tit != m_lanFailureLastMs.end() &&
            QDateTime::currentMSecsSinceEpoch() - tit->second > kLanFailureDecayMs) {
            m_lanFailureCount.erase(key);
            m_lanFailureLastMs.erase(key);
        }

        // Skip LAN only if (a) LAN has NOT demonstrably worked for any peer this
        // session and (b) this peer has hit the failure threshold. (a) is the
        // decisive counter-evidence: if even one LAN connection has succeeded,
        // AP client isolation is provably off, so these failures are transient
        // (peer not ready / a dropped packet) — abandoning the fast LAN path
        // here would be wrong.
        const auto fit = m_lanFailureCount.find(key);
        if (!m_lanProvenThisSession &&
            fit != m_lanFailureCount.end() && fit->second >= effectiveThreshold) {
            qCInfo(lcNet) << "connectToPeer: skipping LAN" << address.toString()
                          << "for" << id.value << "—" << fit->second
                          << "prior failures (threshold" << effectiveThreshold
                          << ") suggest AP client isolation"
                          << (effectiveThreshold < kIsolatedLanThreshold
                              ? "(threshold lowered: cellular detected)"
                              : "")
                          << ". Use relay/CGNAT path instead.";
            // Emit peerNeedsRelookup so AppController promotes the
            // next-address logic to try a non-LAN candidate.
            emit peerNeedsRelookup(id);
            return;
        }
    }

    // Reject bare UUID peer IDs — these come from stale builds and will never
    // complete a useful handshake.  UUID format: 8-4-4-4-12 hex groups.
    static const QRegularExpression kUuidRx(
        QStringLiteral("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"),
        QRegularExpression::CaseInsensitiveOption);
    if (kUuidRx.match(id.value).hasMatch()) {
        qCDebug(lcNet) << "Ignoring UUID peer (stale build):" << id.value;
        return;
    }

    // Connection guards — allow parallel multi-address racing:
    //   CONNECTED       → skip (peer already reachable)
    //   CONNECTING same addr → skip (exact duplicate)
    //   CONNECTING diff addr → ALLOW (parallel race, first wins)
    // This is the key change from the old guard which blocked ALL parallel
    // attempts when connecting was true. Multi-address racing significantly
    // improves cross-network reliability when a peer has both LAN and public IPs.
    {
        const auto it = m_sessions.find(key);
        if (it != m_sessions.end()) {
            const auto &s = *it->second;
            if (s.connected) {
                qCDebug(lcNet) << "Already connected to" << id.value << "— skipping.";
                return;
            }
            // Same address: exact duplicate — skip.
            if (s.connecting
                && s.address.host == address.host
                && s.address.port == address.port) {
                qCDebug(lcNet) << "Already connecting to" << id.value
                               << "at" << address.toString() << "— duplicate skipped.";
                return;
            }
            // Different address while connecting: SKIP — let the existing
            // attempt finish. Cancelling it here caused heap corruption because
            // the transport disconnect could re-enter NetworkManager before
            // we replaced the session transport. The existing attempt will
            // fail naturally and peerNeedsRelookup will retry with fresh addrs.
            if (s.connecting) {
                qCDebug(lcNet) << "Already connecting to" << id.value
                               << "(different addr" << address.toString() << ") — queuing for retry.";
                // Queue so it is tried if current attempt fails.
                it->second->pendingAddrs.append(address);
                return;
            }
        }
    }

    qCInfo(lcNet) << "Connecting to" << id.value
                  << "at ws://" + address.host + ":" + QString::number(address.port) + "/snap";

    auto &session      = ensureSession(id);
    session.address    = address;
    session.connecting = true;
    session.connectStartedMs = QDateTime::currentMSecsSinceEpoch();
    // Bump epoch so any lambdas from prior connect attempts self-discard.
    const quint64 myEpoch = ++session.epoch;

    // Tear down any stale transport before creating a new one.
    // IMPORTANT: use deleteLater(), not reset() — if this connectToPeer call
    // was triggered from inside the old transport's own timeout signal chain
    // (timer → abort → disconnected → onTransportDisconnected → retry → here),
    // calling reset() destroys the transport while Qt's signal dispatch stack
    // still has frames inside it → crash in doActivate.
    if (session.transport) {
        auto *old = session.transport.release();
        old->QObject::disconnect(); // prevent any further signals
        old->disconnect();          // close the socket
        old->deleteLater();         // destroy after current event loop iteration
    }

    auto *transport = new WebSocketTransport(nullptr);

    connect(transport, &ITransport::connected, this, [this, id, myEpoch]() {
        const std::string k = id.value.toStdString();
        auto sessionIt = m_sessions.find(k);
        if (sessionIt == m_sessions.end()) return;
        auto &s = *sessionIt->second;
        if (s.epoch != myEpoch) return; // stale callback — session was replaced

        qCInfo(lcNet) << "Transport connected to" << id.value;
        // NetworkStats — record success with measured latency. The address
        // class tells us WHICH PATH worked (same-LAN vs CGNAT vs public)
        // which is what we'll use next session to reorder candidates.
        // Named `connectedKlass` to avoid shadowing the outer `klass` from
        // the enclosing function's address-classification step.
        const qint64 latencyMs = s.connectStartedMs > 0
            ? QDateTime::currentMSecsSinceEpoch() - s.connectStartedMs : 0;
        const auto connectedKlass = Network::AddressClass::classify(s.address.host);
        Network::Stats::NetworkStats::instance().recordAttempt(
            id, connectedKlass, Network::Stats::Outcome::Success, latencyMs);
        if (connectedKlass == Network::AddressClass::Class::Rfc1918) {
            // LAN works → AP client isolation is provably off this session.
            // Clear any stale failure count for this peer and lift the guard
            // globally so other same-LAN peers aren't wrongly skipped.
            m_lanProvenThisSession = true;
            m_lanFailureCount.erase(id.value.toStdString());
            m_lanFailureLastMs.erase(id.value.toStdString());
        }
        s.connected  = true;
        s.connecting = false;
        s.retryCount = 0;
        for (const QByteArray &msg : std::exchange(s.pendingQueue, {}))
            s.transport->send(msg);
        emit peerConnected(id);
    });

    connect(transport, &ITransport::disconnected,
            this, [this, id, myEpoch]() { onTransportDisconnected(id, myEpoch); });

    connect(transport, &ITransport::messageReceived,
            this, [this, id, myEpoch](const QByteArray &data) {
                const auto it = m_sessions.find(id.value.toStdString());
                if (it == m_sessions.end() || it->second->epoch != myEpoch) return;
                emit payloadReceived(id, data);
            });

    connect(transport, &ITransport::errorOccurred,
            this, [this, id, myEpoch](const QString &err) {
                qCWarning(lcNet) << "Transport error for" << id.value << ":" << err;
                const auto it2 = m_sessions.find(id.value.toStdString());
                if (it2 == m_sessions.end() || it2->second->epoch != myEpoch) return;
                emit networkError(QStringLiteral("Peer %1: %2").arg(id.value, err));
                // "Network unreachable" means the address is on an unreachable
                // subnet (wrong network, stale private IP). Skip straight to
                // max retries so peerNeedsRelookup fires immediately rather
                // than wasting 5 × 8s burning through pointless retries.
                // "Host unreachable" is the same — routing table says no route.
                const bool hopeless =
                    err.contains(QStringLiteral("Network unreachable"), Qt::CaseInsensitive)
                    || err.contains(QStringLiteral("Host unreachable"), Qt::CaseInsensitive)
                    || err.contains(QStringLiteral("Connection refused"), Qt::CaseInsensitive);
                if (hopeless)
                    it2->second->retryCount = kMaxRetries; // trigger immediate relookup
            });

    session.transport.reset(transport);
    transport->connectToPeer(address);
}

void NetworkManager::promoteInboundSession(const Core::UserId &realId,
                                            const Core::UserId &inboundId)
{
    const std::string realKey    = realId.value.toStdString();
    const std::string inboundKey = inboundId.value.toStdString();

    auto inboundIt = m_sessions.find(inboundKey);
    if (inboundIt == m_sessions.end()) return; // inbound session gone
    if (!inboundIt->second->connected)  return; // not actually connected

    auto &realSession    = ensureSession(realId);
    auto &inboundSession = *inboundIt->second;

    // If the real-ID session already has a live connected transport, do nothing —
    // outbound must have succeeded simultaneously.
    if (realSession.connected) return;

    // Steal the inbound transport into the real-ID session.
    // Disconnect the old signals (keyed on inboundId) before moving the transport
    // so we don't get disconnected/error callbacks pointing at the wrong session.
    if (inboundSession.transport) {
        inboundSession.transport->QObject::disconnect();
    }

    realSession.transport  = std::move(inboundSession.transport);
    realSession.connected  = true;
    realSession.connecting = false;

    // Re-wire the signals on the stolen transport to use realId.
    // Bump epoch so any old inbound callbacks become stale.
    const quint64 promotedEpoch = ++realSession.epoch;
    if (realSession.transport) {
        connect(realSession.transport.get(), &ITransport::messageReceived,
                this, [this, realId, promotedEpoch](const QByteArray &data) {
                    const auto it = m_sessions.find(realId.value.toStdString());
                    if (it == m_sessions.end() || it->second->epoch != promotedEpoch) return;
                    emit payloadReceived(realId, data);
                });
        connect(realSession.transport.get(), &ITransport::disconnected,
                this, [this, realId, promotedEpoch]() {
                    onTransportDisconnected(realId, promotedEpoch);
                });
        connect(realSession.transport.get(), &ITransport::errorOccurred,
                this, [this, realId, promotedEpoch](const QString &err) {
                    const auto it = m_sessions.find(realId.value.toStdString());
                    if (it == m_sessions.end() || it->second->epoch != promotedEpoch) return;
                    qCWarning(lcNet) << "Transport error for" << realId.value << ":" << err;
                    emit networkError(QStringLiteral("Peer %1: %2").arg(realId.value, err));
                });
    }

    // Flush any queued messages that were waiting for the real-ID session to connect.
    for (const QByteArray &msg : std::exchange(realSession.pendingQueue, {})) {
        realSession.transport->send(msg);
    }

    // Mark the inbound session as gone so it doesn't interfere with retries.
    m_sessions.erase(inboundIt);

    qCInfo(lcNet) << "Promoted inbound session" << inboundId.value
                  << "→" << realId.value << "(outbound was not connected)";
}

void NetworkManager::disconnectPeer(Core::UserId id)
{
    auto it = m_sessions.find(id.value.toStdString());
    if (it != m_sessions.end()) {
        if (it->second->transport) {
            it->second->transport->QObject::disconnect();
            it->second->transport->disconnect();
        }
        m_sessions.erase(it);
    }
}

// ─── Private slots ────────────────────────────────────────────────────────────

void NetworkManager::onInboundConnection()
{
    // Cap total inbound sessions to prevent memory exhaustion from
    // port scanners or reconnect floods filling m_sessions.
    constexpr int kMaxSessions = 64;
    while (m_server->hasPendingConnections()) {
        QWebSocket *raw = m_server->nextPendingConnection();
        if (!raw) continue;
        if (static_cast<int>(m_sessions.size()) >= kMaxSessions) {
            qCWarning(lcNet) << "Max sessions reached — rejecting inbound from"
                             << raw->peerAddress().toString();
            raw->close(QWebSocketProtocol::CloseCodePolicyViolated,
                       QStringLiteral("Server full"));
            raw->deleteLater();
            continue;
        }

        // Wrap the accepted socket in our ITransport abstraction.
        // adoptSocket() takes ownership of `raw`.
        auto *transport = new WebSocketTransport(nullptr);
        transport->adoptSocket(raw);

        // Temporary peer ID until we implement an identity handshake protocol.
        // Format: "inbound:ip:port" — deterministic, avoids collisions.
        const QString tempId = QStringLiteral("inbound:%1:%2")
                                   .arg(raw->peerAddress().toString())
                                   .arg(raw->peerPort());
        const Core::UserId peerId{ tempId };

        qCInfo(lcNet) << "Inbound connection from" << tempId;

        auto &session       = ensureSession(peerId);
        session.connected   = true;
        session.connecting  = false;
        session.transport.reset(transport);

        const quint64 inboundEpoch = ++session.epoch;
        connect(transport, &ITransport::messageReceived,
                this, [this, peerId, inboundEpoch](const QByteArray &data) {
                    const auto it = m_sessions.find(peerId.value.toStdString());
                    if (it == m_sessions.end() || it->second->epoch != inboundEpoch) return;
                    emit payloadReceived(peerId, data);
                });
        connect(transport, &ITransport::disconnected,
                this, [this, peerId, inboundEpoch]() {
                    onTransportDisconnected(peerId, inboundEpoch);
                });
        connect(transport, &ITransport::errorOccurred,
                this, [this, peerId, inboundEpoch](const QString &err) {
                    const auto it = m_sessions.find(peerId.value.toStdString());
                    if (it == m_sessions.end() || it->second->epoch != inboundEpoch) return;
                    qCWarning(lcNet) << "Inbound error from" << peerId.value << ":" << err;
                });

        emit peerConnected(peerId);
    }
}

// Per-peer timestamp for discovery cooldown.
static QHash<QString, qint64> s_lastDiscoveryMs;
// Our own public IP — used to skip hairpin addresses in onPeerDiscovered.
static QString s_selfPublicIp;

void NetworkManager::injectPayload(const Core::UserId &sender,
                                    const QByteArray  &payload)
{
    emit payloadReceived(sender, payload);
}

static void pruneDiscoveryMap()
{
    // Evict entries older than 60 s — they no longer serve any rate-limiting
    // purpose and would otherwise accumulate one entry per peer seen forever.
    const qint64 cutoff = QDateTime::currentMSecsSinceEpoch() - 60'000;
    for (auto it = s_lastDiscoveryMs.begin(); it != s_lastDiscoveryMs.end(); ) {
        if (it.value() < cutoff)
            it = s_lastDiscoveryMs.erase(it);
        else
            ++it;
    }
}

void NetworkManager::suppressDiscovery(const Core::UserId &id, int durationMs)
{
    // Push the rate-limit timestamp far into the future so onPeerDiscovered
    // skips connecting for durationMs. AppController calls this when it owns
    // the retry loop for a peer so the two paths don't race.
    s_lastDiscoveryMs[id.value] =
        QDateTime::currentMSecsSinceEpoch() + static_cast<qint64>(durationMs);
    pruneDiscoveryMap(); // keep map bounded
}

void NetworkManager::notifyPunchReady(const Core::UserId &peerId)
{
    // The hole punch just completed — the NAT mapping is now open.
    // Extend the active transport's connect timer by 8 seconds so the
    // TCP SYN/ACK handshake has time to traverse the freshly opened hole.
    // Without this, the connect timer may expire within 1-3s of punch
    // completing, causing a spurious timeout even though connectivity exists.
    const auto it = m_sessions.find(peerId.value.toStdString());
    if (it == m_sessions.end()) return;
    auto &s = *it->second;
    if (!s.connecting || !s.transport) return;

    // Cast to WebSocketTransport to access the timer.
    // If the transport is not a WebSocketTransport (e.g. relay), skip.
    auto *wst = qobject_cast<WebSocketTransport *>(s.transport.get());
    if (!wst) return;

    qCInfo(lcNet) << "Hole punch ready for" << peerId.value
                  << "— extending connect timeout by 8s";
    wst->extendConnectTimeout(8000);
}

void NetworkManager::setSelfPublicIp(const QString &ip)
{
    s_selfPublicIp = ip;
}

void NetworkManager::onPeerDiscovered(Core::UserId id, Core::PeerAddress address)
{
    if (id == m_selfId) return;

    // Reject bare UUID IDs from stale builds — they will never handshake correctly
    // and generate endless timeout noise in the log and status bar.
    // Valid IDs look like "username@ip"; UUIDs match 8-4-4-4-12 hex groups.
    static const QRegularExpression kUuidRx(
        QStringLiteral("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"),
        QRegularExpression::CaseInsensitiveOption);
    if (kUuidRx.match(id.value).hasMatch()) {
        qCDebug(lcNet) << "Ignoring UUID peer ID (stale build):" << id.value;
        return;
    }

    // V32.35+: forward EVERY discovered address to DirectDiscovery's LAN-probe
    // table BEFORE the hairpin/rate-limit returns below. Those returns suppress
    // a connection ATTEMPT, but the address itself is still a useful LAN
    // candidate to remember — and for two same-WiFi phones the gossip-learned
    // 192.168.x address is exactly what the LAN probe needs to go direct.
    emit lanPeerAddressLearned(id, address);

    // Skip hairpin addresses BEFORE rate-limiting. If we don't, the public
    // IP arrives first → updates rate-limit timestamp → hairpin skip → return.
    // Then the LAN IP arrives a moment later from the same DHT lookup, but
    // gets rate-limited because the hairpin "consumed" the rate window.
    // Result: same-network peers can't connect.  Hairpin is a no-op, so it
    // shouldn't count as a discovery attempt for rate-limiting purposes.
    if (!s_selfPublicIp.isEmpty() && address.host == s_selfPublicIp) {
        qCInfo(lcNet) << "[HAIRPIN SKIP] Peer" << id.value
                      << "@ same public IP" << address.toString() << "— skipping";
        return;
    }

    // Rate-limit: ignore duplicate DHT results for the same peer within 5s.
    // The DHT often returns multiple addresses (LAN, hotspot, public) in rapid
    // succession; we only want to initiate one connection attempt at a time.
    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    const qint64 lastMs = s_lastDiscoveryMs.value(id.value, 0);
    if (nowMs - lastMs < 5000) {
        qCDebug(lcNet) << "Peer discovered (rate-limited):" << id.value
                       << "@" << address.toString();
        return;
    }
    s_lastDiscoveryMs[id.value] = nowMs;
    pruneDiscoveryMap();

    qCInfo(lcNet) << "Peer discovered:" << id.value << "@" << address.toString();
    // Update the stored address in case the peer moved (dynamic IP).
    auto it = m_sessions.find(id.value.toStdString());
    if (it != m_sessions.end()) {
        it->second->address = address;
        if (it->second->connected) return; // already established — leave it

        if (it->second->connecting) {
            // A session is connecting (possibly a stale relay waiting for the
            // other side). If the new address is a LAN address, prefer it:
            // abort the stale connecting session and connect via LAN directly.
            // V32.32: replaced 5-line inline subnet check with helper.
            // isPrivate covers RFC1918 (10/8, 172.16/12, 192.168/16) and is
            // bit-exact identical to what was here. Saves 30 lines across
            // the file when this pattern is repeated.
            const bool newIsLan = SnapClone::Network::AddressClass::isPrivate(address.host);
            if (!newIsLan) return; // not LAN — don't override existing attempt
            qCInfo(lcNet) << "LAN address discovered for connecting peer"
                          << id.value << "— aborting stale session, connecting via LAN";
            // Drop the stale transport so connectToPeer creates a fresh one.
            // Bump epoch FIRST so any pending disconnected() callbacks from
            // the dying transport see a mismatched epoch and self-discard.
            ++it->second->epoch;
            it->second->connecting = false;
            if (it->second->transport) {
                it->second->transport->QObject::disconnect(); // detach all signals
                it->second->transport.release()->deleteLater();
            }
        }
    }
    connectToPeer(std::move(id), std::move(address));
}

// onTransportDisconnected takes the peer id explicitly.
// We deliberately do NOT use sender() here because:
//   a) The transport may have already been replaced in the session.
//   b) sender() returns nullptr in queued connection contexts.
void NetworkManager::onTransportDisconnected(const Core::UserId &id, quint64 epoch)
{
    auto it = m_sessions.find(id.value.toStdString());
    if (it == m_sessions.end()) return;

    // Stale callback from a prior transport — the session has already moved on.
    // Without this guard, parallel-connect replacements cause double-erase and
    // the "corrupted double-linked list" / segfault seen in the logs.
    if (it->second->epoch != epoch) {
        qCDebug(lcNet) << "Discarding stale disconnect for" << id.value
                       << "(epoch" << epoch << "!=" << it->second->epoch << ")";
        return;
    }

    auto &session      = *it->second;
    // Stats: distinguish "we never made it to connected" (timeout/refused
    // during initial handshake) from "established session was dropped"
    // (long-running connection died — not a candidate-evaluation signal,
    // we want to keep that profile's rate high).
    if (session.connecting && !session.connected) {
        const auto klass = Network::AddressClass::classify(session.address.host);
        Network::Stats::NetworkStats::instance().recordAttempt(
            id, klass, Network::Stats::Outcome::Timeout);

        // LAN isolation: bump per-peer counter so future LAN attempts to
        // this peer get fast-skipped. See connectToPeer's LAN isolation
        // guard for the matching read site.
        if (klass == Network::AddressClass::Class::Rfc1918) {
            const auto key = id.value.toStdString();
            const int n = ++m_lanFailureCount[key];
            m_lanFailureLastMs[key] = QDateTime::currentMSecsSinceEpoch();
            if (n == kIsolatedLanThreshold) {
                qCInfo(lcNet) << "LAN isolation detected for" << id.value
                              << "after" << n
                              << "failed RFC1918 attempts. Will skip LAN"
                              << "for this peer this session.";
            }
        }
    } else if (session.connected) {
        // We HAD a connection that just dropped. If it was LAN, reset the
        // failure count — the AP isolation is gone (or never was), and a
        // future LAN attempt may succeed.
        const auto klass = Network::AddressClass::classify(session.address.host);
        if (klass == Network::AddressClass::Class::Rfc1918) {
            m_lanFailureCount.erase(id.value.toStdString());
        }
    }
    session.connected  = false;
    session.connecting = false;

    qCInfo(lcNet) << "Peer disconnected:" << id.value;
    emit peerDisconnected(id);

    // No outbound address — promoted from inbound, or peer moved to new IP.
    // Give the peer 8s to reconnect inbound before triggering DHT re-resolution.
    // This handles the common case where NAT kills the TCP socket (30-120s idle
    // timeout) but the peer is still online and will reconnect immediately.
    // Without this grace window we aggressively relookup and hammer the DHT
    // while the peer is in the middle of their own reconnect.
    if (session.address.host.isEmpty() || session.address.port == 0) {
        qCInfo(lcNet) << "Inbound session dropped for" << id.value
                      << "— waiting 8s for peer to reconnect before DHT re-lookup.";
        if (session.transport) {
            // Disconnect all signals BEFORE releasing ownership so no more
            // callbacks fire into a half-destroyed session.
            session.transport->QObject::disconnect();
            // Use deleteLater() — we may be inside this transport's own
            // signal emission (e.g. disconnected() from relay TCP close).
            // Calling the destructor synchronously while still on the
            // signal's call stack causes heap corruption.
            session.transport.release()->deleteLater();
        }
        session.connected  = false;
        session.connecting = false;
        const Core::UserId idCopy2 = id;
        const quint64 graceEpoch   = session.epoch;
        QTimer::singleShot(8000, this, [this, idCopy2, graceEpoch]() {
            const auto it2 = m_sessions.find(idCopy2.value.toStdString());
            if (it2 == m_sessions.end()) return;
            // If the peer reconnected inbound during the grace window, the
            // session epoch will have advanced — don't relookup.
            if (it2->second->epoch != graceEpoch) {
                qCInfo(lcNet) << idCopy2.value << "reconnected during grace window — skipping relookup.";
                return;
            }
            // Still disconnected — request DHT relookup.
            m_sessions.erase(it2);
            emit peerNeedsRelookup(idCopy2);
        });
        return;
    }

    // Don't retry if another session to the same host:port is already live.
    // This happens when the user manually added a peer ("manual@host:port")
    // AND gossip discovered the same physical device ("user@ip"). Once the
    // gossip session is promoted and connected, the manual@ session is redundant.
    const QString &targetHost = session.address.host;
    const quint16  targetPort = session.address.port;
    for (const auto &[key, other] : m_sessions) {
        if (key == id.value.toStdString()) continue; // skip self
        if (other && other->connected
            && other->address.host == targetHost
            && other->address.port == targetPort) {
            qCInfo(lcNet) << "Session to" << targetHost << ":" << targetPort
                          << "already live via" << QString::fromStdString(key)
                          << "— dropping redundant" << id.value;
            if (it->second->transport) {
                it->second->transport->QObject::disconnect();
                it->second->transport->disconnect();
            }
            m_sessions.erase(it);
            return;
        }
    }

    // Before retrying outbound, check if this peer already has a live
    // inbound-promoted session. If so, snaps can already flow both ways
    // through that socket — hammering a firewalled outbound port is pointless.
    const bool hasLiveInbound = [&]() {
        for (const auto &[key, other] : m_sessions) {
            if (key == id.value.toStdString()) continue;
            if (other && other->connected
                && other->address.host.isEmpty()) // inbound-promoted = no outbound addr
                return true;
        }
        return false;
    }();

    if (hasLiveInbound) {
        qCInfo(lcNet) << id.value << "has live inbound session — skipping outbound retry.";
        if (session.transport) {
            auto t3 = std::move(session.transport);
            t3->QObject::disconnect();
            t3->disconnect();
        }
        m_sessions.erase(it);
        return;
    }

    if (session.retryCount < kMaxRetries) {
        const int shift   = qMin(session.retryCount, 10);
        const int delayMs = kRetryBaseMs * (1 << shift);
        ++session.retryCount;

        qCInfo(lcNet) << "Retrying" << id.value << "in" << delayMs
                      << "ms (attempt" << session.retryCount << "of" << kMaxRetries << ")";

        // Prefer the next queued addr (from parallel race) over retrying the same addr.
        const Core::PeerAddress retryAddr =
            session.pendingAddrs.isEmpty() ? session.address
                                           : session.pendingAddrs.takeFirst();
        const quint64 retryEpoch = session.epoch;
        QTimer::singleShot(delayMs, this, [this, id, retryAddr, retryEpoch]() {
            const auto it2 = m_sessions.find(id.value.toStdString());
            if (it2 == m_sessions.end()) return;
            if (it2->second->epoch != retryEpoch) return; // session replaced
            connectToPeer(id, retryAddr);
        });
    } else {
        qCWarning(lcNet) << "Max retries reached for" << id.value
                         << "— removing session, requesting DHT re-lookup.";
        // Disconnect transport signals BEFORE erase so the transport destructor
        // (triggered by unique_ptr reset inside PeerSession) cannot re-enter
        // onTransportDisconnected or any other slot while the session is gone.
        if (session.transport) {
            auto *old = session.transport.release();
            old->QObject::disconnect();
            old->disconnect();
            old->deleteLater(); // defer destruction past current signal chain
        }
        // Copy id by value BEFORE erase — the session (which may have held
        // string data referenced by id) is destroyed by erase.
        const Core::UserId idCopy = id;
        m_sessions.erase(it);
        // Queue the signal so it fires after this function fully returns.
        // Direct emission here caused re-entrant heap operations that
        // corrupted malloc's fastbin structures (segfault in QPainter).
        QTimer::singleShot(150, this, [this, idCopy]() {
            emit peerNeedsRelookup(idCopy);
        });
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

NetworkManager::PeerSession &NetworkManager::ensureSession(const Core::UserId &id)
{
    // operator[] on unordered_map default-constructs the value if missing,
    // then we initialise the unique_ptr if it was just created.
    auto &ptr = m_sessions[id.value.toStdString()];
    if (!ptr) {
        ptr = std::make_unique<PeerSession>();
    }
    return *ptr;
}

bool NetworkManager::hasOutboundSession(const Core::UserId &id) const
{
    // An "outbound" session is one where WE initiated the WebSocket connect.
    // Inbound-promoted sessions have no address set until connectToPeer is called.
    const auto it = m_sessions.find(id.value.toStdString());
    if (it == m_sessions.end()) return false;
    const auto &s = *it->second;
    return (s.connected || s.connecting)
        && !s.address.host.isEmpty()
        && s.address.port > 0;
}

bool NetworkManager::hasAnyLiveSession(const Core::UserId &id) const
{
    const auto it = m_sessions.find(id.value.toStdString());
    if (it == m_sessions.end()) return false;
    return it->second->connected || it->second->connecting;
}

bool NetworkManager::hasEstablishedSession(const Core::UserId &id) const
{
    const auto it = m_sessions.find(id.value.toStdString());
    if (it == m_sessions.end()) return false;
    return it->second->connected;
}

void NetworkManager::consolidateSessions(const Core::UserId &id)
{
    // After race-then-settle period: keep only the established session.
    // Cancel any still-connecting parallel attempts for this peer.
    // We do this by bumping epochs on non-established sessions so their
    // future callbacks become stale and they self-discard.
    int kept = 0, dropped = 0;
    const std::string key = id.value.toStdString();
    for (auto &[k, session] : m_sessions) {
        if (!session) continue;
        // Only touch sessions for this peer
        // Only consolidate sessions explicitly keyed under this peer's ID.
        // Inbound sessions (keyed "inbound:ip:port") are managed separately
        // by promoteInboundSession and don't need to be touched here.
        const bool isThisPeer = (k == key);
        if (!isThisPeer) continue;
        if (session->connected) { ++kept; continue; }
        // Still connecting — close it cleanly
        if (session->transport) {
            session->transport->QObject::disconnect();
            session->transport->disconnect();
        }
        ++session->epoch; // invalidate pending callbacks
        ++dropped;
    }
    if (dropped > 0)
        qCInfo(lcNet) << "Consolidated" << id.value << ": kept"
                      << kept << "established, dropped" << dropped << "connecting.";
}

void NetworkManager::adoptRelayTransport(const Core::UserId &peerId,
                                          ITransport        *relay)
{
    auto &session = ensureSession(peerId);
    if (session.transport) {
        session.transport->QObject::disconnect();
        session.transport->disconnect();
    }
    session.transport.reset(relay);
    session.connecting = true;
    session.connected  = false;
    // Bump epoch: any callbacks from the old transport now self-discard.
    const quint64 myEpoch = ++session.epoch;
    connect(relay, &ITransport::connected, this, [this, peerId, myEpoch]() {
        auto it = m_sessions.find(peerId.value.toStdString());
        if (it == m_sessions.end() || it->second->epoch != myEpoch) return;
        it->second->connected = true;
        it->second->connecting = false;
        qCInfo(lcNet) << "Relay connected to" << peerId.value;
        // Send via session transport — avoids capturing raw relay pointer
        // which may be deleted if the session is replaced between connect
        // and this lambda firing.
        if (it->second->transport) {
            for (const QByteArray &msg : std::exchange(it->second->pendingQueue, {}))
                it->second->transport->send(msg);
        }
        emit peerConnected(peerId);
    });
    connect(relay, &ITransport::disconnected, this,
            [this, peerId, myEpoch]() { onTransportDisconnected(peerId, myEpoch); });
    connect(relay, &ITransport::messageReceived, this,
            [this, peerId, myEpoch](const QByteArray &data) {
                auto it = m_sessions.find(peerId.value.toStdString());
                if (it == m_sessions.end() || it->second->epoch != myEpoch) return;
                emit payloadReceived(peerId, data);
            });
    connect(relay, &ITransport::errorOccurred, this,
            [this, peerId, myEpoch](const QString &err) {
                const auto it = m_sessions.find(peerId.value.toStdString());
                if (it == m_sessions.end() || it->second->epoch != myEpoch) return;
                qCWarning(lcNet) << "Relay error for" << peerId.value << ":" << err;
                emit networkError(QStringLiteral("Relay %1: %2").arg(peerId.value, err));
            });
    qCInfo(lcNet) << "Relay transport adopted for" << peerId.value;
}

void NetworkManager::updateSelfAddress(const Core::UserId    &selfId,
                                       const Core::PeerAddress &newAddr)
{
    m_selfId = selfId;
    // Re-announce to gossip discovery so LAN peers see our new address.
    if (m_discovery) {
        m_discovery->announcePresence(selfId, newAddr);
        qCInfo(lcNet) << "Updated self address to" << newAddr.toString();
    }
}

quint16 NetworkManager::actualListenPort() const
{
    return m_server->isListening() ? m_server->serverPort() : 0;
}

int NetworkManager::connectedPeerCount() const
{
    int count = 0;
    for (const auto &[key, session] : m_sessions) {
        if (session && session->connected) ++count;
    }
    return count;
}

} // namespace SnapClone::Network
