#pragma once
// AppController_p.hpp — private implementation header.
// Included ONLY by AppController*.cpp — not public API.

#include "core/AppController.hpp"
#include "core/LocationManager.hpp"
#include "core/SessionManager.hpp"
#include "core/SnapRouter.hpp"
#include "network/NetworkManager.hpp"
#include "network/GossipDiscovery.hpp"
#include "network/JamiAdapter.hpp"
#include "network/PollinatorDiscovery.hpp"
#include "network/FirewallPortManager.hpp"
#include "network/UPnPPortMapper.hpp"
#include "network/UdpChannel.hpp"
#include "network/RelayTransport.hpp"
#include "network/MagicWormholeRelayTransport.hpp"
#include "network/DirectDiscovery.hpp"
#include "network/DhtResourceManager.hpp"  // V32.34+: 4-tier DHT power mgmt
#include "storage/StorageManager.hpp"
#include "core/AppContext.hpp"
#include "core/PowerManager.hpp"           // V32.34+: PowerDevil/upower/logind
#include "core/SleepState.hpp"             // V32.34+: shared sleep-state machine
#include "crypto/CryptoService.hpp"
#ifdef SNAPCLONE_RUST_CRYPTO
#include "crypto/RustCryptoService.hpp"
#endif
#include "face/FaceLibrary.hpp"  // V32.32: Phase 1 face library

#include <QStandardPaths>
#include <QSettings>
#include <QFile>
#include <QCoreApplication>
#include <QLoggingCategory>
#include <QHostInfo>
#include <QSysInfo>
#include <QTimer>
#include <QDateTime>
#include <QCryptographicHash>
#include <QNetworkInterface>
#include <QDataStream>
#include <QNetworkAccessManager>
#include <QRegularExpression>
#include <QHostAddress>
#include <QTcpSocket>
#include "network/NatTraversal.hpp"
#include <QNetworkReply>
#include <QNetworkRequest>
#include "core/Identity.hpp"
#include <memory>
#include <QHash>
#include <QSet>

Q_DECLARE_LOGGING_CATEGORY(lcApp) // defined in AppController.cpp



// ─── Pimpl definition ────────────────────────────────────────────────────────

