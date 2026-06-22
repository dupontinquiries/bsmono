#include "core/SnapRouter.hpp"
#include "core/EncodeTier.hpp"
#include "network/UdpChannel.hpp"
#include "network/NetworkManager.hpp"
#include "storage/StorageManager.hpp"
#include "crypto/ICryptoService.hpp"

#include <QLoggingCategory>
#include <QDataStream>
#include <QBuffer>
#include <QTimeZone>
#include <QStandardPaths>
#include <QDir>
#include <QFile>
#include <QTimer>
#include <QCryptographicHash>
#include <QSettings>

// ─── Wire protocol v3 — unified state sync ─────────────────────────────────
//
// Goals:
//   - Eliminate explicit request/response message types
//   - Reduce round trips (predictive sending)
//   - Support deltas and future temporal encoding
//   - Move toward QUIC-style ack + stateless flow
//
// Model:
//   All communication: ANNOUNCE → "this is what I have"
//                      DATA     → "here are bytes (full or delta)"
//
//   Messages carry flags instead of types:
//     F_ANNOUNCE  metadata only
//     F_DATA      includes payload
//     F_DELTA     payload is patch vs baseHash
//     F_ACK       piggyback acknowledgment
//     F_SYNC      state reconciliation
//     F_HINT      predictive hint (pre-allocate)
//
//   No HAVE/NEED: sender predicts and includes DATA immediately.
//   No standalone ACK: every message carries ack number (QUIC-style).
//   Sync: ANNOUNCE|SYNC exchanges known hashes — gaps inferred automatically.
//
// Compatibility:
//   v1/v2 remain supported. v3 used when both peers have negotiated it.
//   Fallback to v2 is automatic.
// ─────────────────────────────────────────────────────────────────────────────

Q_LOGGING_CATEGORY(lcRouter, "snapclone.router")

namespace SnapClone::Core {

namespace {
// V32.42+: how long a snap lives locally before auto-expiry, unless the user
// keeps it (manual save / monthly export, which clear expires_at). Configurable
// via "storage/retentionDays"; defaults to 30 days so the monthly export
// reminder can actually preserve a month's moments before they're purged (the
// old hard-coded 24h expired everything long before the monthly prompt). The
// 1..3650-day clamp guards against a corrupt/hostile setting.
qint64 snapRetentionSecs()
{
    const int days = QSettings()
        .value(QStringLiteral("storage/retentionDays"), 30).toInt();
    return static_cast<qint64>(qBound(1, days, 3650)) * 24 * 60 * 60;
}

// V32.43+: compute a snap's expiry timestamp. Power users can turn on
// "keep everything" (storage/keepEverything) — then snaps get no expiry at
// all (an invalid/empty expires_at, the same "never expires" sentinel that
// markSnapKept uses), so they're never auto-purged and the user manages their
// own exports. Regular users get the retention window above.
// V32.45+: ephemeral text gets a much shorter, type-specific TTL than the
// generic media retention window (disappearing-message behaviour). Default 24h,
// override via storage/textRetentionHours.
qint64 textRetentionSecs()
{
    const int hrs = QSettings()
        .value(QStringLiteral("storage/textRetentionHours"), 24).toInt();
    return static_cast<qint64>(qBound(1, hrs, 720)) * 60 * 60;
}

QDateTime snapExpiryFrom(const QDateTime &sentAt,
                         Core::MediaType mediaType = Core::MediaType::Image)
{
    if (QSettings().value(QStringLiteral("storage/keepEverything"), false).toBool())
        return QDateTime();  // no expiry — kept until explicitly cleared
    if (mediaType == Core::MediaType::Text)
        return sentAt.addSecs(textRetentionSecs());   // ephemeral text: own TTL
    return sentAt.addSecs(snapRetentionSecs());
}
} // namespace


// Cap per-peer hash memory to avoid unbounded growth in long sessions.
static constexpr int kMaxKnownHashes = 500;
static void insertCapped(QSet<QString> &set, const QString &hash)
{
    if (set.size() >= kMaxKnownHashes)
        set.clear(); // drop oldest dedup history — causes at most one redundant transfer
    set.insert(hash);
}

// ─── Wire format constants ────────────────────────────────────────────────────

static constexpr quint32 kWireMagic    = 0x534E4150; // "SNAP"
static constexpr quint8  kWireVersion1 = 1;
static constexpr quint8  kWireVersion2 = 2;
static constexpr quint8  kWireVersion3 = 3;
/// v4 adds videoDurationMs (qint32) for video snaps. Backward-compatible:
/// older peers receive v3 wire format and treat videoDurationMs as 0.
static constexpr quint8  kWireVersion4 = 4;
/// v5 adds a `tier` byte (EncodeTier wire value) immediately after
/// videoDurationMs. Identifies which media-quality tier this snap
/// represents — enables tiered delivery where the same logical snap
/// can arrive multiple times at progressively-better quality. v5
/// readers handle v3 and v4 (no tier byte read when version<5,
/// defaults to Standard); v3/v4 readers reject v5 messages via the
/// version check. Same upgrade-together model as v3→v4: one binary
/// handles both sides, so peers always agree on version.
static constexpr quint8  kWireVersion5 = 5;

// v2 sub-message types (legacy — kept for backward compat)
enum class MsgType : quint8 {
    Snap     = 0,
    Offer    = 1,
    Have     = 2,
    Need     = 3,
    Ack      = 4,
    SyncReq  = 5,
    SyncList = 6,
};

// v3 flags (replaces type enum)
static constexpr quint8 F_ANNOUNCE = 1 << 0;
static constexpr quint8 F_DATA     = 1 << 1;
static constexpr quint8 F_DELTA    = 1 << 2;
static constexpr quint8 F_ACK      = 1 << 3;
static constexpr quint8 F_SYNC     = 1 << 4;
static constexpr quint8 F_HINT     = 1 << 5;

// ─── Helpers ──────────────────────────────────────────────────────────────────

static QByteArray sha256(const QByteArray &data)
{
    return QCryptographicHash::hash(data, QCryptographicHash::Sha256).toHex();
}

static QString safeName(const QString &raw, int maxLen = 64)
{
    QString out;
    out.reserve(qMin(raw.size(), maxLen));
    for (const QChar c : raw) {
        if (c.isLetterOrNumber() || c == QLatin1Char('-')) {
            out.append(c);
            if (out.size() >= maxLen) break;
        }
    }
    return out.isEmpty() ? QStringLiteral("unknown") : out;
}

/// Detect the file extension to use based on the actual bytes of the media.
/// The previous code always wrote video as ".mp4" regardless of container,
/// which produced files that:
///   • opened poorly in players that respect the extension (e.g. file
///     managers that pick a default app based on .mp4)
///   • confused the user when sharing externally
///   • broke our own VideoSnapViewerDialog's MoviesLocation save path
///     which preserves the extension verbatim
/// Magic bytes (per RFC 9577 and codec specs):
///   • WebM:     0x1A 0x45 0xDF 0xA3 (EBML header) — also covers .mkv
///                since both use EBML; we still call it .webm because our
///                transcoder targets webmmux with VP8/VP9/AV1 + Opus.
///   • MP4/M4V:  bytes 4..7 = "ftyp"
///   • MOV:      bytes 4..7 = "moov" or "mdat" (Apple QuickTime)
///   • JPEG:     0xFF 0xD8 0xFF
/// Falls back to a media-type default if no magic matches.
static QString detectMediaExtension(const QByteArray &bytes,
                                     Core::MediaType mediaType)
{
    // NOTE: this logic is mirrored + UNIT-TESTED in Rust at
    // snapclone-videosrc::sniff_container (10 tests incl the VVC/HEVC→.mkv
    // cases). Keep the two byte-for-byte in sync; if this grows, prefer driving
    // it from the crate via FFI rather than re-deriving here.
    // EBML (WebM / MKV) — both use 0x1A 0x45 0xDF 0xA3 as the first 4 bytes.
    if (bytes.size() >= 4
        && static_cast<quint8>(bytes[0]) == 0x1A
        && static_cast<quint8>(bytes[1]) == 0x45
        && static_cast<quint8>(bytes[2]) == 0xDF
        && static_cast<quint8>(bytes[3]) == 0xA3) {
        // Both webm and mkv share the EBML magic, so the first 4 bytes can't
        // tell them apart. We must look INSIDE for two signals:
        //   • the matroska CodecID "V_MPEGH/ISO/VVC" (H.266) — webm by spec
        //     cannot carry VVC, so its presence means a real .mkv; calling it
        //     .webm would make external players reject it.
        //   • the EBML DocType: "webm" vs "matroska".
        // Scan a bounded prefix (the header/Tracks element live near the start;
        // 64 KiB is plenty and avoids walking a multi-MB clip).
        const int scan = static_cast<int>(qMin<qsizetype>(bytes.size(), 65536));
        const QByteArray head = bytes.left(scan);
        if (head.contains("V_MPEGH/ISO/VVC")        // H.266/VVC track
            || head.contains("V_MPEGH/ISO/HEVC")) { // H.265 too — also not webm
            return QStringLiteral(".mkv");
        }
        // DocType check: an explicit "matroska" doctype (not "webm") → .mkv.
        const int dt = head.indexOf("matroska");
        const int wd = head.indexOf("webm");
        if (dt >= 0 && (wd < 0 || dt < wd)) {
            return QStringLiteral(".mkv");
        }
        // Otherwise it's our normal webmmux output (VP8/VP9/AV1 + Opus).
        return QStringLiteral(".webm");
    }
    // MP4 / M4V — "ftyp" box at offset 4-7
    if (bytes.size() >= 8 && bytes.mid(4, 4) == "ftyp") {
        return QStringLiteral(".mp4");
    }
    // QuickTime
    if (bytes.size() >= 8
        && (bytes.mid(4, 4) == "moov" || bytes.mid(4, 4) == "mdat")) {
        return QStringLiteral(".mov");
    }
    // JPEG
    if (bytes.size() >= 3
        && static_cast<quint8>(bytes[0]) == 0xFF
        && static_cast<quint8>(bytes[1]) == 0xD8
        && static_cast<quint8>(bytes[2]) == 0xFF) {
        return QStringLiteral(".jpg");
    }
    // PNG
    if (bytes.size() >= 8
        && static_cast<quint8>(bytes[0]) == 0x89
        && bytes.mid(1, 3) == "PNG") {
        return QStringLiteral(".png");
    }
    // Fallback: media-type default. Don't claim .mp4 if we don't know it's MP4.
    return (mediaType == Core::MediaType::Video)
        ? QStringLiteral(".bin")  // unknown video container — generic
        : QStringLiteral(".jpg");
}

static bool writeMediaFile(const QString &snapId, const QByteArray &bytes,
                            QString &pathOut,
                            Core::MediaType mediaType = Core::MediaType::Image)
{
    const QString cacheDir =
        QStandardPaths::writableLocation(QStandardPaths::AppDataLocation)
        + QStringLiteral("/media");
    QDir().mkpath(cacheDir);
    // Detect the actual container from the bytes rather than blindly writing
    // ".mp4" for any video. The sender's pipeline might have produced WebM
    // (our default — webmmux + VP8/9 or AV1), MKV (matroskamux fallback),
    // or MP4 (transcoded externally). Wrong extension = broken external
    // sharing + confused users.
    const QString ext = detectMediaExtension(bytes, mediaType);
    pathOut = cacheDir + QLatin1Char('/') + safeName(snapId) + ext;
    QFile f(pathOut);
    if (!f.open(QIODevice::WriteOnly) || f.write(bytes) < 0) {
        qCWarning(lcRouter) << "Failed to write media:" << f.errorString();
        pathOut.clear();
        return false;
    }
    return true;
}

// ─── v1/v2 serialization (unchanged) ─────────────────────────────────────────

[[maybe_unused]] static QByteArray serializeSnap(const Snap &snap, const QByteArray &media)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    // v3 adds optional location: hasLoc flag + lat/lon/acc as float32
    // (float32 gives ~11m precision — more than enough for this use case)
    const bool hasLoc = snap.hasLocation();
    // Bump to v4 so videoDurationMs travels on the wire. v3 receivers reject
    // the magic/version mismatch and the sender will retry with v3 if needed.
    // (In practice, all peers update together since the app auto-updates.)
    ds << kWireMagic << kWireVersion4
       << snap.id.value
       << snap.senderId.value
       // Round to nearest minute for privacy — prevents cross-message
       // timestamp correlation by peers who receive multiple snaps.
       // Full precision is kept in the local DB; only the wire gets rounded.
       << (snap.sentAt.toMSecsSinceEpoch() / 60000LL * 60000LL)
       << static_cast<qint32>(snap.mediaType)
       << static_cast<qint32>(snap.durationSecs)
       << static_cast<quint8>(hasLoc ? 1 : 0);
    if (hasLoc) {
        ds << static_cast<float>(snap.locLat)
           << static_cast<float>(snap.locLon)
           << static_cast<float>(snap.locAccM);
    }
    // v4: video playback duration in ms. 0 for non-video or unknown.
    ds << static_cast<qint32>(snap.videoDurationMs);
    ds << media;
    return out;
}

