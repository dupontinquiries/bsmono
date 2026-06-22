#pragma once
#include <QtMath>

// ─── Core/Types.hpp ──────────────────────────────────────────────────────────
//
// Canonical value types shared across every module.
//
// Rules:
//   • All types are value-semantic (copyable, movable, equality-comparable).
//   • No Qt-specific types here — this header must remain portable for tests.
//   • Use strong typedefs (via Q_DECLARE_METATYPE or explicit structs) instead
//     of raw primitives so the compiler catches accidental misuse.
// ─────────────────────────────────────────────────────────────────────────────

#include <cmath>
#include <QString>
#include <QUrl>
#include <QDateTime>
#include <QByteArray>
#include <QUuid>
#include <optional>
#include <chrono>

#include "core/EncodeTier.hpp"

namespace SnapClone::Core {

// ─── Identity ────────────────────────────────────────────────────────────────

/// Opaque user identifier — never use raw strings for user IDs.
struct UserId {
    QString value;

    bool operator==(const UserId &) const = default;
    [[nodiscard]] bool isNull() const { return value.isEmpty(); }
    [[nodiscard]] static UserId generate() { return { QUuid::createUuid().toString(QUuid::WithoutBraces) }; }
};

/// Opaque snap (message) identifier.
struct SnapId {
    QString value;
    bool operator==(const SnapId &) const = default;
    [[nodiscard]] static SnapId generate() { return { QUuid::createUuid().toString(QUuid::WithoutBraces) }; }
};

/// Opaque device/peer identifier used in the P2P layer.
struct PeerId {
    QString value;
    bool operator==(const PeerId &) const = default;
    [[nodiscard]] static PeerId generate() { return { QUuid::createUuid().toString(QUuid::WithoutBraces) }; }
};

// ─── Media ───────────────────────────────────────────────────────────────────

/// Type of payload in a Snap.
///
/// Image       — JPEG/PNG byte blob in encryptedBlob (current default)
/// Video       — H.264/MP4 byte blob; plays inline with the same view-once timer
/// Text        — UTF-8 text caption; encryptedBlob contains the bytes
/// Link        — URL + optional preview metadata; renders as rich preview card
/// Location    — pure geo-snap, no media. Just lat/lon/accuracy + optional caption.
enum class MediaType {
    Image,
    Video,
    Text,
    Link,
    Location,
};

/// A snap payload.  The actual bytes may be stored on disk; this struct
/// carries the metadata plus an optional in-memory blob for small payloads.
struct Snap {
    SnapId       id;
    UserId       senderId;
    UserId       recipientId;
    MediaType    mediaType    = MediaType::Image;
    QString      localPath;             ///< path to decrypted media file
    QByteArray   encryptedBlob;         ///< raw media bytes (plain after decrypt; named for historical reasons)
    QDateTime    sentAt;
    QDateTime    expiresAt;             ///< zero-value means "no expiry"
    bool         viewed       = false;
    int          durationSecs = 5;

    /// V32.42+: user kept this snap (manual save or monthly export). Kept snaps
    /// are exempt from time-based expiry/purge — they persist until explicitly
    /// deleted, so the auto-disappear retention can coexist with preservation.
    bool         kept         = false;

    /// SHA-256 hex of the raw media bytes.  Empty for legacy snaps.
    /// Used for deduplication: if the peer already has a blob with this hash
    /// we send only metadata (SyncMessage::Offer) not the full payload.
    QString contentHash;

    /// When the recipient acknowledged receipt (UTC ISO string).
    /// Empty until the far end sends a SyncMessage::Ack.
    QDateTime deliveredAt;

    /// Optional sender location at time of capture.
    /// Null (NaN) when sender chose not to share or had no GPS fix.
    double locLat = qQNaN();   ///< WGS-84 decimal degrees, NaN = not set
    double locLon = qQNaN();
    double locAccM = 0.0;      ///< horizontal accuracy in metres