namespace SnapClone::Core {

struct AppController::Impl {
    std::unique_ptr<Network::NetworkManager>     network;
    std::unique_ptr<Network::FirewallPortManager> firewall;
    std::unique_ptr<Storage::StorageManager>     storage;
    std::unique_ptr<Crypto::ICryptoService>      crypto;
    // V32.32: Phase 1 face library. Always constructed in initialize()
    // even when SNAPCLONE_RUST_FACE is off — in that case the methods
    // safely return NotCompiled and the consumers degrade to "feature
    // unavailable" without crashing or branching on the build flag.
    std::unique_ptr<Face::Library>               face;
    // V32.32: Optional face detector. Constructed only when a model
    // file is found at MODEL_PATH (env var) or the default location.
    // May be null — callers must check.
    std::unique_ptr<Face::Detector>              faceDetector;
    std::unique_ptr<SessionManager>              session;
    std::unique_ptr<SnapRouter>                  router;
    std::unique_ptr<Network::IDiscovery>         jami;    // secondary discovery backend (JamiDiscovery or DhtDiscovery)
    // Maps transport-level ID (e.g. "inbound:192.168.2.x:port") to the peer's
    // real user ID revealed by the SHAK handshake.  Used to rewrite senderId
    // on inbound snaps so the inbox shows "alice@192.168.2.10" not "inbound:...".
    QHash<QString, QString>                      transportToRealId;
    QHash<QString, QTimer*>                      stabilityTimers;
    // Per-peer address queue for sequential connection cycling.
    // Populated from DHT results; tryNextPeerAddress() dequeues one per attempt.
    QHash<QString, QList<Core::PeerAddress>>     pendingAddresses;
    QString                                      stableId;
    // Relay server URL (configurable via Settings → Network → Relay URL).
    // Default: empty = relay disabled. Set to ws://yourserver:9900 to enable.
    QString                                      relayUrl;
    Core::PeerAddress                            selfAddr;    // our current WS endpoint
    // V32.37+ delivery fix: selfAddr gets OVERWRITTEN with the public/reflexive
    // address once STUN resolves it (onPublicIpResolved). That lost our LAN
    // address, so CONNREQs to a same-WiFi peer carried only the public IP — which
    // is a hairpin for them (same router) and gets skipped, leaving nothing to
    // connect to. We now preserve the LAN address separately so CONNREQs can
    // carry BOTH, letting a same-LAN peer connect directly. Empty until the first
    // announce sets it.
    Core::PeerAddress                            lanAddr;     // our LAN endpoint (never clobbered by STUN)
    // Pending DHT lookups: prevents duplicate conn* heap allocations when
    // connectByDhtId is called multiple times for the same peer.
    QSet<QString>                                pendingDhtLookups;
    // Active peerDiscovered connections per peer — must be disconnected
    // before starting a new lookup to prevent double-lambda heap corruption.
    QHash<QString, QMetaObject::Connection>      activeLookupHandles;
    // Per-peer lookup generation: incremented on each new lookup.
    // Stale lambdas self-discard when their generation != current.
    QHash<QString, int>                          lookupGeneration;
    // Per-peer relookup attempt counter: caps the DHT retry loop.
    QHash<QString, int>                          relookupCount;
    QSet<QString>                                mailboxPublished; // peers we've already left a mailbox request for
    static constexpr int kMaxRelookups = 10; // DHT bootstrap can take 30+ seconds
    // V32.47: call-aware backoff. AppController sees inbound voice/floor-control
    // frames (and is told about outbound ones), so it can tell when a call is
    // live and quiet the background reconnection churn that would otherwise
    // compete with it. callActive() = a voice frame within this window.
    qint64                                       lastVoiceActivityMs = 0;
    static constexpr qint64 kCallActiveWindowMs   = 20000;   // 20 s hysteresis
    static constexpr qint64 kInCallRelookupMs     = 300000;  // 5 min relookup spacing in-call
    // Last address attempted per peer — used to mark it failed in the DB.
    QHash<QString, Core::PeerAddress>            lastAttemptedAddr;
    // STUN reflexive punch addresses received from DHT (port != 9876).
    // tryNextPeerAddress uses this to punch to the correct CGNAT port.
    //
    // V32.34+: was a plain QHash that grew unbounded — over a long
    // session connecting to many transient peers (DHT random walks,
    // mailbox encounters, group chats) this would accumulate forever.
    // Each entry is ~40-60 bytes so it's not catastrophic per-entry,
    // but unbounded growth is a real memory leak. Two defenses:
    //
    //   1. Per-entry timestamp + periodic TTL sweep. NAT mappings
    //      typically expire in 30-300s; punch addrs older than 30min
    //      are almost certainly stale. punchAddrTtlTimer walks the
    //      map every 5min and drops entries with insertedAtMs older
    //      than kPunchAddrTtlMs.
    //   2. Hard cap with batch eviction. If the map reaches
    //      kMaxPunchAddrs (1024 — comfortably above any plausible
    //      session size), drop the oldest 25% to amortize the cost.
    //
    // The two work together: TTL keeps RAM low under normal use; the
    // cap is a backstop against pathological scenarios (DHT flooding,
    // a test harness with thousands of peers).
    struct PunchAddrEntry {
        Core::PeerAddress addr;
        qint64            insertedAtMs;
    };
    QHash<QString, PunchAddrEntry>               peerPunchAddrV2;
    QTimer                                       punchAddrTtlTimer;
    static constexpr qint64 kPunchAddrTtlMs   = 30 * 60 * 1000; // 30 min
    static constexpr int    kMaxPunchAddrs    = 1024;
    static constexpr int    kPunchAddrEvictBatch = 256;          // ~25% of cap

    // Compatibility shim — exposes the old QHash<QString, PeerAddress>
    // surface so the many call sites don't have to be rewritten in this
    // one commit. Reads pull through the timestamped store; writes go
    // through `setPeerPunchAddr` which also stamps the time.
    class PeerPunchAddrView {
    public:
        explicit PeerPunchAddrView(QHash<QString, PunchAddrEntry> *map) : m(map) {}
        [[nodiscard]] bool contains(const QString &k) const { return m->contains(k); }
        Core::PeerAddress value(const QString &k) const {
            const auto it = m->find(k);
            return it != m->end() ? it->addr : Core::PeerAddress{};
        }
        Core::PeerAddress value(const QString &k, const Core::PeerAddress &def) const {
            const auto it = m->find(k);
            return it != m->end() ? it->addr : def;
        }
        Core::PeerAddress operator[](const QString &k) const { return value(k); }
        void clear() { m->clear(); }
    private:
        QHash<QString, PunchAddrEntry> *m;
    };
    PeerPunchAddrView                            peerPunchAddr{&peerPunchAddrV2};