static bool deserializeSnap(const QByteArray &data, Snap &out, QByteArray &mediaOut)
{
    QDataStream ds(data);
    ds.setByteOrder(QDataStream::BigEndian);
    quint32 magic = 0;
    quint8  version = 0;
    ds >> magic >> version;
    if (magic != kWireMagic) return false;
    if (version != kWireVersion1 && version != kWireVersion2
        && version != kWireVersion3 && version != kWireVersion4) return false;

    qint64 sentAtMs = 0;
    qint32 mediaType = 0, durationSecs = 0;
    ds >> out.id.value >> out.senderId.value >> sentAtMs >> mediaType >> durationSecs;

    constexpr int kMaxIdLen = 256;
    if (out.id.value.size() > kMaxIdLen || out.senderId.value.size() > kMaxIdLen)
        return false;

    if (version >= kWireVersion3) {
        quint8 hasLoc = 0;
        ds >> hasLoc;
        if (hasLoc) {
            float lat = 0.f, lon = 0.f, acc = 0.f;
            ds >> lat >> lon >> acc;
            out.locLat  = static_cast<double>(lat);
            out.locLon  = static_cast<double>(lon);
            out.locAccM = static_cast<double>(acc);
        }
    }

    // v4: videoDurationMs precedes the media payload. Older versions don't
    // have it on the wire; field stays at default 0 for those.
    if (version >= kWireVersion4) {
        qint32 vdur = 0;
        ds >> vdur;
        out.videoDurationMs = vdur;
    }

    ds >> mediaOut;
    if (ds.status() != QDataStream::Ok) return false;

    out.sentAt       = QDateTime::fromMSecsSinceEpoch(sentAtMs, QTimeZone::utc());
    out.expiresAt    = snapExpiryFrom(out.sentAt, static_cast<Core::MediaType>(mediaType));
    out.mediaType    = static_cast<MediaType>(mediaType);
    out.durationSecs = static_cast<int>(durationSecs);
    return true;
}