    [[nodiscard]] bool hasLocation() const
    {
        return !qIsNaN(locLat) && !qIsNaN(locLon);
    }

    [[nodiscard]] bool isExpired() const
    {
        // Kept snaps never expire — the user (or the monthly export) chose to
        // preserve them.
        return !kept && expiresAt.isValid()
               && QDateTime::currentDateTimeUtc() > expiresAt;
    }

    /// Conversation grouping key — the "other party" in this 1:1 thread.
    /// Returns the senderId if `selfId` matches the recipient (inbound),
    /// or the recipientId if `selfId` matches the sender (outbound).
    /// If neither matches (transit snap, debug log inspection, etc.)
    /// returns senderId as a fallback so the key is still well-defined.
    ///
    /// Used by the inbox to group snaps by conversation thread. Future
    /// group chats would store a separate groupId field; this helper
    /// covers the 1:1 case only.
    ///
    /// Why this isn't just `senderId`: outbound snaps in the user's
    /// inbox view should appear in the same thread as inbound snaps
    /// from the same person. Without this normalization, an outbound
    /// snap to Alice would group under self.id and an inbound snap
    /// from Alice would group under Alice.id, splitting the thread.
    [[nodiscard]] UserId conversationKey(const UserId &selfId) const
    {
        if (recipientId.value == selfId.value) return senderId;     // inbound
        if (senderId.value    == selfId.value) return recipientId;  // outbound
        return senderId;  // fallback for malformed/transit snaps
    }

    // ─── Optional payload metadata (extensions) ──────────────────────────────
    // These fields are zero/empty for legacy Image snaps. They're only populated
    // when mediaType matches.

    /// User-supplied caption overlay (any media type). UTF-8, ≤ 200 chars
    /// after trimming. Rendered as bottom overlay on Image/Video, as title
    /// on Link, as primary text on Location.
    QString caption;

    /// Video-only: duration of the source video in milliseconds.
    /// Used to show progress in the viewer. 0 = unknown.
    int videoDurationMs = 0;

    /// Video-only: width/height of the source video. 0 = unknown.
    int videoWidth  = 0;
    int videoHeight = 0;

    /// Media-quality tier this snap was encoded at. Default Standard for
    /// legacy snaps and call sites unaware of tiering. Sent on the wire
    /// (v5+) so the receiver knows which tier of a multi-tier delivery
    /// this is. The receiver may overwrite a lower-tier snap with a
    /// higher-tier arrival of the same logical content, keyed by snap id.
    Media::EncodeTier tier = Media::EncodeTier::Standard;

    /// Link-only: the URL the user wants to share. Stored separately
    /// from encryptedBlob so we can render previews without decoding.
    QUrl  linkUrl;

    /// Link-only: optional Open Graph metadata harvested from the URL.
    /// linkTitle/linkDescription show in the preview card; linkImageBlob
    /// is a small JPEG thumbnail (< 32 KB) the sender pre-fetched.
    QString    linkTitle;
    QString    linkDescription;
    QByteArray linkImageBlob;

    [[nodiscard]] bool hasCaption()  const { return !caption.trimmed().isEmpty(); }
    [[nodiscard]] bool hasVideo()    const { return mediaType == MediaType::Video; }
    [[nodiscard]] bool hasLink()     const { return mediaType == MediaType::Link && linkUrl.isValid(); }
    [[nodiscard]] bool isLocationSnap() const { return mediaType == MediaType::Location; }
};

// ─── Location ────────────────────────────────────────────────────────────────

/// Last known position of a peer (or self), cached locally.
/// TTL: caller must check recordedAt — anything older than 5 minutes is stale.
struct PeerLocation {
    QString   peerId;
    double    lat        = 0.0;   ///< WGS-84 decimal degrees
    double    lon        = 0.0;
    double    accuracyM  = 0.0;   ///< horizontal accuracy in metres (0 = unknown)
    QString   ssid;               ///< WiFi SSID when fix was taken
    QDateTime recordedAt;         ///< UTC timestamp of the fix
    QString   source;             ///< "gps" | "snap"

