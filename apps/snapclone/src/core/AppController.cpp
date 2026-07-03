#include "AppController_p.hpp"
#include "network/AddressClassification.hpp"  // V32.32: isPrivate/isLoopback helpers
#include "network/DhtService.hpp"             // app-wide single DHT access point
#include <QNetworkInformation>
#include <QSettings>   // V32.34+: dht/parkScreenOff opt-in flag
#include <QNetworkReply>
#include <QNetworkRequest>
#include <QNetworkAccessManager>
#include <QRegularExpression>
#include <QNetworkInterface>
#include <memory>
#include "core/Timings.hpp"
#include "core/ActivityGate.hpp"

Q_LOGGING_CATEGORY(lcApp, "snapclone.app")

namespace SnapClone::Core {

// ─── Construction ─────────────────────────────────────────────────────────────

AppController::AppController(QObject *parent)
    : QObject(parent)
    , d(std::make_unique<Impl>())
{}

AppController::~AppController()
{
    // V32.37+ SHUTDOWN SEGFAULT FIX. Members destruct in REVERSE declaration
    // order, which puts `network` (declared first) LAST — i.e. NetworkManager is
    // torn down AFTER router/session/storage/crypto. But NetworkManager::stop()
    // (run from its dtor) closes transports, which can emit peerDisconnected and
    // schedule deleteLater() — firing slots that dereference d->router / d->storage
    // / d->crypto that are, by then, already destroyed. That's the use-after-free
    // crash seen at shutdown (segfault right after the network stats summary).
    //
    // Tear the network + discovery down NOW, while every subsystem it might call
    // back into is still alive. We sever Qt signal connections first so no queued
    // slot can fire into a half-destroyed object, then reset the pointers. The
    // remaining members then auto-destruct cleanly in reverse order.
    if (d) {
        if (d->network) d->network->disconnect();   // sever all signals to us/others
        if (d->jami)    d->jami->disconnect();
        // Drop the node from the app-wide service before it is destroyed. nullptr
        // force-clears (there is only ever one node) and needs no backend-specific
        // cast; we do it explicitly because the disconnect() above also severs the
        // service's own destroyed()-watch connection.
        Network::DhtService::instance()->clearDiscovery(nullptr);
        if (d->pollinator) d->pollinator->disconnect();
        d->jami.reset();        // discovery backends down first
        d->pollinator.reset();
        d->network.reset();     // then the transport manager, while router/storage live
    }
    // d (and its remaining members) auto-destruct here.
}

// ─── Initialization ──────────────────────────────────────────────────────────

void AppController::initialize()
{
    qCInfo(lcApp) << "Initializing subsystems…";

    // ── Storage ───────────────────────────────────────────────────────────
    const QString dbPath = QStandardPaths::writableLocation(
                               QStandardPaths::AppDataLocation) + "/db/snapclone.sqlite";
    // Check for a zombie process holding port 9876 TCP.
    // A prior crashed session prevents the WS server from binding.
    {
        QTcpSocket probe;
        probe.connectToHost(QStringLiteral("127.0.0.1"), 9876);
        if (probe.waitForConnected(200)) {
            qCWarning(lcApp) << "╔══════════════════════════════════════════╗";
            qCWarning(lcApp) << "║ ZOMBIE on port 9876 — run: pkill -f SnapClone ║";
            qCWarning(lcApp) << "╚══════════════════════════════════════════╝";
        }
        probe.abort();
    }

    d->storage = std::make_unique<Storage::StorageManager>(dbPath);
    if (!d->storage->open()) {
        qCCritical(lcApp) << "Failed to open database at" << dbPath;
        emit errorOccurred(tr("Database initialisation failed."));
        return;
    }

    // ── Crypto ────────────────────────────────────────────────────────────
#ifdef SNAPCLONE_RUST_CRYPTO
    d->crypto = std::make_unique<Crypto::RustCryptoService>();
    qCInfo(lcApp) << "Using Rust AES-256-GCM crypto backend.";
#else
    d->crypto = std::make_unique<Crypto::CryptoService>();
    qCInfo(lcApp) << "Using Qt HMAC-SHA256 placeholder crypto backend.";
#endif

    // ── Face library (V32.32, Phase 1) ────────────────────────────────────
    // Process-wide instance owned here; consumed by SettingsPage, and
    // (in a future turn) by the photo capture path + inbox path. When
    // SNAPCLONE_RUST_FACE is OFF the Library still constructs; its
    // methods just report NotCompiled. Consumers don't need to branch.
    //
    // Enabled state is persisted in QSettings under "face/enabled"
    // (same key SettingsPage reads/writes). Apply the stored state on
    // startup so the library matches the user's choice from the last
    // session without requiring a UI interaction.
    d->face = std::make_unique<Face::Library>();
    {
        QSettings s;
        const bool faceEnabled = s.value(
            QStringLiteral("face/enabled"), false).toBool();
        d->face->setEnabled(faceEnabled);
        if (d->face->isAvailable()) {
            qCInfo(lcApp) << "Face library available — enabled:" << faceEnabled;
        } else {
            qCInfo(lcApp) << "Face library: SNAPCLONE_RUST_FACE not built —"
                             " full-image snaps only.";
        }
    }

    // V32.32: Face detector — optional companion to the library. Loads
    // a YOLO ONNX model from disk to detect face bounding boxes in
    // captured / received images. The library *stores* face crops;
    // the detector *finds* faces to crop.
    //
    // Construction is opt-in (model file must be present). Doing it at
    // startup costs ~50-200ms of model load, but downstream callers
    // can rely on the detector being ready when consent is granted
    // mid-session without paying the load cost at the moment of
    // capture (which would visibly delay the camera).
    //
    // Path resolution order:
    //   1. MODEL_PATH env var (matches the camera-bridge convention)
    //   2. <AppDataLocation>/models/face.onnx (per-user install location)
    //   3. ./models/face.onnx (dev / portable build)
    //
    // If none of those exist, the detector is left null and downstream
    // code falls back to "no detection — send full image as today."
    {
        const QByteArray envPath = qgetenv("MODEL_PATH");
        QStringList candidates;
        if (!envPath.isEmpty()) candidates << QString::fromUtf8(envPath);
        candidates << QStandardPaths::writableLocation(
                          QStandardPaths::AppDataLocation)
                       + QStringLiteral("/models/face.onnx");
        candidates << QStringLiteral("models/face.onnx");
        QString found;
        for (const QString &p : candidates) {
            if (QFile::exists(p)) { found = p; break; }
        }
        if (!found.isEmpty()) {
            auto det = std::make_unique<Face::Detector>(found);
            if (det->isAvailable()) {
                d->faceDetector = std::move(det);
                qCInfo(lcApp) << "Face detector loaded from" << found;
            } else {
                qCWarning(lcApp) << "Face detector model present at" << found
                                 << "but failed to load — detection disabled";
            }
        } else {
            qCInfo(lcApp) << "Face detector model not found; tried:" << candidates;
        }
    }

    // ── Session ───────────────────────────────────────────────────────────
    d->session = std::make_unique<SessionManager>();

    // ── AppContext (V32.21) ────────────────────────────────────────────────
    // Built early so the network layer can hold a back-pointer for adaptive
    // decisions. Attach to system-level signals (QGuiApplication state,
    // QNetworkInformation transport/metered) immediately.
    //
    // V32.34+ battery: PowerManager is now constructed here and attached to
    // AppContext. This finally turns ON the PowerDevil/upower/logind/sys
    // integration that was written long ago but never instantiated. With
    // it attached, AppContext stops using optimistic defaults (screenOn=
    // true, onAc=false) and instead reflects real device state — which
    // flows into DhtResourceManager (4-tier DHT mode) and, via the
    // screenActiveChanged signal, into CameraWidget (pause camera when
    // screen off).
    d->appContext = std::make_unique<AppContextProvider>(this);
    d->appContext->attachApplicationState();
    d->appContext->attachNetworkInformation();

    // Construct PowerManager and attach. Parented to `this` for lifetime;
    // also held in d->power for explicit access (e.g. CameraWidget wiring
    // via AppController::powerManager()).
    d->power = std::make_unique<Core::PowerManager>(this);
    d->appContext->attachPowerManager(d->power.get());

    // Feed PowerManager's screen / battery / thermal state into the
    // process-wide ActivityGate so periodic timers across the app can gate on
    // it with a single connect. Foreground state the gate sources itself from
    // QGuiApplication; these add screen-off, low-battery and thermal awareness.
    // Seed once with the current state, then track via signals. (The lambdas
    // capture the PowerManager pointer, not `this`; the connections are torn
    // down when d->power — parented to this — is destroyed, so the pointer is
    // always live when they fire.)
    {
        auto *gate = Core::ActivityGate::instance();
        Core::PowerManager *pm = d->power.get();
        gate->setScreenActive(pm->isScreenActive());
        gate->setBatterySaver(pm->isBatterySaver());
        gate->setThermalCritical(pm->thermalState() == Core::ThermalState::Critical);
        connect(pm, &Core::PowerManager::screenActiveChanged,
                gate, &Core::ActivityGate::setScreenActive);
        connect(pm, &Core::PowerManager::batteryLevelChanged, gate,
                [gate, pm](int) { gate->setBatterySaver(pm->isBatterySaver()); });
        connect(pm, &Core::PowerManager::thermalStateChanged, gate,
                [gate](Core::ThermalState s) {
                    gate->setThermalCritical(s == Core::ThermalState::Critical);
                });
    }
    qCInfo(lcApp) << "PowerManager attached:"
                  << "plasma=" << d->power->isPlasma()
                  << "upower=" << d->power->upowerConnected()
                  << "battery=" << d->power->batteryPct() << "%";

    // V32.34+ battery: the SleepStateMachine folds PowerManager (screen /
    // charge) + AppContext (foreground / idle) into ONE shared state enum
    // and emits sleepStateChanged. Both inputs exist now (power above,
    // appContext just before). Components subscribe via
    // AppController::sleepStateMachine() instead of independently
    // interpreting raw screen/app/battery signals. Parented to `this`.
    d->sleepState = std::make_unique<Core::SleepStateMachine>(
        d->power.get(), d->appContext.get(), this);
    connect(d->sleepState.get(), &Core::SleepStateMachine::sleepStateChanged,
            this, [](Core::SleepState s) {
                qCInfo(lcApp) << "SleepState →" << Core::sleepStateName(s);
            });

    // ── Network ────────────────────────────────────────────────────────────
    // Primary backend: Gossip (LAN discovery via UDP multicast).
    auto gossip = std::make_unique<Network::GossipDiscovery>();
    d->network = std::make_unique<Network::NetworkManager>(std::move(gossip));
    // V32.27: hand AppContext to NetworkManager so its LAN-isolation
    // threshold can adapt to connection type. Safe to attach even if
    // context is still in default-defaults — NetworkManager just falls
    // back to the static threshold when context.connectionType isn't
    // Cellular. Order matters: AppContext was constructed just above.
    d->network->setAppContext(d->appContext.get());

    // Secondary backend: DHT peer discovery for internet-scale connections.
    // Priority: full libjami > opendht-direct > stub (no-op).
#if defined(SNAPCLONE_OPENDHT_BACKEND) && !defined(SNAPCLONE_JAMI_BACKEND)
    d->jami = std::make_unique<Network::DhtDiscovery>();
    qCInfo(lcApp) << "Using OpenDHT direct backend for internet discovery.";
    // V32.28: hand AppContext to DhtDiscovery so its 30s bootstrap-
    // retry stretches when reachability has recently flapped. Cast
    // is safe — we just created it as DhtDiscovery.
    if (auto *dhtDisc = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
        dhtDisc->setAppContext(d->appContext.get());
        // Register the one and only DHT node with the app-wide service, so every
        // UI surface ties to a single handle and a second node can't be created.
        Network::DhtService::instance()->registerDiscovery(dhtDisc);
    }
#else
    // JamiDiscovery: real libjami when compiled in, no-op stub when not.
    d->jami = std::make_unique<Network::JamiDiscovery>();
#endif
    // DHT peer discovery goes through AppController's address queue
    // (connectByDhtId → tryNextPeerAddress → connectToPeer).
    // We do NOT forward DHT results to NetworkManager::onPeerDiscovered here
    // because that would bypass address-queue prioritisation and connect to
    // the first result (often a stale private addr) before trying public IPs.
    // Gossip (LAN) results are handled by NetworkManager's own internal wiring.
    // When a peer left a connection request in our DHT mailbox while we were
    // offline, connect to them as soon as we see the request on startup.
#if defined(SNAPCLONE_OPENDHT_BACKEND)
    if (auto *dhtDisc = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
        // When a DHT lookup returns a peer's public key, register it immediately.
        // This enables encryptFor before any direct connection has been made.
        connect(dhtDisc, &Network::DhtDiscovery::peerPublicKeyDiscovered,
                this, [this](const Core::UserId &peerId, const QByteArray &pubKey) {
                    if (pubKey.isEmpty()) return;
                    // Skip redundant ECDH if we already have a session key.
                    // DHT returns 3-4 records per peer per batch, each firing
                    // this signal — avoid sc_derive_session_key running 4x for
                    // the same peer key.
                    if (d->crypto->hasSessionKey(peerId)) {
                        qCDebug(lcApp) << "[PUBKEY] Already have session key for"
                                       << peerId.value << "— skipping re-derive";
                        return;
                    }
                    // V32.33: TOFU check (CRYPTO_REVIEW.md S-20). DHT-published
                    // pubkeys can be planted (S-12), so the same swap defense
                    // applies here as in the SHAK path. See AppController_connect.cpp
                    // for the canonical comment block.
                    const QByteArray storedKey = d->storage->loadPeerPublicKey(peerId);
                    if (!storedKey.isEmpty() && storedKey != pubKey) {
                        qCWarning(lcApp) << "[PUBKEY] DHT-arrived key for" << peerId.value
                                         << "differs from stored — recording pending,"
                                         << " keeping existing.";
                        d->storage->recordPendingKeyChange(peerId, pubKey, storedKey);
                        d->crypto->registerPeerPublicKey(peerId, storedKey);
                        return;
                    }
                    d->crypto->registerPeerPublicKey(peerId, pubKey);
                    d->storage->savePeerPublicKey(peerId, pubKey);
                    qCInfo(lcApp) << "[PUBKEY] Registered public key for" << peerId.value
                                  << "from DHT (" << pubKey.size() << "bytes)"
                                  << "— mailbox encryption now available";
                });
        connect(dhtDisc, &Network::DhtDiscovery::connectionRequestReceived,
                this, [this](const Core::UserId &fromId,
                             const QByteArray &blob) {
                    // blob = sc-from_id + "\n" + nonce(12) + ciphertext
                    // Extract and decrypt the body
                    constexpr int kNonceSize = 12;
                    const qsizetype nl = blob.indexOf('\n');
                    if (nl < 1 || blob.size() <= nl + 1 + kNonceSize) {
                        qCWarning(lcApp) << "Malformed connection request from" << fromId.value;
                        return;
                    }
                    const QByteArray nonce      = blob.mid(nl + 1, kNonceSize);
                    const QByteArray ciphertext = blob.mid(nl + 1 + kNonceSize);
                    const QString nonceHex = nonce.toHex();
                    if (d->seenMailboxNonces.contains(nonceHex)) {
                        // V32.34+: aggregate duplicate-nonce logs. Instead of
                        // spamming one line per dup, count them and emit a
                        // single summary via a flush timer that fires ~500ms
                        // after the last dup. Reads as ONE line per mailbox
                        // check regardless of how many neighboring DHT keys
                        // yielded the same blob.
                        ++d->mailboxDupCount;
                        d->mailboxDupLastFrom = fromId.value;
                        if (!d->mailboxDupFlush) {
                            d->mailboxDupFlush = new QTimer(this);
                            d->mailboxDupFlush->setSingleShot(true);
                            d->mailboxDupFlush->setInterval(500);
                            QObject::connect(d->mailboxDupFlush, &QTimer::timeout,
                                             this, [this]() {
                                if (d->mailboxDupCount <= 0) return;
                                qCDebug(lcApp) << "[MAILBOX] Duplicate nonce from"
                                               << d->mailboxDupLastFrom
                                               << "— skipped"
                                               << d->mailboxDupCount << "copies";
                                d->mailboxDupCount = 0;
                                d->mailboxDupLastFrom.clear();
                            });
                        }
                        d->mailboxDupFlush->start();  // resets to 500ms
                        return;
                    }
                    // V32.33+ Phase 1 audit: nonce-mark-as-seen moved to
                    // AFTER successful decryption (was previously here,
                    // before the decrypt attempt). Rationale: a bogus or
                    // failed-decrypt blob shouldn't consume a slot in the
                    // 500-entry cache, otherwise an attacker can spam
                    // bogus blobs to evict legitimate nonces (FIFO
                    // eviction), then replay legitimate captured blobs.
                    // Only nonces from blobs we actually accepted need
                    // replay protection — failed-decrypt nonces aren't
                    // "real messages" so don't need cache slots.
                    qCInfo(lcApp) << "[MAILBOX] Received blob from" << fromId.value
                                  << "— blob:" << blob.size() << "bytes"
                                  << "nonce:" << nonceHex
                                  << "ciphertext:" << ciphertext.size() << "bytes";
                    const auto dec = d->crypto->decryptFrom(fromId, ciphertext, nonce);
                    if (!dec) {
                        qCWarning(lcApp) << "[MAILBOX] Decrypt failed for" << fromId.value
                                         << "— no session key yet.";
                        // Key not yet registered. Two recovery paths:
                        // 1. DHT lookup for fromId — peerPublicKeyDiscovered will fire
                        //    and register their key. Then connect directly.
                        // 2. Retry decrypt after a short delay — if DHT already has their
                        //    key in flight (QueuedConnection), it arrives momentarily.
                        if (fromId.value.startsWith(QStringLiteral("sc-"))) {
                            qCInfo(lcApp) << "[MAILBOX] Triggering DHT lookup for" << fromId.value
                                          << "to fetch their public key, then retrying decrypt";
                            connectByDhtId(fromId.value); // also fetches their pubkey via DHT
                            // Retry decrypt after 3s — DHT key should be registered by then.
                            const QByteArray blobCopy = blob;
                            const Core::UserId fromIdCopy = fromId;
                            QTimer::singleShot(Timings::kMailboxDecryptRetryMs, this, [this, fromIdCopy, blobCopy]() {
                                constexpr int kNonceSize2 = 12;
                                const qsizetype nl2 = blobCopy.indexOf('\n');
                                if (nl2 < 1 || blobCopy.size() <= nl2 + 1 + kNonceSize2) return;
                                const QByteArray nonce2      = blobCopy.mid(nl2 + 1, kNonceSize2);
                                const QByteArray ciphertext2 = blobCopy.mid(nl2 + 1 + kNonceSize2);
                                // V32.33+ Phase 1 audit: if the original
                                // processing succeeded between the time
                                // we scheduled this retry and now (e.g.
                                // a duplicate of the same blob arrived
                                // and decrypted successfully), the nonce
                                // is now in seen. Skip retry — we already
                                // processed this blob.
                                const QString nonceHex2early = nonce2.toHex();
                                if (d->seenMailboxNonces.contains(nonceHex2early)) {
                                    qCDebug(lcApp) << "[MAILBOX] Retry skipping — already processed";
                                    return;
                                }
                                const auto dec2 = d->crypto->decryptFrom(fromIdCopy, ciphertext2, nonce2);
                                if (!dec2) {
                                    qCWarning(lcApp) << "[MAILBOX] Retry decrypt still failed for"
                                                     << fromIdCopy.value << "— key not yet available";
                                    return;
                                }
                                qCInfo(lcApp) << "[MAILBOX] Retry decrypt succeeded for" << fromIdCopy.value;
                                // V32.33+ Phase 1 audit: retry success also
                                // adds the nonce to seenMailboxNonces for
                                // replay protection.
                                constexpr int kMaxSeenNoncesRetry = 500;
                                if (d->seenMailboxNonces.size() >= kMaxSeenNoncesRetry) {
                                    const int toRemove = kMaxSeenNoncesRetry / 4;
                                    for (int ei = 0; ei < toRemove && !d->seenMailboxNonceOrder.isEmpty(); ++ei)
                                        d->seenMailboxNonces.remove(d->seenMailboxNonceOrder.takeFirst());
                                }
                                d->seenMailboxNonces.insert(nonceHex2early);
                                d->seenMailboxNonceOrder.append(nonceHex2early);
                                // Re-parse and connect
                                const QStringList p2 = QString::fromUtf8(dec2->plaintext)
                                    .split(QLatin1Char('\n'), Qt::SkipEmptyParts);
                                if (p2.size() >= 5 && p2[0] == QLatin1String("CONNREQ")) {
                                    auto parseA = [](const QString &s) -> Core::PeerAddress {
                                        const qsizetype c = s.lastIndexOf(QLatin1Char(':'));
                                        if (c < 0) return {};
                                        return { s.left(c), static_cast<quint16>(s.mid(c+1).toUShort(nullptr)) };
                                    };
                                    const Core::PeerAddress a2 = parseA(p2[2]);
                                    if (!a2.host.isEmpty()) {
                                        d->storage->cacheDhtResult(fromIdCopy, a2);
                                        if (!d->peerPunchAddr.contains(fromIdCopy.value) && p2.size() > 3)
                                            d->setPeerPunchAddr(fromIdCopy.value, parseA(p2[3]));
                                    }
                                }
                                connectByDhtId(fromIdCopy.value);
                            });
                        }
                        return;
                    }
                    // Plaintext: "CONNREQ\nfrom\nhost:port\npunch_host:punch\ntimestamp\n"
                    // V32.33+ Phase 1 audit: now that decryption succeeded,
                    // mark this nonce as seen so replays of THIS specific
                    // blob get rejected. Cap cache size with FIFO eviction.
                    constexpr int kMaxSeenNonces = 500;
                    if (d->seenMailboxNonces.size() >= kMaxSeenNonces) {
                        const int toRemove = kMaxSeenNonces / 4; // evict 25% at a time
                        for (int ei = 0; ei < toRemove && !d->seenMailboxNonceOrder.isEmpty(); ++ei)
                            d->seenMailboxNonces.remove(d->seenMailboxNonceOrder.takeFirst());
                    }
                    d->seenMailboxNonces.insert(nonceHex);
                    d->seenMailboxNonceOrder.append(nonceHex);

                    const QStringList parts = QString::fromUtf8(dec->plaintext)
                        .split(QLatin1Char('\n'), Qt::SkipEmptyParts);
                    if (parts.size() < 4) return;

                    // ── PEERREFL: peer is reporting our own UDP reflexive addr ──────────
                    if (parts[0] == QLatin1String("PEERREFL")) {
                        if (parts.size() < 4) return;
                        const qint64 ts2 = parts[3].toLongLong();
                        if (QDateTime::currentSecsSinceEpoch() - ts2 > 600) return; // stale
                        const QString seenStr = parts[2];
                        const qsizetype c2 = seenStr.lastIndexOf(QLatin1Char(':'));
                        if (c2 < 0) return;
                        const QString seenHost = seenStr.left(c2);
                        const quint16 seenPort = static_cast<quint16>(seenStr.mid(c2+1).toUShort(nullptr));
                        if (seenHost.isEmpty() || seenPort == 0) return;

                        // A peer saw our UDP punch arrive from seenHost:seenPort.
                        // This is our actual CGNAT UDP reflexive addr — more reliable than
                        // STUN when STUN servers are blocked, because it came from a real peer.
                        const Core::PeerAddress peerRefl{ seenHost, seenPort };
                        qCInfo(lcApp) << "[PEERREFL] Peer" << fromId.value
                                      << "reports our UDP reflexive:" << peerRefl.toString()
                                      << "— updating DHT announce";

                        // Only use it if STUN hasn't already given us something better.
                        const Core::PeerAddress stunRefl = d->nat
                            ? d->nat->reflexiveAddress() : Core::PeerAddress{};
                        if (stunRefl.host.isEmpty()) {
                            // STUN failed — use peer-reported reflexive.
                            // Re-announce to DHT so other peers can punch to the right addr.
                            if (d->jami && !d->stableId.isEmpty()) {
                                const Core::UserId selfId2{ d->stableId };
                                d->jami->announcePresence(selfId2, peerRefl);
                                d->storage->cacheDhtResult(selfId2, peerRefl);
                                qCInfo(lcApp) << "[PEERREFL] Announced peer-reflexive addr to DHT:"
                                              << peerRefl.toString();
                            }
                        } else {
                            qCDebug(lcApp) << "[PEERREFL] STUN already gave us" << stunRefl.toString()
                                           << "— ignoring peer-reflexive" << peerRefl.toString();
                        }
                        return;
                    }

                    // ── CONNREQ: peer wants us to connect to them ─────────────────────
                    if (parts[0] != QLatin1String("CONNREQ") || parts.size() < 5) return;
                    const qint64 ts = parts[4].toLongLong();
                    if (QDateTime::currentSecsSinceEpoch() - ts > 600) {
                        qCInfo(lcApp) << "Stale connection request from" << fromId.value
                                      << "— ignoring";
                        return;
                    }
                    // parts[2] = TCP/WS addr, parts[3] = UDP punch addr (may differ)
                    // V32.34+ fix: the connreq sender may write the address WITH a
                    // ws://|wss:// scheme prefix. PeerAddress.host must be a BARE
                    // host (no scheme) because PeerAddress::toString() re-adds the
                    // scheme — otherwise we get the "ws://ws://host:port" double-
                    // prefix seen in the field logs, which NetworkManager then
                    // rejects as an invalid target ("refusing invalid target
                    // ws://ws://..."), wasting a connection attempt before relay
                    // fallback. Strip any leading scheme here and set tls from it,
                    // then split host:port on the LAST colon of the remainder.
                    auto parseAddr = [](QString s) -> Core::PeerAddress {
                        bool tls = false;
                        if (s.startsWith(QLatin1String("wss://"), Qt::CaseInsensitive)) {
                            tls = true;
                            s = s.mid(6);
                        } else if (s.startsWith(QLatin1String("ws://"), Qt::CaseInsensitive)) {
                            s = s.mid(5);
                        }
                        // Defensive: a malformed double-scheme ("ws://ws://...")
                        // from an older/buggy peer would leave another scheme on
                        // the front — strip repeatedly so we always reach the bare
                        // host:port. Bounded loop (max a few iterations).
                        while (s.startsWith(QLatin1String("ws://"), Qt::CaseInsensitive)
                            || s.startsWith(QLatin1String("wss://"), Qt::CaseInsensitive)) {
                            if (s.startsWith(QLatin1String("wss://"), Qt::CaseInsensitive)) {
                                tls = true; s = s.mid(6);
                            } else {
                                s = s.mid(5);
                            }
                        }
                        // Strip a trailing path if present (e.g. "host:port/snap").
                        const qsizetype slash = s.indexOf(QLatin1Char('/'));
                        if (slash >= 0) s = s.left(slash);
                        const qsizetype c = s.lastIndexOf(QLatin1Char(':'));
                        if (c < 0) return {};
                        Core::PeerAddress a;
                        a.host = s.left(c);
                        a.port = static_cast<quint16>(s.mid(c + 1).toUShort(nullptr));
                        a.tls  = tls;
                        return a;
                    };
                    const Core::PeerAddress fromAddr  = parseAddr(parts[2]); // TCP
                    const Core::PeerAddress punchAddr = parts.size() > 3
                                                      ? parseAddr(parts[3])  // UDP punch
                                                      : fromAddr;
                    if (fromAddr.host.isEmpty()) return;
                    // V32.37+ delivery fix: optional 6th field = sender's LAN
                    // address. When both peers are on the same WiFi, the public
                    // TCP/punch addrs are a HAIRPIN (shared router IP) which we
                    // correctly skip — but without the LAN address there's then
                    // nothing left to try and the snap stays queued forever. The
                    // sender now appends its LAN addr; cache it FIRST so
                    // connectByDhtId prefers it over the hairpin public IP. Older
                    // senders omit it (parts.size()==5) — harmless, we fall back
                    // to the public path exactly as before.
                    Core::PeerAddress lanAddr;
                    if (parts.size() >= 6) {
                        lanAddr = parseAddr(parts[5]);
                        if (!lanAddr.host.isEmpty()
                            && lanAddr.host != fromAddr.host) {  // not just the public IP again
                            qCInfo(lcApp) << "Connection request includes LAN addr"
                                          << lanAddr.toString()
                                          << "— caching as preferred (same-WiFi direct path)";
                            d->storage->cacheDhtResult(fromId, lanAddr);
                        }
                    }
                    qCInfo(lcApp) << "Connection request from" << fromId.value
                                  << "@ TCP" << fromAddr.toString()
                                  << "UDP punch" << punchAddr.toString()
                                  << "— decrypted OK";
                    d->storage->cacheDhtResult(fromId, fromAddr);
                    // Cache the punch addr separately if it differs — used by initiateHolePunch
                    if (!punchAddr.host.isEmpty() && punchAddr != fromAddr)
                        d->storage->cacheDhtResult(fromId, punchAddr);
                    // Store as a preferred punch addr for this peer
                    if (!punchAddr.host.isEmpty())
                        d->setPeerPunchAddr(fromId.value, punchAddr);
                    connectByDhtId(fromId.value);

                    // PEERREFL handling: if this is a peer-reflexive report
                    // (peer saw our UDP arrive from a certain addr), use it as our
                    // own reflexive addr for future DHT announces and punch attempts.
                    // Handled below in the PEERREFL branch of the same mailbox callback.
                });
    }
#endif // SNAPCLONE_OPENDHT_BACKEND
    connect(d->jami.get(), &Network::IDiscovery::peerDiscovered,
            this, [this](const Core::UserId &id, const Core::PeerAddress &addr) {
                qCInfo(lcApp) << "Peer discovered via DHT:" << id.value
                              << "@" << addr.toString();
                // Address queue classification happens in connectByDhtId's lambda.
            });

    // Also persist the resolved address so reconnects and mutual-connect
    // attempts can find a cached address without a second DHT lookup.
    // On network change: re-fetch public IP, clear stale DHT cache,
    // and force fresh re-announce so peers can find us at our new address.
    connect(d->jami.get(), &Network::IDiscovery::networkChanged,
            this, [this]() {
                // d->stableId is set before jami->start(), so it is always
                // valid here. We read it from d rather than capturing selfId
                // (which is a local variable defined 100+ lines after this
                // connect() call and would be dangling by the time the signal
                // fires).
                if (d->stableId.isEmpty()) return;
                const Core::UserId netChangeSelfId{ d->stableId };
                qCInfo(lcApp) << "Network changed (DHT topology) — refreshing public IP.";
                // Do NOT purge DHT cache here — this signal fires for DHT topology
                // changes (peer churn), not just our own IP change. Purging on every
                // topology change would force all peers to do expensive fresh lookups.
                // Cache is purged in onNetworkIpChanged() when our public IP actually changes.
                onNetworkIpChanged(d->selfAddr.host);
            });

    // Note: peerDiscovered address-save is handled in the first connect block above.

#ifdef SNAPCLONE_JAMI_BACKEND
    // jamiPayloadReceived and jamiAccountReady are JamiDiscovery-specific signals
    // (full libjami conversation API). Guard them so OpenDHT-only builds don't
    // try to connect to non-existent signals.
    {
        auto *jamiDisc = static_cast<Network::JamiDiscovery *>(d->jami.get());
        connect(jamiDisc, &Network::JamiDiscovery::jamiPayloadReceived,
                this, [this](const Core::UserId &senderId,
                             const QByteArray &payload,
                             const QString & /*convId*/) {
                    const auto it = d->transportToRealId.find(senderId.value);
                    const Core::UserId effective =
                        (it != d->transportToRealId.end())
                            ? Core::UserId{ *it } : senderId;
                    d->router->routeInbound(payload, effective);
                });
        connect(jamiDisc, &Network::JamiDiscovery::jamiAccountReady,
                this, [](const Network::JamiAccountInfo &info) {
                    qCInfo(lcApp) << "Jami account ready. ID:" << info.jamiId;
                });
    }
#endif // SNAPCLONE_JAMI_BACKEND

    // ── Firewall port request ─────────────────────────────────────────────
    d->firewall = std::make_unique<Network::FirewallPortManager>();
    connect(d->firewall.get(), &Network::FirewallPortManager::portOpened,
            this, [](const Network::PortResult &r) {
                qCInfo(lcApp) << (r.success ? "Port opened:" : "Port request failed:")
                              << r.message;
            });
    connect(d->firewall.get(), &Network::FirewallPortManager::allPortsProcessed,
            this, [this](bool ok) {
                if (!ok) {
                    emit errorOccurred(tr(
                        "Could not open required ports automatically. "
                        "Use Network Diagnostics (🔍) for manual instructions."));
                }
                qCInfo(lcApp) << "Firewall setup complete.";
            });

    d->firewall->requestPorts({
        { 9876, QStringLiteral("tcp") },   // WebSocket
        { 9876, QStringLiteral("udp") },   // UDP hole punch (same port as TCP)
        { 9877, QStringLiteral("udp") },   // Gossip multicast
    });

    // ── Router ────────────────────────────────────────────────────────────
    d->router = std::make_unique<SnapRouter>(
        d->network.get(), d->storage.get(), d->crypto.get());

    connectSubsystems();

    // ── Stable node identity ─────────────────────────────────────────────
    // The stable ID (e.g. "sc-a3f7c291") is generated once and persisted to
    // disk. It stays constant across IP changes, restarts, and network changes.
    // This is the ID we announce into the DHT and show in Settings.
    const QString stableId = Identity::loadOrGenerate();
    d->stableId = stableId;

    // For the session / SHAK handshake we still use "user@ip" as a human-readable
    // display name, but map it to the stable ID internally.
    const QString osUser = qEnvironmentVariable("USER",
                           qEnvironmentVariable("USERNAME", QStringLiteral("user")));

    QString primaryIp;
    for (const QHostAddress &addr : QNetworkInterface::allAddresses()) {
        if (!addr.isLoopback() && addr.protocol() == QAbstractSocket::IPv4Protocol) {
            primaryIp = addr.toString();
            break;
        }
    }
    if (primaryIp.isEmpty()) primaryIp = QStringLiteral("127.0.0.1");

    // The session username shown in the UI: "user@ip (sc-XXXXXXXX)"
    const QString displayName = QStringLiteral("%1@%2 (%3)").arg(osUser, primaryIp, stableId);

    const QString hostId = QSysInfo::machineUniqueId().isEmpty()
                               ? QHostInfo::localHostName()
                               : QString::fromLatin1(QSysInfo::machineUniqueId());
    const QByteArray autoHash =
        QCryptographicHash::hash(hostId.toUtf8(), QCryptographicHash::Sha256).toHex();

    qCInfo(lcApp) << "Stable ID:" << stableId << "Display:" << displayName;
    d->session->beginLogin(displayName, QString::fromLatin1(autoHash));

    // Use the stable ID as the network/crypto identity so it survives IP changes.
    const UserId selfId{ stableId };
    d->router->setSelfUserId(selfId);
    d->crypto->setSelfUserId(selfId); // must be before generateIdentityKeyPair

    // Ensure we have an identity key pair — generated once, persisted to disk.
    // This must happen before network start so the handshake can send the key.
    if (d->crypto->publicIdentityKey(selfId).isEmpty()) {
        if (d->crypto->generateIdentityKeyPair(selfId)) {
            qCInfo(lcApp) << "Generated new identity key pair for" << selfId.value;
        } else {
            qCWarning(lcApp) << "Failed to generate identity key pair — crypto will not work";
        }
    } else {
        qCInfo(lcApp) << "Loaded existing identity key for" << selfId.value;
    }

    // Push our X25519 public key to DhtDiscovery so it's embedded in every
    // DHT announce — allows peers to encrypt mailbox requests before first connect.
    #if defined(SNAPCLONE_OPENDHT_BACKEND)
    if (auto *dht = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
        const QByteArray selfPubKey = d->crypto->publicIdentityKey(selfId);
        if (!selfPubKey.isEmpty()) {
            dht->setSelfPublicKey(selfPubKey);
            qCInfo(lcApp) << "Published X25519 public key in DHT ("
                          << selfPubKey.size() << "bytes)";
        } else {
            qCWarning(lcApp) << "No public key available — mailbox encryption disabled";
        }
    }
    #endif // SNAPCLONE_OPENDHT_BACKEND
    // Start network with the resolved user identity.
    d->network->start(selfId);

    // Pollinator local beacon (UDP multicast for Nearby screen + auto-LAN-connect)
    d->pollinator = std::make_unique<Network::PollinatorDiscovery>();
    d->pollinator->setSelfId(d->stableId, displayName);

    // Auto-connect: when a peer beacons on LAN, connect directly to their IP.
    // No DHT needed — the beacon includes the sender's LAN address.
    // This is the primary path for same-network peers (home WiFi, office, etc.)
    // Beacon is started/stopped via ContactsPage::startBeacon (Nearby UI lives there now).
    connect(d->pollinator.get(), &Network::PollinatorDiscovery::peerBeaconed,
            this, [this](const Core::UserId &peerId,
                         const QString &/*displayName*/,
                         const QString &senderAddress) {
        if (d->network->hasAnyLiveSession(peerId)) return; // already connected
        const Core::PeerAddress lanAddr{ senderAddress, 9876 };
        qCInfo(lcApp) << "[BEACON] LAN peer" << peerId.value
                      << "@ " << senderAddress << "— connecting directly";
        d->network->connectToPeer(peerId, lanAddr);
    });

    // Start the secondary DHT discovery backend.
#ifdef SNAPCLONE_JAMI_BACKEND
    qCInfo(lcApp) << "Starting Jami DHT backend for internet-scale discovery…";
#elif defined(SNAPCLONE_OPENDHT_BACKEND)
    qCInfo(lcApp) << "Starting OpenDHT backend for internet-scale discovery…";
#endif
    d->jami->start();

    // V32.34+ battery: wire the DHT resource manager. It subscribes to
    // PowerManager and drives a 4-tier DHT mode (Full/Maintenance/Minimal/
    // Off) based on battery %, charging, screen-on/off, thermal state and
    // power profile. This is the single biggest idle-battery lever: a
    // backgrounded phone on battery drops to Minimal (rare re-announce)
    // or Off rather than the constant Full-rate DHT traffic the app used
    // to emit 24/7.
    //
    // Only meaningful with a real DHT backend; the stub backend ignores
    // mode changes. Construct regardless — it's cheap and keeps the wiring
    // uniform across build configs. The manager only acts on a
    // DhtDiscovery*, so we hand it the casted pointer (nullptr-safe: it
    // null-checks before every call).
#if defined(SNAPCLONE_OPENDHT_BACKEND)
    if (auto *dhtDisc = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
        d->dhtResource = std::make_unique<Network::DhtResourceManager>(
            dhtDisc, d->power.get(), this);
        // V32.34+ POWER: opt-in aggressive screen-off — park DHT fully when
        // the screen is off (on battery, no active session) to silence the
        // node's background port chatter, the dominant pocketed-radio cost.
        // Gated behind SC_DHT_PARK_SCREEN_OFF=1 (env) or the QSettings flag
        // "dht/parkScreenOff" so it can be validated on-device before becoming
        // default. See docs/TODO.md "Limit DHT PORT traffic when screen-off".
        {
            bool park = qEnvironmentVariableIntValue("SC_DHT_PARK_SCREEN_OFF") == 1;
            if (!park) {
                QSettings s;
                park = s.value(QStringLiteral("dht/parkScreenOff"), false).toBool();
            }
            if (park) d->dhtResource->setAggressiveScreenOff(true);
        }
        // Active P2P sessions should pin DHT to Full regardless of battery.
        // We bridge NetworkManager's peerConnected/peerDisconnected into
        // the resource manager's session-active flag. Tracking a small
        // local count handles the "multiple peers" case: DHT stays Full
        // until the LAST peer disconnects, not just any one.
        connect(d->network.get(), &Network::NetworkManager::peerConnected,
                this, [this](Core::UserId) {
                    ++d->liveSessionCount;
                    if (d->dhtResource) d->dhtResource->setSessionActive(true);
                });
        connect(d->network.get(), &Network::NetworkManager::peerDisconnected,
                this, [this](Core::UserId) {
                    if (d->liveSessionCount > 0) --d->liveSessionCount;
                    if (d->dhtResource && d->liveSessionCount == 0)
                        d->dhtResource->setSessionActive(false);
                });
        qCInfo(lcApp) << "DhtResourceManager wired — power-aware DHT mode active";
    }
#endif
    // Check DHT mailbox 5s after startup, then every 5 minutes.
    // Covers the case where a peer publishes after we started up.
    auto *mailboxTimer = new QTimer(this);
    mailboxTimer->setInterval(Timings::kMailboxCheckMs);
    connect(mailboxTimer, &QTimer::timeout, this, [this]() {
        // V32.47: pause periodic mailbox polling while a call is live — it's
        // background reconnection work that can wait until the call ends. The
        // timer keeps ticking and resumes on the next tick once the call is over.
        if (callActive()) {
            qCDebug(lcApp) << "[MAILBOX] Periodic check skipped — call active";
            return;
        }
        if (d->jami && !d->stableId.isEmpty()) {
#if defined(SNAPCLONE_OPENDHT_BACKEND)
            if (auto *dht = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
                qCInfo(lcApp) << "[MAILBOX] Periodic check for connection requests";
                dht->checkConnectionRequests(Core::UserId{ d->stableId });
            }
#endif // SNAPCLONE_OPENDHT_BACKEND
        }
    });
    mailboxTimer->start();
    // Mailbox checks at 5s, 30s, and 90s after startup.
    // The 5s check catches requests published before we started.
    // The 30s and 90s checks catch requests published by peers who started
    // around the same time and whose DHT write took a few seconds to propagate.
    for (int delayS : {5, 30, 90}) {
        QTimer::singleShot(delayS * 1000, this, [this, delayS]() {
            if (d->jami && !d->stableId.isEmpty()) {
#if defined(SNAPCLONE_OPENDHT_BACKEND)
                if (auto *dht = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
                    qCInfo(lcApp) << "[MAILBOX] Startup check for connection requests (t+"
                                  << delayS << "s)";
                    dht->checkConnectionRequests(Core::UserId{ d->stableId });
                }
#endif // SNAPCLONE_OPENDHT_BACKEND
            }
        });
    }

    // Startup reconnect: proactively connect to all known sc- contacts from DB.
    // Without this, the app only connects when a peer initiates or when the user
    // manually enters a peer's sc-ID again after every restart.
    // Two attempts: 8s (catches fast DHT bootstrap) and 20s (catches slow bootstrap).
    auto reconnectKnownContacts = [this]() {
        const Core::UserId self{ d->stableId };
        const auto contacts = d->storage->loadContacts(self);
        int scheduled = 0;
        for (const Core::Contact &c : contacts) {
            // Only auto-connect to stable sc- IDs — not manual IP entries.
            if (!c.id.value.startsWith(QStringLiteral("sc-"))) continue;
            if (d->network->hasEstablishedSession(c.id)) continue;
            if (d->network->hasAnyLiveSession(c.id)) continue;
            // Stagger lookups 500ms apart to avoid hammering the DHT.
            QTimer::singleShot(scheduled * 500, this, [this, id = c.id]() {
                if (!d->network->hasAnyLiveSession(id)) {
                    qCInfo(lcApp) << "[STARTUP] Auto-reconnecting to known contact:" << id.value;
                    connectByDhtId(id.value);
                }
            });
            ++scheduled;
        }
        if (scheduled > 0)
            qCInfo(lcApp) << "[STARTUP] Scheduled reconnect to" << scheduled << "known contact(s)";
    };
    QTimer::singleShot(Timings::kReconnectFastMs, this, reconnectKnownContacts);
    QTimer::singleShot(20000, this, reconnectKnownContacts); // second pass after slow DHT bootstrap
    // announcePresence publishes our WebSocket endpoint into the DHT so peers
    // can find us by UserId.  The address is filled in by NetworkManager at start;
    // we pass selfId so the backend knows what key to publish under.
    // Announce with LAN IP immediately so local peers can connect.
    // Use the port the WS server actually bound — may be 9877/9878 if 9876 was taken.
    const quint16 wsPort = d->network->actualListenPort();
    const quint16 announcePort = (wsPort > 0) ? wsPort : 9876;
    const Core::PeerAddress selfAddr{ primaryIp, announcePort };
    d->jami->announcePresence(selfId, selfAddr);
    d->selfAddr = selfAddr;
    // V32.37+ delivery fix: primaryIp here is our LAN address (chosen by
    // pickBestLanAddress before STUN runs). Preserve it separately so it survives
    // the later selfAddr overwrite with the public IP — CONNREQs need the LAN
    // address to reach a same-WiFi peer.
    if (!primaryIp.isEmpty()) d->lanAddr = selfAddr;
    qCInfo(lcApp) << "Self address:" << selfAddr.toString()
                  << "(WS port" << announcePort << ")";

    // UPnP IGD port mapping: ask the home router to forward our WS port.
    // This lets peers on other networks TCP-connect to our public IP:port
    // even without manual router configuration.
    d->upnp = std::make_unique<Network::UPnPPortMapper>();
    connect(d->upnp.get(), &Network::UPnPPortMapper::mappingAdded,
            this, [announcePort](quint16, const QString &) {
                qCInfo(lcApp) << "UPnP: router forwarding port" << announcePort
                              << "— inbound TCP connections now possible.";
            });
    connect(d->upnp.get(), &Network::UPnPPortMapper::mappingFailed,
            this, [](const QString &reason) {
                qCInfo(lcApp) << "UPnP unavailable (manual port forward needed):" << reason;
            });
    // NAT-PMP fallback: fires if UPnP times out but NAT-PMP succeeds (Bell Hub 3000/4000)
    connect(d->upnp.get(), &Network::UPnPPortMapper::portMapped,
            this, [announcePort](quint16 extPort, const QString &method) {
                qCInfo(lcApp) << method << ": router mapped external port" << extPort
                              << "→ internal" << announcePort
                              << "— inbound connections now possible!";
            });
    d->upnp->start(primaryIp, announcePort);

    // Fast IP-change poll: re-announces gossip + DHT within 5s of interface change.
    // Handles the common case where a phone starts on hotspot, then joins home WiFi.
    d->lastKnownPrimaryIp = primaryIp;
    // Load relay URL from settings (empty = disabled).
    d->relayUrl = QSettings().value(
        QStringLiteral("network/relayUrl"), QString()).toString();
    // Built-in fallback: Magic Wormhole public transit relay.
    // Free, FOSS, no registration required. Used when no custom relay is set.
    // Users can override in Settings → Relay Server.
    static const QString kBuiltinRelay =
        QStringLiteral("tcp://transit.magic-wormhole.io:4001");
    if (d->relayUrl.isEmpty()) {
        d->relayUrl = kBuiltinRelay;
        qCInfo(lcApp) << "Using built-in relay:" << d->relayUrl;
    } else {
        qCInfo(lcApp) << "Relay URL configured:" << d->relayUrl;
    }

    // Restore peer public keys from DB so session keys are available immediately.
    // Without this, encryptFor() fails for offline contacts until they come online
    // and re-do the handshake or DHT lookup. With persisted keys we can queue snaps
    // for offline peers right away — the SYNC mechanism delivers them on reconnect.
    {
        const auto storedKeys = d->storage->loadAllPeerPublicKeys();
        int restored = 0;
        for (const auto &[peerId, pubKey] : storedKeys) {
            d->crypto->registerPeerPublicKey(peerId, pubKey);
            ++restored;
        }
        if (restored > 0)
            qCInfo(lcApp) << "[STARTUP] Restored" << restored << "peer public key(s) from DB";
    }

    // Start zero-infrastructure direct discovery (parallel, lower priority than DHT).
    d->directDiscovery = std::make_unique<Network::DirectDiscovery>();
    // Load known peers from storage for LAN probe
    QList<Network::KnownPeer> knownPeers;
    for (const Core::Contact &contact : d->storage->loadContacts(selfId)) {
        const Core::UserId contactId = contact.id;
        if (contactId.value.isEmpty()) continue;
        const auto addrOpt = d->storage->loadPeerAddress(contactId);
        if (addrOpt && !addrOpt->host.isEmpty())
            knownPeers.append({ contactId, *addrOpt,
                                QDateTime::currentMSecsSinceEpoch() - 60000 });
    }
    d->directDiscovery->start(selfId, announcePort, knownPeers);

    // V32.35+ LAN-DIRECT FIX (second half): feed gossip-discovered peer
    // addresses into DirectDiscovery's known-peer table. Previously
    // m_knownPeers was ONLY seeded from storage (DHT/public addresses), so the
    // LAN probe's subnet check always failed for two same-WiFi phones — their
    // stored address was public, never their 192.168.x LAN IP. NetworkManager
    // re-exposes the gossip peerDiscovered signal, and the GossipDiscovery fix
    // above now emits the datagram's LAN SOURCE IP for same-subnet JOINs — so
    // updateKnownPeer now lands a routable LAN address, and the next LAN-probe
    // tick will actually emit a direct candidate instead of skipping.
    connect(d->network.get(), &Network::NetworkManager::lanPeerAddressLearned,
            this, [this](const Core::UserId &id, const Core::PeerAddress &addr) {
                if (d->directDiscovery)
                    d->directDiscovery->updateKnownPeer(id, addr);
            });

    // DirectDiscovery findings feed into connectByDhtId as additional candidates.
    connect(d->directDiscovery.get(),
            &Network::DirectDiscovery::peerDiscovered,
            this, [this](const Core::UserId &peerId,
                         const Core::PeerAddress &addr) {
                // Only act on contacts we care about
                // (Check by seeing if they appear in our friends list)
                const auto friends = d->storage->loadFriends(
                    Core::UserId{ d->stableId });
                const bool isKnown = std::any_of(
                    friends.begin(), friends.end(),
                    [&](const Core::UserProfile &f) {
                        return f.id.value == peerId.value;
                    });
                if (!isKnown) return;
                if (d->network->hasEstablishedSession(peerId)) return;
                qCInfo(lcApp) << "Direct discovery:" << peerId.value
                              << "@" << addr.toString();
                // Inject into address queue directly — skip DHT round-trip
                d->pendingAddresses[peerId.value].prepend(addr);
                const QString pc = peerId.value;
                QTimer::singleShot(0, this, [this, pc]() {
                    tryNextPeerAddress(pc);
                });
            });

    // ── QNetworkInformation: instant reachability detection ─────────────────
    // Qt6's native reachability API fires immediately when WiFi/mobile data
    // is lost or regained — no polling lag. Used as primary mechanism.
    // The 3s poll timer below catches SSID changes (same connectivity type,
    // different network) which QNetworkInformation doesn't report.
    if (QNetworkInformation::loadBackendByFeatures(
            QNetworkInformation::Feature::Reachability)) {
        auto *netInfo = QNetworkInformation::instance();
        if (netInfo) {
            connect(netInfo, &QNetworkInformation::reachabilityChanged,
                    this, [this](QNetworkInformation::Reachability reach) {
                const bool nowOnline =
                    (reach == QNetworkInformation::Reachability::Online ||
                     reach == QNetworkInformation::Reachability::Local ||
                     reach == QNetworkInformation::Reachability::Site);
                if (nowOnline == d->isOnline) return; // no change
                d->isOnline = nowOnline;
                qCInfo(lcApp) << "Network reachability changed:"
                              << (nowOnline ? "ONLINE" : "OFFLINE");
                emit networkReachabilityChanged(nowOnline);
                // V32.21: feed flap event into AppContext so the FSM /
                // reconnect schedulers can back off while the network is
                // unstable. Every transition counts as a flap — even the
                // good one (offline→online), because the moments right
                // after coming back online are when DHCP / DNS / NAT
                // mappings are most fragile.
                if (d->appContext) d->appContext->noteReachabilityFlap();
                if (nowOnline) {
                    onNetworkCameOnline();
                } else {
                    onNetworkWentOffline();
                }
            });
            qCInfo(lcApp) << "QNetworkInformation backend loaded:"
                          << netInfo->backendName()
                          << "— instant reachability detection active";
        }
    } else {
        qCInfo(lcApp) << "QNetworkInformation unavailable — using poll timer only";
    }

    // ── Poll timer: catches SSID changes (same reachability, new network) ──
    // 10s poll: QNetworkInformation handles offline/online immediately;
    // this timer only catches IP changes after reachability is confirmed.
    // 10s detects a change within one gossip heartbeat period (15s).
    d->networkPollTimer.setInterval(Timings::kNetworkPollMs);
    // Battery: a network-change poll never needs ms precision; coarse-align it.
    d->networkPollTimer.setTimerType(Qt::VeryCoarseTimer);
    connect(&d->networkPollTimer, &QTimer::timeout, this, [this]() {
        // Find current primary IPv4 (first non-loopback, non-link-local)
        QString currentIp;
        for (const QNetworkInterface &ni : QNetworkInterface::allInterfaces()) {
            if (!ni.flags().testFlag(QNetworkInterface::IsUp)) continue;
            if (!ni.flags().testFlag(QNetworkInterface::IsRunning)) continue;
            if (ni.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
            for (const QNetworkAddressEntry &e : ni.addressEntries()) {
                if (e.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
                if (e.ip().isLoopback()) continue;
                if (e.ip().isLinkLocal()) continue;
                currentIp = e.ip().toString();
                break;
            }
            if (!currentIp.isEmpty()) break;
        }

        // Detect going offline (IP disappeared)
        if (currentIp.isEmpty()) {
            if (d->isOnline) {
                d->isOnline  = false;
                d->wasOffline = true;
                qCInfo(lcApp) << "Poll: no IP found — device appears offline";
                emit networkReachabilityChanged(false);
                onNetworkWentOffline();
            }
            return;
        }

        // Detect coming back online after offline period
        if (!d->isOnline) {
            d->isOnline  = true;
            d->wasOffline = false;
            qCInfo(lcApp) << "Poll: IP reappeared (" << currentIp << ") — device online";
            emit networkReachabilityChanged(true);
            // Force onNetworkCameOnline even if IP unchanged — handles
            // the case where QNetworkInformation didn't fire (backend missing)
            const QString oldIp = d->lastKnownPrimaryIp;
            d->lastKnownPrimaryIp = currentIp;
            emit localNetworkChanged(oldIp, currentIp);
            onNetworkCameOnline();
            return;
        }

        // No reachability change — check for SSID / IP change
        if (currentIp == d->lastKnownPrimaryIp) return;

        const QString oldIp = d->lastKnownPrimaryIp;
        qCInfo(lcApp) << "Network change detected: primary IP" << oldIp << "→" << currentIp;
        d->lastKnownPrimaryIp = currentIp;
        emit localNetworkChanged(oldIp, currentIp);
        // V32.21: feed SSID-change-equivalent event into AppContext.
        // We can't reliably distinguish "WiFi SSID changed" from "kernel
        // chose a different interface as primary" without OS-specific
        // helpers, so we treat any primary-IP change as SSID-change-like —
        // both have the same downstream consequence (DHCP / NAT mapping
        // settling, brief instability where retries are wasted).
        if (d->appContext) d->appContext->noteSsidChange();
        onNetworkIpChanged(currentIp);
    });
    d->networkPollTimer.start();

    // Start UDP data channel — parallel transport for CGNAT environments.
    // Binds on the same port as the punch socket (ShareAddress).
    // Becomes active after holePunchReady fires.
    d->udpChannel = std::make_unique<Network::UdpChannel>();
    if (d->router) d->router->setUdpChannel(d->udpChannel.get());
    // V32.28: hand AppContext to UdpChannel so its retransmit timer
    // can scale aggression with app state — skipping every other
    // tick when conservative+idle (halves cellular byte cost while
    // the user isn't actively waiting). CONNECT retransmits stay at
    // full cadence so relay fallback is reached in bounded time.
    d->udpChannel->setAppContext(d->appContext.get());
    // Wire UdpChannel I/O through the punch socket (NatTraversal owns it).
    // NatTraversal forwards SCUD frames via callback; UdpChannel sends back
    // through writeDatagram. This avoids SO_REUSEPORT delivery ambiguity.
    d->udpChannel->start(selfId,
        [this](const QByteArray &data, const QString &host, quint16 port) {
            if (d->nat) d->nat->writeDatagram(data, host, port);
        });
    // UdpCallback wired below after NatTraversal is created (line ~680)

    // Asynchronously discover our public (NAT-traversal) IP and re-announce.
    // This is essential for cross-network connections: the DHT must publish
    // the address peers on other networks can actually reach us at.
    // We query a lightweight HTTPS service; if it fails we keep the LAN IP.
    auto *nam = new QNetworkAccessManager(this);
    auto *reply = nam->get(QNetworkRequest(
        QUrl(QStringLiteral("https://api.ipify.org"))));
    const UserId capturedSelfId = selfId;
    connect(reply, &QNetworkReply::finished, this,
            [this, reply, capturedSelfId, nam]() {
        reply->deleteLater();
        nam->deleteLater();
        if (reply->error() != QNetworkReply::NoError) {
            qCWarning(lcApp) << "Public IP lookup failed:" << reply->errorString()
                             << "— DHT announce uses LAN IP only.";
            return;
        }
        const QString publicIp = QString::fromLatin1(
            reply->readAll().trimmed());
        // Basic validation: must look like an IPv4 address
        static const QRegularExpression kIpRx(
            QStringLiteral(R"(^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$)"));
        if (!kIpRx.match(publicIp).hasMatch()) {
            qCWarning(lcApp) << "Public IP response invalid:" << publicIp;
            return;
        }
        if (publicIp == d->selfAddr.host) return; // same as LAN (unlikely)
        qCInfo(lcApp) << "Public IP:" << publicIp
                      << "— re-announcing to DHT for cross-network reachability.";
        const Core::PeerAddress publicAddr{ publicIp, d->selfAddr.port };
        d->selfAddr = publicAddr;
        d->jami->announcePresence(capturedSelfId, publicAddr);
        d->storage->savePeerAddress(capturedSelfId, publicAddr);
        // Tell NetworkManager our public IP so hairpin addresses are skipped.
        d->network->setSelfPublicIp(publicIp);
    });

    // Purge stale snaps from a previous session.
    const int purged = d->storage->purgeExpiredSnaps();
    if (purged > 0)
        qCInfo(lcApp) << "Purged" << purged << "expired snap(s)";

    // Purge DHT cache entries older than 1 hour every hour.
    d->dhtCachePurgeTimer.setInterval(Timings::kDhtCachePurgeMs);
    connect(&d->dhtCachePurgeTimer, &QTimer::timeout, this, [this]() {
        d->storage->purgeDhtCache(60);
        qCDebug(lcApp) << "DHT cache purged (entries older than 1h).";
    });
    d->dhtCachePurgeTimer.start();

    // V32.34+: TTL sweep for peerPunchAddr. Walks the map every 5min
    // and drops entries older than kPunchAddrTtlMs (30 min). NAT
    // mappings typically expire in 30-300s anyway, so half-hour-old
    // punch addrs are functionally stale; keeping them just bloats
    // RAM in long-running sessions. Hard cap kMaxPunchAddrs (1024)
    // in setPeerPunchAddr is the backstop for pathological cases.
    d->punchAddrTtlTimer.setInterval(5 * 60 * 1000);
    connect(&d->punchAddrTtlTimer, &QTimer::timeout, this, [this]() {
        const int before = d->peerPunchAddrV2.size();
        d->sweepPunchAddrs();
        const int after = d->peerPunchAddrV2.size();
        if (after != before) {
            qCDebug(lcApp) << "peerPunchAddr TTL sweep:" << (before - after)
                           << "expired,"  << after << "remaining";
        }
    });
    d->punchAddrTtlTimer.start();

    // ── NAT Traversal ─────────────────────────────────────────────────────
    d->nat = std::make_unique<Network::NatTraversal>(this);
    d->nat->setSelfId(Core::UserId{ d->stableId }); // required for punch sc-ID payload
    // V32.21: hand the AppContext to NAT so the hole-punch loop can scale
    // burst aggression with current app state. Safe to call even if the
    // context is still in default-defaults — NAT just falls back to fixed
    // behaviour when context.prefersConservative() is false.
    d->nat->setAppContext(d->appContext.get());

    // Wire SCUD callback NOW on the actual NatTraversal instance.
    // Must be here — not before this line — or it sets the callback on a stale
    // (or null) pointer and every SCUD frame arrives with udpCb: NULL.
    d->nat->setUdpDatagramCallback(
        [this](const QByteArray &data, const QString &ip, quint16 port) {
            if (d->udpChannel) d->udpChannel->onDatagram(data, ip, port);
        });

    // When STUN gives us our reflexive address, re-announce it to the DHT.
    // This is the addr:port that other-network peers can actually reach us at.
    connect(d->nat.get(), &Network::NatTraversal::reflexiveAddressReady,
            this, [this](const Core::PeerAddress &reflexive) {
        if (d->stableId.isEmpty()) return;
        const Core::UserId stunRetrySelfId{ d->stableId };

        // STUN via TCP parallel path returns port=0 (can't determine UDP punch port).
        // Use the IP for DHT announcement but skip punch-port entries (port 0 is invalid).
        const bool tcpStunOnly = (reflexive.port == 0 && !reflexive.host.isEmpty());
        if (tcpStunOnly) {
            qCInfo(lcApp) << "TCP STUN (parallel) gave public IP only:"
                          << reflexive.host << "— announcing TCP addr, no punch port";
            const Core::UserId tcpSelfId{ d->stableId };
            const quint16 tcpStunWsPort = d->selfAddr.port > 0 ? d->selfAddr.port : 9876;
            const Core::PeerAddress tcpOnlyAddr{ reflexive.host, tcpStunWsPort };
            d->jami->announcePresence(tcpSelfId, tcpOnlyAddr);
            d->storage->savePeerAddress(tcpSelfId, tcpOnlyAddr);
            d->storage->cacheDhtResult(tcpSelfId, tcpOnlyAddr);
            d->network->setSelfPublicIp(reflexive.host);
            // Now that we have a public IP, retry any pending peer lookups
            // that previously failed due to missing STUN address.
            const QList<QString> pending = d->pendingDhtLookups.values();
            for (const QString &peerId : d->relookupCount.keys()) {
                if (!d->network->hasAnyLiveSession(Core::UserId{ peerId })) {
                    qCInfo(lcApp) << "STUN now available — retrying pending peer:" << peerId;
                    QTimer::singleShot(500, this, [this, peerId]() {
                        connectByDhtId(peerId);
                    });
                }
            }
            return;
        }

        // STUN failed (empty reflexive) — fall back to ipify IP we already have.
        // Still announce publicIP:9876 so peers can attempt TCP.
        if (reflexive.host.isEmpty()) {
            if (!d->selfAddr.host.isEmpty()) {
                qCInfo(lcApp) << "STUN failed — using ipify IP for DHT announce:"
                              << d->selfAddr.host;
                d->network->setSelfPublicIp(d->selfAddr.host);
                // Re-announce with TCP port so DHT stays fresh
                const Core::PeerAddress tcpOnly{ d->selfAddr.host, d->selfAddr.port > 0
                                                    ? d->selfAddr.port : quint16(9876) };
                d->jami->announcePresence(stunRetrySelfId, tcpOnly);
                d->storage->savePeerAddress(stunRetrySelfId, tcpOnly);
                d->storage->cacheDhtResult(stunRetrySelfId, tcpOnly);
            }
            return;
        }

        // STUN was done FROM the punch socket, so reflexive.port is the actual
        // UDP port the NAT assigned for our punch traffic.
        //
        // We announce TWO addresses to the DHT:
        //   1. publicIP:9876   — our TCP WebSocket listen port (for direct connects)
        //   2. publicIP:reflexive.port — our UDP punch port (for hole-punch rendezvous)
        //
        // For port-preserving NATs (most home routers), the punch port and TCP
        // port may differ but the NAT reuses the same external port for same
        // internal port. Announcing both gives peers the best chance.
        const quint16 tcpPort   = d->selfAddr.port > 0 ? d->selfAddr.port : 9876;
        const quint16 punchPort = reflexive.port;

        qCInfo(lcApp) << "STUN reflexive:" << reflexive.host
                      << "punch port:" << punchPort
                      << "tcp port:" << tcpPort;
        // Publish our STUN punch port to DHT so peers punch to the right addr.
        // Critical for CGNAT where punch port ≠ WS port.
        if (d->jami && punchPort != tcpPort)
            d->jami->setPunchPort(punchPort);

        // Always announce TCP endpoint (port 9876) — this is what peers
        // connect to. Peers who receive our punch port from DHT may try
        // TCP there but it only works on port-preserving NATs.
        const Core::PeerAddress tcpAddr{ reflexive.host, tcpPort };
        d->jami->announcePresence(stunRetrySelfId, tcpAddr);
        d->storage->savePeerAddress(stunRetrySelfId, tcpAddr);
        d->storage->cacheDhtResult(stunRetrySelfId, tcpAddr);
        // If STUN host ≠ ipify host (CGNAT uses different exit IPs per protocol),
        // announce the ipify IP for TCP AND the STUN host with punch port for UDP.
        // Peers TCP-connect to ipify addr, and hole-punch to STUN addr:punchPort.
        if (!d->selfAddr.host.isEmpty() && d->selfAddr.host != reflexive.host) {
            qCInfo(lcApp) << "STUN IP" << reflexive.host
                          << "differs from ipify IP" << d->selfAddr.host
                          << "— announcing both";
            // TCP fallback via ipify IP
            const Core::PeerAddress ipifyAddr{ d->selfAddr.host, tcpPort };
            d->jami->announcePresence(stunRetrySelfId, ipifyAddr);
            d->storage->cacheDhtResult(stunRetrySelfId, ipifyAddr);
        }
        // Always announce STUN reflexive addr with punch port separately.
        // This gives peers the correct {host, port} for UDP hole punch.
        // The punch port in the WS-addr payload tells peers what port to punch;
        // a SEPARATE entry with punchPort as the wsPort ensures the host is also right.
        if (punchPort != tcpPort) {
            // Announce stunHost:punchPort as a dedicated punch-only entry.
            // Peers see this as a high-port addr → no TCP attempt, direct punch.
            const Core::PeerAddress punchOnlyAddr{ reflexive.host, punchPort };
            d->jami->announcePresence(stunRetrySelfId, punchOnlyAddr);
            d->storage->cacheDhtResult(stunRetrySelfId, punchOnlyAddr);
            qCInfo(lcApp) << "Announced punch-only addr:" << punchOnlyAddr.toString();
        }

        // Do NOT announce the punch port to DHT.
        // Peers would try WebSocket (TCP) to it, which fails — the punch
        // port is UDP only. Punch port is used directly in hole-punch,
        // exchanged implicitly when the peer connects (not via DHT lookup).
        // punchPort is used by initiateHolePunch() which reads d->nat directly.
        (void)punchPort;

        // STUN succeeded — reset relookup counters for disconnected peers so
        // they get fresh retry attempts now that we have a public address.
        // This helps when STUN was slow (e.g. hotspot DNS delays) and all
        // attempts were burned before STUN finished.
        for (const QString &peerId : d->relookupCount.keys()) {
            if (!d->network->hasAnyLiveSession(Core::UserId{ peerId })) {
                d->relookupCount.remove(peerId);
                qCInfo(lcApp) << "STUN succeeded — reset relookup counter for" << peerId;
                QTimer::singleShot(200, this, [this, peerId]() {
                    if (!d->network->hasAnyLiveSession(Core::UserId{ peerId }))
                        connectByDhtId(peerId);
                });
            }
        }

        // selfAddr tracks our TCP endpoint for hairpin detection.
        // Use ipify/known-public IP if known, else STUN IP.
        // Don't use a private/LAN addr as tcpHost — it would mislead
        // hairpin detection after network-change resets selfAddr to LAN.
        // V32.32: replaced local 5-line lambda with the standard helper.
        // Same bit-exact semantics (RFC1918 only — no CGNAT, link-local,
        // or loopback). If we ever want the broader "not routable" check,
        // switch to AddressClass::isPrivate or a new isNonRoutable helper.
        const bool selfAddrIsPublic = !d->selfAddr.host.isEmpty()
                                   && !SnapClone::Network::AddressClass::isPrivate(d->selfAddr.host)
                                   && d->selfAddr.host != reflexive.host;
        const QString tcpHost = selfAddrIsPublic
            ? d->selfAddr.host   // ipify public IP (already set)
            : reflexive.host;    // STUN reflexive (fallback)
        d->selfAddr = { tcpHost, tcpPort };
        // Hairpin detection uses the ipify/STUN host
        d->network->setSelfPublicIp(tcpHost);
        // Also tell DHT to use STUN reflexive host for punch-only entries
        if (!reflexive.host.isEmpty())
            d->jami->setStunHost(reflexive.host);
    });

    // When hole punch completes, the NAT pinhole is open on both sides.
    // Now attempt TCP connects: try the punched port first (works for
    // port-preserving NATs), then port 9876 (our WebSocket listen port).
    connect(d->nat.get(), &Network::NatTraversal::holePunchReady,
            this, [this](const Core::UserId &peerId,
                         const Core::PeerAddress &punchedAddr) {
        qCInfo(lcApp) << "Hole punch ready for" << peerId.value
                      << "@" << punchedAddr.toString();
        emit peerConnectionStatus(peerId,
            QStringLiteral("punching"),
            tr("Hole punch open — waiting for UDP…"));
        // Extend the active WebSocket connect timer so TCP has time to
        // traverse the freshly opened NAT hole (prevents spurious timeouts
        // when punch and WS connect overlap near the timer deadline).
        d->network->notifyPunchReady(peerId);
        d->storage->savePeerAddress(peerId, punchedAddr);
        d->storage->cacheDhtResult(peerId, punchedAddr);

        // Punch succeeded — UDP path is open. Establish UDP channel.
        if (d->udpChannel && !d->udpChannel->isConnected(peerId))
            d->udpChannel->connectToPeer(peerId, punchedAddr);

        // Try TCP WebSocket on the WS port (9876) to reach the peer's server.
        if (!d->network->hasEstablishedSession(peerId)) {
            const Core::PeerAddress lastAddr = d->lastAttemptedAddr.value(peerId.value);
            const Core::PeerAddress wsAddr{ punchedAddr.host, 9876 };
            if (!lastAddr.host.isEmpty() && lastAddr.port <= 9999)
                d->network->connectToPeer(peerId, lastAddr);
            if (wsAddr.host != lastAddr.host)
                d->network->connectToPeer(peerId, wsAddr);
            if (punchedAddr.port != 9876) {
                const Core::PeerAddress punchWs{ punchedAddr.host, 9876 };
                d->network->connectToPeer(peerId, punchWs);
            }
        }
        d->lastAttemptedAddr[peerId.value] = punchedAddr;

        // Spray-punch fallback: if no SCUD CONNECT_ACK arrives within 20s,
        // the punchedAddr may be the peer's TCP NAT IP (ipify) rather than their
        // actual UDP CGNAT addr (which STUN would reveal). Load all historically
        // cached addresses and punch to any with a non-standard port — those are
        // old STUN-reflexive punch-only addrs that may be the correct CGNAT path.
        // This opens our stateful firewall entry for those sources so if the peer
        // sends UDP FROM one of them, our router lets it through.
        const QString sprayPeerId = peerId.value;
        const Core::PeerAddress sprayPrimary = punchedAddr;
        QTimer::singleShot(Timings::kSprayCooldownMs, this, [this, sprayPeerId, sprayPrimary]() {
            const Core::UserId pid{ sprayPeerId };
            if (d->udpChannel && d->udpChannel->isConnected(pid)) return; // already up
            if (!d->nat) return;

            qCInfo(lcApp) << "[SPRAY-PUNCH] No UDP CONNECT_ACK from" << sprayPeerId
                          << "after 20s — spraying all cached CGNAT addrs to open firewall";

            // Load all historically cached addresses for this peer.
            const auto cached = d->storage->loadDhtCache(pid, 120); // last 2h
            int sprayed = 0;
            for (const Core::PeerAddress &addr : cached) {
                // Skip the addr we already punched (primary) and standard WS addrs.
                // Target punch-only addrs (port != 9876) — these are STUN reflexive
                // addrs from previous sessions where the peer had a different CGNAT port.
                if (addr.host == sprayPrimary.host && addr.port == sprayPrimary.port) continue;
                if (addr.port == 9876) continue; // already tried via primary path
                qCInfo(lcApp) << "[SPRAY-PUNCH] Punching cached CGNAT addr" << addr.toString()
                              << "for" << sprayPeerId;
                d->nat->initiateHolePunch(pid, addr);
                // Also send SCUD CONNECTs to this addr — punch alone doesn't establish
                // the UDP channel. UdpChannel races the new addr with the current one.
                if (d->udpChannel) d->udpChannel->connectToPeer(pid, addr);
                ++sprayed;
                if (sprayed >= 3) break; // cap to avoid flooding
            }
            if (sprayed == 0)
                qCInfo(lcApp) << "[SPRAY-PUNCH] No alternate cached CGNAT addrs for" << sprayPeerId
                              << "— only ipify addr known. STUN needed on peer device.";
        });
    });

    // Peer-assisted STUN: when we receive a punch from a peer, report their
    // actual CGNAT UDP addr back to them via DHT mailbox.
    // This is a top-level connect (not nested in holePunchReady) so it is wired
    // exactly once regardless of how many times holePunchReady fires.
    connect(d->nat.get(), &Network::NatTraversal::punchSeen,
            this, [this](const Core::UserId &peerId,
                         const Core::PeerAddress &seenAddr) {
        const QString key = QStringLiteral("peerrefl:") + peerId.value;
        if (d->mailboxPublished.contains(key)) return;
        if (!d->crypto || !d->jami) return;
        d->mailboxPublished.insert(key);
        const qint64 ts = QDateTime::currentSecsSinceEpoch();
        const QString plain = QStringLiteral("PEERREFL\n%1\n%2:%3\n%4\n")
            .arg(d->stableId).arg(seenAddr.host).arg(seenAddr.port).arg(ts);
        const auto enc = d->crypto->encryptFor(peerId, plain.toUtf8());
        if (!enc) return;
        QByteArray blob;
        blob.append(d->stableId.toUtf8());
        blob.append('\n');
        blob.append(enc->nonce);
        blob.append(enc->ciphertext);
        qCInfo(lcApp) << "[PEERREFL] Reporting" << peerId.value
                      << "seen from" << seenAddr.toString();
#if defined(SNAPCLONE_OPENDHT_BACKEND)
        if (auto *dht = qobject_cast<Network::DhtDiscovery*>(d->jami.get()))
            dht->publishConnectionRequest(peerId, blob);
#endif // SNAPCLONE_OPENDHT_BACKEND
    });

    // UDP channel: inbound payload — route through SnapRouter identically
    // to WebSocket payloads. The router is transport-agnostic.
    connect(d->udpChannel.get(), &Network::UdpChannel::payloadReceived,
            this, [this](const Core::UserId &peerId, const QByteArray &data) {
                qCInfo(lcApp) << "UDP payload from" << peerId.value
                              << "(" << data.size() << "bytes)";
                // Route through the same dispatcher as TCP payloads.
                // SHAK and PTBL arriving via UDP get the same treatment as TCP.
                d->network->injectPayload(peerId, data);
            });

    // UDP channel ready — log and suppress redundant TCP retries.
    connect(d->udpChannel.get(), &Network::UdpChannel::channelReady,
            this, [this](const Core::UserId &peerId,
                         const Core::PeerAddress &addr) {
                qCInfo(lcApp) << "UDP channel active with" << peerId.value
                              << "@" << addr.toString()
                              << "— data can flow over UDP pinhole.";
                emit peerStatUpdate(peerId,
                    QStringLiteral("UDP  ") + addr.host
                    + QLatin1Char(':') + QString::number(addr.port));

                // Send SHAK and emit peerConnected via a deferred call so the
                // sendShak() method is fully available (initialize() may still be running).
                const Core::UserId shakTarget = peerId;
                QTimer::singleShot(0, this, [this, shakTarget]() {
                    sendShak(shakTarget);
                    if (!d->network->hasEstablishedSession(shakTarget)) {
                        emit peerConnected(shakTarget);
                        // V32.27: see AppController_connect.cpp counterpart
                        // — record successful session completion for the
                        // outbound path too. Storage-level no-op if the
                        // user row doesn't exist (e.g. transient peer that
                        // hasn't been saved as a contact yet).
                        if (d->storage) d->storage->touchLastConnected(shakTarget);
                        d->relookupCount.remove(shakTarget.value);
                        d->mailboxPublished.remove(shakTarget.value);
                        d->pendingAddresses.remove(shakTarget.value);
                        d->lastAttemptedAddr.remove(shakTarget.value);
                    }
                });

                // Exchange peer tables and trigger snap sync over UDP-only path.
                if (d->directDiscovery)
                    d->directDiscovery->onPeerConnected(peerId, d->network.get());
                if (!d->network->hasEstablishedSession(peerId) && d->router)
                    d->router->onPeerConnected(peerId);
                // Stop maintenance punches — CGNAT mapping no longer needed.
                if (d->nat) d->nat->stopHolePunch(peerId);
                // Suppress further TCP connection attempts — UDP path is live.
                d->network->suppressDiscovery(peerId, Timings::kDiscoverySuppressMs);
            });

    // UDP channel failed: CONNECT gave up after ~30s with no ACK.
    // If relay is configured, activate it as a fallback transport.
    // This is the last-resort path for symmetric NAT / fully-blocked UDP scenarios.
    connect(d->udpChannel.get(), &Network::UdpChannel::channelFailed,
            this, [this](const Core::UserId &peerId) {
                qCWarning(lcApp) << "UDP channel failed for" << peerId.value
                                 << "(30s CONNECT timeout) — trying relay fallback";
                if (d->network->hasEstablishedSession(peerId)) return; // TCP won
                if (d->relayUrl.isEmpty()) {
                    qCWarning(lcApp) << "No relay URL configured — relay disabled by user.";
                    qCWarning(lcApp) << "Set a relay in Settings to reach peers behind symmetric NAT.";
                    emit peerConnectionStatus(peerId,
                        QStringLiteral("failed"),
                        tr("UDP blocked — no relay configured"));
                    return;
                }
                const Core::UserId selfId2{ d->stableId };
                // Select relay transport based on URL scheme
                if (d->relayUrl.startsWith(QStringLiteral("tcp://"), Qt::CaseInsensitive)) {
                    auto *relay = new Network::MagicWormholeRelayTransport(this);
                    relay->connectToRelay(selfId2, peerId, d->relayUrl);
                    d->network->adoptRelayTransport(peerId, relay);
                } else {
                    auto *relay = new Network::RelayTransport(this);
                    relay->connectToRelay(selfId2, peerId, d->relayUrl);
                    d->network->adoptRelayTransport(peerId, relay);
                }
                emit peerStatUpdate(peerId, tr("Relay"));
            });

    // UDP send fallback: when TCP has no session, try UdpChannel.
    connect(d->network.get(), &Network::NetworkManager::udpSendNeeded,
            this, [this](const Core::UserId &peerId, const QByteArray &payload) {
                if (d->udpChannel && d->udpChannel->isConnected(peerId)) {
                    qCDebug(lcApp) << "UDP send fallback ->" << peerId.value
                                   << "(" << payload.size() << "bytes)";
                    d->udpChannel->sendPayload(peerId, payload);
                }
            });

    // V32.36+ (device logs): when NetworkManager queues a snap for an
    // unconnected peer, actively establish the connection instead of waiting
    // for the discovery layer. Debounce per peer (a burst of queued snaps must
    // trigger ONE connect attempt, not one per snap) — connectByDhtId itself is
    // idempotent ("lookup already in flight") but the debounce keeps the logs
    // and work clean. connectByDhtId consults the cached LAN address and drives
    // the LAN / DHT / punch paths.
    connect(d->network.get(), &Network::NetworkManager::connectionNeeded,
            this, [this](const Core::UserId &peerId) {
                const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
                const qint64 lastMs = d->lastConnectKickMs.value(peerId.value, 0);
                if (nowMs - lastMs < Timings::kConnectKickDebounceMs) {
                    return; // already kicked a connect for this peer recently
                }
                d->lastConnectKickMs[peerId.value] = nowMs;
                if (peerId.value.startsWith(QStringLiteral("sc-"))) {
                    qCInfo(lcApp) << "[CONNECT] Snap queued for" << peerId.value
                                  << "— actively establishing connection";
                    connectByDhtId(peerId.value);
                }
            });

    // STUN retry with exponential backoff — stops after kStunRetryMaxAttempts.
    // Each failure doubles the interval (30s → 60s → 120s → 5min cap).
    // Uses shared_ptr so both lambdas (timer + networkPoll reset) share the
    // counters safely — raw delete caused heap corruption when the timer
    // lambda deleted the ints but the networkPoll lambda still held pointers.
    auto *stunRetryTimer = new QTimer(this);
    stunRetryTimer->setSingleShot(true);
    auto stunRetryMs    = std::make_shared<int>(Timings::kStunRetryBaseMs);
    auto stunRetryCount = std::make_shared<int>(0);

    connect(stunRetryTimer, &QTimer::timeout, this,
            [this, stunRetryTimer, stunRetryMs, stunRetryCount]() {
        if (!d->nat) return;
        if (!d->nat->reflexiveAddress().host.isEmpty()) {
            qCInfo(lcApp) << "STUN already succeeded — stopping retry timer.";
            return; // shared_ptrs cleaned up when both lambdas are destroyed
        }
        if (*stunRetryCount >= Timings::kStunRetryMaxAttempts) {
            qCInfo(lcApp) << "STUN: max retries reached — stopping until network change.";
            return;
        }
        ++(*stunRetryCount);
        qCInfo(lcApp) << "STUN retry" << *stunRetryCount
                      << "— next in" << (*stunRetryMs / 1000) << "s";
        d->nat->restartStunOnly();
        *stunRetryMs = qMin(*stunRetryMs * 2, Timings::kStunRetryMaxMs);
        stunRetryTimer->start(*stunRetryMs);
    });

    // Reset STUN backoff on actual network/IP change — not on every poll tick.
    // Previously connected to networkPollTimer (fired every 3s), which defeated
    // the exponential backoff entirely. Now wired to localNetworkChanged instead.
    connect(this, &AppController::localNetworkChanged, this,
            [stunRetryCount, stunRetryMs](const QString &, const QString &) {
        *stunRetryCount = 0;
        *stunRetryMs    = Timings::kStunRetryBaseMs;
    });
    stunRetryTimer->start(Timings::kStunRetryBaseMs);

    // When STUN cross-server check confirms symmetric NAT, activate relay
    // for all contacts immediately rather than waiting 10 relookup cycles.
    connect(d->nat.get(), &Network::NatTraversal::symmetricNatDetected,
            this, [this]() {
        qCWarning(lcApp) << "Symmetric NAT confirmed — activating relay for all peers";
        if (d->relayUrl.isEmpty()) {
            qCWarning(lcApp) << "No relay URL configured — peers on AT&T/CGNAT unreachable.";
            return;
        }
        // Activate relay for every pending peer connection.
        const Core::UserId selfId2{ d->stableId };
        for (const QString &peerId : d->pendingAddresses.keys()) {
            const Core::UserId pid{ peerId };
            if (!d->network->hasAnyLiveSession(pid)) {
                // Select relay transport based on URL scheme
                if (d->relayUrl.startsWith(QStringLiteral("tcp://"), Qt::CaseInsensitive)) {
                    auto *relay = new Network::MagicWormholeRelayTransport(this);
                    relay->connectToRelay(selfId2, pid, d->relayUrl);
                    d->network->adoptRelayTransport(pid, relay);
                } else {
                    auto *relay = new Network::RelayTransport(this);
                    relay->connectToRelay(selfId2, pid, d->relayUrl);
                    d->network->adoptRelayTransport(pid, relay);
                }
                emit peerStatUpdate(pid, tr("Relay (active)"));
            }
        }
    });

    // When hole punch fails (symmetric NAT), fall back to TURN relay.
    connect(d->nat.get(), &Network::NatTraversal::holePunchFailed,
            this, [this](const Core::UserId &peerId) {
        qCWarning(lcApp) << "Hole punch failed for" << peerId.value
                         << "— symmetric NAT detected.";
        if (d->relayUrl.isEmpty()) {
            qCWarning(lcApp) << "No relay URL configured; peer unreachable."
                             << "Set Settings → Network → Relay URL to enable relay.";
            return;
        }
        if (d->network->hasEstablishedSession(peerId)) return;
        qCInfo(lcApp) << "Trying relay fallback for" << peerId.value;
        const Core::UserId relaySelfId{ d->stableId };
        // Select relay transport based on URL scheme
        if (d->relayUrl.startsWith(QStringLiteral("tcp://"), Qt::CaseInsensitive)) {
            auto *relay = new Network::MagicWormholeRelayTransport(this);
            relay->connectToRelay(relaySelfId, peerId, d->relayUrl);
            d->network->adoptRelayTransport(peerId, relay);
        } else {
            auto *relay = new Network::RelayTransport(this);
            relay->connectToRelay(relaySelfId, peerId, d->relayUrl);
            d->network->adoptRelayTransport(peerId, relay);
        }
    });

    d->nat->loadStunServersFromSettings(); // read enabled servers from settings
    d->nat->start(); // begins STUN discovery

    d->location = std::make_unique<LocationManager>(this);
    d->location->start();
    // When we get a fresh fix, write it to the location cache
    connect(d->location.get(), &LocationManager::locationUpdated,
            this, [this](double lat, double lon, double accM, const QString &ssid) {
        if (d->storage) {
            d->storage->upsertPeerLocation(
                d->stableId, lat, lon, accM, ssid,
                QDateTime::currentMSecsSinceEpoch(),
                QStringLiteral("gps"));
        }
    });

    // V32.28: purge ancient location entries on startup. Previously
    // this was 10 minutes which defeated the whole point of persisted
    // locations — opening the app on a new day wiped everything.
    // Wayfinder's UI design explicitly humanizes "2h ago" / "3d ago"
    // for offline contacts, so multi-day-old locations are
    // intentionally part of the display story. Bumping to 7 days
    // gives us 1 week of "where was this contact last week" without
    // unbounded growth. Per-contact UI policy decides whether to
    // dim or hide stale entries.
    if (d->storage) d->storage->purgeStaleLocations(7LL * 24 * 60 * 60 * 1000);

    qCInfo(lcApp) << "All subsystems ready.";
}


// ─── Network reachability handlers ───────────────────────────────────────────
//
// Called when QNetworkInformation or the poll timer detects a connectivity
// change. These three methods centralise all the state-reset and reconnect
// logic that was previously scattered across the poll timer lambda.

void AppController::onNetworkWentOffline()
{
    qCInfo(lcApp) << "[NETWORK] Device went offline — pausing DHT, clearing stale state";

    // Stop expensive background timers while offline — saves battery and
    // avoids filling logs with failed DNS/STUN/DHT attempts.
    d->networkPollTimer.stop();
    d->dhtCachePurgeTimer.stop();

    // Mark all currently-connected peers as offline in the DB so the UI
    // reflects reality immediately (don't wait for TCP timeout).
    const auto contacts = d->storage->loadContacts(Core::UserId{ d->stableId });
    for (const Core::Contact &c : contacts) {
        if (!c.online) continue;
        Core::Contact offline = c;
        offline.online   = false;
        offline.lastSeen = QDateTime::currentDateTimeUtc();
        d->storage->saveContact(offline);
        emit peerDisconnected(c.id);
    }

    // Emit a status update on every connected peer so their cards
    // show "Offline" rather than a stale "Connected" or "Internet" string.
    // (peerStatUpdate with empty string clears the stat line.)
    for (const Core::Contact &c : contacts) {
        emit peerConnectionStatus(c.id,
            QStringLiteral("offline"),
            tr("Device offline"));
    }

    // Offline: poll at 3s — reconnection is dominated by STUN/DHT
    // round-trips (4-60s), not the poll rate. 1s was wasting battery.
    d->networkPollTimer.setInterval(3000);
    d->networkPollTimer.start();
}

void AppController::onNetworkCameOnline()
{
    qCInfo(lcApp) << "[NETWORK] Device came online — reconnecting subsystems";

    // Restore normal poll interval.
    d->networkPollTimer.setInterval(Timings::kNetworkPollMs);
    d->dhtCachePurgeTimer.start();

    // Purge stale DHT cache — IPs from before the offline period are likely
    // wrong now (DHCP renewal, new SSID, CGNAT remapping).
    d->storage->purgeDhtCache(0);

    // Reset all connection attempt counters so every peer gets a fresh
    // kMaxRelookups budget on the new network.
    d->relookupCount.clear();
    d->pendingAddresses.clear();
    d->peerPunchAddr.clear();
    d->pendingDhtLookups.clear();

    // Find our new IP — may have changed while offline.
    QString newIp;
    for (const QNetworkInterface &ni : QNetworkInterface::allInterfaces()) {
        if (!ni.flags().testFlag(QNetworkInterface::IsUp)) continue;
        if (!ni.flags().testFlag(QNetworkInterface::IsRunning)) continue;
        if (ni.flags().testFlag(QNetworkInterface::IsLoopBack)) continue;
        for (const QNetworkAddressEntry &e : ni.addressEntries()) {
            if (e.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
            if (e.ip().isLoopback() || e.ip().isLinkLocal()) continue;
            newIp = e.ip().toString();
            break;
        }
        if (!newIp.isEmpty()) break;
    }
    if (!newIp.isEmpty()) {
        d->lastKnownPrimaryIp = newIp;
        onNetworkIpChanged(newIp);
    }

    // Check DHT mailbox immediately — peers may have published connection
    // requests while we were offline.
    QTimer::singleShot(500, this, [this]() {
        if (d->jami && !d->stableId.isEmpty()) {
#if defined(SNAPCLONE_OPENDHT_BACKEND)
            if (auto *dht = qobject_cast<Network::DhtDiscovery*>(d->jami.get())) {
                qCInfo(lcApp) << "[MAILBOX] Post-reconnect check for connection requests";
                dht->checkConnectionRequests(Core::UserId{ d->stableId });
            }
#endif
        }
    });

    // Reconnect all known contacts, staggered 300ms apart to avoid
    // hammering the DHT simultaneously.
    QTimer::singleShot(1500, this, [this]() {
        const Core::UserId self{ d->stableId };
        const auto contacts = d->storage->loadContacts(self);
        int scheduled = 0;
        for (const Core::Contact &c : contacts) {
            if (!c.id.value.startsWith(QStringLiteral("sc-"))) continue;
            if (d->network->hasAnyLiveSession(c.id)) continue;
            QTimer::singleShot(scheduled * 300, this, [this, id = c.id]() {
                if (!d->network->hasAnyLiveSession(id)) {
                    qCInfo(lcApp) << "[RECONNECT] Post-online reconnect to" << id.value;
                    emit peerConnectionStatus(id,
                        QStringLiteral("connecting"),
                        tr("Reconnecting…"));
                    connectByDhtId(id.value);
                }
            });
            ++scheduled;
        }
        if (scheduled > 0)
            qCInfo(lcApp) << "[RECONNECT] Scheduled" << scheduled
                          << "post-online reconnect(s)";
    });
}

void AppController::onNetworkIpChanged(const QString &newIp)
{
    qCInfo(lcApp) << "[NETWORK] IP changed to" << newIp << "— updating announcements";

    const quint16 wsPort = d->network->actualListenPort();
    const quint16 port   = (wsPort > 0) ? wsPort : 9876;
    const Core::PeerAddress newAddr{ newIp, port };
    d->selfAddr = newAddr;

    if (d->stableId.isEmpty()) return;
    const Core::UserId selfId{ d->stableId };

    // Update gossip so LAN peers discover our new IP within one heartbeat.
    d->network->updateSelfAddress(selfId, newAddr);

    // Re-announce to DHT so internet peers can find our new address.
    if (d->jami) d->jami->announcePresence(selfId, newAddr);

    // Restart STUN to learn our new public/punch address.
    // Only restart STUN — don't tear down the punch socket or active UDP channels.
    if (d->nat) d->nat->restartStunOnly();

    // Fetch new public IP asynchronously — SSID change may mean new CGNAT exit.
    // Reuse d->publicIpNam (persistent member) so rapid IP changes don't accumulate
    // QNetworkAccessManager instances that never complete.
    if (d->publicIpReply) {
        d->publicIpReply->abort();
        d->publicIpReply = nullptr;
    }
    if (!d->publicIpNam) d->publicIpNam = new QNetworkAccessManager(this);
    d->publicIpReply = d->publicIpNam->get(QNetworkRequest(QUrl(QStringLiteral("https://api.ipify.org"))));
    auto *rep = d->publicIpReply;
    connect(rep, &QNetworkReply::finished, this, [this, rep, selfId, port]() {
        if (rep != d->publicIpReply) { rep->deleteLater(); return; } // stale reply
        d->publicIpReply = nullptr;
        rep->deleteLater();
        if (rep->error() != QNetworkReply::NoError) return;
        const QString publicIp =
            QString::fromLatin1(rep->readAll().trimmed());
        static const QRegularExpression kIpRx(
            QStringLiteral(R"(^\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}$)"));
        if (!kIpRx.match(publicIp).hasMatch()) return;
        qCInfo(lcApp) << "[NETWORK] Public IP after SSID change:" << publicIp;
        const Core::PeerAddress pubAddr{ publicIp, port };
        d->selfAddr = pubAddr;
        if (d->jami) d->jami->announcePresence(selfId, pubAddr);
        d->storage->savePeerAddress(selfId, pubAddr);
        d->network->setSelfPublicIp(publicIp);
    });

    // Reset relookup state — on the new network peers may be reachable
    // without the backoff that accumulated on the previous one.
    d->relookupCount.clear();
    d->pendingAddresses.clear();
    d->peerPunchAddr.clear();
}


} // namespace SnapClone::Core