// ─── v2 message builders (legacy) ────────────────────────────────────────────

static QByteArray buildOffer(const Snap &snap, const QString &contentHash)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kWireMagic << kWireVersion2
       << static_cast<quint8>(MsgType::Offer)
       << snap.id.value << contentHash
       << snap.senderId.value
       // Round to nearest minute for privacy — prevents cross-message
       // timestamp correlation by peers who receive multiple snaps.
       // Full precision is kept in the local DB; only the wire gets rounded.
       << (snap.sentAt.toMSecsSinceEpoch() / 60000LL * 60000LL)
       << static_cast<qint32>(snap.mediaType)
       << static_cast<qint32>(snap.durationSecs);
    return out;
}

static QByteArray buildHave(const QString &snapId, const QString &hash)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kWireMagic << kWireVersion2
       << static_cast<quint8>(MsgType::Have)
       << snapId << hash;
    return out;
}

static QByteArray buildNeed(const QString &snapId)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kWireMagic << kWireVersion2
       << static_cast<quint8>(MsgType::Need)
       << snapId;
    return out;
}

static QByteArray buildAck(const QString &snapId)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kWireMagic << kWireVersion2
       << static_cast<quint8>(MsgType::Ack)
       << snapId;
    return out;
}

[[maybe_unused]] static QByteArray buildSyncReq(qint64 sinceMs)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kWireMagic << kWireVersion2
       << static_cast<quint8>(MsgType::SyncReq)
       << sinceMs;
    return out;
}

static QByteArray buildSyncList(const QList<Snap> &snaps)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);
    ds << kWireMagic << kWireVersion2
       << static_cast<quint8>(MsgType::SyncList)
       << static_cast<quint32>(snaps.size());
    for (const Snap &s : snaps)
        ds << s.id.value << s.contentHash;
    return out;
}

// ─── v3/v4/v5 message builder ────────────────────────────────────────────────
//
// v3 wire format:
//   [magic:4][version:1][flags:1][reserved:2]
//   [snapId:QString][contentHash:QString]
//   [seq:4][ack:4]
//   [mediaType:1][durationSecs:2]
//   if F_SYNC:  [since:8][hashCount:4][hash:QString × N]
//   if F_DATA:  [payload:QByteArray]
//
// v4 wire format (additive):
//   identical to v3, plus a [videoDurationMs:4] qint32 immediately after
//   durationSecs and BEFORE the F_SYNC/F_DATA tail. v4 readers handle v3
//   too (no field read when version==3); v3 readers reject v4 messages
//   on the version check.
//
// v5 wire format (additive):
//   identical to v4, plus a [tier:1] quint8 (EncodeTier wire value)
//   immediately after videoDurationMs, still BEFORE F_SYNC/F_DATA. v5
//   readers handle v3/v4 (no tier read when version<5, defaults to
//   Standard); v3/v4 readers reject v5 on the version check. Same
//   "easier to upgrade both sides together" rationale — one binary
//   on both ends of the conversation.

static QByteArray buildV3(quint8 flags,
                            const QString &snapId,
                            const QString &contentHash,
                            quint32 seq,
                            quint32 ack,
                            const QByteArray &payload = {},
                            qint64 sinceMs = 0,
                            const QList<QString> &knownHashes = {},
                            quint8 mediaType = 0,
                            quint16 durationSecs = 5,
                            qint32 videoDurationMs = 0,
                            Core::Media::EncodeTier tier = Core::Media::EncodeTier::Standard)
{
    QByteArray out;
    QDataStream ds(&out, QIODevice::WriteOnly);
    ds.setByteOrder(QDataStream::BigEndian);

    ds << kWireMagic << kWireVersion5 << flags << quint8(0) << quint16(0); // reserved
    ds << snapId << contentHash;
    ds << seq << ack;
    // mediaType + durationSecs always present (header layout consistent across
    // v3, v4, v5 up to this point)
    ds << mediaType << durationSecs;
    // v4: video playback duration in milliseconds. 0 for non-video / unknown.
    // Sits between header and F_SYNC/F_DATA tail so older v3 readers cannot
    // partially-parse it as something else (they reject the v4 version byte
    // earlier and bail).
    ds << videoDurationMs;
    // v5: tier byte. Standard (2) for back-compat at unaware call sites.
    ds << Core::Media::tierToWire(tier);

    if (flags & F_SYNC) {
        ds << sinceMs;
        ds << static_cast<quint32>(knownHashes.size());
        for (const QString &h : knownHashes)
            ds << h;
    }

    if (flags & F_DATA)
        ds << payload;

    return out;
}

// ─── SnapRouter ───────────────────────────────────────────────────────────────

SnapRouter::SnapRouter(
    Network::NetworkManager *network,
    Storage::StorageManager *storage,
    Crypto::ICryptoService  *crypto,
    QObject                 *parent)
    : QObject(parent)
    , m_network(network)
    , m_storage(storage)
    , m_crypto(crypto)
{}

// ─── Version negotiation ──────────────────────────────────────────────────────

void SnapRouter::setPeerVersion(const UserId &peerId, quint8 version)
{
    if (version >= kWireVersion3)
        m_v3Peers.insert(peerId.value);
}

void SnapRouter::setUdpChannel(Network::UdpChannel *ch)
{
    m_udpChannel = ch;
}

void SnapRouter::clearPeerState(const UserId &peerId)
{
    m_syncRepliedTo.remove(peerId.value);
    m_v3Peers.remove(peerId.value);
    m_peerKnownHashes.remove(peerId.value);
    m_peerSeq.remove(peerId.value);
    m_peerAckRecv.remove(peerId.value);
    m_v3PendingAck.remove(peerId.value); // Bug 5 fix
    // V32.34+: also clear the SYNC dedup timestamp. Otherwise it lingers
    // for every ever-connected peer — small (8 bytes per entry) but
    // unbounded growth across long sessions / many transient peers.
    m_lastSyncHandledMs.remove(peerId.value);
}

bool SnapRouter::isV3Peer(const UserId &id) const
{
    return m_v3Peers.contains(id.value);
}

// ─── Shared send helper ───────────────────────────────────────────────────────

bool SnapRouter::encryptAndSend(const UserId &peerId, const QByteArray &plain)
{
    const auto enc = m_crypto->encryptFor(peerId, plain);
    if (!enc) {
        qCWarning(lcRouter) << "Encrypt failed for" << peerId.value << ":" << enc.error();
        return false;
    }
    QByteArray wire;
    wire.reserve(enc->nonce.size() + enc->ciphertext.size());
    wire.append(enc->nonce);
    wire.append(enc->ciphertext);
    // Prefer UDP channel when available — bypasses TCP/router NAT blocking.
    // TCP sendToPeer queues as fallback for when UDP isn't connected yet.
    if (m_udpChannel && m_udpChannel->isConnected(peerId)) {
        qCDebug(lcRouter) << "Sending via UDP channel to" << peerId.value;
        m_udpChannel->sendPayload(peerId, wire);
    } else {
        m_network->sendToPeer(peerId, wire);
    }
    return true;
}

// ─── v3 predictive heuristic ─────────────────────────────────────────────────

bool SnapRouter::shouldSendDataImmediately(const UserId &peerId,
                                            const QString &contentHash) const
{
    // If we have no record of what this peer knows, send data immediately.
    // Better to spend one extra transmission than to wait for a round trip.
    const auto it = m_peerKnownHashes.constFind(peerId.value);
    if (it == m_peerKnownHashes.constEnd()) return true;
    // If peer has already acknowledged this hash, they have the data.
    return !it->contains(contentHash);
}