    /// Straight-line distance in metres to another location (Haversine).
    [[nodiscard]] double distanceTo(double toLat, double toLon) const;

    /// Human-readable age: "just now", "2 min ago", etc.
    [[nodiscard]] QString ageString() const;

    [[nodiscard]] bool isValid() const { return !peerId.isEmpty(); }

    /// Great-circle distance to another point in kilometres (haversine formula).
    /// Returns 0 if either point is unset (both lat and lon == 0 is treated as unset).
    [[nodiscard]] double distanceKmTo(double otherLat, double otherLon) const
    {
        if (lat == 0.0 && lon == 0.0) return 0.0;
        if (otherLat == 0.0 && otherLon == 0.0) return 0.0;
        constexpr double kEarthRadiusKm = 6371.0;
        constexpr double kPi = 3.14159265358979323846;
        const double dLat = (otherLat - lat) * kPi / 180.0;
        const double dLon = (otherLon - lon) * kPi / 180.0;
        const double a = std::sin(dLat / 2.0) * std::sin(dLat / 2.0)
                       + std::cos(lat * kPi / 180.0) * std::cos(otherLat * kPi / 180.0)
                       * std::sin(dLon / 2.0) * std::sin(dLon / 2.0);
        return kEarthRadiusKm * 2.0 * std::atan2(std::sqrt(a), std::sqrt(1.0 - a));
    }

    /// Human-readable distance string ("450 m", "1.2 km", "12 km").
    [[nodiscard]] QString distanceString(double otherLat, double otherLon) const
    {
        const double km = distanceKmTo(otherLat, otherLon);
        if (km <= 0.0) return {};
        if (km < 1.0)  return QStringLiteral("%1 m").arg(qRound(km * 1000.0));
        if (km < 10.0) return QStringLiteral("%1 km").arg(km, 0, 'f', 1);
        return QStringLiteral("%1 km").arg(qRound(km));
    }
};

// ─── User ────────────────────────────────────────────────────────────────────

struct UserProfile {
    UserId  id;
    QString displayName;
    QString avatarPath;
    bool    online = false;
};

/// Persistent relationship state for a contact.
/// Pending  = connectByDhtId was called; peer has never successfully connected.
/// Known    = at least one successful peerConnected event recorded.
enum class ContactState : int {
    Pending = 0,   ///< Searching / never connected
    Known   = 1,   ///< Connected at least once
};

/// A permanent address-book entry for a peer.
/// nickname is user-chosen; displayName is what the peer sends in their SHAK.
/// sc_id is their stable DHT ID (sc-XXXXXXXX); vcard_uid is the UID from
/// an imported vCard (used for deduplication on re-import).
struct Contact {
    UserId       id;                               ///< Stable DHT ID used for routing
    QString      nickname;                         ///< User-chosen local name (shown in UI)
    QString      displayName;                      ///< Name the peer advertises (from SHAK)
    QString      notes;                            ///< Free-form notes
    QString      avatarPath;
    QString      vcardUid;                         ///< vCard UID for import/export dedup
    bool         online      = false;
    ContactState state       = ContactState::Pending;
    QDateTime    lastSeen;                         ///< UTC of last successful peerConnected

    /// The name to display: nickname if set, otherwise displayName or id.
    [[nodiscard]] QString label() const {
        if (!nickname.isEmpty())    return nickname;
        if (!displayName.isEmpty()) return displayName;
        return id.value;
    }
};

// ─── Network Addressing ──────────────────────────────────────────────────────

/// The address of a peer in the distributed network.
struct PeerAddress {
    QString host;
    quint16 port = 0;
    bool    tls  = false;   ///< true → wss://, false → ws://

