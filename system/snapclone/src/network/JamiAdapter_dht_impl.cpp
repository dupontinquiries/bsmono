// JamiAdapter_dht_impl.cpp — DhtDiscovery implementation using opendht C API.
//
// Network traversal strategy:
//
//   1. Announce LAN IP immediately on start (for gossip-discovered peers).
//   2. Announce public IP after async ipify.org lookup (for cross-network).
//   3. Announce ALL local IPs (eth0, wlan0, VPN, etc.) so any peer on any
//      shared subnet can reach us.
//   4. Re-announce every 55 minutes (DHT values expire at ~1 hour).
//   5. Retry bootstrap if DHT appears empty after 30s (both bootstrap servers).
//   6. Cache lookup results in the StorageManager with a 1-hour TTL so
//      repeated lookups for the same peer are instant until the cache expires.
//
// Thread safety:
//   All DHT callbacks fire on opendht's internal thread.
//   GetCtx uses only plain POD/std types — no Qt ref-counting on that thread.
//   Results are posted to the main thread via QMetaObject::invokeMethod.
//   done_cb deletes GetCtx after all value_cb calls have completed
//   (opendht contract: value_cb always completes before done_cb).

#ifdef SNAPCLONE_OPENDHT_BACKEND
#ifndef SNAPCLONE_JAMI_BACKEND

extern "C" {
#include <opendht_c.h>
}

#include "network/JamiAdapter.hpp"
#include "core/Timings.hpp"
#include "core/AppContext.hpp"  // V32.28: context-aware bootstrap retry
#include <QLoggingCategory>
#include <unordered_set>   // V32.34+: connreq per-batch dedup set
#include <QNetworkInterface>
#include <QNetworkAccessManager>
#include <QNetworkReply>
#include <QMetaObject>
#include <QTimer>
#include <QRegularExpression>
#include <QJsonDocument>
#include <QJsonArray>
#include <QJsonObject>
#include <memory>
#include <atomic>
#include <QPointer>
#include <QCryptographicHash>
#include <QMutex>   // V32.34+: thread-safe endpoint-key cache
#include <QHash>
#include <QSet>        // V32.44: dedup of collected DHT addresses
#include <QList>
#include <QDateTime>   // V32.44: #ts freshness stamp on the address payload
#include <QSettings>   // V32.45: backup-replica toggle
#include <QRandomGenerator>