// ─── v3 outbound ─────────────────────────────────────────────────────────────

void SnapRouter::sendV3(const UserId &peerId, quint8 flags,
                         const QString &snapId, const QString &contentHash,
                         const QByteArray &payload,
                         quint8 mediaType, quint16 durationSecs,
                         qint32 videoDurationMs,
                         Core::Media::EncodeTier tier)
{
    const quint32 seq = ++m_peerSeq[peerId.value];
    const quint32 ack = m_peerAckRecv.value(peerId.value, 0);
    const QByteArray plain = buildV3(flags, snapId, contentHash, seq, ack,
                                     payload, 0, {}, mediaType, durationSecs,
                                     videoDurationMs, tier);
    encryptAndSend(peerId, plain);
}

void SnapRouter::sendV3Sync(const UserId &peerId)
{
    // Build our known-hash list for this peer (snaps we've sent them).
    // Limit to last 30 days to cap message size.
    const QDateTime since = QDateTime::currentDateTimeUtc().addDays(-30);
    const qint64 sinceMs  = since.toMSecsSinceEpoch();

    // Collect hashes of outbox snaps for this peer.
    const auto outboxSnaps = m_storage->loadOutboxSnaps(m_selfUserId);
    QList<QString> knownHashes;
    knownHashes.reserve(qMin(outboxSnaps.size(), 500));
    for (const Snap &s : outboxSnaps) {
        if (!s.contentHash.isEmpty() && s.sentAt >= since)
            knownHashes.append(s.contentHash);
        if (knownHashes.size() >= 500) break;
    }

    const quint32 seq = ++m_peerSeq[peerId.value];
    const quint32 ack = m_peerAckRecv.value(peerId.value, 0);
    const QByteArray plain = buildV3(
        F_ANNOUNCE | F_SYNC, {}, {}, seq, ack, {}, sinceMs, knownHashes);
    encryptAndSend(peerId, plain);
    qCInfo(lcRouter) << "v3 ANNOUNCE|SYNC sent to" << peerId.value
                     << "with" << knownHashes.size() << "known hashes";
}

// ─── Outbound routing ─────────────────────────────────────────────────────────

void SnapRouter::routeOutbound(const Snap &plainSnap)
{
    qCDebug(lcRouter) << "Outbound snap" << plainSnap.id.value
                      << "→" << plainSnap.recipientId.value;

    if (plainSnap.recipientId.isNull()) {
        emit routingFailed(plainSnap.id, QStringLiteral("No recipient specified"));
        return;
    }

    const QString contentHash = sha256(plainSnap.encryptedBlob);

    // Persist outbox copy first — always, even if peer is offline.
    // The SYNC mechanism delivers undelivered outbox snaps on the next connection.
    Snap outboxCopy = plainSnap;
    outboxCopy.contentHash = contentHash;
    if (!plainSnap.encryptedBlob.isEmpty())
        writeMediaFile(plainSnap.id.value, plainSnap.encryptedBlob,
                       outboxCopy.localPath);
    outboxCopy.encryptedBlob = {};
    m_storage->saveSnap(outboxCopy);

    // Offline check: attempt a test encryption to see if session key exists.
    // If it fails (no key for this peer), the snap is already in the outbox.
    // Signal the caller so it can trigger a reconnect + show "pending" in UI.
    {
        const auto testEnc = m_crypto->encryptFor(plainSnap.recipientId,
                                                  QByteArray(1, '\0'));
        if (!testEnc) {
            qCInfo(lcRouter) << "Snap" << plainSnap.id.value
                             << "queued — no session key for" << plainSnap.recipientId.value
                             << "(peer offline). Will deliver on reconnect via SYNC.";
            emit snapQueuedForOfflinePeer(plainSnap.id, plainSnap.recipientId);
            return;
        }
    }

    // Always use v3 — all current builds support it.
    // We upgrade peers to v3 on first contact; v2 is only used by very old clients.
    // isV3Peer() starts false but we optimistically assume v3 and downgrade on
    // receiving a v2 message. Mark the peer as v3 now so the first snap uses it.
    m_v3Peers.insert(plainSnap.recipientId.value);

    if (isV3Peer(plainSnap.recipientId)) {
        // ── v3 path: predictive send ──────────────────────────────────────
        // If peer probably doesn't have the blob, send ANNOUNCE|DATA now.
        // If they likely have it, send ANNOUNCE only — ack piggyback tells us.
        const bool sendData =
            shouldSendDataImmediately(plainSnap.recipientId, contentHash);
        const quint8 flags = sendData
            ? (F_ANNOUNCE | F_DATA | F_ACK)
            : (F_ANNOUNCE | F_ACK);

        sendV3(plainSnap.recipientId, flags,
               plainSnap.id.value, contentHash,
               sendData ? plainSnap.encryptedBlob : QByteArray{},
               static_cast<quint8>(plainSnap.mediaType),
               static_cast<quint16>(qBound(0, plainSnap.durationSecs, 65535)),
               static_cast<qint32>(plainSnap.videoDurationMs),
               plainSnap.tier);

        qCInfo(lcRouter) << "v3" << (sendData ? "ANNOUNCE|DATA" : "ANNOUNCE")
                         << "sent for" << plainSnap.id.value
                         << "hash=" << contentHash.left(8) << "…";

        if (!sendData) {
            // Keep the blob pending in case peer requests it (shouldn't happen
            // in v3 but good safety net).
            m_pendingOffers[plainSnap.id.value] = {
                plainSnap.recipientId, contentHash, plainSnap.encryptedBlob
            };
        } else {
            // Bug 5 fix: don't mark delivered until piggybacked ACK received.
            // Record seq → snapId so we can deliver on ACK.
            const quint32 sentSeq = m_peerSeq.value(plainSnap.recipientId.value, 0);
            m_v3PendingAck[plainSnap.recipientId.value][sentSeq] = plainSnap.id.value;
            // Record that peer now has this hash (optimistic for dedup — not for delivery).
            insertCapped(m_peerKnownHashes[plainSnap.recipientId.value], contentHash);
            qCDebug(lcRouter) << "v3 snap" << plainSnap.id.value
                              << "pending ACK at seq" << sentSeq;
        }
        return;
    }

    // ── v2 path (unchanged) ───────────────────────────────────────────────
    m_pendingOffers[plainSnap.id.value] = {
        plainSnap.recipientId, contentHash, plainSnap.encryptedBlob
    };

    // Offer timeout — keep blob for 5 min to cover relay pairing time.
    const QString snapIdCopy = plainSnap.id.value;
    QTimer::singleShot(5 * 60'000, this, [this, snapIdCopy]() {
        if (m_pendingOffers.remove(snapIdCopy))
            qCDebug(lcRouter) << "OFFER timeout — dropped blob for" << snapIdCopy;
    });

    const QByteArray offerPlain = buildOffer(plainSnap, contentHash);
    if (!encryptAndSend(plainSnap.recipientId, offerPlain))
        emit routingFailed(plainSnap.id, QStringLiteral("Encrypt OFFER failed"));
    else
        qCInfo(lcRouter) << "OFFER sent for" << plainSnap.id.value
                         << "hash=" << contentHash.left(8) << "…";
}

// ─── Peer connected ───────────────────────────────────────────────────────────

void SnapRouter::onPeerConnected(const UserId &peerId)
{
    // Always use v3 sync — promote peer and send v3 state reconciliation.
    // If the peer is an old v2-only client they will simply ignore the v3 message
    // (unknown magic version) and we will see their v2 SyncReq arriving, at which
    // point we downgrade gracefully in handleV2Message.
    m_v3Peers.insert(peerId.value);
    sendV3Sync(peerId);
}