    [[nodiscard]] QString toString() const {
        const QString scheme = tls ? QStringLiteral("wss") : QStringLiteral("ws");
        return QStringLiteral("%1://%2:%3").arg(scheme, host).arg(port);
    }

    bool operator==(const PeerAddress &o) const noexcept {
        return host == o.host && port == o.port && tls == o.tls;
    }
    bool operator!=(const PeerAddress &o) const noexcept { return !(*this == o); }
};

// ─── V32.31: location-source taxonomy ────────────────────────────────────────
//
// A peer/self location can come from multiple signal sources, each with
// its own confidence/recency/accuracy profile. We track which is the
// authoritative source for any given fix so the UI can show provenance
// ("this fix is from GPS" vs "from a landmark match") and so resolution
// algorithms can weight signals appropriately.
//
// String mapping (for storage / wire compat with V32.x and earlier):
//   Unknown   -> ""        (legacy rows pre-V32.31)
//   Gps       -> "gps"      (LocationManager fresh fix)
//   Manual    -> "manual"   (QSettings location/manualLat+Lon)
//   Snap      -> "snap"     (sender attached location to outbound snap)
//   Geocoded  -> "geocoded" (future: Nominatim address picker)
//   Network   -> "network"  (future: WiFi BSSID lookup, MLS replacement)
//   WiFi      -> "wifi"     (current SSID matches a known {ssid,lat,lon})
//   Landmark  -> "landmark" (matched a stored Landmark by SSID/pHash/edge)
//   Derived   -> "derived"  (pHash-matched a prior snap with known loc)
enum class LocationSource {
    Unknown = 0,
    Gps,
    Manual,
    Snap,
    Geocoded,
    Network,
    WiFi,
    Landmark,
    Derived,
};

/// String round-trip helpers. Implementation lives in LocationManager.cpp
/// (NOT inline here) to keep Types.hpp's include footprint small —
/// Types.hpp is included transitively by hundreds of TUs and inline-
/// definitions here pull QStringLiteral + switch machinery into every
/// one of them. V32.31 had these inline initially; moved out after a
/// build cascade made it clear that bloating Types.hpp is high-risk.
[[nodiscard]] QString        locationSourceToString(LocationSource s);
[[nodiscard]] LocationSource locationSourceFromString(const QString &s);

// ─── V32.31: Landmark — user-tagged places ───────────────────────────────────
//
// A Landmark is a user-curated record of "this place is at these coords"
// that can later help the app derive location when GPS isn't available.
//
// Privacy posture: landmarks are LOCAL ONLY in V32.31. The schema
// includes fields needed for future DHT publish (signature, scope,
// geohash) but no announce/discover code runs in this version. Going
// public is a deliberate later step that requires the privacy review
// to land first.
//
// Photo handling: the original photo is NEVER stored in the landmark
// itself or shared. Two transforms-of-the-photo are computed instead:
//   • photoBlurHash: 8-byte perceptual hash of a heavily-blurred 64x64
//     version. Matches "is this the same building silhouette?" while
//     destroying text/faces/license plates.
//   • photoEdgeHash: 4-byte binary fingerprint of a Sobel edge map.
//     Discards color/texture, keeps line geometry. Matches the
//     same-alley-different-day case.
// Both may be all-zeros if the user added the landmark without a photo.
//
// Scope governs eventual DHT behaviour (Phase 2):
//   • Private   -> never shared
//   • Contacts  -> shared peer-to-peer with confirmed contacts only
//   • Public    -> announced on DHT (with explicit user confirm + warning)
enum class LandmarkScope {
    Private  = 0,
    Contacts = 1,
    Public   = 2,
};

struct Landmark {
    QString    id;              ///< sc-landmark-<uuid>
    QString    creatorId;       ///< sc-... (owner's stable identity)
    QString    displayName;     ///< user-visible label, e.g. "My apartment"
    double     lat       = 0.0; ///< WGS-84 degrees
    double     lon       = 0.0;
    double     accuracyM = 0.0; ///< horizontal accuracy reported at capture

