#pragma once

// ─── Storage/StorageManager.hpp ──────────────────────────────────────────────
//
// Manages all local persistence via SQLite (through Qt SQL).
//
// Schema overview:
//   snaps(id TEXT PK, sender_id TEXT, recipient_id TEXT, media_type INT,
//         local_path TEXT, sent_at TEXT, expires_at TEXT, viewed INT,
//         duration_secs INT, content_hash TEXT, delivered_at TEXT)
//
//   users(id TEXT PK, display_name TEXT, avatar_path TEXT, online INT,
//         nickname TEXT, notes TEXT, sc_id TEXT, vcard_uid TEXT)
//
//   peer_addresses(user_id TEXT, host TEXT, port INT [PK composite],
//                  last_seen TEXT, fail_count INT)
//
//   dht_cache(peer_id TEXT, host TEXT, port INT [PK composite],
//             cached_at TEXT, fail_count INT)
//
//   peer_public_keys(peer_id TEXT PK, public_key BLOB, updated_at TEXT)
//
// Design notes:
//   • Schema migrations are applied in sequence; the version is stored in
//     the "user_version" PRAGMA.  Never alter an existing migration — append.
//   • All write operations are wrapped in transactions for atomicity.
//   • Read operations return values by value (not references) so the caller
//     does not need to hold a lock.
//   • I/O-heavy queries should be called via QtConcurrent::run; this class
//     itself is not thread-safe beyond QSqlDatabase's thread affinity rules.
// ─────────────────────────────────────────────────────────────────────────────

#include <QObject>
#include <QSqlDatabase>
#include <optional>
#include <QList>
#include <QHash>   // V32.34+: loadConversationLastMeta
#include <QSet>    // V32.34+: loadAllSnapIds
#include <QPair>
#include "core/Types.hpp"

namespace SnapClone::Storage {

class StorageManager : public QObject
{
    Q_OBJECT

public:
    explicit StorageManager(const QString &dbPath, QObject *parent = nullptr);
    ~StorageManager() override;

    /// Open the database and run any pending migrations.
    [[nodiscard]] bool open();

    // ── Snap CRUD ─────────────────────────────────────────────────────────
    bool           saveSnap(const Core::Snap &snap);
    bool           markSnapViewed(const Core::SnapId &id);
    /// V32.42+: keep/un-keep a snap. Keeping sets kept=1 and clears expires_at
    /// so the snap is exempt from auto-expiry/purge (manual save + monthly
    /// export). Returns false on DB error.
    bool           markSnapKept(const Core::SnapId &id, bool kept = true);
    /// V32.42+: keep every not-yet-kept snap (clears their expiry). Called after
    /// a successful monthly export. Returns the number affected.
    int            keepAllSnaps();
    bool           deleteSnap(const Core::SnapId &id);
    [[nodiscard]] std::optional<Core::Snap>  loadSnap(const Core::SnapId &id) const;
    [[nodiscard]] QList<Core::Snap>          loadInboxSnaps(const Core::UserId &userId) const;
    [[nodiscard]] QList<Core::Snap>          loadOutboxSnaps(const Core::UserId &userId) const;

    // ── Conversation grouping (V32.15) ────────────────────────────────────
    //
    // ConversationSummary surfaces per-thread metadata for the future
    // inbox-grouping UI. A "conversation" is the set of snaps where the
    // OTHER party (not self) is the same UserId — see Snap::conversationKey.
    //
    // The structure is intentionally narrow: just the fields the inbox
    // would need to render a collapsed thread row (other party id, most-
    // recent snap timestamp, unread count, total count). To display
    // individual snaps within a thread, callers fetch them on demand
    // via loadConversationSnaps() — keeps the summary list cheap to load
    // even with thousands of historical snaps.
    //
    // The flat loadInboxSnaps/loadOutboxSnaps API is preserved for the
    // current flat UI; the new methods are additive. When the UI is
    // updated to group, it can switch over without a schema migration —
    // these queries run against the existing snaps table.
    struct ConversationSummary {
        Core::UserId otherParty;     // the not-self UserId in this thread
        QDateTime    lastSnapAt;     // sentAt of most recent snap (in or out)
        int          unreadCount;    // inbound + !viewed
        int          totalCount;     // inbound + outbound, expired or not
        bool         hasOutbound;    // at least one snap sent by self
        bool         hasInbound;     // at least one snap received from other
    };

    /// Returns one row per distinct conversation partner for `selfId`.
    /// Sorted by lastSnapAt DESC — most-recent threads first, which is
    /// what an inbox-style grouped UI typically wants.
    /// Excludes expired snaps from counts (matches the existing inbox
    /// behavior where expired entries don't appear in the list).
    [[nodiscard]] QList<ConversationSummary>
        loadConversations(const Core::UserId &selfId) const;

    /// Returns all snaps in a single thread (both directions, oldest-first).
    /// `otherParty` is the not-self UserId of the conversation. Excludes
    /// expired snaps. Intended for thread-detail UI later; not used by
    /// the current flat inbox.
    [[nodiscard]] QList<Core::Snap>
        loadConversationSnaps(const Core::UserId &selfId,
                              const Core::UserId &otherParty) const;