// ─── Inbound dispatch ─────────────────────────────────────────────────────────


// Store sender location from a received snap into the location cache.
// Called for every successfully decrypted inbound snap that carries coords.
static void cacheSnapSenderLocation(Storage::StorageManager *storage,
                                     const Snap &snap)
{
    if (!storage || !snap.hasLocation()) return;
    storage->upsertPeerLocation(
        snap.senderId.value,
        snap.locLat, snap.locLon, snap.locAccM,
        QString{},                            // SSID unknown from remote
        snap.sentAt.toMSecsSinceEpoch(),      // use snap send time as fix time
        QStringLiteral("snap"));
}

void SnapRouter::routeInbound(const QByteArray &encryptedPayload,
                               const UserId &senderId)
{
    constexpr int kNonceSize = 12;
    if (encryptedPayload.size() <= kNonceSize) {
        qCWarning(lcRouter) << "Payload too short from" << senderId.value;
        return;
    }

    const QByteArray nonce      = QByteArray::fromRawData(
        encryptedPayload.constData(), kNonceSize);
    const QByteArray ciphertext = QByteArray::fromRawData(
        encryptedPayload.constData() + kNonceSize,
        encryptedPayload.size() - kNonceSize);

    const auto dec = m_crypto->decryptFrom(senderId, ciphertext, nonce);
    if (!dec) {
        qCWarning(lcRouter) << "Decrypt failed from" << senderId.value
                            << ":" << dec.error();
        return;
    }

    QDataStream peek(dec->plaintext);
    peek.setByteOrder(QDataStream::BigEndian);
    quint32 magic = 0;
    quint8  version = 0;
    peek >> magic >> version;
    if (magic != kWireMagic) {
        qCWarning(lcRouter) << "Invalid magic from" << senderId.value;
        return;
    }

    if (version == kWireVersion5 || version == kWireVersion4 || version == kWireVersion3) {
        // Auto-promote peer to v3+ (videoDurationMs + tier handled inside).
        m_v3Peers.insert(senderId.value);
        handleV3Message(dec->plaintext, senderId);
        return;
    }
    if (version == kWireVersion2) {
        quint8 typeRaw = 0;
        peek >> typeRaw;
        handleV2Message(dec->plaintext, static_cast<MsgType>(typeRaw), senderId);
        return;
    }
    if (version == kWireVersion1) {
        handleInboundSnap(dec->plaintext, senderId);
        return;
    }

    qCWarning(lcRouter) << "Unknown wire version" << version << "from" << senderId.value;
}

// ─── v3 inbound ──────────────────────────────────────────────────────────────

void SnapRouter::handleV3Message(const QByteArray &plain, const UserId &senderId)
{
    QDataStream ds(plain);
    ds.setByteOrder(QDataStream::BigEndian);

    quint32 magic = 0;
    quint8  version = 0, flags = 0, reserved8 = 0;
    quint16 reserved16 = 0;
    ds >> magic >> version >> flags >> reserved8 >> reserved16;

    QString snapId, contentHash;
    ds >> snapId >> contentHash;

    quint32 seq = 0, ack = 0;
    ds >> seq >> ack;
    // mediaType + durationSecs (always present in v3 and v4)
    quint8  wireMediaType = 0;
    quint16 wireDuration  = 5;
    ds >> wireMediaType >> wireDuration;

    // v4: videoDurationMs immediately follows durationSecs. v3 messages
    // skip this field — older peers stay at 0 which is the default for
    // images, file snaps, and pre-v4 video snaps.
    qint32 wireVideoDurationMs = 0;
    if (version >= kWireVersion4) {
        ds >> wireVideoDurationMs;
    }

    // v5: tier byte (EncodeTier wire value) follows videoDurationMs.
    // v3/v4 messages skip — older peers stay at Standard (the only tier
    // their senders could emit).
    Core::Media::EncodeTier wireTier = Core::Media::EncodeTier::Standard;
    if (version >= kWireVersion5) {
        quint8 tierRaw = 0;
        ds >> tierRaw;
        wireTier = Core::Media::tierFromWire(tierRaw);
    }

    if (ds.status() != QDataStream::Ok) {
        qCWarning(lcRouter) << "v3/v4 header parse failed from" << senderId.value;
        return;
    }

    // Validate field lengths.
    constexpr int kMaxIdLen = 256;
    if (snapId.size() > kMaxIdLen || contentHash.size() > 128) {
        qCWarning(lcRouter) << "v3 field length exceeded from" << senderId.value;
        return;
    }

    // Update piggybacked ack — track highest seq the peer has received from us.
    if (ack > m_peerAckRecv.value(senderId.value, 0)) {
        m_peerAckRecv[senderId.value] = ack;
        // Bug 5 fix: flush pending v3 snaps that peer has now acknowledged.
        auto &pending = m_v3PendingAck[senderId.value];
        auto it = pending.begin();
        while (it != pending.end() && it.key() <= ack) {
            const Core::SnapId sid{ it.value() };
            m_storage->markSnapDelivered(sid);
            emit outboundDelivered(sid);
            qCInfo(lcRouter) << "v3 ACK confirmed delivery of snap" << it.value()
                             << "(seq" << it.key() << "<= ack" << ack << ")";
            it = pending.erase(it);
        }
    }

    // Record that sender has this hash if they announced it.
    if ((flags & F_ANNOUNCE) && !contentHash.isEmpty())
        insertCapped(m_peerKnownHashes[senderId.value], contentHash);

    if (flags & F_SYNC) {
        // Bug 6 fix: pass the already-positioned stream instead of re-parsing from scratch
        handleV3Sync(ds, senderId);
        // Sync may have fully handled the message.
        if (!(flags & F_DATA)) return;
    }

    if (flags & F_DATA) {
        // Full snap payload included — extract and store.
        QByteArray payload;
        ds >> payload;
        if (ds.status() != QDataStream::Ok || payload.isEmpty()) {
            qCWarning(lcRouter) << "v3 DATA payload missing/corrupt from" << senderId.value;
            return;
        }
        if (flags & F_DELTA) {
            // Delta decoding not yet implemented — fall back to treating as full.
            qCDebug(lcRouter) << "v3 DELTA received — treating as full (not yet supported)";
        }

        Snap snap;
        snap.id          = Core::SnapId{ snapId };
        snap.senderId    = senderId;
        snap.recipientId = m_selfUserId;
        snap.contentHash = sha256(payload);
        snap.sentAt      = QDateTime::currentDateTimeUtc();
        snap.expiresAt   = snapExpiryFrom(snap.sentAt, snap.mediaType);
        // Bug 4 fix: use values from wire header, not hardcoded defaults
        snap.mediaType    = static_cast<MediaType>(wireMediaType);
        snap.durationSecs = static_cast<int>(wireDuration);
        // v4 carries actual playback duration for video snaps; v3 leaves
        // this at 0 (we'd have to probe the file to recover it). Receiver
        // can still play the video either way; this is metadata for UI
        // (progress bar length, "X seconds" label).
        snap.videoDurationMs = static_cast<int>(wireVideoDurationMs);
        // v5: media-quality tier identifies which encode this snap is.
        // Same logical content may arrive multiple times at different
        // tiers (thumbnail → preview → standard). v3/v4 senders only
        // ever produced Standard so the default is correct for them.
        snap.tier = wireTier;

        if (snap.isExpired()) return;

        if (!writeMediaFile(snapId, payload, snap.localPath, snap.mediaType)) {
            // Disk write failed (no space / permissions / bad path). If we
            // proceed, saveSnap persists a snap with an EMPTY localPath: a live
            // view still works off encryptedBlob, but after a restart the blob is
            // cleared and the snap becomes permanently "Media file not found" —
            // the receiver effectively never got it. So DON'T save or ACK here.
            // Returning without recording the seq means no piggybacked ACK goes
            // back, so the sender's outbox keeps the snap and the SYNC
            // hash-reconciliation re-offers it on the next connection — giving
            // the disk a chance to recover (transient full/locked state).
            qCWarning(lcRouter) << "Inbound snap" << snapId
                                << "from" << senderId.value
                                << "— media write FAILED; not storing/ACKing so SYNC"
                                   " will re-deliver. Check device storage.";
            // This inbound frame carried F_ANNOUNCE, which already inserted
            // contentHash into m_peerKnownHashes[sender] above (~line 831). Left
            // there, our next SYNC would report this hash as KNOWN → the sender
            // would suppress re-delivery → the failed snap is lost forever.
            // Un-record it so SYNC correctly says we DON'T have it and the sender
            // re-offers on the next connection.
            if (!contentHash.isEmpty())
                m_peerKnownHashes[senderId.value].remove(contentHash);
            return;
        }
        snap.encryptedBlob = std::move(payload);
        // Guard: recipientId must be set (setSelfUserId called) before saving.
        // If it's empty the snap won't appear in loadInboxSnaps (WHERE recipient_id = ?).
        if (snap.recipientId.isNull()) {
            qCWarning(lcRouter) << "Inbound snap" << snapId
                                << "dropped: selfUserId not yet set";
            return;
        }
        m_storage->saveSnap(snap);

        // Piggybacked ACK in next outgoing message — no separate ACK needed.
        // Update our received seq for this sender.
        qCInfo(lcRouter) << "v3 DATA received for snap" << snapId
                         << "from" << senderId.value;
        cacheSnapSenderLocation(m_storage, snap);

        // Dedup: skip UI emission if we already processed this snap this session.
        // (Storage is already idempotent via INSERT OR REPLACE.)
        if (isRecentInbound(snap.id)) {
            qCDebug(lcRouter) << "Inbound snap" << snap.id.value
                              << "is duplicate — UI emission skipped";
        } else {
            rememberInbound(snap.id);
            emit inboundReady(std::move(snap));
        }
        return;
    }

    if ((flags & F_ANNOUNCE) && !(flags & F_DATA)) {
        // Announce-only: sender has data we may want.
        // Check if we already have it.
        if (!contentHash.isEmpty()) {
            const QString existingPath = m_storage->findBlobByHash(contentHash);
            if (!existingPath.isEmpty()) {
                // We have it — record and do nothing (ack comes piggyback).
                insertCapped(m_peerKnownHashes[m_selfUserId.value], contentHash);
            }
            // If we don't have it, it'll come via SYNC reconciliation.
            // In v3 we don't send explicit NEED — we let SYNC handle gaps.
        }
    }
}