    /// Geohash precision ~7 chars (~76m cell). Used as DHT lookup key
    /// when (and only when) scope=Public. Coarseness is deliberate —
    /// prevents fingerprinting "this exact house" from a query.
    QString    geohash7;

    /// Optional associated network. Captured at landmark creation;
    /// used at resolution time to match "I'm on the same WiFi → I'm
    /// probably at this landmark." ssid is plaintext (user-visible);
    /// bssidHash is a SHA-256 truncated to 8 bytes (privacy: don't
    /// leak the raw MAC).
    QString    ssid;
    QByteArray bssidHash;       ///< 8 bytes; empty = no associated network

    /// Photo-derived fingerprints. See class header.
    QByteArray photoBlurHash;   ///< 8 bytes; empty = no photo
    QByteArray photoEdgeHash;   ///< 4 bytes; empty = no photo

    LandmarkScope scope     = LandmarkScope::Private;
    qint64     createdAtMs  = 0;

    /// V32.31: signature reserved for Phase 2 DHT publish. Ed25519
    /// over a canonical serialization of all fields above. Empty in
    /// V32.31 local-only landmarks; populated when publish path lands.
    QByteArray signature;

    [[nodiscard]] bool hasPhoto() const {
        return !photoBlurHash.isEmpty() || !photoEdgeHash.isEmpty();
    }
    [[nodiscard]] bool hasNetwork() const { return !bssidHash.isEmpty(); }
    [[nodiscard]] bool isValid() const   { return !id.isEmpty(); }

    /// Great-circle distance in metres from this landmark to (lat,lon).
    /// Returns 0 for invalid inputs (both this->lat and this->lon zero).
    ///
    /// Inlined here (not in LocationManager.cpp where PeerLocation's
    /// matching helper lives) because StorageManager::loadLandmarksNear
    /// in the storage lib references it, and the storage lib does not
    /// link against snapclone_core — putting the body in core would
    /// produce an undefined-reference at storage's link step.
    [[nodiscard]] inline double distanceMTo(double otherLat, double otherLon) const
    {
        // Treat (0,0) as "unset" — matches PeerLocation::distanceKmTo convention.
        // Plain comparison rather than qFuzzyIsNull to avoid pulling extra
        // Qt headers / overload resolution into every TU that includes
        // this header — a previous attempt with qFuzzyIsNull here was
        // suspected of triggering a cascade of template/ambiguous-overload
        // errors across the build.
        if (lat == 0.0 && lon == 0.0) return 0.0;
        constexpr double R = 6371000.0; // Earth radius in metres
        const double phi1 = qDegreesToRadians(lat);
        const double phi2 = qDegreesToRadians(otherLat);
        const double dPhi = qDegreesToRadians(otherLat - lat);
        const double dLam = qDegreesToRadians(otherLon - lon);
        const double a    = qSin(dPhi/2)*qSin(dPhi/2)
                          + qCos(phi1)*qCos(phi2)*qSin(dLam/2)*qSin(dLam/2);
        return R * 2.0 * qAtan2(qSqrt(a), qSqrt(1.0 - a));
    }
};

} // namespace SnapClone::Core

// Allow use in QVariant / signal-slot across thread boundaries.
Q_DECLARE_METATYPE(SnapClone::Core::UserId)
Q_DECLARE_METATYPE(SnapClone::Core::SnapId)
Q_DECLARE_METATYPE(SnapClone::Core::PeerId)
Q_DECLARE_METATYPE(SnapClone::Core::Snap)
Q_DECLARE_METATYPE(SnapClone::Core::UserProfile)
Q_DECLARE_METATYPE(SnapClone::Core::PeerAddress)
Q_DECLARE_METATYPE(SnapClone::Core::Landmark)
Q_DECLARE_METATYPE(SnapClone::Core::LandmarkScope)
Q_DECLARE_METATYPE(SnapClone::Core::LocationSource)