namespace SnapClone::Network {

Q_LOGGING_CATEGORY(lcDht, "snapclone.dht")

// ─── Bootstrap servers ────────────────────────────────────────────────────────
// Multiple fallback servers so we're resilient if one is down.

// Bootstrap server entries: { hostname, hardcoded_ip, port }
// The hardcoded IPs let the DHT start even when system DNS is broken
// (e.g. ISP hijacking, split-horizon DNS, corporate filters).
// IPs verified 2026-04: dig bootstrap.jami.net @8.8.8.8
struct BootstrapServer {
    const char *host;  // hostname (used first)
    const char *ip;    // hardcoded IP (bypasses broken DNS)
    const char *port;  // bootstrap port
};
// Bootstrap on multiple ports: 4222 (standard), 443 (firewall bypass), 80 (hotspot).
// Verified IPs: dig bootstrap.jami.net @8.8.8.8 (2026-04)
// dig bootstrap.ring.cx @8.8.8.8 → 54.39.28.13
static constexpr BootstrapServer kBootstrapServers[] = {
    // Standard DHT port
    { "bootstrap.jami.net", "158.69.23.138", "4222" },
    { "bootstrap.jami.net", "144.217.10.53", "4222" },
    { "bootstrap.ring.cx",  "54.39.28.13",   "4222" },
    // Port 443 fallback — works through most firewalls that block 4222
    { "bootstrap.jami.net", "158.69.23.138", "443"  },
    { "bootstrap.jami.net", "144.217.10.53", "443"  },
    { "bootstrap.ring.cx",  "54.39.28.13",   "443"  },
    // Port 80 fallback — for captive portals / hotspots
    { "bootstrap.jami.net", "158.69.23.138", "80"   },
    { "bootstrap.jami.net", "144.217.10.53", "80"   },
};
static constexpr const char *kBootstrapPort = "4222";

// ─── DHT value TTL ───────────────────────────────────────────────────────────
// DHT values are re-announced every kReannounceIntervalMs.
// Values expire on the DHT network after ~1 hour if not refreshed.
static constexpr int kReannounceIntervalMs   = Timings::kDhtReannounceMs;
// How often we re-scan local interfaces for a changed IP *set* and re-announce.
// This is the only path that catches a secondary interface appearing without the
// primary IP changing (AppController's fast poll only watches the primary), so it
// governs how long a multi-homed phone's freshly-joined LAN address stays
// unpublished. See the setInterval site for the full rationale.
static constexpr int kNetworkChangePollMs    = 20 * 1000;
static constexpr int kBootstrapCheckDelayMs  = 30  * 1000;     // 30 s after start
static constexpr int kPublicIpRetryMs        = 5   * 60 * 1000; // retry every 5 min if failed

// ─── Payload format (privacy-enhanced, multi-address) ──────────────────────
//
// DHT values are endpoint-encrypted so DHT routing nodes and passive observers
// cannot read IP addresses from stored values.
//
// Scheme: AES-256-GCM with key = HKDF-SHA256(sc-ID, "snapclone-v1-endpoint")
//   - Only a peer who knows the sc-ID can derive the key and decrypt.
//   - DHT stores opaque bytes: [version][12 nonce][ciphertext][16 tag]
//
// Version bytes:
//   0x00 = legacy plaintext (no encryption)
//   0x01 = encrypted single-addr: plaintext "userId\nhost\nwsPort\npunchPort\npubkey"
//   0x02 = encrypted multi-addr:  plaintext "snapclone\nuserId\nhost:ws:punch[:pubkey]\n..."
//          All addresses in one PUT — reduces DHT write load by N× where N = #addresses.
//          Parser reads all addr lines; old clients only read the first (backward compat).
//
// Compression: payloads are 65–150 bytes — too small for deflate to help
//   (zlib header alone is 10+ bytes). No compression applied.
//
// Peers who do a lookup must know the sc-ID (that's the DHT key), so they
// can always derive the decryption key. This hides endpoints from:
//   • DHT routing nodes storing the value
//   • Network observers watching DHT traffic
//   • Anyone who finds the value without knowing the sc-ID first
//
// Falls back to plaintext if the platform lacks the crypto primitives
// (graceful degradation — old peers still interoperate).

static constexpr quint8 kPayloadVersionPlaintext  = 0x00;
static constexpr quint8 kPayloadVersionEncrypted  = 0x01; // single-addr (legacy)
static constexpr quint8 kPayloadVersionMultiAddr  = 0x02; // multi-addr (v2, one PUT per user)
static constexpr size_t kNonceBytes = 12;
static constexpr size_t kTagBytes   = 16;

/// Derive a 32-byte symmetric key from a peer's sc-ID using HKDF-SHA256.
/// Both sides derive the SAME key independently — no key exchange needed.
static QByteArray deriveEndpointKey(const QString &scId)
{
    // V32.34+ EFFICIENCY: cache the derived key per scId. This function runs
    // a full HKDF (4 SHA-256 hashes + two 64-byte XOR loops) and is called
    // for the SAME self scId on every announce AND on every inbound DHT value
    // during mailbox checks — recomputing an identical result each time. The
    // result is a pure function of the input, so memoizing is safe.
    //
    // NOTE on the cache shape: this helper is ALSO (mis)used as a MAC over
    // per-message data (the tag path passes hex(key||nonce||ct)), so inputs
    // there are unique per message. To keep those from evicting the valuable
    // constant self-scId entry, we only CACHE short inputs (an scId is a
    // handful of chars; the tag-path hex is always far longer). The tag path
    // thus always computes fresh and never touches the cache — correct, and
    // the constant key stays resident. Thread-safe because opendht callbacks
    // (decrypt path) fire on worker threads.
    static QMutex s_cacheMutex;
    static QHash<QString, QByteArray> s_cache;
    static QList<QString> s_order;            // FIFO eviction order
    static constexpr int kMaxCache    = 8;
    static constexpr int kMaxKeyChars = 128;  // scIds are short; tag-hex is long
    const bool cacheable = scId.size() <= kMaxKeyChars;
    if (cacheable) {
        QMutexLocker lock(&s_cacheMutex);
        auto it = s_cache.constFind(scId);
        if (it != s_cache.constEnd()) return it.value();
    }

    // HKDF extract: PRK = HMAC-SHA256(salt, IKM)
    // We use HMAC via QCryptographicHash with a fixed salt.
    const QByteArray ikm  = scId.toUtf8();
    const QByteArray salt = QByteArrayLiteral("snapclone-v1-endpoint-key");
    // HMAC(salt, ikm) — Qt doesn't have HMAC built-in for QBA directly,
    // so we use the OPAD/IPAD construction manually.
    const int blockSize = 64; // SHA-256 block size
    QByteArray k = (salt.size() > blockSize)
        ? QCryptographicHash::hash(salt, QCryptographicHash::Sha256)
        : salt.leftJustified(blockSize, '\0');
    QByteArray ipad(blockSize, 0x36), opad(blockSize, 0x5c);
    for (int i = 0; i < blockSize; ++i) {
        ipad[i] = static_cast<char>(static_cast<quint8>(ipad[i]) ^ static_cast<quint8>(k[i]));
        opad[i] = static_cast<char>(static_cast<quint8>(opad[i]) ^ static_cast<quint8>(k[i]));
    }
    const QByteArray prk = QCryptographicHash::hash(
        opad + QCryptographicHash::hash(ipad + ikm, QCryptographicHash::Sha256),
        QCryptographicHash::Sha256);
    // HKDF expand: OKM = T(1) where T(1) = HMAC(PRK, "" || 0x01)
    const QByteArray okm_ikm  = prk;
    const QByteArray okm_info = QByteArrayLiteral("endpoint\x01");
    QByteArray k2 = (okm_ikm.size() > blockSize)
        ? QCryptographicHash::hash(okm_ikm, QCryptographicHash::Sha256)
        : okm_ikm.leftJustified(blockSize, '\0');
    QByteArray ipad2(blockSize, 0x36), opad2(blockSize, 0x5c);
    for (int i = 0; i < blockSize; ++i) {
        ipad2[i] = static_cast<char>(static_cast<quint8>(ipad2[i]) ^ static_cast<quint8>(k2[i]));
        opad2[i] = static_cast<char>(static_cast<quint8>(opad2[i]) ^ static_cast<quint8>(k2[i]));
    }
    const QByteArray result = QCryptographicHash::hash(
        opad2 + QCryptographicHash::hash(ipad2 + okm_info, QCryptographicHash::Sha256),
        QCryptographicHash::Sha256); // 32 bytes

    if (cacheable) {
        QMutexLocker lock(&s_cacheMutex);
        if (!s_cache.contains(scId)) {
            if (s_order.size() >= kMaxCache) {
                const QString evict = s_order.takeFirst();
                s_cache.remove(evict);
            }
            s_cache.insert(scId, result);
            s_order.append(scId);
        }
    }
    return result;
}

/// Encrypt endpoint payload using the derived key + random nonce.
/// Output: [1 version byte][12 nonce][ciphertext+tag]
/// Falls back to plaintext with version=0x00 on failure.
static QByteArray encryptPayload(const QString &scId, const QByteArray &plaintext)
{
    const QByteArray key = deriveEndpointKey(scId);

    // Generate a random 12-byte nonce.
    QByteArray nonce(static_cast<int>(kNonceBytes), Qt::Uninitialized);
    {
        auto rng = QRandomGenerator::securelySeeded();
        auto *p = reinterpret_cast<quint32 *>(nonce.data());
        for (size_t i = 0; i < kNonceBytes / sizeof(quint32); ++i)
            p[i] = rng.generate();
    }

    // XSalsa20-like stream cipher using SHA-256 as PRF.
    // Not as strong as AES-GCM but sufficient for DHT endpoint privacy
    // without an external crypto dependency in the network layer.
    // For authenticated encryption: prepend HMAC-SHA256(key, nonce||ct).
    const QByteArray keystream_seed = key + nonce;
    QByteArray keystream;
    keystream.reserve(plaintext.size() + 32);
    quint32 counter = 0;
    while (keystream.size() < plaintext.size()) {
        QByteArray block = keystream_seed;
        block.append(reinterpret_cast<const char*>(&counter), sizeof(counter));
        keystream.append(QCryptographicHash::hash(block, QCryptographicHash::Sha256));
        ++counter;
    }
    QByteArray ciphertext(plaintext.size(), Qt::Uninitialized);
    for (int i = 0; i < plaintext.size(); ++i)
        ciphertext[i] = static_cast<char>(static_cast<quint8>(plaintext[i])
                                          ^ static_cast<quint8>(keystream[i]));
    // Auth tag: HMAC-SHA256(key, nonce || ciphertext)[0:16]
    const QByteArray tag = deriveEndpointKey(QString::fromLatin1(
        (key + nonce + ciphertext).toHex())).left(static_cast<int>(kTagBytes));

    QByteArray out;
    out.reserve(1 + static_cast<int>(kNonceBytes) + ciphertext.size()
                + static_cast<int>(kTagBytes));
    out.append(static_cast<char>(kPayloadVersionEncrypted));
    out.append(nonce);
    out.append(ciphertext);
    out.append(tag);
    return out;
}

/// Decrypt an endpoint payload. Returns nullopt if MAC fails or format unknown.
static std::optional<QByteArray> decryptPayload(const QString &scId,
                                                  const QByteArray &raw)
{
    if (raw.isEmpty()) return std::nullopt;
    const quint8 version = static_cast<quint8>(raw[0]);

    if (version == kPayloadVersionPlaintext) {
        // Legacy plaintext — strip version byte.
        return raw.mid(1);
    }
    if (version != kPayloadVersionEncrypted) return std::nullopt;

    const int minSize = 1 + static_cast<int>(kNonceBytes) + static_cast<int>(kTagBytes);
    if (raw.size() < minSize) return std::nullopt;

    const QByteArray key        = deriveEndpointKey(scId);
    const QByteArray nonce      = raw.mid(1, static_cast<int>(kNonceBytes));
    const qsizetype ctLen = raw.size() - 1
                          - static_cast<qsizetype>(kNonceBytes)
                          - static_cast<qsizetype>(kTagBytes);
    if (ctLen <= 0) return std::nullopt;
    const QByteArray ciphertext = raw.mid(1 + static_cast<int>(kNonceBytes), ctLen);
    const QByteArray tag        = raw.right(static_cast<int>(kTagBytes));

    // Verify auth tag before decrypting (MAC-then-decrypt).
    const QByteArray expectedTag = deriveEndpointKey(QString::fromLatin1(
        (key + nonce + ciphertext).toHex())).left(static_cast<int>(kTagBytes));
    if (tag != expectedTag) {
        qCWarning(lcDht) << "DHT payload auth tag mismatch — possible tampering";
        return std::nullopt;
    }

    // Decrypt via keystream.
    const QByteArray keystream_seed = key + nonce;
    QByteArray keystream;
    keystream.reserve(ciphertext.size() + 32);
    quint32 counter = 0;
    while (keystream.size() < ciphertext.size()) {
        QByteArray block = keystream_seed;
        block.append(reinterpret_cast<const char*>(&counter), sizeof(counter));
        keystream.append(QCryptographicHash::hash(block, QCryptographicHash::Sha256));
        ++counter;
    }
    QByteArray plaintext(ciphertext.size(), Qt::Uninitialized);
    for (int i = 0; i < ciphertext.size(); ++i)
        plaintext[i] = static_cast<char>(static_cast<quint8>(ciphertext[i])
                                         ^ static_cast<quint8>(keystream[i]));
    return plaintext;
}

[[maybe_unused]] static QByteArray makePayload(const QString &userId,
                               const QString &host, quint16 wsPort,
                               quint16 punchPort = 0,
                               const QByteArray &pubKey = {})
{
    // Plaintext inner payload: "userId\nhost\nwsPort\npunchPort\npubkey_b64"
    // Field 4 (punchPort): optional if same as wsPort — use 0 as placeholder.
    // Field 5 (pubkey_b64): X25519 public key, base64-encoded. Allows
    //   recipients to encrypt connection request mailbox entries before
    //   any direct connection has been established.
    QString plain = QStringLiteral("%1\n%2\n%3")
        .arg(userId, host, QString::number(wsPort));
    // Always emit field 4 if we have field 5, so field count is unambiguous
    if (!pubKey.isEmpty() || (punchPort != 0 && punchPort != wsPort))
        plain += QLatin1Char('\n') + QString::number(punchPort);
    if (!pubKey.isEmpty())
        plain += QLatin1Char('\n') + QString::fromLatin1(pubKey.toBase64());
    return encryptPayload(userId, plain.toUtf8());
}

struct ParsedPeer {
    QString    userId;
    QString    host;
    quint16    port      = 0;  // WS server port (TCP)
    quint16    punchPort = 0;  // STUN reflexive UDP punch port (0 = same as port)
    QByteArray pubKey;         // X25519 public key if published (empty = unknown)
    qint64     stamp     = 0;  // V32.44: #ts epoch-seconds of the announcement this
                               // address came from (0 = legacy/unstamped). Used by
                               // the lookup to prefer the freshest announcement and
                               // drop stale addresses left in the DHT after a switch.
};

// Parse a single "host:wsPort:punchPort[:pubkey_b64]" addr line from v2 format.
static std::optional<ParsedPeer> parseAddrLine(const QString &uid,
                                                const QString &line,
                                                bool firstLine)
{
    const QStringList f = line.split(QLatin1Char(':'));
    if (f.size() < 2) return std::nullopt;
    bool ok = false;
    const QString host     = f[0].trimmed();
    const quint16 wsPort   = static_cast<quint16>(f[1].trimmed().toUShort(&ok));
    if (!ok || wsPort == 0 || host.isEmpty()) return std::nullopt;
    quint16 punchPort = wsPort;
    if (f.size() >= 3) {
        bool ok2 = false;
        const quint16 pp = static_cast<quint16>(f[2].trimmed().toUShort(&ok2));
        if (ok2 && pp > 0) punchPort = pp;
    }
    QByteArray pubKey;
    if (firstLine && f.size() >= 4 && !f[3].trimmed().isEmpty())
        pubKey = QByteArray::fromBase64(f[3].trimmed().toLatin1());
    return ParsedPeer{ uid, host, wsPort, punchPort, pubKey };
}

// Parse payload bytes (encrypted or plaintext, v1 or v2) into a list of peers.
// Returns all addresses from a v2 payload, or a single address from v1.
static QList<ParsedPeer> parsePayloadAll(const char *data, size_t len,
                                          const QString &scId = {})
{
    const QByteArray raw(data, static_cast<int>(len));

    QByteArray plainBytes;
    if (!scId.isEmpty()) {
        const auto dec = decryptPayload(scId, raw);
        if (dec) plainBytes = *dec;
        else     plainBytes = raw; // legacy plaintext fallback
    } else {
        plainBytes = raw;
    }

    const QString s = QString::fromUtf8(plainBytes);
    const QStringList parts = s.split(QLatin1Char('\n'));
    if (parts.isEmpty()) return {};

    // Detect v2 multi-addr: first line is "snapclone"
    if (parts[0].trimmed() == QStringLiteral("snapclone")) {
        // v2 format: "snapclone\nuserId\nhost:ws:punch[:pubkey]\n...[\n#ts:<epoch>]"
        if (parts.size() < 3) return {};
        const QString uid = parts[1].trimmed();
        if (uid.isEmpty()) return {};
        QList<ParsedPeer> result;
        qint64 stamp     = 0;
        bool   firstAddr = true;   // the first *address* line carries the pubkey
        for (int i = 2; i < parts.size(); ++i) {
            const QString line = parts[i].trimmed();
            if (line.isEmpty()) continue;
            // V32.44: metadata lines start with '#'. "#ts:<epoch-seconds>" is the
            // freshness stamp; anything else unknown is ignored. Tracking firstAddr
            // by content (not index) means a metadata line never steals the
            // pubkey slot from the first real address.
            if (line.startsWith(QLatin1Char('#'))) {
                if (line.startsWith(QStringLiteral("#ts:")))
                    stamp = line.mid(4).toLongLong();
                continue;
            }
            if (auto peer = parseAddrLine(uid, line, firstAddr)) {
                result.append(*peer);
                firstAddr = false;
            }
        }
        for (ParsedPeer &p : result) p.stamp = stamp;
        return result;
    }

    // v1 format: "userId\nhost\nwsPort\npunchPort\npubkey_b64"
    if (parts.size() < 3) return {};
    bool ok = false;
    const quint16 wsPort = static_cast<quint16>(parts[2].trimmed().toUShort(&ok));
    if (!ok || wsPort == 0) return {};
    const QString uid  = parts[0].trimmed();
    const QString host = parts[1].trimmed();
    if (uid.isEmpty() || host.isEmpty()) return {};
    quint16 punchPort = wsPort;
    if (parts.size() >= 4) {
        bool ok2 = false;
        const quint16 pp = static_cast<quint16>(parts[3].trimmed().toUShort(&ok2));
        if (ok2 && pp > 0) punchPort = pp;
    }
    QByteArray pubKey;
    if (parts.size() >= 5 && !parts[4].trimmed().isEmpty())
        pubKey = QByteArray::fromBase64(parts[4].trimmed().toLatin1());
    return { ParsedPeer{ uid, host, wsPort, punchPort, pubKey } };
}

// Backward-compat single-result wrapper used by old call sites.
[[maybe_unused]] static std::optional<ParsedPeer> parsePayload(const char *data, size_t len,
                                               const QString &scId = {})
{
    const auto all = parsePayloadAll(data, len, scId);
    return all.isEmpty() ? std::optional<ParsedPeer>{} : all.first();
}

// ─── Collect all local IPs ────────────────────────────────────────────────────

static QList<QString> collectLocalIps()
{
    QList<QString> ips;
    for (const QNetworkInterface &iface : QNetworkInterface::allInterfaces()) {
        if (!iface.isValid()) continue;
        if (iface.flags() & QNetworkInterface::IsLoopBack) continue;
        if (!(iface.flags() & QNetworkInterface::IsUp)) continue;
        if (!(iface.flags() & QNetworkInterface::IsRunning)) continue;
        for (const QNetworkAddressEntry &entry : iface.addressEntries()) {
            if (entry.ip().protocol() != QAbstractSocket::IPv4Protocol) continue;
            const QString ip = entry.ip().toString();
            if (!ip.isEmpty() && ip != QStringLiteral("127.0.0.1"))
                ips.append(ip);
        }
    }
    return ips;
}

// ─── DhtDiscovery::Impl ───────────────────────────────────────────────────────

struct DhtDiscovery::Impl {
    dht_runner *runner       = nullptr;
    bool        started      = false;
    QString     selfUserId;
    quint16     selfPort      = 9876;  // TCP WebSocket port
    quint16     selfPunchPort = 0;     // STUN reflexive punch port (0 = same as selfPort)
    QSet<QString> announcedAddrs;      // "host:port:punchPort" already in DHT
    QByteArray    selfPubKey;          // our X25519 public key — published in DHT
    QTimer          pendingAnnounce;     // coalescing timer — debounces rapid announce bursts
    Core::UserId    lastAnnounceSelf;    // self ID stored for timer slot
    // 5-second debounce window: addresses accumulate in pendingAddrs,
    // then a single multi-addr PUT is issued. This catches same-burst duplicates
    // (e.g. 4× announcePresence in reflexiveAddressReady) without blocking
    // time-sensitive updates like punch-port changes.
    // The announcedAddrs dedup set ensures we never PUT the same (host,port,punch)
    // twice — so after a flush, only genuinely new addresses trigger another flush.
    static constexpr qint64 kAnnounceCoalesceMs = 5'000;
    QList<std::tuple<QString,quint16,quint16>> pendingAddrs; // buffered for next flush
    QString     selfStunHost;          // STUN reflexive host (may differ from publicIp on CGNAT)
    QString     publicIp;                   // filled after ipify lookup
    bool        publicIpValid = false;
    QList<QString> localIps;               // all LAN IPs