void SnapRouter::handleV3Sync(QDataStream &ds, const UserId &senderId)
{
    // V32.34+: per-peer SYNC dedup window. If we processed a SYNC from
    // this peer within kSyncDedupMs (~250ms), drop this one silently.
    //
    // Why this matters: Phone A logs showed identical SYNC requests
    // arriving within ~5ms, each triggering a fresh re-send of every
    // undelivered snap. The duplicate request was almost certainly
    // an upstream issue (TCP retransmit + relay echo, or duplicate
    // socket-ready signal), but the COST landed on us: 2× the snap
    // send fanout, 2× the ANNOUNCE round trips, 2× the log noise.
    //
    // 250ms is short enough that a legitimate "user just got online,
    // and again 300ms later" sequence still triggers two SYNCs (those
    // are real signals of fresh interest), but long enough to absorb
    // the duplicate-delivery patterns we've observed.
    //
    // We drop SILENTLY — no log at info level — because the alternative
    // would log on every dedup hit and re-create the noise we were
    // trying to eliminate. A counter is the right way to surface this
    // if it ever needs visibility; deferred.
    constexpr qint64 kSyncDedupMs = 250;
    const qint64 nowMs = QDateTime::currentMSecsSinceEpoch();
    const qint64 lastMs = m_lastSyncHandledMs.value(senderId.value, 0);
    if (lastMs > 0 && (nowMs - lastMs) < kSyncDedupMs) {
        qCDebug(lcRouter) << "v3 SYNC from" << senderId.value
                          << "dropped — within dedup window ("
                          << (nowMs - lastMs) << "ms since last)";
        return;
    }
    m_lastSyncHandledMs.insert(senderId.value, nowMs);

    // Bug 6 fix: ds is already positioned past the v3 header (magic, version,
    // flags, reserved, snapId, contentHash, seq, ack, mediaType, durationSecs).
    // Read directly from the current position — no header re-parse.
    qint64 sinceMs = 0;
    quint32 hashCount = 0;
    ds >> sinceMs >> hashCount;

    constexpr quint32 kMaxHashes = 2000;
    if (hashCount > kMaxHashes) hashCount = kMaxHashes;

    QSet<QString> peerKnown;
    peerKnown.reserve(hashCount);
    for (quint32 i = 0; i < hashCount; ++i) {
        QString h;
        ds >> h;
        if (ds.status() != QDataStream::Ok) break;
        if (h.size() <= 128) peerKnown.insert(h);
    }

    // Merge into our record of what this peer knows.
    m_peerKnownHashes[senderId.value].unite(peerKnown);
    if (m_peerKnownHashes[senderId.value].size() > 500)
        m_peerKnownHashes[senderId.value].clear(); // overflow: restart dedup

    qCInfo(lcRouter) << "v3 SYNC from" << senderId.value
                     << "— peer knows" << peerKnown.size() << "hashes";

    // Find snaps we have for this peer that they don't know about.
    const QDateTime since = QDateTime::fromMSecsSinceEpoch(sinceMs, QTimeZone::utc());
    const auto undelivered = m_storage->undeliveredSnapIds(senderId, since);

    // Stagger snap sends by kSnapSendStaggerMs per snap to prevent UDP channel
    // overwrite: UdpChannel::sendPayload replaces any in-progress transfer,
    // so firing 100 sendV3() calls simultaneously means only the last snap
    // is actually delivered over UDP. WebSocket peers are unaffected (buffered),
    // but we stagger universally for simplicity and to avoid overwhelming the network.
    static constexpr int kSnapSendStaggerMs = 60; // 60ms between snaps
    static constexpr int kMaxSyncBurst      = 50; // cap to avoid 5s stagger for large backlogs

    int sent = 0;
    for (const SnapId &id : undelivered) {
        if (sent >= kMaxSyncBurst) break;

        const auto snap = m_storage->loadSnap(id);
        if (!snap) continue;

        const bool peerHasBlob = peerKnown.contains(snap->contentHash);
        const quint8 outFlags = peerHasBlob
            ? (F_ANNOUNCE | F_ACK)
            : (F_ANNOUNCE | F_DATA | F_ACK);

        QByteArray payload;
        if (!peerHasBlob && !snap->localPath.isEmpty()) {
            QFile snapFile(snap->localPath);
            if (snapFile.open(QIODevice::ReadOnly))
                payload = snapFile.readAll();
        }

        if (!peerHasBlob && payload.isEmpty()) continue;

        // Stagger: ANNOUNCE-only snaps send immediately (no UDP bulk transfer).
        // DATA snaps are spaced apart so UDP chunked transfers complete before
        // the next one starts. Last snap in the group has no delay.
        const int delayMs = peerHasBlob ? 0 : sent * kSnapSendStaggerMs;
        const UserId senderIdCopy = senderId;
        const QString snapIdVal   = snap->id.value;
        const QString snapHash    = snap->contentHash;
        QByteArray payloadCopy    = payload;
        // Carry media metadata on the wire so the receiver re-creates the snap
        // with the right mediaType/duration. Previously these defaulted to
        // image/5s on SYNC resend — videos arrived as broken images and view-
        // once timers were wrong.
        const quint8  mt   = static_cast<quint8>(snap->mediaType);
        const quint16 ds_  = static_cast<quint16>(qBound(0, snap->durationSecs, 65535));
        const qint32  vdms = static_cast<qint32>(snap->videoDurationMs);
        const auto    tier = snap->tier;

        if (delayMs == 0) {
            sendV3(senderId, outFlags, snapIdVal, snapHash, payloadCopy,
                   mt, ds_, vdms, tier);
            // Register ack mapping AFTER sendV3 so we read the seq that was actually used.
            // sendV3 increments m_peerSeq[peerId] before sending.
            if (!peerHasBlob) {
                m_peerKnownHashes[senderId.value].insert(snapHash);
                const quint32 sentSeq = m_peerSeq.value(senderId.value, 0);
                m_v3PendingAck[senderId.value][sentSeq] = snapIdVal;
            }
        } else {
            const QString snapHashCopy = snapHash;
            QTimer::singleShot(delayMs, this,
                [this, senderIdCopy, outFlags, snapIdVal, snapHashCopy,
                 payloadCopy, mt, ds_, vdms, tier]() {
                    sendV3(senderIdCopy, outFlags, snapIdVal, snapHashCopy,
                           payloadCopy, mt, ds_, vdms, tier);
                    // Register after sendV3 so seq is correct.
                    m_peerKnownHashes[senderIdCopy.value].insert(snapHashCopy);
                    const quint32 sentSeq = m_peerSeq.value(senderIdCopy.value, 0);
                    m_v3PendingAck[senderIdCopy.value][sentSeq] = snapIdVal;
                });
        }
        ++sent;
    }

    if (sent > 0)
        qCInfo(lcRouter) << "v3 SYNC: sent" << sent << "snap(s) to" << senderId.value;

    // Reply with our own sync state only if this is the FIRST sync from this peer
    // this session. Without this guard, each side re-sends SYNC in response to
    // the other's SYNC, creating an infinite ping-pong of empty sync messages.
    if (!m_syncRepliedTo.contains(senderId.value)) {
        m_syncRepliedTo.insert(senderId.value);
        sendV3Sync(senderId);
    }
}