    // V32.34+ EFFICIENCY: two single-query helpers that let the inbox rebuild
    // its state WITHOUT the per-conversation N+1 (previously
    // reloadConversationsFromStorage did one loadConversationSnaps PER
    // conversation just to read the last snap's media type/direction and to
    // populate the dedup id-set — loading every snap of every thread).

    /// Per-conversation last-snap metadata in ONE query: maps partner UserId
    /// → { mediaType, isOutbound } for the most-recent non-expired snap in
    /// each thread. Lets the grouped inbox render row previews without
    /// fetching whole threads.
    struct ConversationLastMeta {
        Core::MediaType mediaType  = Core::MediaType::Image;
        bool            isOutbound = false;
    };
    [[nodiscard]] QHash<QString, ConversationLastMeta>
        loadConversationLastMeta(const Core::UserId &selfId) const;

    /// Lightweight snap "stubs" for selfId (non-expired) in ONE query: each
    /// carries only the fields the inbox's in-memory dedup + expiry bookkeeping
    /// needs — id, senderId, recipientId, viewed — NOT media bytes, paths,
    /// hashes, durations, or locations. Used to seed the inbox dedup set so a
    /// later live duplicate is recognized AND a restored snap's expiry can
    /// still compute its conversation key / inbound-ness correctly.
    [[nodiscard]] QList<Core::Snap>
        loadSnapStubs(const Core::UserId &selfId) const;

    // ── User / friend CRUD ────────────────────────────────────────────────
    bool           saveUser(const Core::UserProfile &profile);
    bool           saveContact(const Core::Contact &contact);
    [[nodiscard]] std::optional<Core::UserProfile> loadUser(const Core::UserId &id) const;
    [[nodiscard]] QList<Core::UserProfile>         loadFriends(const Core::UserId &selfId) const;
    [[nodiscard]] std::optional<Core::Contact>        loadContact(const Core::UserId &id) const;
    /// Delete a contact and any associated peer-address rows.
    /// Snaps to/from this contact are NOT deleted (they have their own lifecycle).
    /// Returns true if the contact existed and was removed; false otherwise.
    bool           deleteContact(const Core::UserId &id);
    [[nodiscard]] QList<Core::Contact>             loadContacts(const Core::UserId &selfId) const;

    /// V32.27: update users.last_connected_at for `id` to now (ms-since-
    /// epoch). Called from AppController whenever a peer session reaches
    /// the established state. Distinct from `last_seen_at` (touched on
    /// any presence signal) — this fires only on real session completion
    /// and feeds future staleness-based eviction policies. No-op if the
    /// row doesn't exist (e.g. peer was deleted between events).
    void           touchLastConnected(const Core::UserId &id);

    // ── Landmarks (V32.31: user-tagged places) ────────────────────────────
    /// Insert or replace a landmark. Returns false on DB error.
    bool           saveLandmark(const Core::Landmark &lm);
    /// Look up by id; returns nullopt if not found.
    [[nodiscard]] std::optional<Core::Landmark> loadLandmark(const QString &id) const;
    /// All landmarks (any scope, any creator). Used by UI for the
    /// "My landmarks" list. Order: created_at_ms DESC (newest first).
    [[nodiscard]] QList<Core::Landmark>         loadAllLandmarks() const;
    /// Landmarks within `radiusM` of the given point. Cheap geohash-prefix
    /// pre-filter, then exact Haversine. Returns sorted by distance.
    /// Caller must already have a fix to query this.
    [[nodiscard]] QList<Core::Landmark>         loadLandmarksNear(
        double lat, double lon, double radiusM) const;
    /// Delete by id. Returns true if a row was removed.
    bool           deleteLandmark(const QString &id);

    // ── Peer addresses ────────────────────────────────────────────────────
    bool           savePeerAddress(const Core::UserId &id, const Core::PeerAddress &addr);
    [[nodiscard]] std::optional<Core::PeerAddress>  loadPeerAddress(const Core::UserId &id) const;
    [[nodiscard]] QList<Core::PeerAddress>          loadAllPeerAddresses(const Core::UserId &id) const;
    void           markPeerAddressFailed(const Core::UserId &id, const Core::PeerAddress &addr);
    void           clearPeerAddress(const Core::UserId &id); ///< Mark all addresses exhausted → DHT lookup.

    // ── Peer public keys (for offline snap encryption) ────────────────────
    bool           savePeerPublicKey(const Core::UserId &id, const QByteArray &pubKey);
    [[nodiscard]] QByteArray loadPeerPublicKey(const Core::UserId &id) const;
    /// Returns all stored peer public keys. Used at startup to restore session keys.
    [[nodiscard]] QList<QPair<Core::UserId, QByteArray>> loadAllPeerPublicKeys() const;