    /// Write helper that timestamps the entry AND enforces the cap.
    /// Call this instead of `peerPunchAddr[id] = addr;` directly.
    void setPeerPunchAddr(const QString &id, const Core::PeerAddress &addr) {
        // Cap check BEFORE insert so the new entry fits afterwards.
        if (peerPunchAddrV2.size() >= kMaxPunchAddrs) {
            // Build a list of (insertedAtMs, key) pairs, sort, drop the
            // oldest batch. O(N log N) but only runs when cap is hit;
            // amortized cost is constant.
            QList<QPair<qint64, QString>> ages;
            ages.reserve(peerPunchAddrV2.size());
            for (auto it = peerPunchAddrV2.constBegin();
                 it != peerPunchAddrV2.constEnd(); ++it) {
                ages.append({it.value().insertedAtMs, it.key()});
            }
            std::sort(ages.begin(), ages.end(),
                      [](const auto &a, const auto &b){ return a.first < b.first; });
            const int n = qMin<int>(kPunchAddrEvictBatch, ages.size());
            for (int i = 0; i < n; ++i) {
                peerPunchAddrV2.remove(ages[i].second);
            }
        }
        peerPunchAddrV2.insert(id, PunchAddrEntry{
            addr, QDateTime::currentMSecsSinceEpoch()
        });
    }

    /// TTL sweep: drop entries older than kPunchAddrTtlMs. Called by the
    /// punchAddrTtlTimer every 5 minutes.
    void sweepPunchAddrs() {
        const qint64 cutoff = QDateTime::currentMSecsSinceEpoch() - kPunchAddrTtlMs;
        QStringList toRemove;
        for (auto it = peerPunchAddrV2.constBegin();
             it != peerPunchAddrV2.constEnd(); ++it) {
            if (it.value().insertedAtMs < cutoff) toRemove.append(it.key());
        }
        for (const QString &k : toRemove) peerPunchAddrV2.remove(k);
    }