// ─── v2 handlers (unchanged) ─────────────────────────────────────────────────

void SnapRouter::handleHaveOrNeed(const QString &snapId, bool recipientHas,
                                   const UserId &recipient)
{
    const auto it = m_pendingOffers.find(snapId);
    if (it == m_pendingOffers.end()) {
        // Bug 11 fix: offer blob timed out of memory but peer sent NEED.
        // Reload from storage localPath and re-send rather than silently dropping.
        if (!recipientHas) {
            const auto stored = m_storage->loadSnap(Core::SnapId{ snapId });
            if (stored && !stored->localPath.isEmpty()) {
                QFile f(stored->localPath);
                if (f.open(QIODevice::ReadOnly)) {
                    const QByteArray blob = f.readAll();
                    if (!blob.isEmpty()) {
                        qCInfo(lcRouter) << "NEED for expired offer" << snapId
                                         << "— reloading from disk and resending";
                        // Use sendV3() — NOT encryptAndSend(serializeSnap(...)).
                        // serializeSnap() produces snap-content format (kWireMagic),
                        // but routeInbound expects buildV3 transport format.
                        // Calling encryptAndSend(serializeSnap()) produces a frame
                        // the receiver cannot parse ("v3 header parse failed").
                        sendV3(recipient,
                               F_ANNOUNCE | F_DATA | F_ACK,
                               snapId,
                               stored->contentHash,
                               blob,
                               static_cast<quint8>(stored->mediaType),
                               static_cast<quint16>(
                                   qBound(0, stored->durationSecs, 65535)),
                               static_cast<qint32>(stored->videoDurationMs),
                               stored->tier);
                        // Track ACK so we know when delivery is confirmed.
                        const quint32 sentSeq = m_peerSeq.value(recipient.value, 0);
                        m_v3PendingAck[recipient.value][sentSeq] = snapId;
                        insertCapped(m_peerKnownHashes[recipient.value],
                                     stored->contentHash);
                        return;
                    }
                }
            }
            qCWarning(lcRouter) << "NEED for unknown offer (blob unavailable):" << snapId;
        } else {
            qCDebug(lcRouter) << "HAVE for unknown offer:" << snapId << "— already delivered";
        }
        return;
    }

    if (recipientHas) {
        qCInfo(lcRouter) << "HAVE: recipient has blob for" << snapId << "— no retransmit.";
        m_storage->markSnapDelivered(Core::SnapId{ snapId });
        emit outboundDelivered(Core::SnapId{ snapId });
        m_pendingOffers.erase(it);
        return;
    }

    qCInfo(lcRouter) << "NEED: sending full payload for" << snapId;
    const PendingOffer &offer = it.value();

    Snap stub;
    stub.id          = Core::SnapId{ snapId };
    stub.senderId    = m_selfUserId;
    stub.recipientId = recipient;
    if (const auto stored = m_storage->loadSnap(stub.id)) {
        stub.sentAt          = stored->sentAt;
        stub.mediaType       = stored->mediaType;
        stub.durationSecs    = stored->durationSecs;
        // Pull videoDurationMs from storage (now persisted via migration 11).
        stub.videoDurationMs = stored->videoDurationMs;
        // Pull tier from storage (persisted via migration 12).
        stub.tier            = stored->tier;
    }
    stub.encryptedBlob = offer.mediaBlob;

    // Use sendV3() — same fix as expired-offer re-send path.
    // encryptAndSend(serializeSnap()) produces snap-content format which
    // routeInbound cannot parse ("v3 header parse failed").
    const QString hash = offer.contentHash.isEmpty()
        ? m_storage->findBlobByHash(snapId) : offer.contentHash;
    sendV3(recipient,
           F_ANNOUNCE | F_DATA | F_ACK,
           snapId, hash, offer.mediaBlob,
           static_cast<quint8>(stub.mediaType),
           static_cast<quint16>(qBound(0, stub.durationSecs, 65535)),
           static_cast<qint32>(stub.videoDurationMs),
           stub.tier);
    const quint32 sentSeq = m_peerSeq.value(recipient.value, 0);
    m_v3PendingAck[recipient.value][sentSeq] = snapId;
    insertCapped(m_peerKnownHashes[recipient.value], hash);

    m_pendingOffers.erase(it);
}