    // ── V32.33: TOFU on peer pubkey changes (CRYPTO_REVIEW.md S-20) ───────
    //
    // When an inbound SHAK presents a public key different from the one
    // we already have stored for that peer ID, we do NOT silently replace
    // it (that's the persistent-MITM vector). Instead we record the
    // attempted change in `pending_key_changes` and leave the existing
    // pubkey untouched. The user reviews the pending change in Settings
    // and either accepts it (legitimate reinstall) or rejects it (likely
    // attack). On accept, the new pubkey is promoted to peer_public_keys;
    // on reject, the row is just deleted (and the same attacker will
    // retrigger the same prompt next SHAK — by design).
    //
    // Schema: pending_key_changes(peer_id PK, new_public_key BLOB,
    //         old_public_key BLOB, detected_at TEXT,
    //         attempt_count INTEGER DEFAULT 1)
    //
    // `attempt_count` increments if the same new pubkey keeps showing up —
    // useful UX signal ("this attempt has been seen 12 times").

    /// Records that an inbound SHAK presented `newKey` for `id`, which
    /// differs from the stored pubkey (`oldKey`). If an identical pending
    /// row exists (same peer + same new key), bumps attempt_count instead
    /// of duplicating. Returns true on success.
    bool recordPendingKeyChange(const Core::UserId &id,
                                const QByteArray &newKey,
                                const QByteArray &oldKey);

    /// Returns all pending key changes for user review.
    struct PendingKeyChange {
        Core::UserId  peerId;
        QByteArray    newKey;
        QByteArray    oldKey;
        QString       detectedAt;  // ISO-8601 from sqlite
        int           attemptCount = 1;
    };
    [[nodiscard]] QList<PendingKeyChange> loadPendingKeyChanges() const;

    /// User accepted the new pubkey. Atomically: move new key to
    /// peer_public_keys (replacing), delete the pending row. Returns true
    /// on success.
    bool acceptPendingKeyChange(const Core::UserId &id);

    /// User rejected the new pubkey. Just deletes the pending row. The
    /// existing peer_public_keys entry is untouched (real Alice's key
    /// stays valid). If the same attacker SHAKs again, a new pending row
    /// will be created. Returns true on success.
    bool rejectPendingKeyChange(const Core::UserId &id);

    /// Returns true iff there is a pending key change for this peer.
    /// Used by the SHAK handler to dedupe: if there's already a pending
    /// change for sc-XXXX presenting key K, and the same K shows up
    /// again, recordPendingKeyChange handles it via attempt_count.
    /// But callers may want to skip work entirely if they know one is
    /// pending.
    [[nodiscard]] bool hasPendingKeyChange(const Core::UserId &id) const;

    /// Wipe peer_addresses, users (except self), and all snaps. Then VACUUM.
    /// Does NOT delete the DB file — schema stays intact for next launch.
    void purgeAllData(const Core::UserId &selfId);

    // ── DHT address cache (1-hour TTL) ───────────────────────────────────
    void cacheDhtResult(const Core::UserId &id, const Core::PeerAddress &addr);
    void markDhtAddressFailed(const Core::UserId &id, const Core::PeerAddress &addr);
    [[nodiscard]] QList<Core::PeerAddress> loadDhtCache(
        const Core::UserId &id, int maxAgeMinutes = 60) const;
    [[nodiscard]] QList<QPair<Core::PeerAddress,int>> loadDhtCacheWithAge(
        const Core::UserId &id, int maxAgeMinutes = 60) const;
    [[nodiscard]] QStringList listRecentPeers(int maxAgeMinutes = 120) const;
    void purgeDhtCache(int olderThanMinutes = 60);
    void purgeDhtCacheForPeer(const Core::UserId &id);

    // ── Maintenance ───────────────────────────────────────────────────────
    /// Remove all snaps that have passed their expiry date.
    int purgeExpiredSnaps();

    // ── Sync / dedup ──────────────────────────────────────────────────────
    [[nodiscard]] QString       findBlobByHash(const QString &contentHash) const;
    bool                        markSnapDelivered(const Core::SnapId &id);
    [[nodiscard]] QList<Core::SnapId> undeliveredSnapIds(
                                    const Core::UserId &recipientId,
                                    const QDateTime    &sinceTime) const;

    // ── Peer location cache ───────────────────────────────────────────────
    void upsertPeerLocation(const QString &peerId,
                            double lat, double lon, double accuracyM,
                            const QString &ssid, qint64 recordedAtMs,
                            const QString &source);
    [[nodiscard]] std::optional<Core::PeerLocation>
        loadPeerLocation(const QString &peerId, qint64 maxAgeMs = 0) const;
    void purgeStaleLocations(qint64 maxAgeMs);

signals:
    void snapSaved(SnapClone::Core::SnapId id);
    void snapDeleted(SnapClone::Core::SnapId id);

private:
    void applyMigrations();
    void repairSchema();
    void applyMigration(int version, const QString &sql);

    QSqlDatabase m_db;
    QString      m_connectionName; // unique per instance — prevents Qt duplicate connection warning
    int          m_currentVersion = 0;
    QString      m_dbPath;

    static constexpr int kTargetSchemaVersion = 9; // must match kMigrations.last().first
};

} // namespace SnapClone::Storage