    std::unique_ptr<Network::PollinatorDiscovery> pollinator;
    // Tracks snap IDs with an active view-expiry timer to prevent double-firing.
    QSet<QString>                                activeViewTimers;
    // Hourly DHT cache purge timer.
    QTimer                                       dhtCachePurgeTimer;
    // Fast network monitor: checks primary IP every 5s.
    // Catches interface changes (e.g. phone joins home WiFi after hotspot)
    // and immediately re-announces with the new correct address.
    QTimer                                       networkPollTimer;
    QString                                      lastKnownPrimaryIp;
    bool                                         isOnline        = true;
    bool                                         wasOffline      = false;
    // Persistent NAM for public IP (ipify) fetches.
    // Reusing one instance prevents NAM accumulation on rapid SSID changes.
    QNetworkAccessManager                       *publicIpNam     = nullptr;
    QNetworkReply                               *publicIpReply   = nullptr;
    // NAT traversal: STUN + UDP hole punching + TURN relay fallback.
    std::unique_ptr<Network::NatTraversal>       nat;
    std::unique_ptr<Network::UPnPPortMapper>     upnp;
    std::unique_ptr<Network::UdpChannel>         udpChannel;
    std::unique_ptr<LocationManager>             location;
    std::unique_ptr<Network::DirectDiscovery>    directDiscovery;
    // V32.34+ battery: power + DHT resource managers. PowerManager owns the
    // PowerDevil/upower/logind/sys integration and emits screen/battery/
    // thermal signals. DhtResourceManager subscribes to PowerManager and
    // drives the 4-tier DHT mode (Full/Maintenance/Minimal/Off) based on
    // those signals. Both were written long ago but never instantiated —
    // this is the wiring that finally turns power management ON. Order
    // matters: power must outlive dhtResource (which holds a raw pointer
    // to it), so power is declared FIRST and destroyed LAST.
    std::unique_ptr<Core::PowerManager>          power;
    std::unique_ptr<Network::DhtResourceManager> dhtResource;
    /// V32.34+ battery: shared sleep-state machine. Depends on `power` and
    /// `appContext`; declared after `power` so it's destroyed before it
    /// (it holds raw pointers to both). Components read it via
    /// AppController::sleepStateMachine().
    std::unique_ptr<Core::SleepStateMachine>     sleepState;
    /// V32.34+ battery: count of currently-connected peers. Bridges
    /// NetworkManager's per-peer connect/disconnect signals into
    /// DhtResourceManager::setSessionActive (which is a single bool).
    /// DHT stays pinned to Full while > 0.
    int                                          liveSessionCount = 0;
    // Tracks transport IDs we have already responded to with a handshake.
    // The handshake protocol is one-shot: initiator sends → responder sends once.
    // Without this guard, each side keeps re-sending on every received handshake,
    // causing an infinite loop that overflows the call stack (segfault).
    QSet<QString>                                handshakeRespondedTo;
    // V32.33+ PQ Phase C: per-peer pending ML-KEM ephemeral decapsulation key.
    //
    // When we INITIATE a SHAK_v2 we generate a fresh ML-KEM-768 keypair,
    // ship the ek (1184 bytes) to the peer, and stash the dk (2400 bytes)
    // here keyed by peer-id until the peer's responder SHAK_v2 (carrying
    // the ml-kem ciphertext) arrives. On receipt we decapsulate with the
    // stashed dk to get the PQ shared secret, combine with X25519,
    // arrive at a hybrid session key, and then DROP the dk.
    //
    // The map MUST be size-bounded — without a cap, an attacker who
    // probes our sendShakV2 path repeatedly (e.g. by spoofing initiator
    // SHAKs that prompt us to retry) could accumulate ~2.5 KB per entry
    // and balloon memory. Bound: kMaxPendingPqEphemerals (defined in
    // sendShakV2 callsite).
    //
    // Each entry has a creation timestamp so old entries can be evicted
    // when the cap is hit OR by a periodic sweep (filed for future).
    struct PqEphemeral {
        QByteArray decapsulationKey;  // 2400-byte ML-KEM dk
        QByteArray encapsulationKey;  // 1184-byte ML-KEM ek (for retries)
        qint64     createdMs;         // QDateTime::currentMSecsSinceEpoch()
    };
    QHash<QString, PqEphemeral>                  pendingPqEphemerals;
    // Per-peer derived hybrid session key, if PQ-v2 SHAK has completed.
    // Distinct from RustCryptoService's m_sessionKeys (which holds the
    // X25519-only session key from the legacy SHAK path). When both
    // exist for the same peer, the hybrid key takes precedence for
    // payload sealing.
    //
    // Why a SEPARATE store: keeping the two paths cleanly separated
    // means a partial v2 rollout (some peers v2, some v1) doesn't
    // accidentally mix key material. Phase E will fold these once v1
    // is retired.
    QHash<QString, QByteArray>                   pqSessionKeys;
    // Seen mailbox nonces — prevents redundant decrypt when the DHT periodic
    // check re-delivers the same blob (DHT entries live for ~10 min).
    // Cleared on startup; bounded by session lifetime (max ~1 entry per peer).
    QSet<QString>                                seenMailboxNonces;
    QList<QString>                               seenMailboxNonceOrder; // FIFO for ordered eviction
    // V32.36+: per-peer last "connect kick" timestamp (ms). sendToPeer emits
    // connectionNeeded when it queues a snap for an unconnected peer; this
    // debounces a burst of queued snaps into one connect attempt per peer.
    QHash<QString, qint64>                       lastConnectKickMs;
    // V32.34+: per-window mailbox duplicate-nonce counter. Same blob can
    // arrive multiple times per DHT check (iterator yields the same
    // entry from neighboring keys, or relay echo). Without aggregation
    // the log filled with 5-7 identical "Duplicate nonce — skipping"
    // lines per mailbox check. Now we count and emit ONE line per
    // mailbox check with "(N copies)" suffix via mailboxDupFlushTimer.
    int                                          mailboxDupCount   = 0;
    QString                                      mailboxDupLastFrom;  // for the suffix
    QTimer                                      *mailboxDupFlush   = nullptr;
    // Inbound strangers awaiting UI approval: peerId → displayName.
    // If user approves, we proceed with peerConnected; if denied, drop session.
    QHash<QString, QString>                      pendingInbound;
    // Session-level deny list — inbound connections from these IDs are dropped.
    QSet<QString>                                deniedPeers;
    QList<QString>                               deniedPeersOrder; // FIFO for capped eviction
    QSet<QString>                                activeRelays;  // peers with active relay transport (prevents duplicate)
    // V32.21: app-state snapshot for adaptive network decisions.
    // Lives for the whole app lifetime. Network-layer consumers
    // (NatTraversal, UdpChannel, AppController reconnect, etc) hold
    // a pointer back and consult `context()` before deciding backoff
    // or aggression. See core/AppContext.hpp for the design notes.
    std::unique_ptr<AppContextProvider>           appContext;
};

} // namespace SnapClone::Core
// Each AppController*.cpp opens its own "namespace SnapClone::Core {" after this include.