void SnapRouter::handleV2Message(const QByteArray &plain, MsgType type,
                                  const UserId &senderId)
{
    QDataStream ds(plain);
    ds.setByteOrder(QDataStream::BigEndian);
    quint32 magic = 0;
    quint8  version = 0, typeRaw = 0;
    ds >> magic >> version >> typeRaw; // skip header

    switch (type) {

    case MsgType::Snap:
        handleInboundSnap(plain, senderId);
        break;

    case MsgType::Offer: {
        QString snapId, contentHash, fromId;
        qint64  sentAtMs = 0;
        qint32  mediaType = 0, durationSecs = 0;
        ds >> snapId >> contentHash >> fromId >> sentAtMs >> mediaType >> durationSecs;
        if (ds.status() != QDataStream::Ok) break;

        constexpr int kMaxIdLen = 256;
        if (snapId.size() > kMaxIdLen || contentHash.size() > 128
         || fromId.size() > kMaxIdLen) {
            qCWarning(lcRouter) << "OFFER field length exceeded from" << senderId.value;
            break;
        }

        const QString existingPath = m_storage->findBlobByHash(contentHash);
        if (!existingPath.isEmpty()) {
            Snap snap;
            snap.id           = Core::SnapId{ snapId };
            snap.senderId     = senderId;
            snap.recipientId  = m_selfUserId;
            snap.sentAt       = QDateTime::fromMSecsSinceEpoch(sentAtMs, QTimeZone::utc());
            snap.expiresAt    = snapExpiryFrom(snap.sentAt, snap.mediaType);
            snap.mediaType    = static_cast<MediaType>(mediaType);
            snap.durationSecs = static_cast<int>(durationSecs);
            snap.localPath    = existingPath;
            snap.contentHash  = contentHash;
            m_storage->saveSnap(snap);
            cacheSnapSenderLocation(m_storage, snap);
            // Dedup UI emission (storage already idempotent)
            if (isRecentInbound(snap.id)) {
                qCDebug(lcRouter) << "Duplicate inbound snap" << snap.id.value
                                  << "(ANNOUNCE path) — UI emission skipped";
            } else {
                rememberInbound(snap.id);
                emit inboundReady(std::move(snap));
            }
            encryptAndSend(senderId, buildHave(snapId, contentHash));
            qCInfo(lcRouter) << "HAVE sent for" << snapId << "(dedup)";
        } else {
            encryptAndSend(senderId, buildNeed(snapId));
            qCDebug(lcRouter) << "NEED sent for" << snapId;
        }
        break;
    }

    case MsgType::Have: {
        QString snapId, contentHash;
        ds >> snapId >> contentHash;
        if (ds.status() != QDataStream::Ok || snapId.size() > 256) break;
        handleHaveOrNeed(snapId, true, senderId);
        break;
    }

    case MsgType::Need: {
        QString snapId;
        ds >> snapId;
        if (ds.status() != QDataStream::Ok || snapId.size() > 256) break;
        handleHaveOrNeed(snapId, false, senderId);
        break;
    }

    case MsgType::Ack: {
        QString snapId;
        ds >> snapId;
        if (ds.status() != QDataStream::Ok || snapId.size() > 256) break;
        m_storage->markSnapDelivered(Core::SnapId{ snapId });
        qCInfo(lcRouter) << "ACK: snap" << snapId << "delivered to" << senderId.value;
        break;
    }

    case MsgType::SyncReq: {
        qint64 sinceMs = 0;
        ds >> sinceMs;
        if (ds.status() != QDataStream::Ok) break;

        const qint64 thirtyDaysAgoMs =
            QDateTime::currentDateTimeUtc().addDays(-30).toMSecsSinceEpoch();
        if (sinceMs < thirtyDaysAgoMs) sinceMs = thirtyDaysAgoMs;
        const QDateTime since = QDateTime::fromMSecsSinceEpoch(sinceMs, QTimeZone::utc());

        const auto snapIds = m_storage->undeliveredSnapIds(senderId, since);
        QList<Snap> snaps;
        snaps.reserve(qMin(static_cast<int>(snapIds.size()), 1000));
        for (const auto &id : snapIds) {
            if (snaps.size() >= 1000) break;
            if (auto s = m_storage->loadSnap(id)) snaps.append(*s);
        }
        encryptAndSend(senderId, buildSyncList(snaps));
        qCInfo(lcRouter) << "SYNC_LIST sent:" << snaps.size() << "snap(s) to" << senderId.value;
        break;
    }

    case MsgType::SyncList: {
        quint32 count = 0;
        ds >> count;
        constexpr quint32 kMaxSyncItems = 1000;
        if (count > kMaxSyncItems) count = kMaxSyncItems;

        for (quint32 i = 0; i < count; ++i) {
            QString snapId, contentHash;
            ds >> snapId >> contentHash;
            if (ds.status() != QDataStream::Ok) break;

            if (m_storage->loadSnap(Core::SnapId{ snapId })) {
                encryptAndSend(senderId, buildAck(snapId));
            } else if (!m_storage->findBlobByHash(contentHash).isEmpty()) {
                encryptAndSend(senderId, buildHave(snapId, contentHash));
            } else {
                encryptAndSend(senderId, buildNeed(snapId));
            }
        }
        break;
    }

    default:
        // Unknown message type — likely a peer running a newer wire format
        // OR a malformed/malicious packet. Rate-limit logging so an attacker
        // can't fill our logs by spamming unknown types.
        {
            static qint64 s_lastUnknownLogMs = 0;
            static int s_unknownSince = 0;
            ++s_unknownSince;
            const qint64 now = QDateTime::currentMSecsSinceEpoch();
            if (now - s_lastUnknownLogMs > 30'000) {
                s_lastUnknownLogMs = now;
                qCWarning(lcRouter) << "Unknown MsgType" << typeRaw
                                    << "from" << senderId.value
                                    << "(" << s_unknownSince
                                    << "occurrences in last 30s)";
                s_unknownSince = 0;
            }
            break;
        }

    } // switch
}

// ─── Full inbound snap (v1/v2) ────────────────────────────────────────────────

void SnapRouter::handleInboundSnap(const QByteArray &plain, const UserId &senderId)
{
    Snap snap;
    QByteArray mediaBytes;
    if (!deserializeSnap(plain, snap, mediaBytes)) {
        qCWarning(lcRouter) << "Deserialise failed from" << senderId.value;
        return;
    }

    snap.senderId    = senderId;
    snap.recipientId = m_selfUserId;
    snap.contentHash = sha256(mediaBytes);

    if (snap.isExpired()) {
        qCWarning(lcRouter) << "Expired snap from" << senderId.value;
        return;
    }

    if (!mediaBytes.isEmpty())
        writeMediaFile(snap.id.value, mediaBytes, snap.localPath, snap.mediaType);

    snap.encryptedBlob = std::move(mediaBytes);
    m_storage->saveSnap(snap);

    encryptAndSend(senderId, buildAck(snap.id.value));
    // Dedup UI emission (storage already idempotent)
    if (isRecentInbound(snap.id)) {
        qCDebug(lcRouter) << "Duplicate inbound snap" << snap.id.value
                          << "(v1/v2 path) — UI emission skipped";
    } else {
        rememberInbound(snap.id);
        emit inboundReady(std::move(snap));
    }
}



void SnapRouter::setSelfUserId(const UserId &id)
{
    m_selfUserId = id;
}


// ─── Inbound deduplication ────────────────────────────────────────────────────
// When a peer reconnects after a brief offline window, it may re-send snaps
// it sent us before disconnect (the piggyback ACK we'd normally send hadn't
// reached them yet). Storage uses INSERT OR REPLACE so duplicate writes are
// no-ops, but inboundReady would emit twice and the UI would show the snap
// twice. This bounded cache holds the last N snap IDs we processed; if a
// duplicate arrives, we still update storage but skip the UI emission.

bool SnapRouter::isRecentInbound(const SnapId &id) const
{
    return m_recentInbound.contains(id.value);
}

void SnapRouter::rememberInbound(const SnapId &id)
{
    m_recentInbound.append(id.value);
    while (m_recentInbound.size() > kInboundDedupCacheMax)
        m_recentInbound.removeFirst();
}

} // namespace SnapClone::Core
