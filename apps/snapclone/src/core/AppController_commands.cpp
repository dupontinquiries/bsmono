#include "AppController_p.hpp"
#include "core/Timings.hpp"
#include "network/AddressClassification.hpp"
#include "network/NetworkStats.hpp"
#include <QNetworkInterface>
#include <QHostAddress>

namespace SnapClone::Core {

namespace {

namespace AC = SnapClone::Network::AddressClass;

// V32.39+: a peer that advertises one of OUR OWN local IPs is unreachable at
// that address — connecting there loops straight back to us. This bites on
// phone hotspots: both phones default to the gateway IP 172.16.42.1 when each
// is its own hotspot host, so a peer's advertised "ws://172.16.42.1:9876" is
// ALSO our own server address. The old hairpin guard only compared against
// selfAddr.host (the PUBLIC IP after STUN), so the LAN self-address slipped
// through and we connected to ourselves — the log showed a self↔self handshake
// looping saveContact / SHK2 / wayfinder-refresh for our own id (sc-6eb25e40).
// Treat any address whose host is one of our live interface IPs as a hairpin.
bool addrIsOwnLocalHost(const QString &host)
{
    if (host.isEmpty()) return false;
    const QHostAddress target(host);
    if (target.isNull()) return false;
    for (const QNetworkInterface &ni : QNetworkInterface::allInterfaces()) {
        if (!ni.flags().testFlag(QNetworkInterface::IsUp)) continue;
        if (ni.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
        for (const QNetworkAddressEntry &e : ni.addressEntries()) {
            if (e.ip() == target) return true;
        }
    }
    return false;
}

// V32.25: shared NetworkStats reranker, used from both the cache-hit
// connect path AND the live-DHT-lookup arrival path. Previously only
// the cache-hit path applied the learned per-peer ordering; live-DHT
// results were appended to the queue in raw arrival order, so the
// first lookup-after-cache-miss got worse routing decisions than the
// second one (which finally benefited from cached + reranked data).
// This made the cold-start case feel worse than steady-state with the
// same peer — opposite of what we wanted.
//
// The rerank is a stable_sort by learned class priority. Addresses
// whose class the learner has no data for keep their original
// heuristic position (assigned a large priority value).
//
// Callable with an empty `addrs` (no-op). Logs at Info level so
// real-device traces can confirm both call sites are firing.
//
// When DebugFlag::NoFsmTuning is ON, NetworkStats::rankClassesForPeer
// returns an empty list and this function is a no-op — preserving the
// A/B switch.
inline void applyNetworkStatsRerank(QList<Core::PeerAddress> &addrs,
                                    const Core::UserId &peerId,
                                    const char *callSite)
{
    if (addrs.size() < 2) return; // nothing to sort
    const auto rankedClasses =
        Network::Stats::NetworkStats::instance().rankClassesForPeer(peerId);
    if (rankedClasses.isEmpty()) return;

    // Build a per-class priority map (lower = higher priority).
    QHash<AC::Class, int> priorityByClass;
    for (int i = 0; i < rankedClasses.size(); ++i)
        priorityByClass[rankedClasses[i]] = i;

    constexpr int kUnknown = 1000;
    std::stable_sort(addrs.begin(), addrs.end(),
        [&priorityByClass, kUnknown](const Core::PeerAddress &a,
                            const Core::PeerAddress &b) {
            const auto ka = AC::classify(a.host);
            const auto kb = AC::classify(b.host);
            const int pa = priorityByClass.value(ka, kUnknown);
            const int pb = priorityByClass.value(kb, kUnknown);
            return pa < pb;
        });
    qCInfo(lcApp) << "[" << callSite << "] applied learned rerank for"
                  << peerId.value << "—"
                  << Network::Stats::NetworkStats::instance().summaryFor(peerId);
}

} // anon

// ─── Accessors ───────────────────────────────────────────────────────────────

Network::NetworkManager     *AppController::networkManager()  const { return d->network.get();  }
Network::IDiscovery         *AppController::dhtDiscovery()    const { return d->jami.get();     }
QString                      AppController::stableId()        const { return d->stableId;       }
Network::FirewallPortManager *AppController::firewallManager() const { return d->firewall.get(); }
Storage::StorageManager     *AppController::storageManager()  const { return d->storage.get();  }
Face::Library               *AppController::faceLibrary()     const { return d->face.get();     }
Face::Detector              *AppController::faceDetector()    const { return d->faceDetector.get(); }
Crypto::ICryptoService      *AppController::cryptoService()   const { return d->crypto.get();   }
SessionManager              *AppController::sessionManager()  const { return d->session.get();  }
SnapRouter                  *AppController::snapRouter()       const { return d->router.get();   }
AppContextProvider          *AppController::appContext()       const { return d->appContext.get(); }
Network::PollinatorDiscovery *AppController::pollinatorDiscovery() const { return d->pollinator.get(); }
Core::PowerManager          *AppController::powerManager()   const { return d->power.get();     }
Core::SleepStateMachine     *AppController::sleepStateMachine() const { return d->sleepState.get(); }

// ─── Commands ────────────────────────────────────────────────────────────────

void AppController::sendSnap(const Snap &snap)
{
    qCDebug(lcApp) << "Routing outbound snap" << snap.id.value;
    d->router->routeOutbound(snap);
}

void AppController::markSnapViewed(const SnapId &id)
{
    // Guard against double-call: if the user opens a snap twice quickly
    // (or InboxWidget and AppController both trigger this), don't start
    // two expiry timers and emit snapViewExpired twice.
    if (d->activeViewTimers.contains(id.value)) return;

    d->storage->markSnapViewed(id);

    const std::optional<Snap> snap = d->storage->loadSnap(id);
    if (!snap.has_value()) return;

    // Cap duration to prevent integer overflow (durationSecs from wire)
    // and ensure snaps always expire. Clamp to [1, 30] seconds.
    constexpr int kMinSecs = 1, kMaxSecs = 30;
    int clampedSecs = qBound(kMinSecs, snap->durationSecs, kMaxSecs);

    // V32.34+: for VIDEO snaps the view window must not be shorter than the
    // clip itself, or the snap "expires" mid-playback and the viewer is torn
    // down before the video finishes. (This became visible once
    // snapViewExpired actually closes the open viewer — previously the early
    // expiry was silently ignored and the dialog stayed up.) durationSecs is
    // the Snapchat-style "how long you may look" window, set from the clip
    // length at send time, but a sub-second/rounded value (or a stub that
    // arrived with durationSecs<videoDurationMs) could undercut playback. Use
    // the actual videoDurationMs (rounded UP) when it's longer, plus a small
    // grace margin so the viewer reaches clean EOS before we close it.
    if (snap->mediaType == MediaType::Video) {
        constexpr int kPlaybackGraceSecs = 2;   // let EOS land before close
        constexpr int kVideoMaxSecs = 120;       // hard ceiling for video
        if (snap->videoDurationMs > 0) {
            const int videoSecs = (snap->videoDurationMs + 999) / 1000; // ceil
            clampedSecs = qBound(clampedSecs,
                                 videoSecs + kPlaybackGraceSecs,
                                 kVideoMaxSecs);
        } else {
            // Unknown clip length (older pre-v4 snap, or videoDurationMs
            // didn't survive). Don't let a 1s window cut playback short —
            // floor video at a reasonable minimum so it can play through.
            // The dialog's own EOS handling + the MobileDialog safety timer
            // still bound the upper end if the clip is shorter.
            constexpr int kVideoUnknownFloorSecs = 30;
            clampedSecs = qMax(clampedSecs, kVideoUnknownFloorSecs);
        }
    }
    const int durationMs = clampedSecs * 1000;

    // Insert AFTER the early returns so we don't leak entries into
    // activeViewTimers for snaps whose timer we never actually start.
    d->activeViewTimers.insert(id.value);

    QTimer::singleShot(durationMs, this, [this, id]() {
        d->activeViewTimers.remove(id.value);
        qCInfo(lcApp) << "Snap view expired:" << id.value;
        emit snapViewExpired(id);
    });
}

void AppController::addFriend(const UserId &friendId, const PeerAddress &address)
{
    d->storage->savePeerAddress(friendId, address);
    d->network->connectToPeer(friendId, address);
}


// ─── tryNextPeerAddress ───────────────────────────────────────────────────────
// Dequeue the next candidate address for a peer and attempt connection.
// Called after a fast-fail (3s timeout) without triggering a full DHT relookup.
// Only triggers relookup once the entire address queue is exhausted.
void AppController::tryNextPeerAddress(const QString &peerId)
{
    const Core::UserId pid{ peerId };
    qCInfo(lcApp) << "[TRY-ADDR]" << peerId
                  << "queue:" << d->pendingAddresses.value(peerId).size()
                  << "live:" << d->network->hasAnyLiveSession(pid)
                  << "estab:" << d->network->hasEstablishedSession(pid);
    if (d->network->hasEstablishedSession(pid)) return; // already up

    auto &queue = d->pendingAddresses[peerId];

    // Skip any addresses identical to the one that just failed — they may
    // have been queued before the attempt and retrying immediately is futile.
    const Core::PeerAddress lastFailed = d->lastAttemptedAddr.value(peerId);
    while (!queue.isEmpty() && queue.first() == lastFailed) {
        qCInfo(lcApp) << "[TRY-ADDR] Skipping duplicate of last-failed addr:"
                      << queue.takeFirst().toString();
    }

    if (queue.isEmpty()) {
        // Address queue exhausted — need a fresh DHT lookup.
        qCInfo(lcApp) << "Address queue exhausted for" << peerId
                      << "— triggering DHT relookup.";
        d->pendingAddresses.remove(peerId);
        connectByDhtId(peerId);
        return;
    }

    const Core::PeerAddress addr = queue.takeFirst();
    d->lastAttemptedAddr[peerId] = addr;
    qCInfo(lcApp) << "Trying next address for" << peerId
                  << ":" << addr.toString()
                  << "(" << queue.size() << "remaining in queue)";

    // V32.32: helper replaces 5-line inline RFC1918 check.
    const bool isPublic = !SnapClone::Network::AddressClass::isPrivate(addr.host);

    d->network->connectToPeer(pid, addr);
    if (isPublic && d->nat) {
        // Prefer the stored STUN punch addr (correct CGNAT port) over WS addr
        const Core::PeerAddress punchAddr =
            d->peerPunchAddr.value(peerId, addr);
        d->nat->initiateHolePunch(pid, punchAddr);
        if (d->udpChannel && punchAddr.port != addr.port)
            d->udpChannel->connectToPeer(pid, punchAddr);
    }
}


// ─── buildFallbackQueue ───────────────────────────────────────────────────────
// Given a peer's known public host and primary port, returns a prioritised
// list of addresses to try in order.  The caller pushes the whole list into
// d->pendingAddresses[peerId] and tryNextPeerAddress drains it sequentially.
//
// Tier 0  (immediate)   : announced port — peer explicitly chose it
// Tier 1  (500 ms gap)  : 8080, 80        — common HTTP-alt / transparent-proxy
// Tier 2  (1.5 s gap)   : WSS:443, WSS:5349 — TLS-only firewalls, HTTP CONNECT
//
// We do NOT include UDP-only ports (3478) here because WebSocket is TCP.
// Hole-punch over UDP is handled separately via NatTraversal.
static QList<Core::PeerAddress> buildFallbackQueue(const QString &host,
                                                    quint16 primaryPort)
{
    QList<Core::PeerAddress> q;
    // Tier 0 — announced port (already being tried as the initial attempt,
    // but include it so the queue represents the full ordered set for logging)
    if (primaryPort != 9876 && primaryPort != 0)
        q.append({ host, primaryPort, false });

    // Tier 1 — plain WS on common firewall-friendly ports
    // Skip if primary is already one of these
    for (quint16 p : { quint16(9876), quint16(8080), quint16(80) }) {
        if (p != primaryPort)
            q.append({ host, p, false });
    }

    // Tier 2 — TLS WebSocket (wss://) — penetrates HTTPS-only proxies
    // Port 443: standard HTTPS, HTTP CONNECT tunnelling, most proxies pass it
    // Port 5349: TURNS/TLS, some VoIP-aware networks whitelist it
    q.append({ host, 443,  true  });
    q.append({ host, 5349, true  });

    return q;
}

void AppController::connectByDhtId(const QString &dhtId)
{
    if (dhtId.isEmpty()) return; // guard: never look up empty ID
    if (!d->jami) return;
    const UserId peerId{ dhtId };

    // Cap total relookup attempts per peer to break the infinite loop when
    // the peer is genuinely unreachable (e.g. switched networks, offline).
    const int attempts = d->relookupCount.value(dhtId, 0);
    if (attempts >= Impl::kMaxRelookups) {
        qCWarning(lcApp) << "connectByDhtId: max relookup attempts for"
                         << dhtId << "— P2P failed. Trying relay fallback.";
        d->relookupCount.remove(dhtId);
        d->pendingDhtLookups.remove(dhtId);
        // Try relay as final fallback (covers symmetric NAT, corp WiFi, etc.)
        if (!d->relayUrl.isEmpty() &&
            !d->network->hasAnyLiveSession(Core::UserId{ dhtId })) {
            d->activeRelays.insert(dhtId);
            qCInfo(lcApp) << "Activating relay fallback for" << dhtId;
            const Core::UserId relaySelf{ d->stableId };
            const Core::UserId peerId2{ dhtId };
            // Select relay transport based on URL scheme
            if (d->relayUrl.startsWith(QStringLiteral("tcp://"), Qt::CaseInsensitive)) {
                auto *relay = new Network::MagicWormholeRelayTransport(this);
                relay->connectToRelay(relaySelf, peerId2, d->relayUrl);
                d->network->adoptRelayTransport(peerId2, relay);
            } else {
                auto *relay = new Network::RelayTransport(this);
                relay->connectToRelay(relaySelf, peerId2, d->relayUrl);
                d->network->adoptRelayTransport(peerId2, relay);
            }
        } else if (d->relayUrl.isEmpty()) {
            qCWarning(lcApp) << "No relay URL configured — relay disabled by user.";
        }
        // Still schedule a fresh attempt after 60s in case network changes.
        const QString idCopy = dhtId;
        QTimer::singleShot(60000, this, [this, idCopy]() {
            if (!d->network->hasAnyLiveSession(Core::UserId{ idCopy }))
                connectByDhtId(idCopy);
        });
        return;
    }

    // If we already have an active outbound session, don't duplicate.
    if (d->network->hasOutboundSession(peerId)) {
        qCInfo(lcApp) << "connectByDhtId: already have session for" << dhtId;
        d->relookupCount.remove(dhtId); // connected — reset counter
        d->pendingAddresses.remove(dhtId); // connected — clear address queue
        return;
    }

    // If a DHT lookup is already in flight for this peer, don't start another.
    // This is the main fix for the segfault: prevents multiple heap conn* pointers
    // for the same peer accumulating and being double-deleted.
    if (d->pendingDhtLookups.contains(dhtId)) {
        qCInfo(lcApp) << "connectByDhtId: lookup already in flight for" << dhtId;
        return;
    }

    // Show "connecting" status immediately so the UI updates the moment the user
    // taps Connect — regardless of whether we hit the cache or do a fresh lookup.
    // Only emit on attempt 0 (first try); relookup path emits its own "relookup" status.
    if (attempts == 0) {
        emit peerConnectionStatus(Core::UserId{ dhtId },
            QStringLiteral("connecting"),
            tr("Looking up in network..."));
        // Persist as Pending so it survives app restarts and appears in the
        // Connecting group on next launch. MAX(state) in the upsert means
        // this will not downgrade a Known contact back to Pending.
        const auto existing = d->storage->loadContact(Core::UserId{ dhtId });
        if (!existing.has_value()) {
            Core::Contact pending;
            pending.id          = Core::UserId{ dhtId };
            pending.displayName = dhtId; // placeholder until SHAK delivers real name
            pending.state       = Core::ContactState::Pending;
            pending.online      = false;
            d->storage->saveContact(pending);
        }
    }

    // Check DHT cache (1-hour TTL) before starting a live lookup.
    // NOTE: We deliberately do NOT use loadPeerAddress() as a fast-path here.
    // That path returned a single cached address without public/private
    // classification or parallel live-DHT-lookup, causing infinite retries
    // against stale private IPs (e.g. 172.20.10.x from a previous session).
    // Prioritise public IPs over RFC1918 private ranges — private addresses
    // are only reachable on the same LAN and cause "Network unreachable" spam
    // when tried cross-network. STUN reflexive address goes first.
    // Single SQL query returns all cached addrs with age in seconds.
    // High-port (punch) addrs expire after 5 min; stable WS addrs after 60 min.
    const auto cachedWithAge = d->storage->loadDhtCacheWithAge(peerId, 60);
    QList<Core::PeerAddress> cachedAddrsRaw;
    for (const auto &entry : cachedWithAge) {
        if (entry.first.port != 9876 && entry.second > 300) continue; // stale punch port
        cachedAddrsRaw.append(entry.first);
    }
    if (!cachedAddrsRaw.isEmpty()) {
        // Same-/24 detection: if a candidate LAN address shares our own /24,
        // we're on the same WiFi and the LAN address is guaranteed-fastest
        // route. Reorder to LAN-first in that case. Without this, two phones
        // on Bell home WiFi try the public IP first → router can't hairpin
        // → timeout 6-10s → eventually fall through to LAN. Wastes ~10s per
        // connection on every cold start in the common case.
        //
        // Three-way partition: each cached address goes into exactly one
        // bucket based on its relationship to OUR current LAN address.
        //   • same-LAN  — private AND shares our /24 → fast direct path
        //   • public    — routable externally → STUN+hairpin or relay
        //   • far-LAN   — private but different /24 → likely unreachable,
        //                 keep as last-resort (VPN'd networks etc.)
        namespace AC = SnapClone::Network::AddressClass;
        const QString selfHost = d->selfAddr.host;
        QList<Core::PeerAddress> sameLanAddrs, publicAddrs, privateAddrs;
        for (const Core::PeerAddress &a : cachedAddrsRaw) {
            if (AC::isPublic(a.host)) {
                publicAddrs.append(a);
            } else if (AC::isSameLan24(a.host, selfHost)) {
                sameLanAddrs.append(a);
            } else {
                privateAddrs.append(a);
            }
        }
        // STUN reflexive address first among public addrs.
        const Core::PeerAddress reflexive = d->nat
            ? d->nat->reflexiveAddress() : Core::PeerAddress{};
        QList<Core::PeerAddress> ordered;
        // Same-LAN first (guaranteed-reachable, fastest path).
        ordered.append(sameLanAddrs);
        // Then public, STUN-reflexive first.
        for (const Core::PeerAddress &a : publicAddrs) {
            if (!reflexive.host.isEmpty() && a.host == reflexive.host)
                ordered.prepend(a);
            else
                ordered.append(a);
        }
        // Different-LAN private addrs LAST — unreachable in most cases but
        // occasionally useful when both peers VPN into the same network.
        ordered.append(privateAddrs);

        // ── NetworkStats reranking (V31 — learned situational adaptiveness) ─
        // The heuristic ordering above is "what *should* work fastest based
        // on classification alone." It's a good prior. NetworkStats gives
        // us per-peer empirical data — if this specific peer has always
        // connected via public IP and never via LAN (e.g. has firewall
        // rules that block cross-device LAN traffic), the heuristic is
        // actively wrong for them. We use learned data to rerank.
        //
        // When DebugFlag::NoFsmTuning is ON, rankClassesForPeer returns
        // an empty list and the helper is a no-op. That's the A/B switch
        // for testing whether learning helps.
        //
        // V32.25: extracted the rerank into a shared helper so the live-
        // DHT-lookup path (further down in this same function, when
        // results stream in via the peerDiscovered signal) can call it
        // too. Before V32.25, only the cache-hit path was reranked, so
        // the very first connect after install ran with the heuristic
        // alone and only later connects benefited from learning. That
        // made the cold-start case feel worse than steady-state.
        applyNetworkStatsRerank(ordered, peerId, "cache-hit");

        qCInfo(lcApp) << "connectByDhtId: DHT cache hit for" << dhtId
                      << "(" << ordered.size() << "addr(s),"
                      << sameLanAddrs.size() << "same-LAN,"
                      << publicAddrs.size() << "public,"
                      << privateAddrs.size() << "different-LAN)";

        d->relookupCount[dhtId] = attempts + 1;

        // ALWAYS start a live DHT lookup in parallel with cache hit.
        // The live lookup refreshes stale public addrs (CGNAT IPs change
        // every session). If still disconnected when results arrive, connect.
        {
            qCInfo(lcApp) << "Cache hit — starting parallel live DHT lookup for fresh addrs";
            // Don't return early: try the private addrs (may be same LAN)
            // AND do a fresh DHT lookup for the public addr.
            // Use pendingDhtLookups to avoid double-lookup.
            if (!d->pendingDhtLookups.contains(dhtId)) {
                d->pendingDhtLookups.insert(dhtId);
                auto liveHandle = std::make_shared<QMetaObject::Connection>();
                *liveHandle = connect(
                    d->jami.get(), &Network::IDiscovery::peerDiscovered,
                    this, [this, peerId, liveHandle, dhtId](const Core::UserId &id,
                                                             const Core::PeerAddress &addr) {
                        if (id.value != peerId.value) return;
                        QObject::disconnect(*liveHandle);
                        d->pendingDhtLookups.remove(dhtId);
                        // Only act on public addresses from this live lookup
                        // V32.32: helper replaces 4-line inline RFC1918 check.
                        const bool isPriv =
                            SnapClone::Network::AddressClass::isPrivate(addr.host);
                        if (isPriv || addr.host == d->selfAddr.host) return;
                        d->storage->cacheDhtResult(peerId, addr);
                        // Cache updated — the main connection flow (queue cycling)
                        // will use the fresh addr on its next attempt. We do NOT
                        // call connectToPeer here because the parallel lookup runs
                        // alongside the main live lookup; duplicate connections
                        // from two independent lambdas corrupt the heap via
                        // double epoch manipulation.
                        if (!d->network->hasAnyLiveSession(peerId) && d->nat) {
                            qCInfo(lcApp) << "Live DHT fresh addr for" << dhtId
                                          << ":" << addr.toString()
                                          << "— updating cache only";
                        }
                    });
                d->jami->lookupPeer(peerId);
                // V32.45: on a relookup (primary path already failed), also
                // query the backup replica — the peer may have just switched
                // networks and published a fresher address in the other key.
                if (attempts > 0) d->jami->lookupPeerReplica(peerId);
                QTimer::singleShot(15000, this, [this, dhtId, liveHandle]() {
                    QObject::disconnect(*liveHandle);
                    d->pendingDhtLookups.remove(dhtId);
                    d->lastAttemptedAddr.remove(dhtId); // don't leak failed-peer entries
                });
            }
        }

        // Connect strategy:
        //  • Hairpin (same public IP as us) → skip, gossip handles same-LAN
        //  • Private/LAN addr              → connect immediately
        //  • Public cross-network addr     → TCP immediately + punch in parallel
        bool punchStarted = false;
        for (const Core::PeerAddress &a : ordered) {
            d->lastAttemptedAddr[dhtId] = a;
            const bool hairpin  = (!d->selfAddr.host.isEmpty() && a.host == d->selfAddr.host)
                                  || addrIsOwnLocalHost(a.host);   // V32.39+: also skip our own LAN IP (self-loop)
            // AddressClass::isPrivate handles RFC1918 + CGNAT + link-local +
            // ULA + loopback in one shot — replaces the inline lambda that
            // used to live in this function (removed in the same-LAN-first
            // refactor).
            const bool privAddr = AC::isPrivate(a.host);
            if (hairpin) {
                qCDebug(lcApp) << "Skipping hairpin addr" << a.toString();
                continue;
            }
            if (privAddr) {
                // Only connect to private addr if it's on our current subnet.
                const QHostAddress pa(a.host);
                bool onOurSubnet = false;
                for (const QNetworkInterface &ni : QNetworkInterface::allInterfaces()) {
                    if (!ni.flags().testFlag(QNetworkInterface::IsUp)) continue;
                    for (const QNetworkAddressEntry &e : ni.addressEntries()) {
                        if (e.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
                        if (e.ip().isLoopback()) continue;
                        if (pa.isInSubnet(e.ip(), e.prefixLength())) { onOurSubnet = true; break; }
                    }
                    if (onOurSubnet) break;
                }
                if (onOurSubnet) {
                    d->network->connectToPeer(peerId, a);
                } else {
                    qCInfo(lcApp) << "Skipping private addr not on our subnet:" << a.toString();
                }
            } else if (a.port > 10000) {
                // High port → STUN reflexive punch-only addr.
                // Low ports (9876, 8080, 80, 443, 5349) are WS endpoints → TCP.
                qCInfo(lcApp) << "Cache: punch-only addr" << a.toString()
                              << "— storing + firing hole punch";
                d->setPeerPunchAddr(dhtId, a);
                if (d->nat) d->nat->initiateHolePunch(peerId, a);
                if (d->udpChannel) d->udpChannel->connectToPeer(peerId, a);
                punchStarted = true;
            } else {
                // Public address — connect to primary immediately, then populate
                // the tiered fallback queue (WITHOUT the primary) so
                // tryNextPeerAddress tries alternate ports if primary fails.
                // Primary is intentionally excluded from the queue — it is being
                // tried right now and adding it would cause an immediate retry
                // on the same address that just failed.
                d->network->connectToPeer(peerId, a);
                const auto fallbacks = buildFallbackQueue(a.host, a.port);
                auto &pq = d->pendingAddresses[dhtId];
                for (const Core::PeerAddress &fb : fallbacks) {
                    // Skip primary port (already tried) and dedup.
                    if (fb.port == a.port && fb.tls == a.tls) continue;
                    if (!pq.contains(fb)) pq.append(fb);
                }
                // Also mark primary as the last attempted so peerNeedsRelookup
                // can skip it and go straight to next in queue.
                d->lastAttemptedAddr[dhtId] = a;
                if (!punchStarted && d->nat) {
                    d->nat->initiateHolePunch(peerId, a);
                    punchStarted = true;
                }
            }
        }
        return;
    }

    qCInfo(lcApp) << "connectByDhtId: DHT lookup for" << dhtId
                  << "(attempt" << (attempts + 1) << "of" << Impl::kMaxRelookups << ")";

    // Publish a connection request to the peer's DHT mailbox so they can
    // connect back to us even if DHT routing to our key is poor.
    // This is especially important when one side is on a restrictive network
    // (hotspot, CGNAT) where their DHT can find bootstrap nodes but not peer keys.
    //
    // V32.46: throttle via the shared mailboxPublished set (the same mechanism
    // the relookup-failure path in AppController_connect.cpp uses). A connreq
    // lives ~10 min in the DHT; this path previously republished on EVERY ~60 s
    // relookup with no throttle, piling distinct-nonce copies into the target's
    // mailbox (the 4× "passing to AppController" bursts on the receive side) and
    // burning puts + battery. Now both paths share one throttle → at most one
    // live note per target until its TTL lapses.
    if (d->crypto && d->jami && d->crypto->hasSessionKey(Core::UserId{ dhtId })
        && !d->mailboxPublished.contains(dhtId)) {
        const qint64 ts = QDateTime::currentSecsSinceEpoch();
        // V32.33+ Phase 1 audit: receiver requires 5-field CONNREQ
        // (CONNREQ\nfrom\ntcp_addr\npunch_addr\nts). Previously this
        // path emitted 4 fields (CONNREQ\nfrom\naddr\nts), which the
        // receiver rejected with "parts.size() < 5 return". Result:
        // mailbox-backed reconnection from this path silently never
        // worked — when one side was on a restrictive network, the
        // CONNREQ would get published but the recipient would reject
        // it as malformed. Match the canonical format from
        // AppController_connect.cpp:667 — use selfAddr for both TCP
        // and punch slots, since we don't have a separate STUN
        // reflexive addr available here.
        const Core::PeerAddress reflFallback = d->nat
            ? d->nat->reflexiveAddress() : Core::PeerAddress{};
        // V32.34+ fix: write BARE host:port (no ws:// scheme) to match the
        // other CONNREQ producer in AppController_connect.cpp and the
        // receiver's parseAddr (which treats PeerAddress.host as schemeless).
        // Using PeerAddress::toString() here previously embedded "ws://",
        // which the receiver re-prefixed → "ws://ws://host:port" → rejected
        // as an invalid target. The receiver is now scheme-tolerant too, but
        // keeping the wire format uniform (bare host:port) is the clean fix.
        const QString tcpStr = QStringLiteral("%1:%2")
            .arg(d->selfAddr.host).arg(d->selfAddr.port);
        const QString udpStr = !reflFallback.host.isEmpty()
            ? QStringLiteral("%1:%2").arg(reflFallback.host).arg(reflFallback.port)
            : tcpStr;
        // V32.37+ delivery fix: append our LAN address as a 6th field. selfAddr
        // here is the PUBLIC IP (STUN overwrote it), which is a hairpin for a
        // same-WiFi peer. d->lanAddr preserves our 192.168.x.x address so the
        // peer can connect directly on the LAN. Backward-compatible: older
        // receivers parse the first 5 fields and ignore the 6th. Omit if the LAN
        // addr is empty or identical to the public one (single-homed / no NAT).
        const QString lanStr = (!d->lanAddr.host.isEmpty()
                                && d->lanAddr.host != d->selfAddr.host)
            ? QStringLiteral("%1:%2").arg(d->lanAddr.host).arg(d->lanAddr.port)
            : QString();
        const QString plain = lanStr.isEmpty()
            ? QStringLiteral("CONNREQ\n%1\n%2\n%3\n%4\n")
                .arg(d->stableId).arg(tcpStr).arg(udpStr).arg(ts)
            : QStringLiteral("CONNREQ\n%1\n%2\n%3\n%4\n%5\n")
                .arg(d->stableId).arg(tcpStr).arg(udpStr).arg(ts).arg(lanStr);
        const auto enc = d->crypto->encryptFor(Core::UserId{ dhtId }, plain.toUtf8());
        if (enc) {
            QByteArray blob;
            blob.append(d->stableId.toUtf8());
            blob.append('\n');
            blob.append(enc->nonce);
            blob.append(enc->ciphertext);
#if defined(SNAPCLONE_OPENDHT_BACKEND)
            if (auto *dht = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
                d->mailboxPublished.insert(dhtId);
                // Allow a refresh once the ~10 min TTL lapses, but only if we
                // still haven't connected (mirrors the connect.cpp path).
                const QString midCopy = dhtId;
                QTimer::singleShot(10 * 60 * 1000, this, [this, midCopy]() {
                    if (!d->network->hasAnyLiveSession(Core::UserId{ midCopy }))
                        d->mailboxPublished.remove(midCopy);
                });
                qCInfo(lcApp) << "[MAILBOX] Publishing connection request to"
                              << dhtId << "mailbox";
                dht->publishConnectionRequest(Core::UserId{ dhtId }, blob);
            }
#endif // SNAPCLONE_OPENDHT_BACKEND
        }
    }

    // Disconnect any previous peerDiscovered lambda for this peer.
    // Without this, stale lambdas from prior lookups fire alongside the
    // new one, double-populating the address queue and corrupting the heap.
    if (d->activeLookupHandles.contains(dhtId)) {
        QObject::disconnect(d->activeLookupHandles.take(dhtId));
    }
    const int myGeneration = d->lookupGeneration[dhtId] + 1;
    d->lookupGeneration[dhtId] = myGeneration;
    d->pendingDhtLookups.insert(dhtId);
    d->relookupCount[dhtId] = attempts + 1;
    d->pendingAddresses.remove(dhtId); // fresh lookup = fresh address queue

    // One-shot signal connection. 'this' as context means Qt auto-disconnects
    // if AppController is destroyed before the lookup completes.
    // We use a QMetaObject::Connection stored inline — NOT on the heap —
    // to avoid the use-after-free that occurred when connectByDhtId was called
    // again before the first lookup's lambda fired and deleted its conn*.
    // The connection is stored in d->pendingDhtLookups (conceptually); the
    // actual disconnect happens inside the lambda via the captured handle.
    // Collect ALL DHT results for this peer into a dedup set, then connect.
    // The old one-shot approach disconnected after the first result (often a
    // stale private IP), missing the public address that arrived a moment later.
    auto seenAddrs       = std::make_shared<QSet<QString>>();
    auto queueStarted    = std::make_shared<bool>(false);
    auto earlyPunchFired = std::make_shared<bool>(false);
    auto handle = std::make_shared<QMetaObject::Connection>();
    const QString dhtIdCopy = dhtId;
    const int capturedGen = myGeneration;
    *handle = connect(
        d->jami.get(), &Network::IDiscovery::peerDiscovered,
        this, [this, peerId, handle, seenAddrs, queueStarted, earlyPunchFired, capturedGen](const Core::UserId &id,
                                                 const Core::PeerAddress &addr) {
            if (id.value != peerId.value) return;

            // Stale lambda: a newer lookup superseded this one.
            if (d->lookupGeneration.value(peerId.value, 0) != capturedGen) {
                QObject::disconnect(*handle);
                return;
            }

            // Note: pendingDhtLookups may not contain this peer after
            // the first result (it's removed to allow a new lookup to start).
            // We do NOT exit here — generation check above handles stale lambdas.

            // Dedup: skip addresses we've already processed this lookup.
            const QString addrKey = addr.host + QLatin1Char(':') + QString::number(addr.port);
            if (seenAddrs->contains(addrKey)) return;
            seenAddrs->insert(addrKey);

            // First result: mark lookup as no longer pending so 15s timer
            // can still fire but connectByDhtId won't restart another lookup.
            // Keep the signal connection alive to collect more results.
            const bool firstResult = (seenAddrs->size() == 1);
            if (firstResult)
                d->pendingDhtLookups.remove(peerId.value);

            d->storage->savePeerAddress(peerId, addr);
            d->storage->cacheDhtResult(peerId, addr);

            // Already fully connected — nothing to do.
            if (d->network->hasEstablishedSession(peerId)) {
                QObject::disconnect(*handle);
                return;
            }

            // Classify address: skip hairpin and unreachable private subnets.
            const bool isHairpin = (!d->selfAddr.host.isEmpty()
                && addr.host == d->selfAddr.host)
                || addrIsOwnLocalHost(addr.host);   // V32.39+: also skip our own LAN IP (self-loop)
            if (isHairpin) return; // gossip handles same-public-IP peers

            // V32.32: helper replaces 6-line inline lambda. Same RFC1918 set.
            const bool isPrivate = SnapClone::Network::AddressClass::isPrivate(addr.host);

            bool onOurSubnet = false;
            if (isPrivate) {
                const QHostAddress pa(addr.host);
                for (const QNetworkInterface &ni : QNetworkInterface::allInterfaces()) {
                    if (!ni.flags().testFlag(QNetworkInterface::IsUp)) continue;
                    for (const QNetworkAddressEntry &e : ni.addressEntries()) {
                        if (e.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
                        if (e.ip().isLoopback()) continue;
                        if (pa.isInSubnet(e.ip(), e.prefixLength())) { onOurSubnet = true; break; }
                    }
                    if (onOurSubnet) break;
                }
                if (!onOurSubnet) {
                    // Gateway probe: if peer addr shares /24 with our interface,
                    // also try .1 — the phone acting as hotspot sits at .1.
                    const QHostAddress peerA(addr.host);
                    for (const QNetworkInterface &ni2 : QNetworkInterface::allInterfaces()) {
                        if (!ni2.flags().testFlag(QNetworkInterface::IsUp)) continue;
                        for (const QNetworkAddressEntry &e2 : ni2.addressEntries()) {
                            if (e2.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
                            if (e2.ip().isLoopback()) continue;
                            if (peerA.isInSubnet(e2.ip(), 24)) {
                                const quint32 base = e2.ip().toIPv4Address() & 0xFFFFFF00u;
                                const QString gw = QHostAddress(base | 1u).toString();
                                if (gw != addr.host && gw != e2.ip().toString()) {
                                    Core::PeerAddress gwAddr{ gw, addr.port };
                                    auto &gQ = d->pendingAddresses[peerId.value];
                                    if (!std::any_of(gQ.begin(), gQ.end(),
                                        [&](const Core::PeerAddress &a) {
                                            return a.host == gw && a.port == addr.port;
                                        })) {
                                        gQ.prepend(gwAddr);
                                        qCInfo(lcApp) << "Gateway probe queued:"
                                                      << gwAddr.toString();
                                    }
                                }
                                break;
                            }
                        }
                    }
                    qCDebug(lcApp) << "Skipping private addr not on our subnet:" << addr.toString();
                    return;
                }
            }

            // Punch-only address: port != 9876 means it's a STUN reflexive punch port
            // announced via DHT field 4. Don't try TCP WebSocket here, but DO
            // fire hole punch — this is the whole point of the punch port.
            if (!onOurSubnet && !isPrivate && addr.port > 10000) {
                // Keep the punch-only addr with the HIGHER port (more likely to be
                // the STUN reflexive) over a low-port punch-only addr.
                // For same port, the first arrival wins (usually the STUN result).
                const Core::PeerAddress existing = d->peerPunchAddr.value(peerId.value);
                const bool update = existing.host.isEmpty()
                    || (addr.port != 9876 && addr.port >= existing.port);
                if (update) d->setPeerPunchAddr(peerId.value, addr);
                qCInfo(lcApp) << "Punch-only addr from DHT:" << addr.toString()
                              << (update ? "— stored" : "— kept existing"
                                  + existing.toString()) << "(no TCP)";
                if (d->nat) d->nat->initiateHolePunch(peerId, addr);
                if (d->udpChannel) d->udpChannel->connectToPeer(peerId, addr);
                // If we received a DIFFERENT IP than previously known, the earlier
                // UdpChannel connect may be targeting a stale CGNAT addr. Fire SCUD
                // to the new addr too so we race both paths simultaneously.
                if (!existing.host.isEmpty() && existing.host != addr.host
                    && d->udpChannel && !d->udpChannel->isConnected(peerId)) {
                    qCInfo(lcApp) << "[DUAL-PUNCH] New CGNAT IP for" << peerId.value
                                  << "(" << existing.host << "→" << addr.host << ")"
                                  << "— racing both addrs";
                    // connectToPeer above already switches to new addr.
                    // Also re-punch old addr in case packets are in flight.
                    if (d->nat && !existing.host.isEmpty())
                        d->nat->initiateHolePunch(peerId, existing);
                }
                return;
            }

            // ── Tiered waterfall queue ──────────────────────────────────────
            // Tier 0  (immediate)  : announced port          ← try first
            // Tier 1  (sequential) : 9876, 8080, 80          ← plain WS alt ports
            // Tier 2  (sequential) : WSS:443, WSS:5349       ← TLS-only firewalls
            // Tier 3 (relay)       : handled by AppController on exhaustion
            //
            // LAN addresses are prepended so they are tried before public ones.
            // buildFallbackQueue() generates the ordered tier-1 and tier-2 entries.
            auto &queue = d->pendingAddresses[peerId.value];
            const bool alreadyPrimary = std::any_of(queue.begin(), queue.end(),
                [&](const Core::PeerAddress &a) {
                    return a.host == addr.host && a.port == addr.port && a.tls == addr.tls;
                });

            if (onOurSubnet || isPrivate) {
                // LAN: prepend primary only — fallback ports not useful on LAN
                if (!alreadyPrimary) queue.prepend(addr);
            } else {
                // Public: primary first, then tiered fallbacks
                if (!alreadyPrimary) queue.append(addr);
                for (const Core::PeerAddress &fb : buildFallbackQueue(addr.host, addr.port)) {
                    if (fb.port == addr.port && fb.tls == addr.tls) continue; // skip duplicate primary
                    const bool hasFb = std::any_of(queue.begin(), queue.end(),
                        [&](const Core::PeerAddress &a) {
                            return a.host == fb.host && a.port == fb.port && a.tls == fb.tls;
                        });
                    if (!hasFb) queue.append(fb);
                }
            }

            // Fan-out: attempt primary public address immediately.
            // NetworkManager deduplicates same-addr attempts and queues
            // extras in pendingAddrs. tryNextPeerAddress drains the rest.
            if (!isPrivate && !onOurSubnet
                && !d->network->hasEstablishedSession(peerId)) {
                qCInfo(lcApp) << "[LIVE-FANOUT] Tier-0 connect to"
                              << addr.toString() << "for" << peerId.value;
                d->network->connectToPeer(peerId, addr);
            } else if (onOurSubnet && !alreadyPrimary) {
                // LAN addr — connect immediately
                d->network->connectToPeer(peerId, addr);
            }

            // If not currently connecting, kick off address queue cycling.
            // Fire hole punch on FIRST public addr arrival — deferred one tick
            // so all results in the same batch are processed first.
            // This ensures peerPunchAddr (the correct CGNAT port) is set
            // before we pick the punch target, rather than punching to
            // the WS port which won't open the right CGNAT mapping.
            if (!*earlyPunchFired && !isPrivate && !onOurSubnet && d->nat &&
                !d->network->hasEstablishedSession(peerId)) {
                *earlyPunchFired = true;
                const QString earlyPeerId = peerId.value;
                QTimer::singleShot(0, this, [this, earlyPeerId]() {
                    const Core::UserId pid{ earlyPeerId };
                    if (d->network->hasEstablishedSession(pid)) return;
                    // Prefer stored punch addr (STUN reflexive, correct CGNAT port)
                    // over the WS addr — otherwise we punch to :9876 which
                    // doesn't open the phone's :44425 CGNAT mapping.
                    const Core::PeerAddress &target =
                        d->peerPunchAddr.contains(earlyPeerId)
                            ? d->peerPunchAddr[earlyPeerId]
                            : d->pendingAddresses.value(earlyPeerId).value(0);
                    if (target.host.isEmpty() || !d->nat) return;
                    qCInfo(lcApp) << "Early punch to" << target.toString()
                                  << "— minimizing CGNAT timing gap";
                    d->nat->initiateHolePunch(pid, target);
                });
            }

            // Start the address queue the first time a valid address enters it.
            // We use queueStarted (not seenAddrs->size()==1) because seenAddrs
            // is incremented for ALL results including those skipped by subnet
            // or hairpin checks — so size()==1 often never fires for valid addrs.
            if (!*queueStarted && !d->network->hasAnyLiveSession(peerId)) {
                *queueStarted = true;
                qCInfo(lcApp) << "[ADDR-QUEUE] Starting queue for" << peerId.value
                              << "(" << d->pendingAddresses[peerId.value].size()
                              << "addr(s) so far, more may arrive)";
                const QString pidCopy = peerId.value;
                QTimer::singleShot(0, this, [this, pidCopy]() {
                    // V32.25: apply NetworkStats rerank just before draining.
                    // Live-DHT results stream in over seconds and were
                    // previously processed in raw arrival order — meaning a
                    // peer that we KNOW always connects via public IP would
                    // still get a stale-private LAN address tried first if
                    // that's what arrived first from DHT. The cache-hit path
                    // already reranked, so steady-state worked; cold-start
                    // didn't. Reranking here closes that gap.
                    //
                    // Done inside the singleShot lambda (not at queueStarted
                    // latch time) so the rerank sees whatever results
                    // arrived in the same event-loop turn before the deferred
                    // tick. Results arriving after this point go to the
                    // queue tail in arrival order, which is acceptable —
                    // mirrors the cache-hit semantics.
                    auto &pendingQueue = d->pendingAddresses[pidCopy];
                    applyNetworkStatsRerank(pendingQueue, Core::UserId{ pidCopy },
                                            "live-DHT");
                    qCInfo(lcApp) << "[QUEUE-TICK] singleShot fired for"
                                  << pidCopy << "queue:"
                                  << d->pendingAddresses.value(pidCopy).size();
                    tryNextPeerAddress(pidCopy);
                });
            }
        });

    // Store handle so we can disconnect it on next lookup for this peer.
    d->activeLookupHandles[dhtIdCopy] = *handle;

    d->jami->lookupPeer(peerId);
    // V32.45: relookup escalation — also hit the backup replica key once the
    // primary path has failed at least once (the peer may have just moved).
    if (attempts > 0) d->jami->lookupPeerReplica(peerId);

    // If no DHT result arrives within 15 seconds, clear the pending flag
    // so the next connectByDhtId call can start a fresh lookup rather
    // than being blocked by the "lookup already in flight" guard.
    // 60s timeout — DHT bootstrap can take 30-45s on mobile networks.
    // After attempt 1: retry immediately (bootstrap may now be complete).
    // After attempt 3+: also activate relay proactively — don't wait for kMaxRelookups.
    const int lookupTimeoutMs = Timings::kDhtLookupTimeoutMs;
    const int capturedAttempts = attempts; // attempt number THIS lookup was started at
    QTimer::singleShot(lookupTimeoutMs, this, [this, dhtId, handle, capturedAttempts]() {
        if (!d->pendingDhtLookups.contains(dhtId)) return;

        qCInfo(lcApp) << "DHT lookup timed out for" << dhtId
                      << "— clearing pending flag for retry.";
        QObject::disconnect(*handle);
        d->pendingDhtLookups.remove(dhtId);

        if (d->network->hasAnyLiveSession(Core::UserId{ dhtId })) return;

        // Read the REAL attempt count from the live counter, not the captured value.
        // capturedAttempts only reflects the attempt number when THIS timer was set.
        // The counter may have advanced further if immediate retries ran in parallel.
        const int realAttempts = d->relookupCount.value(dhtId, capturedAttempts);

        qCInfo(lcApp) << "DHT timeout — total attempts so far:" << realAttempts
                      << "for" << dhtId;

        // Retry immediately on early attempts so DHT bootstrap has time to settle.
        if (realAttempts <= 2) {
            qCInfo(lcApp) << "DHT timeout on attempt" << realAttempts
                          << "— retrying immediately";
            QTimer::singleShot(500, this, [this, dhtId]() {
                connectByDhtId(dhtId);
            });
        }

        // Activate relay proactively after 2 failed attempts (~2 minutes).
        // Both sides connect to relay simultaneously — when phone times out
        // on attempt 2, the laptop has also been failing and will reach
        // its own relay activation. The relay pairs them when both connect.
        if (realAttempts >= 2 && !d->relayUrl.isEmpty()
            && !d->network->hasAnyLiveSession(Core::UserId{ dhtId })
            && !d->activeRelays.contains(dhtId)) {
            d->activeRelays.insert(dhtId);
            qCInfo(lcApp) << "DHT failed" << realAttempts << "times for" << dhtId
                          << "— activating relay NOW";
            const Core::UserId relaySelf{ d->stableId };
            const Core::UserId relayPeer{ dhtId };
            if (d->relayUrl.startsWith(QStringLiteral("tcp://"), Qt::CaseInsensitive)) {
                auto *relay = new Network::MagicWormholeRelayTransport(this);
                relay->connectToRelay(relaySelf, relayPeer, d->relayUrl);
                d->network->adoptRelayTransport(relayPeer, relay);
            } else {
                auto *relay = new Network::RelayTransport(this);
                relay->connectToRelay(relaySelf, relayPeer, d->relayUrl);
                d->network->adoptRelayTransport(relayPeer, relay);
            }
        }
    });
}



std::optional<Core::PeerLocation> AppController::lastKnownLocation(
    const Core::UserId &peerId) const
{
    // V32.28: self-location has three potential sources — manual
    // override (QSettings location/manualLat+Lon), in-memory
    // LocationManager (live GPS), and storage cache (cross-restart
    // persistence). For SELF queries, LocationManager is consulted
    // first because it also handles the manual-override path —
    // without this, a stored GPS fix from earlier would always win
    // over a freshly set manual override. For PEER queries, only
    // storage is consulted (LocationManager only knows OUR location).
    //
    // Storage TTL bumped from 5min to 30min: a stationary user with
    // one GPS fix shouldn't see Wayfinder go to "no fix" while
    // they're just sitting still. 30min matches the design which
    // already humanizes "2h ago" / "3d ago" for offline contacts.
    if (peerId.value == d->stableId && d->location) {
        if (auto loc = d->location->lastKnownLocation(d->stableId))
            return loc;
        // LocationManager miss → fall through to storage cache. The
        // storage may have a slightly stale fix that's still useful;
        // the LocationManager path only returns nullopt when there's
        // genuinely no signal AND no manual override.
    }
    if (!d->storage) return std::nullopt;
    constexpr qint64 kTtlMs = 30 * 60 * 1000;
    return d->storage->loadPeerLocation(peerId.value, kTtlMs);
}


// V32.47: call-aware backoff. noteVoiceActivity() is called from both the
// inbound voice path (AppController) and the outbound one (MainWindow's send
// lambda), so a call shows as active regardless of who is talking. callActive()
// reports true for a short window after the last voice/floor frame — long
// enough to bridge conversational gaps, short enough that the background loop
// resumes promptly once the call ends.
void AppController::noteVoiceActivity()
{
    d->lastVoiceActivityMs = QDateTime::currentMSecsSinceEpoch();
}

bool AppController::callActive() const
{
    return d->lastVoiceActivityMs != 0
        && QDateTime::currentMSecsSinceEpoch() - d->lastVoiceActivityMs
               < Impl::kCallActiveWindowMs;
}


bool AppController::sendLocationSnap(const Core::UserId &recipientId, const QString &caption)
{
    // PIMPL access — members live on d-> (the AppController::Private struct).
    if (!d->location) {
        qCWarning(lcApp) << "sendLocationSnap: no LocationManager available";
        return false;
    }
    const auto loc = d->location->lastKnownLocation(d->stableId);
    if (!loc.has_value()) {
        qCInfo(lcApp) << "sendLocationSnap: no location fix yet — refusing to send";
        return false;
    }

    Core::Snap snap;
    snap.id          = Core::SnapId::generate();
    snap.recipientId = recipientId;
    if (d->session) {
        if (auto self = d->session->currentUserId())
            snap.senderId = *self;
    }
    snap.mediaType   = Core::MediaType::Location;
    snap.sentAt      = QDateTime::currentDateTimeUtc();
    snap.expiresAt   = snap.sentAt.addSecs(24 * 3600);  // 24h, not view-once
    snap.caption     = caption;
    snap.locLat      = loc->lat;
    snap.locLon      = loc->lon;
    snap.locAccM     = loc->accuracyM;
    // No encryptedBlob — location snaps carry only metadata

    sendSnap(snap);
    qCInfo(lcApp) << "Location snap sent to" << recipientId.value
                  << "lat=" << loc->lat << "lon=" << loc->lon;
    return true;
}

} // namespace SnapClone::Core