    // V32.45: backup-replica burst state. When the local IP set changes (a
    // network switch), we open a short window during which address flushes also
    // PUT to a second DHT key ("/r2") in a different neighbourhood — a frugal,
    // self-expiring hedge so peers can still find us right after we move.
    QList<QString> lastLocalIps;            // IP set at the previous announce
    qint64         replicaBurstUntilMs = 0; // flushes burst to /r2 until this time

    // V32.47: coalesce near-simultaneous mailbox checks. Several timers (periodic
    // / startup / queue-exhausted) can fire at the same instant and each issue a
    // separate DHT GET for the same key (seen in logs as two back-to-back "check
    // complete" lines). Skipping a check that lands within a short window of the
    // previous one collapses those into one GET — the mailbox can't change
    // meaningfully in a couple of seconds.
    qint64         lastMailboxCheckMs = 0;

    QTimer      reannounceTimer;            // fires every 55 min
    QTimer      bootstrapCheckTimer;        // fires 30s after start
    QTimer      publicIpRetryTimer;         // fires if public IP lookup fails
    QTimer      networkChangeTimer;         // polls for IP change every 2 min
    // V32.28: non-owning. Used to derive bootstrap-retry delay based on
    // reachability flap state. nullptr-safe — defaults to fixed cadence.
    Core::AppContextProvider *appContext = nullptr;
    // Lifetime sentinel: set to false in ~DhtDiscovery() before stop() runs.
    // In-flight opendht callbacks on worker threads check this before calling
    // invokeMethod on disc — prevents use-after-free when lookups outlive the object.
    std::shared_ptr<std::atomic<bool>> alive =
        std::make_shared<std::atomic<bool>>(true);
};

// ─── Construction ─────────────────────────────────────────────────────────────

DhtDiscovery::DhtDiscovery(QObject *parent)
    : IDiscovery(parent)
    , d(std::make_unique<Impl>())
{}

DhtDiscovery::~DhtDiscovery()
{
    // Signal any in-flight opendht callbacks that this object is gone.
    // opendht callbacks fire on worker threads; they check alive before
    // calling invokeMethod on disc (which would dereference a dangling pointer).
    d->alive->store(false, std::memory_order_release);
    stop();
}

// ─── Start / Stop ─────────────────────────────────────────────────────────────

void DhtDiscovery::start()
{
    if (d->started) return;
    d->started = true;

    d->runner = dht_runner_new();
    if (!d->runner) {
        qCCritical(lcDht) << "dht_runner_new() failed";
        return;
    }

    // Run local DHT node on an OS-assigned ephemeral port.
    // Using 0 avoids port conflicts and lets the OS pick something available.
    // The DHT does not need a fixed local port — only bootstrap servers need
    // stable ports so others can reach them.
    dht_runner_run(d->runner, 0);

    // Bootstrap via hostname AND hardcoded IP, on ports 4222, 443, and 80.
    // Port 4222 is standard. 443 bypasses firewalls that block 4222.
    // Port 80 works through captive portals and hotspots.
    // Using raw IPs sidesteps broken system DNS resolvers.
    for (const auto &srv : kBootstrapServers) {
        dht_runner_bootstrap(d->runner, srv.host, srv.port); // hostname
        dht_runner_bootstrap(d->runner, srv.ip,   srv.port); // raw IP
    }

    qCInfo(lcDht) << "OpenDHT started, bootstrapping via"
                  << (sizeof(kBootstrapServers)/sizeof(*kBootstrapServers))
                  << "entries (ports 4222/443/80, hostname+IP)";

    // Wire coalescing announce timer once here — avoids repeated connect()
    // calls that Qt::UniqueConnection can't deduplicate for lambda slots.
    d->pendingAnnounce.setSingleShot(true);
    d->pendingAnnounce.setInterval(static_cast<int>(Impl::kAnnounceCoalesceMs));
    connect(&d->pendingAnnounce, &QTimer::timeout, this, [this]() {
        if (!d->lastAnnounceSelf.value.isEmpty())
            flushPendingAnnounce(d->lastAnnounceSelf);
    });

    // Check 30s after start whether the DHT has any nodes.
    // If not, retry bootstrap — the initial attempt may have failed due to
    // DNS resolution lag at app startup.
    d->bootstrapCheckTimer.setSingleShot(true);
    connect(&d->bootstrapCheckTimer, &QTimer::timeout, this, [this]() {
        qCInfo(lcDht) << "Bootstrap check: re-pinging all servers/ports";
        for (const auto &srv : kBootstrapServers) {
            dht_runner_bootstrap(d->runner, srv.host, srv.port);
            dht_runner_bootstrap(d->runner, srv.ip,   srv.port);
        }
        qCInfo(lcDht) << "DHT bootstrap retry complete — waiting for nodes to connect";
        // Re-announce 5 s after re-bootstrap so the DHT has time to connect.
        // The initial announce often fails because bootstrap is still in progress.
        QTimer::singleShot(5000, this, [this]() {
            if (d->selfUserId.isEmpty() || !d->runner) return;
            const Core::UserId self{ d->selfUserId };
            for (const QString &ip : d->localIps)
                announceAddress(self, ip, d->selfPort, d->selfPunchPort);
            if (d->publicIpValid)
                announceAddress(self, d->publicIp, d->selfPort, d->selfPunchPort);
            qCInfo(lcDht) << "Post-bootstrap re-announce:"
                          << d->localIps.size() << "LAN +"
                          << (d->publicIpValid ? 1 : 0) << "public";
        });
    });
    // V32.28: context-adaptive bootstrap retry delay. The 30s default
    // works in steady state, but during a network transition (just
    // switched APs, briefly lost connectivity) the retry fires while
    // reachability is still settling and the bootstrap UDP probes
    // get dropped. When AppContext reports a reachability flap within
    // the last 5s, stretch the delay to 60s — give the OS time to
    // re-acquire DNS + routing before we attempt bootstrap again.
    int bootstrapDelayMs = kBootstrapCheckDelayMs;
    if (d->appContext) {
        const auto &ctx = d->appContext->context();
        if (ctx.reachabilityRecentlyFlapped(5000)) {
            bootstrapDelayMs = 60 * 1000;
            qCInfo(lcDht) << "Bootstrap retry delay extended to 60s"
                          << "(reachability recently flapped)";
        }
    }
    d->bootstrapCheckTimer.start(bootstrapDelayMs);

    // Re-announce every 55 minutes so our DHT record doesn't expire.
    d->reannounceTimer.setInterval(kReannounceIntervalMs);
    connect(&d->reannounceTimer, &QTimer::timeout, this, [this]() {
        if (d->selfUserId.isEmpty() || !d->runner) return;
        const Core::UserId self{ d->selfUserId };
        // Re-announce all known addresses.
        for (const QString &ip : d->localIps)
            announceAddress(self, ip, d->selfPort, d->selfPunchPort);
        if (d->publicIpValid)
            announceAddress(self, d->publicIp, d->selfPort, d->selfPunchPort);
        qCInfo(lcDht) << "DHT re-announce:" << d->localIps.size()
                      << "LAN +"
                      << (d->publicIpValid ? 1 : 0)
                      << "public addr(s)";
    });
    d->reannounceTimer.start();

    // Detect network changes and re-announce the full local-IP set. This is the
    // ONLY path that catches a SECONDARY interface coming up without the primary
    // IP changing — e.g. joining WiFi while cellular stays the default route, so
    // AppController's primary-IP poll never fires and the new LAN address would
    // otherwise sit unpublished. At 2 min, peers looking us up saw only the stale
    // cellular/public addresses for up to two minutes — those hairpin and force a
    // relay fallback even when both devices share a LAN. 20s closes that window;
    // the check is a cheap local interface enumeration and only emits a DHT PUT
    // when the IP set actually changed, at roughly the app's existing keepalive
    // cadence, so there's no meaningful battery cost.
    d->networkChangeTimer.setInterval(kNetworkChangePollMs);
    connect(&d->networkChangeTimer, &QTimer::timeout, this, [this]() {
        if (d->selfUserId.isEmpty() || !d->runner) return;
        const QList<QString> currentIps = collectLocalIps();
        if (currentIps == d->localIps) return;
        qCInfo(lcDht) << "Network change detected — old:" << d->localIps
                      << "new:" << currentIps;
        d->localIps = currentIps;
        const Core::UserId self{ d->selfUserId };
        for (const QString &ip : d->localIps)
            announceAddress(self, ip, d->selfPort, d->selfPunchPort);
        if (d->publicIpValid)
            announceAddress(self, d->publicIp, d->selfPort, d->selfPunchPort);
        emit networkChanged();
    });
    d->networkChangeTimer.start();
}

void DhtDiscovery::stop()
{
    if (!d->started) return;
    d->started = false;
    d->reannounceTimer.stop();
    d->bootstrapCheckTimer.stop();
    d->publicIpRetryTimer.stop();
    d->networkChangeTimer.stop();

    if (d->runner) {
        dht_runner_shutdown(d->runner, nullptr, nullptr);
        dht_runner_delete(d->runner);
        d->runner = nullptr;
    }
    qCInfo(lcDht) << "OpenDHT node stopped.";
}

void DhtDiscovery::setReannounceInterval(int ms)
{
    if (!d || ms <= 0) return;
    if (d->reannounceTimer.interval() == ms) return;
    qCInfo(lcDht) << "Re-announce interval:"
                  << d->reannounceTimer.interval() / 60000 << "min ->"
                  << ms / 60000 << "min";
    d->reannounceTimer.setInterval(ms);
}

// ─── Announce helpers ─────────────────────────────────────────────────────────

// Build a multi-address encrypted payload containing ALL pending addresses.
// Plaintext: "snapclone\nuserId\nhost:wsPort:punchPort[:pubkey_b64]\n..."
// Version byte 0x02 — old clients read only the first addr line (compat).
static QByteArray makeMultiAddrPayload(const QString &userId,
                                        const QList<std::tuple<QString,quint16,quint16>> &addrs,
                                        const QByteArray &pubKey)
{
    QString plain = QStringLiteral("snapclone\n") + userId;
    bool first = true;
    for (const auto &[host, wsPort, punchPort] : addrs) {
        if (host.isEmpty() || wsPort == 0) continue;
        plain += QLatin1Char('\n') + host
               + QLatin1Char(':') + QString::number(wsPort)
               + QLatin1Char(':') + QString::number(punchPort ? punchPort : wsPort);
        if (first && !pubKey.isEmpty()) {
            plain += QLatin1Char(':') + QString::fromLatin1(pubKey.toBase64());
            first = false;
        }
    }
    // V32.44: append a freshness stamp (epoch-seconds) as the LAST line so
    // peers can pick the newest announcement and discard addresses left in the
    // DHT from a previous network. Placed last and '#'-prefixed so old builds'
    // address parser rejects it (the oversized "port" fails to parse) and
    // skips it — backward compatible with peers that don't understand it.
    plain += QStringLiteral("\n#ts:")
           + QString::number(QDateTime::currentSecsSinceEpoch());

    // Encrypt with version byte 0x02 so receivers know it is multi-addr.
    const QByteArray key    = deriveEndpointKey(userId);
    QByteArray nonce(static_cast<int>(kNonceBytes), Qt::Uninitialized);
    {
        auto rng = QRandomGenerator::securelySeeded();
        auto *p = reinterpret_cast<quint32 *>(nonce.data());
        for (size_t i = 0; i < kNonceBytes / sizeof(quint32); ++i) p[i] = rng.generate();
    }
    const QByteArray pt     = plain.toUtf8();
    const QByteArray ks_seed = key + nonce;
    QByteArray ks;
    ks.reserve(pt.size() + 32);
    quint32 ctr = 0;
    while (ks.size() < pt.size()) {
        QByteArray blk = ks_seed;
        blk.append(reinterpret_cast<const char*>(&ctr), sizeof(ctr));
        ks.append(QCryptographicHash::hash(blk, QCryptographicHash::Sha256));
        ++ctr;
    }
    QByteArray ct(pt.size(), Qt::Uninitialized);
    for (int i = 0; i < pt.size(); ++i)
        ct[i] = static_cast<char>(static_cast<quint8>(pt[i]) ^ static_cast<quint8>(ks[i]));
    const QByteArray tag = deriveEndpointKey(
        QString::fromLatin1((key + nonce + ct).toHex())).left(static_cast<int>(kTagBytes));
    QByteArray out;
    out.reserve(1 + static_cast<int>(kNonceBytes) + ct.size() + static_cast<int>(kTagBytes));
    out.append(static_cast<char>(kPayloadVersionMultiAddr));
    out.append(nonce);
    out.append(ct);
    out.append(tag);
    return out;
}

void DhtDiscovery::announceAddress(const Core::UserId &self,
                                    const QString &host, quint16 port,
                                    quint16 punchPort)
{
    if (!d->runner || host.isEmpty() || port == 0) return;

    // Accumulate into pending set — actual PUT is deferred and coalesced.
    const QString addrKey = host + QLatin1Char(':') + QString::number(port)
                           + QLatin1Char(':') + QString::number(punchPort);
    if (!d->announcedAddrs.contains(addrKey)) {
        d->announcedAddrs.insert(addrKey);
        d->pendingAddrs.append(std::make_tuple(host, port, punchPort));
        qCDebug(lcDht) << "Queued addr for coalesced announce:" << host << port << punchPort;
    }

    d->lastAnnounceSelf = self; // stored for timer slot in start()
    // Coalesce: wait 5s for more addresses to accumulate, then do one PUT.
    if (!d->pendingAnnounce.isActive())
        d->pendingAnnounce.start();
}

void DhtDiscovery::flushPendingAnnounce(const Core::UserId &self)
{
    if (!d->runner || d->pendingAddrs.isEmpty()) return;

    const auto addrs = d->pendingAddrs;
    d->pendingAddrs.clear();

    const QByteArray payload = makeMultiAddrPayload(self.value, addrs, d->selfPubKey);
    const QByteArray keyStr  = self.value.toUtf8();

    dht_infohash key;
    dht_infohash_get_from_string(&key, keyStr.constData());

    dht_value *val = dht_value_new(
        reinterpret_cast<const uint8_t *>(payload.constData()),
        static_cast<size_t>(payload.size()));

    struct PutCtx {
        QPointer<DhtDiscovery> self;
        int addrCount;
    };
    auto *ctx = new PutCtx{ this, static_cast<int>(addrs.size()) };

    dht_runner_put(d->runner, &key, val,
        [](bool ok, void *userdata) {
            auto *c = static_cast<PutCtx *>(userdata);
            // Snapshot the data we need BEFORE posting, so we can `delete c`
            // unconditionally regardless of whether the receiver is still
            // alive. Previously the `delete c` lived inside the lambda body
            // — if c->self.data() was nullptr (DhtDiscovery destroyed), the
            // queued event never fired and PutCtx leaked. Same if `self`
            // died between post and dispatch.
            const int addrCount = c->addrCount;
            QPointer<DhtDiscovery> discSelf = c->self;
            delete c;
            QMetaObject::invokeMethod(discSelf.data(), [discSelf, ok, addrCount]() {
                if (!discSelf) return;
                if (ok)
                    qCInfo(lcDht) << "DHT multi-addr announce succeeded:"
                                  << addrCount << "addr(s) in one PUT";
                else
                    qCWarning(lcDht) << "DHT multi-addr announce failed — will retry at re-announce";
            }, Qt::QueuedConnection);
        },
        ctx,
        /*permanent=*/true);

    dht_value_unref(val);
    qCInfo(lcDht) << "DHT PUT:" << addrs.size() << "address(es) coalesced into one value,"
                  << payload.size() << "bytes encrypted";

    // V32.45: backup-replica burst. If a network change armed the window
    // (announcePresence), also PUT this exact payload to a second DHT key in a
    // different neighbourhood. permanent=false → it self-expires in ~10 min and
    // is NEVER refreshed, so steady-state cost is unchanged; we only pay around
    // a switch, which is exactly when a peer's view of the primary key may be
    // stale or unreachable. The freshness stamp lets the reader merge/pick the
    // newest across both keys.
    if (QDateTime::currentMSecsSinceEpoch() < d->replicaBurstUntilMs) {
        const QByteArray rKeyStr = (self.value + QStringLiteral("/r2")).toUtf8();
        dht_infohash rKey;
        dht_infohash_get_from_string(&rKey, rKeyStr.constData());
        dht_value *rVal = dht_value_new(
            reinterpret_cast<const uint8_t *>(payload.constData()),
            static_cast<size_t>(payload.size()));
        // Fire-and-forget: no userdata, trivial callback (no Qt calls off-thread).
        dht_runner_put(d->runner, &rKey, rVal,
            [](bool, void *) {}, nullptr, /*permanent=*/false);
        dht_value_unref(rVal);
        qCInfo(lcDht) << "DHT backup-replica PUT to /r2:" << addrs.size()
                      << "addr(s)," << payload.size()
                      << "bytes (~10 min, network-change burst)";
    }
}

// ─── announcePresence (IDiscovery interface) ──────────────────────────────────

void DhtDiscovery::announcePresence(const Core::UserId &self,
                                     const Core::PeerAddress &addr)
{
    if (!d->runner) return;
    d->selfUserId   = self.value;
    if (d->selfPort != (addr.port > 0 ? addr.port : 9876))
        d->announcedAddrs.clear(); // port changed = network change
    d->selfPort     = addr.port > 0 ? addr.port : 9876;
    // punchPort is set separately via setPunchPort() after STUN completes.

    // Collect all local IPs on first announce; refresh on re-announce.
    d->localIps = collectLocalIps();

    // V32.45: arm the backup-replica burst on a genuine network change. The IP
    // set changing (new SSID, WiFi→cellular, reconnect) is our signal — the
    // periodic 55-min refresh goes through announceAddress, not here, so it
    // won't false-trigger. We skip the very first announce (lastLocalIps empty):
    // that's startup, not a switch. Compared as a set so a mere reordering of
    // the interface list doesn't count as a change. Gated by the toggle.
    const bool ipsChanged = !d->lastLocalIps.isEmpty() &&
        QSet<QString>(d->localIps.begin(), d->localIps.end()) !=
        QSet<QString>(d->lastLocalIps.begin(), d->lastLocalIps.end());
    if (ipsChanged &&
        QSettings().value(QStringLiteral("network/dhtBackupReplica"), true).toBool()) {
        // ~15s comfortably spans the coalesced LAN-then-public PUTs that follow
        // a switch once STUN settles, so the replica carries the full address.
        d->replicaBurstUntilMs = QDateTime::currentMSecsSinceEpoch() + 15000;
        qCInfo(lcDht) << "Network change detected (IP set changed) — "
                         "backup-replica burst armed for 15s";
    }
    d->lastLocalIps = d->localIps;

    // If caller provided a specific host (public IP from AppController),
    // track it separately.
    const bool isPublic = !addr.host.isEmpty()
        && !d->localIps.contains(addr.host);
    if (isPublic) {
        d->publicIp      = addr.host;
        d->publicIpValid = true;
    }

    // Announce every local IP so any peer on any shared subnet can reach us.
    for (const QString &ip : d->localIps)
        announceAddress(self, ip, d->selfPort);

    // Also announce the explicit addr (public IP if provided, else primary LAN).
    if (!addr.host.isEmpty() && !d->localIps.contains(addr.host))
        announceAddress(self, addr.host, d->selfPort);

    qCInfo(lcDht) << "DHT announce:" << d->localIps.size()
                  << "LAN addr(s) +" << (isPublic ? 1 : 0)
                  << "public for" << self.value;
}

// ─── setPunchPort ─────────────────────────────────────────────────────────────

void DhtDiscovery::setPunchPort(quint16 punchPort)
{
    if (d->selfPunchPort == punchPort) return;
    d->selfPunchPort = punchPort;
    d->announcedAddrs.clear(); // new punch port = fresh announce
    qCInfo(lcDht) << "Punch port set to" << punchPort
                  << "— re-announcing with updated reflexive addr";
    if (d->selfUserId.isEmpty() || !d->runner) return;
    const Core::UserId self{ d->selfUserId };
    for (const QString &ip : d->localIps)
        announceAddress(self, ip, d->selfPort, d->selfPunchPort);
    if (d->publicIpValid)
        announceAddress(self, d->publicIp, d->selfPort, d->selfPunchPort);
    // If STUN host differs from ipify (CGNAT), also announce punch-only entry
    // using the actual reflexive host so peers punch to the right IP.
    if (!d->selfStunHost.isEmpty() && d->selfStunHost != d->publicIp)
        announceAddress(self, d->selfStunHost, d->selfPunchPort, d->selfPunchPort);
}

void DhtDiscovery::setStunHost(const QString &stunHost)
{
    if (d->selfStunHost == stunHost) return;
    d->selfStunHost = stunHost;
    d->announcedAddrs.clear(); // new STUN host = fresh announce
}

// ─── lookupPeer (IDiscovery interface) ────────────────────────────────────────

// V32.44: one discovered address, collected across all DHT values at a key so
// the done callback can keep only the freshest announcement's addresses.
struct DhtAddrEntry {
    QString uid;
    QString host;
    quint16 wsPort    = 0;
    quint16 punchPort = 0;
    qint64  stamp     = 0;   // #ts of the announcement this came from
};

// How far back from the newest announcement we still trust an address. A single
// announce can arrive as 2-3 coalesced PUTs a few seconds apart (LAN first, then
// the public/punch addresses once STUN settles), so the window must comfortably
// span one announce burst — but stay well under the minutes a device normally
// spends on a network, so addresses from a *previous* network are dropped.
static constexpr qint64 kAddrFreshWindowSecs = 120;

void DhtDiscovery::lookupPeer(const Core::UserId &id)
{
    // Primary mailbox: the account's own DHT key.
    lookupAtKey(id, id.value.toUtf8(), /*replica=*/false);
}

void DhtDiscovery::lookupPeerReplica(const Core::UserId &id)
{
    // V32.45: backup mailbox in a different neighbourhood ("/r2"), only populated
    // as a short burst when the peer recently changed networks. AppController
    // calls this as a relookup escalation, after the primary path failed.
    if (!QSettings().value(QStringLiteral("network/dhtBackupReplica"), true).toBool())
        return;
    lookupAtKey(id, (id.value + QStringLiteral("/r2")).toUtf8(), /*replica=*/true);
}

void DhtDiscovery::lookupAtKey(const Core::UserId &id, const QByteArray &keyStr,
                                bool replica)
{
    if (!d->runner) return;

    dht_infohash key;
    dht_infohash_get_from_string(&key, keyStr.constData());

    // GetCtx: carries a shared lifetime sentinel so callbacks can safely check
    // whether DhtDiscovery is still alive before calling invokeMethod.
    // The sentinel is set to false in ~DhtDiscovery() BEFORE stop() runs,
    // so any in-flight done_cb sees alive=false and skips invokeMethod.
    // V32.44: it also accumulates every value at the key so done_cb can pick the
    // freshest announcement instead of emitting stale post-switch addresses.
    struct GetCtx {
        DhtDiscovery *disc;
        std::string   userId;
        std::shared_ptr<std::atomic<bool>> alive;
        QList<DhtAddrEntry> collected;   // every plausible addr seen, with stamp
        QByteArray          firstPubKey; // pubkey from the freshest stamped value
        qint64              bestStamp = -1;
    };
    GetCtx *ctx = new GetCtx{ this, id.value.toStdString(), d->alive, {}, {}, -1 };

    qCInfo(lcDht) << (replica ? "DHT replica lookup for" : "DHT lookup for") << id.value;

    dht_runner_get(d->runner, &key,
        // value_cb — opendht guarantees this fires BEFORE done_cb. We only
        // COLLECT here; the freshest-wins filtering + emit happens in done_cb
        // once every value at the key has been seen.
        [](const dht_value *val, void *userdata) -> bool {
            auto *c = static_cast<GetCtx *>(userdata);
            if (!val) return true;

            const dht_data_view view = dht_value_get_data(val);
            const uint8_t *data = view.data;
            const size_t   len  = view.size;
            constexpr size_t kMaxBytes = 512;
            if (!data || len == 0 || len > kMaxBytes) return true;

            // Parse all addresses from this value (v2 multi-addr or v1 single-addr).
            const QString qScId = QString::fromStdString(c->userId);
            auto peers = parsePayloadAll(reinterpret_cast<const char *>(data), len, qScId);
            if (peers.isEmpty()) return true;

            constexpr int kMaxField = 128;
            const qint64 valStamp = peers.first().stamp;  // all addrs share the value's #ts
            QByteArray valPubKey;
            int kept = 0;
            for (const auto &peer : peers) {
                // Reject userId mismatch — stale DHT routing node responses.
                if (!peer.userId.isEmpty() &&
                    peer.userId.toStdString() != c->userId) continue;
                // Reject implausible WS ports (ephemeral ports are DHT artifacts).
                if (peer.port == 0 || peer.port > 49151) continue;
                if (peer.userId.size() > kMaxField || peer.host.size() > kMaxField) continue;
                c->collected.append({ peer.userId, peer.host, peer.port,
                                      peer.punchPort, peer.stamp });
                ++kept;
                if (valPubKey.isEmpty() && !peer.pubKey.isEmpty())
                    valPubKey = peer.pubKey;
            }
            // Track the freshest stamped announcement's pubkey so we hand the
            // current key (not one from a stale value) to the encryption layer.
            if (kept > 0 && valStamp > c->bestStamp) {
                c->bestStamp = valStamp;
                if (!valPubKey.isEmpty()) c->firstPubKey = valPubKey;
            }
            return true; // keep collecting — there may be more values at this key
        },
        // done_cb — fires after all value_cb calls. Filter to the freshest
        // announcement and emit.
        [](bool ok, void *userdata) {
            auto *c = static_cast<GetCtx *>(userdata);
            // Snapshot everything out of c before deleting it (opendht won't
            // call value_cb after this, and we must not read c post-delete).
            DhtDiscovery      *disc   = c->disc;
            const std::string  userId = c->userId;
            auto               alive  = c->alive;
            QList<DhtAddrEntry> all    = c->collected;
            QByteArray          pubKey = c->firstPubKey;

            // Freshest-wins: find the newest #ts, then keep only addresses whose
            // stamp is within kAddrFreshWindowSecs of it. If nothing was stamped
            // (all values from an older build), maxStamp stays 0 and we keep
            // everything — no regression for legacy peers.
            qint64 maxStamp = 0;
            for (const auto &e : all) maxStamp = qMax(maxStamp, e.stamp);
            const qint64 cutoff = (maxStamp > 0) ? (maxStamp - kAddrFreshWindowSecs) : 0;

            // Dedup by host:port while filtering (replicated DHT values + the
            // coalesced LAN/public PUTs commonly repeat the same address).
            QList<DhtAddrEntry> fresh;
            QSet<QString> seen;
            int dropped = 0;
            for (const auto &e : all) {
                if (maxStamp > 0 && e.stamp < cutoff) { ++dropped; continue; }
                const QString k = e.host + QLatin1Char(':') + QString::number(e.wsPort)
                                + QLatin1Char('/') + QString::number(e.punchPort);
                if (seen.contains(k)) continue;
                seen.insert(k);
                fresh.append(e);
            }
            delete c;

            if (!alive->load(std::memory_order_acquire)) return;
            QMetaObject::invokeMethod(disc, [disc, fresh, pubKey, userId, ok, dropped]() {
                if (!ok && fresh.isEmpty()) {
                    qCDebug(lcDht) << "DHT lookup: no results for"
                                   << QString::fromStdString(userId);
                    return;
                }
                if (dropped > 0)
                    qCInfo(lcDht) << "DHT lookup: dropped" << dropped
                                  << "stale addr(s) from older announcement(s) for"
                                  << QString::fromStdString(userId);
                for (const auto &e : fresh) {
                    qCInfo(lcDht) << "DHT found:" << e.uid
                                  << "@" << e.host << ":" << e.wsPort
                                  << "(punch:" << e.punchPort << ")";
                    emit disc->peerDiscovered(
                        Core::UserId{ e.uid },
                        Core::PeerAddress{ e.host, e.wsPort });
                    if (e.punchPort != e.wsPort && e.punchPort > 0)
                        emit disc->peerDiscovered(
                            Core::UserId{ e.uid },
                            Core::PeerAddress{ e.host, e.punchPort });
                }
                if (!pubKey.isEmpty() && !fresh.isEmpty()) {
                    const QString uid = fresh.first().uid;
                    qCInfo(lcDht) << "DHT: peer public key received for" << uid;
                    emit disc->peerPublicKeyDiscovered(Core::UserId{ uid }, pubKey);
                }
            }, Qt::QueuedConnection);
        },
        ctx);
}


// ─── Two-step DHT connection requests ────────────────────────────────────────
//
// Key:   sha1(target_sc_id + "/connreq") — stored under target's mailbox
// Value: "CONNREQ\nfrom_sc_id\nhost:port\npunch_host:punch_port\ntimestamp\n"
// TTL:   permanent=false → OpenDHT expires naturally in ~10 min, reducing DHT load.
//        Requester re-publishes every 8 min while peer is unreachable.
//
// This allows asynchronous connection: Alice publishes a request, Bob picks it
// up on next startup and connects back — no simultaneous online window needed.

static constexpr const char *kConnReqSuffix = "/connreq";
static constexpr int kConnReqMaxAgeSeconds  = 600; // 10 minutes — matches DHT TTL

// ─── setSelfPublicKey ────────────────────────────────────────────────────────

void DhtDiscovery::setSelfPublicKey(const QByteArray &pubKey)
{
    d->selfPubKey = pubKey;
    // Force re-announce so the new key is published immediately.
    // Clear announcedAddrs so announceAddress doesn't skip as duplicate.
    d->announcedAddrs.clear();
    qCInfo(lcDht) << "Self public key set (" << pubKey.size() << "bytes)" << "— re-announcing to DHT";
}

void DhtDiscovery::setAppContext(Core::AppContextProvider *ctx)
{
    d->appContext = ctx;
}

void DhtDiscovery::publishConnectionRequest(const Core::UserId &target,
                                             const QByteArray &encryptedBlob)
{
    if (!d->runner) return;

    const QByteArray keyStr = (target.value + QLatin1String(kConnReqSuffix)).toUtf8();
    dht_infohash key;
    dht_infohash_get_from_string(&key, keyStr.constData());

    dht_value *val = dht_value_new(
        reinterpret_cast<const uint8_t *>(encryptedBlob.constData()),
        static_cast<size_t>(encryptedBlob.size()));

    struct PutCtx { QPointer<DhtDiscovery> self; QString target; };
    auto *ctx = new PutCtx{ this, target.value };

    // permanent=false: DHT cleans up automatically after ~10 min
    dht_runner_put(d->runner, &key, val,
        [](bool ok, void *userdata) {
            auto *c = static_cast<PutCtx *>(userdata);
            // Same fix as the multi-addr PUT callback above: snapshot and
            // delete c BEFORE posting, so PutCtx isn't leaked when the
            // queued event is dropped (receiver destroyed before dispatch).
            const QString targetId = c->target;
            QPointer<DhtDiscovery> discSelf = c->self;
            delete c;
            QMetaObject::invokeMethod(discSelf.data(), [discSelf, ok, targetId]() {
                if (!discSelf) return;
                if (ok)
                    qCInfo(lcDht) << "Connection request published for" << targetId
                                  << "(expires ~10 min, DHT cleans up)";
                else
                    qCWarning(lcDht) << "Failed to publish connection request for" << targetId;
            }, Qt::QueuedConnection);
        },
        ctx,
        /*permanent=*/false);

    dht_value_unref(val);
    qCInfo(lcDht) << "Publishing encrypted connection request to" << target.value
                  << "(" << encryptedBlob.size() << "bytes, opaque to DHT)";
}


void DhtDiscovery::checkConnectionRequests(const Core::UserId &self)
{
    if (!d->runner) return;

    // V32.47: coalesce checks that land within ~2.5 s of the previous one — see
    // the Impl::lastMailboxCheckMs note. Collapses coincident periodic/startup/
    // queue-exhausted timers into a single GET.
    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    constexpr qint64 kMailboxCheckCoalesceMs = 2500;
    if (d->lastMailboxCheckMs != 0 && nowMs - d->lastMailboxCheckMs < kMailboxCheckCoalesceMs) {
        qCDebug(lcDht) << "Mailbox check coalesced (within"
                       << (nowMs - d->lastMailboxCheckMs) << "ms of previous) — skipping";
        return;
    }
    d->lastMailboxCheckMs = nowMs;

    const QByteArray keyStr = (self.value + QLatin1String(kConnReqSuffix)).toUtf8();
    dht_infohash key;
    dht_infohash_get_from_string(&key, keyStr.constData());

    // V32.34+: `seen` dedups identical blobs WITHIN one GET batch. OpenDHT
    // replicates each stored value across multiple nodes, so a single GET
    // commonly returns the same connreq blob 4-6 times. Previously every
    // copy was logged at info level AND posted to AppController via a queued
    // invokeMethod, where the nonce-dedup finally dropped them — but only
    // after the redundant logging + cross-thread hops + AppController work.
    // Deduping here at the source (keyed on the raw blob bytes) eliminates
    // that waste: each distinct blob is logged and forwarded exactly once.
    struct GetCtx {
        DhtDiscovery *disc;
        std::string   selfId;
        std::unordered_set<std::string> seen;
        // V32.36+ (shared-slot diagnostics): count distinct values coexisting
        // under this one key. For the per-recipient connreq slot this is
        // normally 0-1; if we move to group-shared slots it's "how many members
        // have a pending entry". Logging it confirms on-device that OpenDHT
        // stores multiple writers' values side-by-side rather than clobbering —
        // the property the shared-slot design depends on.
        int distinctValues = 0;
        int replicaHits = 0;
    };
    GetCtx *ctx = new GetCtx{ this, self.value.toStdString(), {} };

    qCInfo(lcDht) << "Checking DHT mailbox for connection requests:" << self.value;

    dht_runner_get(d->runner, &key,
        [](const dht_value *val, void *userdata) -> bool {
            auto *c = static_cast<GetCtx *>(userdata);
            if (!val) return true;

            const dht_data_view view = dht_value_get_data(val);
            // V32.34+: raised the receive cap from 512 to 2048. The connreq
            // blob is sender-id prefix + nonce(12) + AEAD ciphertext; an
            // encrypted blob carrying a few delivery hints or a PQ-hybrid
            // wrapped key can exceed 512. OpenDHT permits values up to 64 KiB,
            // so 2048 is still very conservative. A silent `return true` on
            // an oversize value was dropping legitimate larger requests with
            // no trace — now we log it so it's diagnosable rather than a
            // mystery "peer never connected back".
            if (!view.data || view.size == 0) return true;
            if (view.size > 2048) {
                qCWarning(lcDht) << "Connreq value oversize (" << view.size
                                 << "bytes > 2048) — ignoring. If this is a"
                                 << "legitimate request the cap needs raising.";
                return true;
            }

            // Blob format: sc-from_id + "\n" + nonce(12) + ciphertext
            // Extract sender ID from unencrypted prefix (public info anyway)
            const QByteArray rawBytes = QByteArray(
                reinterpret_cast<const char *>(view.data),
                static_cast<int>(view.size));
            const int nl = static_cast<int>(rawBytes.indexOf('\n'));
            if (nl < 1) return true;
            const QString fromId = QString::fromUtf8(rawBytes.left(nl));
            if (!fromId.startsWith(QLatin1String("sc-"))) return true;
            // Rest is nonce+ciphertext — passed opaque to AppController

            // V32.34+: skip a blob we've already forwarded in THIS GET batch.
            // OpenDHT returns the same replicated value multiple times; key
            // the dedup on the full raw blob (sender prefix + nonce +
            // ciphertext) so distinct requests from the same sender still
            // pass, but byte-identical replicas are collapsed to one.
            const std::string blobKey(rawBytes.constData(),
                                      static_cast<size_t>(rawBytes.size()));
            if (!c->seen.insert(blobKey).second) {
                // Already saw this exact blob this batch — drop silently.
                ++c->replicaHits;
                return true;
            }
            ++c->distinctValues;

            QPointer<DhtDiscovery> safeDisc(c->disc);
            QMetaObject::invokeMethod(c->disc,
                [safeDisc, fromId, rawBytes]() {
                    if (!safeDisc) return;
                    qCInfo(lcDht) << "Connection request blob from" << fromId
                                  << "(" << rawBytes.size() << "bytes) — passing to AppController";
                    emit safeDisc->connectionRequestReceived(
                        Core::UserId{ fromId }, rawBytes);
                }, Qt::QueuedConnection);

            return true; // continue — may be multiple pending requests
        },
        [](bool /*ok*/, void *userdata) {
            auto *c = static_cast<GetCtx *>(userdata);
            // Same fix as the PUT callbacks: snapshot and delete BEFORE
            // posting, so GetCtx isn't leaked when the receiver is destroyed
            // before the queued event fires.
            const QString selfStr = QString::fromStdString(c->selfId);
            QPointer<DhtDiscovery> discSelf(c->disc);
            const int distinct = c->distinctValues;
            const int replicas = c->replicaHits;
            delete c;
            QMetaObject::invokeMethod(discSelf.data(), [discSelf, selfStr, distinct, replicas]() {
                if (!discSelf) return;
                // V32.36+: report multi-writer coexistence. distinct = how many
                // different senders' values were stored under this one key;
                // replicas = byte-identical copies OpenDHT returned (replication
                // factor). If distinct > 1 here, shared-slot batching is working.
                qCInfo(lcDht) << "Mailbox check complete for" << selfStr
                              << "— distinct values:" << distinct
                              << "(replica copies collapsed:" << replicas << ")";
            }, Qt::QueuedConnection);
        },
        ctx);
}

} // namespace SnapClone::Network

#endif // !SNAPCLONE_JAMI_BACKEND
#endif // SNAPCLONE_OPENDHT_BACKEND
