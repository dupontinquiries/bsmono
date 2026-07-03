#include "storage/StorageManager.hpp"

#include <QSettings>
#include <QSqlQuery>
#include <QSqlError>
#include <vector>
#include <QLoggingCategory>
#include <QVariant>
#include <QFile>
#include <QTimeZone>
#include <QRegularExpression>

Q_LOGGING_CATEGORY(lcDb, "snapclone.storage")

namespace SnapClone::Storage {

// ── Privacy-safe timestamp helpers ───────────────────────────────────────────
// Always store UTC. QDateTime::toString(Qt::ISODate) on a local-time value
// embeds the timezone offset (e.g. "+05:30"), leaking the user's location.
// These helpers enforce UTC-only storage throughout.

static QString toUtcIso(const QDateTime &dt) {
    if (!dt.isValid()) return {};
    return dt.toUTC().toString(Qt::ISODate);
}

static QDateTime fromUtcIso(const QString &s) {
    if (s.isEmpty()) return {};
    QDateTime dt = QDateTime::fromString(s, Qt::ISODate);
    if (!dt.isValid()) dt = QDateTime::fromString(s, Qt::ISODateWithMs);
    if (dt.isValid() && dt.timeSpec() == Qt::LocalTime)
        dt = dt.toUTC();  // migrate any legacy local-time strings on read
    return dt;
}

// Round lastSeen to the nearest 5-minute bucket before storage.
// Precise lastSeen timestamps fingerprint the user's presence pattern;
// 5-minute buckets preserve "online this morning" while hiding exact time.
static QString toPrivacyLastSeen(const QDateTime &dt) {
    if (!dt.isValid()) return {};
    const qint64 ms     = dt.toUTC().toMSecsSinceEpoch();
    const qint64 bucket = (ms / 300000LL) * 300000LL;  // 5-minute buckets
    return QDateTime::fromMSecsSinceEpoch(bucket, QTimeZone::utc())
               .toString(Qt::ISODate);
}

// ─── Schema migrations ────────────────────────────────────────────────────────
// Each entry maps a version number to the SQL to apply.
// NEVER modify existing entries — always append new ones.

static const QList<QPair<int, QString>> kMigrations = {
    {
        1,
        QStringLiteral(R"(
            CREATE TABLE IF NOT EXISTS snaps (
                id           TEXT PRIMARY KEY,
                sender_id    TEXT NOT NULL,
                recipient_id TEXT NOT NULL,
                media_type   INTEGER NOT NULL DEFAULT 0,
                local_path   TEXT NOT NULL DEFAULT '',
                sent_at      TEXT NOT NULL,
                expires_at   TEXT NOT NULL DEFAULT '',
                viewed       INTEGER NOT NULL DEFAULT 0,
                duration_sec INTEGER NOT NULL DEFAULT 5
            );

            CREATE TABLE IF NOT EXISTS users (
                id           TEXT PRIMARY KEY,
                display_name TEXT NOT NULL,
                avatar_path  TEXT NOT NULL DEFAULT '',
                online       INTEGER NOT NULL DEFAULT 0
            );

            CREATE TABLE IF NOT EXISTS peer_addresses (
                user_id TEXT PRIMARY KEY,
                host    TEXT NOT NULL,
                port    INTEGER NOT NULL
            );
        )")
    },
    {
        2,
        QStringLiteral(R"(
            CREATE INDEX IF NOT EXISTS idx_snaps_recipient_sent
                ON snaps (recipient_id, sent_at);
            CREATE INDEX IF NOT EXISTS idx_snaps_sender_sent
                ON snaps (sender_id, sent_at);
            CREATE INDEX IF NOT EXISTS idx_snaps_expires
                ON snaps (expires_at)
                WHERE expires_at != '';
        )")
    },
    {
        3,
        QStringLiteral(R"(
            -- Content hash for deduplication: SHA-256 of the raw media bytes.
            -- Two snaps with the same hash share identical media — the receiver
            -- only needs the bytes once, even if sent to multiple people or
            -- retransmitted after a connection drop.
            ALTER TABLE snaps ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';

            -- Tracks when the snap was confirmed received by the far end.
            -- Set when the recipient sends an acknowledgement back via the sync protocol.
            ALTER TABLE snaps ADD COLUMN delivered_at TEXT NOT NULL DEFAULT '';

            -- Index for dedup lookups: "do I already have a blob with this hash?"
            CREATE INDEX IF NOT EXISTS idx_snaps_content_hash
                ON snaps (content_hash)
                WHERE content_hash != '';
        )")
    },
    {
        4,
        QStringLiteral(R"(
            -- Add last_seen + fail_count to peer_addresses.
            -- NOTE: migration 1 already created peer_addresses (user_id, host,
            -- port) WITHOUT these columns, so a "CREATE TABLE IF NOT EXISTS"
            -- here silently no-ops and the columns never appear — which then
            -- made the index below fail with "no such column: last_seen". We
            -- ALTER-add instead; applyMigration() skips an ADD COLUMN
            -- idempotently when the column already exists (older DBs that were
            -- repaired via repairSchema, or partially-migrated DBs).
            -- Use a constant default ('') here: SQLite's support for a
            -- non-constant default like datetime('now') on ADD COLUMN varies by
            -- version, whereas the original CREATE could use it. New rows get a
            -- real timestamp from the INSERT path; existing rows back-fill ''.
            ALTER TABLE peer_addresses ADD COLUMN last_seen TEXT NOT NULL DEFAULT '';
            ALTER TABLE peer_addresses ADD COLUMN fail_count INTEGER NOT NULL DEFAULT 0;
            CREATE INDEX IF NOT EXISTS idx_peer_addr_user
                ON peer_addresses (user_id, last_seen DESC);
        )")
    },
    {
        5,
        QStringLiteral(R"(
            ALTER TABLE users ADD COLUMN nickname TEXT NOT NULL DEFAULT '';
            ALTER TABLE users ADD COLUMN notes TEXT NOT NULL DEFAULT '';
            ALTER TABLE users ADD COLUMN sc_id TEXT NOT NULL DEFAULT '';
            ALTER TABLE users ADD COLUMN vcard_uid TEXT NOT NULL DEFAULT '';
        )")
    },
    {
        6,
        QStringLiteral(R"(
            CREATE TABLE IF NOT EXISTS dht_cache (
                peer_id   TEXT    NOT NULL,
                host      TEXT    NOT NULL,
                port      INTEGER NOT NULL,
                cached_at TEXT    NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (peer_id, host, port)
            );
            CREATE INDEX IF NOT EXISTS idx_dht_cache_peer
                ON dht_cache (peer_id, cached_at DESC);
        )")
    },
    {
        7,
        QStringLiteral(R"(
            CREATE TABLE IF NOT EXISTS peer_public_keys (
                peer_id    TEXT PRIMARY KEY,
                public_key BLOB NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
        )")
    },
    {
        8,
        QStringLiteral(R"(
            -- Add fail_count to dht_cache so stale addresses are deprioritized
            -- within their TTL window rather than retried blindly.
            ALTER TABLE dht_cache ADD COLUMN fail_count INTEGER NOT NULL DEFAULT 0;

            -- Index to prefer fresh, unfailed addresses in loadDhtCache.
            CREATE INDEX IF NOT EXISTS idx_dht_cache_quality
                ON dht_cache (peer_id, fail_count ASC, cached_at DESC);

            -- Reset persisted online flag on all contacts: the flag is session-only
            -- and should never be true at startup (peerConnected sets it live).
            UPDATE users SET online = 0;
        )")
    },
    {
        9,
        QStringLiteral(R"(
            -- contact_state: 0=Pending (searched, never connected), 1=Known (connected ≥ once).
            -- Pending contacts stay visible across app restarts under "Connecting" group.
            ALTER TABLE users ADD COLUMN contact_state INTEGER NOT NULL DEFAULT 0;

            -- last_seen_at: UTC ISO timestamp of last successful peerConnected event.
            -- Shown in UI as "last seen X ago" for offline Known contacts.
            ALTER TABLE users ADD COLUMN last_seen_at TEXT NOT NULL DEFAULT '';
        )")
    },
    {
        10,
        QStringLiteral(
            "CREATE TABLE IF NOT EXISTS peer_locations ("
            "  peer_id TEXT PRIMARY KEY,"
            "  lat REAL NOT NULL,"
            "  lon REAL NOT NULL,"
            "  accuracy_m REAL NOT NULL DEFAULT 0,"
            "  ssid TEXT NOT NULL DEFAULT '',"
            "  recorded_at INTEGER NOT NULL,"
            "  source TEXT NOT NULL DEFAULT 'gps'"
            ");"
            "CREATE INDEX IF NOT EXISTS idx_peer_locations_recorded"
            "  ON peer_locations (recorded_at);"
            "ALTER TABLE snaps ADD COLUMN loc_lat REAL;"
            "ALTER TABLE snaps ADD COLUMN loc_lon REAL;"
            "ALTER TABLE snaps ADD COLUMN loc_acc REAL;")
    },
    {
        11,
        // video_duration_ms: actual playback duration of video snaps in
        // milliseconds. Distinct from duration_sec (view-once timer).
        // Without this column, video snaps lose their playback duration on
        // app restart (the field is set on send but never persisted).
        QStringLiteral(
            "ALTER TABLE snaps ADD COLUMN video_duration_ms INTEGER NOT NULL DEFAULT 0;")
    },
    {
        12,
        // tier: media-quality tier (EncodeTier enum, see core/EncodeTier.hpp).
        // Default 2 = Standard — matches the only tier pre-v32.17 senders
        // could emit. Stored as INTEGER for compactness and so future tiers
        // (HighQuality+, Lossless, etc.) need no schema change. Receiver
        // currently overwrites lower-tier with higher-tier arrivals of the
        // same snap id (last-write-wins); a true (snap_id, tier) compound-
        // key model is deferred until tiered delivery is end-to-end wired.
        QStringLiteral(
            "ALTER TABLE snaps ADD COLUMN tier INTEGER NOT NULL DEFAULT 2;")
    },
    {
        13,
        // V32.27: last_connected_at on users. Tracks the most recent
        // moment we successfully completed a session with this peer.
        // Used for staleness-based eviction policies (e.g. "drop
        // contacts we haven't talked to in 90 days" — pending user
        // opt-in). Distinct from last_seen_at (added in migration 9)
        // which is touched whenever we observe the peer alive at all
        // (e.g. via gossip beacon). last_connected_at is stricter:
        // only fires after the SHAK handshake completes and the
        // session is established.
        //
        // Stored as INTEGER (ms-since-epoch) for cheap comparison
        // without parsing — last_seen_at was added as TEXT before we
        // settled on a consistent timestamp format. Default 0 means
        // "never connected" which is correct for existing rows.
        QStringLiteral(
            "ALTER TABLE users ADD COLUMN last_connected_at INTEGER NOT NULL DEFAULT 0;")
    },
    {
        14,
        // V32.31: landmarks table. User-tagged places that survive
        // restart and (later, Phase 2) can be published on the DHT.
        //
        // Privacy posture: scope defaults to 0 (Private). The schema
        // includes signature + geohash + scope columns now so Phase 2
        // DHT publish needs no migration; V32.31 simply doesn't read
        // or write the DHT side.
        //
        // bssid_hash + photo_blur_hash + photo_edge_hash stored as BLOBs
        // (not TEXT) because they're binary fingerprints, not strings.
        // SQLite handles BLOB IS NULL semantics naturally; empty BLOB
        // is distinct from NULL but we treat both as "absent" in code.
        //
        // No FK from creator_id → users.id deliberately. The landmark
        // creator might not be in our users table (e.g. discovered via
        // DHT from a stranger in Phase 2). Foreign-key constraint
        // would prevent ingesting valid third-party landmarks.
        //
        // Composite index on (geohash7, lat, lon) for "find landmarks
        // near me" queries. Geohash prefix narrows to ~76m cell; the
        // lat/lon predicate does the precise distance filter in code.
        QStringLiteral(
            "CREATE TABLE IF NOT EXISTS landmarks ("
            "  id              TEXT PRIMARY KEY NOT NULL,"
            "  creator_id      TEXT NOT NULL,"
            "  display_name    TEXT NOT NULL DEFAULT '',"
            "  lat             REAL NOT NULL,"
            "  lon             REAL NOT NULL,"
            "  accuracy_m      REAL NOT NULL DEFAULT 0,"
            "  geohash7        TEXT NOT NULL DEFAULT '',"
            "  ssid            TEXT NOT NULL DEFAULT '',"
            "  bssid_hash      BLOB,"
            "  photo_blur_hash BLOB,"
            "  photo_edge_hash BLOB,"
            "  scope           INTEGER NOT NULL DEFAULT 0,"
            "  created_at_ms   INTEGER NOT NULL,"
            "  signature       BLOB"
            ");"
            "CREATE INDEX IF NOT EXISTS idx_landmarks_geohash"
            "  ON landmarks(geohash7);"
            "CREATE INDEX IF NOT EXISTS idx_landmarks_creator"
            "  ON landmarks(creator_id);")
    },
    {
        15,
        // V32.42+: "kept" flag on snaps — the keep/save exemption from expiry.
        // A snap with kept=1 is never auto-purged regardless of expires_at, and
        // isExpired() reports false for it. Set when the user manually saves a
        // snap or when the monthly export writes it out, so that "chats disappear
        // after a while" can coexist with "save to preserve moments". Default 0
        // (existing rows keep their normal expiry).
        QStringLiteral(
            "ALTER TABLE snaps ADD COLUMN kept INTEGER NOT NULL DEFAULT 0;")
    }
};

// ─── Construction ─────────────────────────────────────────────────────────────

StorageManager::StorageManager(const QString &dbPath, QObject *parent)
    : QObject(parent)
    , m_dbPath(dbPath)
{}

StorageManager::~StorageManager()
{
    if (m_db.isOpen()) {
        m_db.close();
    }
    // Remove the connection from Qt's global registry so the name
    // can be reused (important for tests that create many instances).
    m_db = QSqlDatabase(); // reset to invalid before removeDatabase
    if (!m_connectionName.isEmpty())
        QSqlDatabase::removeDatabase(m_connectionName);
}

// ─── Open / migrate ───────────────────────────────────────────────────────────

bool StorageManager::open()
{
    // Use a unique connection name per instance so multiple StorageManager
    // objects (e.g. in unit tests that create/destroy one per test case)
    // don't collide on the same "snapclone_db" name and trigger Qt's
    // "duplicate connection" warning which causes the second open() to
    // silently reuse the old (possibly deleted) connection.
    m_connectionName = QStringLiteral("snapclone_db_%1")
                           .arg(reinterpret_cast<quintptr>(this), 0, 16);
    m_db = QSqlDatabase::addDatabase(QStringLiteral("QSQLITE"), m_connectionName);
    m_db.setDatabaseName(m_dbPath);

    if (!m_db.open()) {
        qCCritical(lcDb) << "Cannot open database:" << m_db.lastError().text();
        return false;
    }

    // DELETE journal + synchronous=FULL: guaranteed fsync on every commit.
    QSqlQuery pragma(m_db);
    pragma.exec(QStringLiteral("PRAGMA journal_mode=DELETE;"));
    pragma.exec(QStringLiteral("PRAGMA synchronous=FULL;"));
    pragma.exec(QStringLiteral("PRAGMA foreign_keys=ON;"));
    pragma.exec(QStringLiteral("PRAGMA cache_size=-8192;"));
    pragma.exec(QStringLiteral("PRAGMA temp_store=MEMORY;"));
    pragma.exec(QStringLiteral("PRAGMA busy_timeout=5000;"));

    applyMigrations();
    repairSchema();

    // synchronous=FULL with DELETE journal — no WAL to checkpoint.

    return true;
}

static bool columnExists(const QSqlDatabase &db,
                         const QString &table, const QString &column)
{
    // PRAGMA table_info does not support bound parameters — the table name
    // is interpolated directly. Validate against the known schema tables
    // to prevent SQL injection if this function were ever called with
    // untrusted input.
    static const QStringList kKnownTables = {
        QStringLiteral("snaps"),
        QStringLiteral("users"),
        QStringLiteral("peer_addresses"),
        QStringLiteral("dht_cache"),
        QStringLiteral("peer_public_keys"),
        QStringLiteral("peer_locations"),
    };
    if (!kKnownTables.contains(table)) {
        qCCritical(lcDb) << "columnExists: unknown table name:" << table;
        return false;
    }
    QSqlQuery q(db);
    q.prepare(QStringLiteral("PRAGMA table_info(%1)").arg(table));
    if (!q.exec()) return false;
    while (q.next()) {
        if (q.value(1).toString().compare(column, Qt::CaseInsensitive) == 0)
            return true;
    }
    return false;
}

void StorageManager::repairSchema()
{
    // Belt-and-suspenders: ensure every expected column exists regardless of
    // migration state. Handles DBs stuck at old versions after partial failures.
    QSqlQuery q(m_db);

    // Simple scalar columns — safe to add with ALTER TABLE.
    struct Col { const char *table; const char *col; const char *def; };
    static const std::vector<Col> kCols = {
        { "snaps", "content_hash", "TEXT NOT NULL DEFAULT ''" },
        { "snaps", "delivered_at", "TEXT NOT NULL DEFAULT ''" },
        { "users", "nickname",     "TEXT NOT NULL DEFAULT ''" },
        { "users", "notes",        "TEXT NOT NULL DEFAULT ''" },
        { "users", "sc_id",        "TEXT NOT NULL DEFAULT ''" },
        { "users", "vcard_uid",    "TEXT NOT NULL DEFAULT ''" },
    };
    for (const auto &c : kCols) {
        if (!columnExists(m_db, QLatin1String(c.table), QLatin1String(c.col))) {
            const QString sql = QStringLiteral("ALTER TABLE %1 ADD COLUMN %2 %3")
                .arg(QLatin1String(c.table), QLatin1String(c.col), QLatin1String(c.def));
            if (!q.exec(sql))
                qCWarning(lcDb) << "repairSchema: failed to add" << c.col
                                << "to" << c.table << ":" << q.lastError().text();
            else
                qCInfo(lcDb) << "repairSchema: added" << c.col << "to" << c.table;
        }
    }

    // dht_cache: create if missing (migration 6). It has no old-schema variant.
    q.exec(QStringLiteral(
        "CREATE TABLE IF NOT EXISTS dht_cache ("
        " peer_id TEXT NOT NULL, host TEXT NOT NULL, port INTEGER NOT NULL,"
        " cached_at TEXT NOT NULL DEFAULT (datetime('now')),"
        " PRIMARY KEY (peer_id, host, port))"));
    q.exec(QStringLiteral(
        "CREATE INDEX IF NOT EXISTS idx_dht_cache_peer"
        " ON dht_cache (peer_id, cached_at DESC)"));

    // contact_state + last_seen_at: added in migration 9.
    // last_connected_at: added in migration 13 (V32.27).
    static const std::vector<Col> kContactCols = {
        { "users", "contact_state",     "INTEGER NOT NULL DEFAULT 0" },
        { "users", "last_seen_at",      "TEXT NOT NULL DEFAULT ''"   },
        { "users", "last_connected_at", "INTEGER NOT NULL DEFAULT 0" },
    };
    for (const auto &c : kContactCols) {
        if (!columnExists(m_db, QLatin1String(c.table), QLatin1String(c.col))) {
            const QString sql = QStringLiteral("ALTER TABLE %1 ADD COLUMN %2 %3")
                .arg(QLatin1String(c.table), QLatin1String(c.col), QLatin1String(c.def));
            if (!q.exec(sql))
                qCWarning(lcDb) << "repairSchema: failed to add" << c.col
                                << ":" << q.lastError().text();
        }
    }

    // loc_ columns on snaps: added in migration 10.
    static const std::vector<Col> kLocCols = {
        { "snaps", "loc_lat", "REAL" },
        { "snaps", "loc_lon", "REAL" },
        { "snaps", "loc_acc", "REAL" },
    };
    for (const auto &c : kLocCols) {
        if (!columnExists(m_db, QLatin1String(c.table), QLatin1String(c.col))) {
            const QString sql = QStringLiteral("ALTER TABLE %1 ADD COLUMN %2 %3")
                .arg(QLatin1String(c.table), QLatin1String(c.col), QLatin1String(c.def));
            if (!q.exec(sql))
                qCWarning(lcDb) << "repairSchema: failed to add" << c.col << ":" << q.lastError().text();
        }
    }
    // peer_locations table: create if missing (migration 10).
    q.exec(QStringLiteral(
        "CREATE TABLE IF NOT EXISTS peer_locations ("
        " peer_id TEXT PRIMARY KEY, lat REAL NOT NULL, lon REAL NOT NULL,"
        " accuracy_m REAL NOT NULL DEFAULT 0, ssid TEXT NOT NULL DEFAULT '',"
        " recorded_at INTEGER NOT NULL, source TEXT NOT NULL DEFAULT 'gps')"));
    q.exec(QStringLiteral(
        "CREATE INDEX IF NOT EXISTS idx_peer_locations_recorded"
        " ON peer_locations (recorded_at)"));

    // Video columns: added in migration 11.
    static const std::vector<Col> kVideoCols = {
        { "snaps", "encoded_duration_ms", "INTEGER NOT NULL DEFAULT 0" },
        { "snaps", "thumbnail_path",      "TEXT NOT NULL DEFAULT ''"   },
        { "snaps", "has_audio",           "INTEGER NOT NULL DEFAULT 0" },
        { "snaps", "is_looping",          "INTEGER NOT NULL DEFAULT 0" },
        // tier: added in migration 12.
        { "snaps", "tier",                "INTEGER NOT NULL DEFAULT 2" },
    };
    for (const auto &c : kVideoCols) {
        if (!columnExists(m_db, QLatin1String(c.table), QLatin1String(c.col))) {
            const QString sql = QStringLiteral("ALTER TABLE %1 ADD COLUMN %2 %3")
                .arg(QLatin1String(c.table), QLatin1String(c.col), QLatin1String(c.def));
            if (!q.exec(sql))
                qCDebug(lcDb) << "repairSchema: video col" << c.col << "skip:" << q.lastError().text();
        }
    }

    // dht_cache.fail_count: added in migration 8.
    if (!columnExists(m_db, QStringLiteral("dht_cache"), QStringLiteral("fail_count"))) {
        q.exec(QStringLiteral(
            "ALTER TABLE dht_cache ADD COLUMN fail_count INTEGER NOT NULL DEFAULT 0"));
        q.exec(QStringLiteral(
            "CREATE INDEX IF NOT EXISTS idx_dht_cache_quality"
            " ON dht_cache (peer_id, fail_count ASC, cached_at DESC)"));
    }

    // peer_public_keys: create if missing (migration 7).
    q.exec(QStringLiteral(
        "CREATE TABLE IF NOT EXISTS peer_public_keys ("
        " peer_id TEXT PRIMARY KEY, public_key BLOB NOT NULL,"
        " updated_at TEXT NOT NULL DEFAULT (datetime('now')))"));

    // V32.33: pending_key_changes — see StorageManager.hpp doc comment.
    // Used by the SHAK TOFU check to record attempted pubkey replacements
    // without applying them. Reviewed by the user in Settings.
    q.exec(QStringLiteral(
        "CREATE TABLE IF NOT EXISTS pending_key_changes ("
        " peer_id TEXT PRIMARY KEY,"
        " new_public_key BLOB NOT NULL,"
        " old_public_key BLOB NOT NULL,"
        " detected_at TEXT NOT NULL DEFAULT (datetime('now')),"
        " attempt_count INTEGER NOT NULL DEFAULT 1)"));

    // peer_addresses: rebuild if EITHER the fail_count column is missing (old
    // pre-v4 schema) OR the PRIMARY KEY is not the composite (user_id, host,
    // port). The latter is the common case on DBs migrated through the buggy
    // migration-4 (its CREATE TABLE IF NOT EXISTS no-op'd, so fail_count got
    // ALTER-added later but the single-column `user_id` PK from migration 1
    // survived) — which breaks savePeerAddress()'s ON CONFLICT upsert. This
    // rebuild runs OUTSIDE the migration transaction (each stmt on its own
    // query), so it avoids the "table is locked" failure a migration-time
    // rebuild hits.
    bool peerAddrNeedsRebuild =
        !columnExists(m_db, QStringLiteral("peer_addresses"),
                      QStringLiteral("fail_count"));
    if (!peerAddrNeedsRebuild) {
        // Inspect the PK: PRAGMA table_info marks PK columns with pk>0. A correct
        // composite PK has THREE columns flagged (user_id, host, port). The old
        // schema flags only one (user_id).
        QSqlQuery pk(m_db);
        int pkCols = 0;
        if (pk.exec(QStringLiteral("PRAGMA table_info(peer_addresses)"))) {
            while (pk.next())
                if (pk.value(5).toInt() > 0) ++pkCols; // col 5 = pk position
        }
        if (pkCols != 3) {
            peerAddrNeedsRebuild = true;
            qCInfo(lcDb) << "repairSchema: peer_addresses PK has" << pkCols
                         << "column(s), expected 3 — rebuilding for composite PK";
        }
    }
    if (peerAddrNeedsRebuild) {
        qCInfo(lcDb) << "repairSchema: rebuilding peer_addresses with fail_count";
        // Capture whether the CURRENT table has last_seen/fail_count BEFORE we
        // rename it — columnExists() validates against a fixed table allowlist
        // and won't accept the transient "_old" name.
        const bool oldHasExtraCols =
            columnExists(m_db, QStringLiteral("peer_addresses"),
                         QStringLiteral("fail_count"));
        q.exec(QStringLiteral("DROP TABLE IF EXISTS peer_addresses_old"));
        q.exec(QStringLiteral("ALTER TABLE peer_addresses RENAME TO peer_addresses_old"));
        q.exec(QStringLiteral(
            "CREATE TABLE peer_addresses ("
            " user_id TEXT NOT NULL, host TEXT NOT NULL, port INTEGER NOT NULL,"
            " last_seen TEXT NOT NULL DEFAULT (datetime('now')),"
            " fail_count INTEGER NOT NULL DEFAULT 0,"
            " PRIMARY KEY (user_id, host, port))"));
        // Preserve last_seen/fail_count when the old table had them (post-v4
        // DBs); otherwise copy only the original three columns (pre-v4 DBs).
        if (oldHasExtraCols) {
            q.exec(QStringLiteral(
                "INSERT OR IGNORE INTO peer_addresses (user_id, host, port, last_seen, fail_count)"
                " SELECT user_id, host, port, last_seen, fail_count FROM peer_addresses_old"));
        } else {
            q.exec(QStringLiteral(
                "INSERT OR IGNORE INTO peer_addresses (user_id, host, port)"
                " SELECT user_id, host, port FROM peer_addresses_old"));
        }
        q.exec(QStringLiteral("DROP TABLE IF EXISTS peer_addresses_old"));
        q.exec(QStringLiteral(
            "CREATE INDEX IF NOT EXISTS idx_peer_addr_user"
            " ON peer_addresses (user_id, last_seen DESC)"));
    }
}

void StorageManager::applyMigrations()
{
    QSqlQuery verQuery(m_db);
    verQuery.exec(QStringLiteral("PRAGMA user_version;"));
    if (verQuery.next()) {
        m_currentVersion = verQuery.value(0).toInt();
    }
    qCDebug(lcDb) << "Schema version:" << m_currentVersion;

    for (const auto &[version, sql] : kMigrations) {
        if (version > m_currentVersion) {
            applyMigration(version, sql);
        }
    }
}

/// Returns true if a column exists in the given table.

void StorageManager::applyMigration(int version, const QString &sql)
{
    qCInfo(lcDb) << "Applying migration to version" << version;

    // V32.33+ Phase 1 audit: check transaction() return. If it fails
    // (e.g. another transaction in flight, db locked), running migration
    // statements outside a transaction would commit each statement
    // individually — leaving inconsistent state on partial failure.
    if (!m_db.transaction()) {
        qCCritical(lcDb) << "Migration" << version
                         << ": failed to start transaction:"
                         << m_db.lastError().text();
        return;
    }
    QSqlQuery query(m_db);

    // Strip SQL line comments before splitting on semicolons.
    // Comments may contain semicolons (e.g. "-- done; continue") which would
    // otherwise produce malformed SQL fragments when the string is split.
    QString stripped;
    for (const QString &line : sql.split(QLatin1Char('\n'))) {
        const QString trimmedLine = line.trimmed();
        if (trimmedLine.startsWith(QStringLiteral("--"))) continue; // skip comment lines
        stripped += line + QLatin1Char('\n');
    }

    const QStringList statements = stripped.split(QLatin1Char(';'), Qt::SkipEmptyParts);
    for (const QString &stmt : statements) {
        const QString trimmed = stmt.trimmed();
        if (trimmed.isEmpty()) continue;

        // SQLite does not support ALTER TABLE ... ADD COLUMN IF NOT EXISTS.
        // Detect this case and skip silently if the column already exists.
        // This makes migrations idempotent even if user_version was not updated
        // on a previous interrupted run (e.g. crash between ALTER and COMMIT).
        static const QRegularExpression kAlterAdd(
            QStringLiteral(R"(ALTER\s+TABLE\s+(\w+)\s+ADD\s+COLUMN\s+(\w+))"),
            QRegularExpression::CaseInsensitiveOption);
        const auto m = kAlterAdd.match(trimmed);
        if (m.hasMatch()) {
            const QString tbl = m.captured(1);
            const QString col = m.captured(2);
            if (columnExists(m_db, tbl, col)) {
                qCDebug(lcDb) << "Column" << col << "already exists in" << tbl
                              << "— skipping ALTER TABLE.";
                continue; // idempotent skip
            }
        }

        if (!query.exec(trimmed)) {
            qCCritical(lcDb) << "Migration" << version
                             << "failed:" << query.lastError().text();
            m_db.rollback();
            return;
        }
    }

    // Commit DDL changes first, THEN set user_version.
    // PRAGMA user_version inside a Qt transaction is unreliable on some
    // SQLite builds — the pragma may be silently dropped if the transaction
    // is not yet committed when the pragma executes. By committing the DDL
    // first and then issuing the pragma as a standalone statement we guarantee
    // both that the schema change lands AND that the version is recorded.
    if (!m_db.commit()) {
        qCCritical(lcDb) << "Migration" << version
                         << "commit failed:" << m_db.lastError().text();
        m_db.rollback();
        return;
    }

    if (version > 0) {
        // Issue user_version outside any transaction so it always persists.
        QSqlQuery vq(m_db);
        if (!vq.exec(QStringLiteral("PRAGMA user_version = %1;").arg(version)))
            qCCritical(lcDb) << "Failed to set user_version to" << version
                             << ":" << vq.lastError().text();
    }

    m_currentVersion = version;
    qCInfo(lcDb) << "Migration to version" << version << "complete.";
}

// ─── Snap CRUD ────────────────────────────────────────────────────────────────

bool StorageManager::saveSnap(const Core::Snap &snap)
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "INSERT OR REPLACE INTO snaps "
        "(id, sender_id, recipient_id, media_type, local_path, "
        " sent_at, expires_at, viewed, duration_sec, content_hash, delivered_at,"
        " loc_lat, loc_lon, loc_acc, video_duration_ms, tier, kept) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"));

    q.addBindValue(snap.id.value);
    q.addBindValue(snap.senderId.value);
    q.addBindValue(snap.recipientId.value);
    q.addBindValue(static_cast<int>(snap.mediaType));
    q.addBindValue(snap.localPath);
    q.addBindValue(toUtcIso(snap.sentAt));
    q.addBindValue(snap.expiresAt.isValid() ? toUtcIso(snap.expiresAt) : QString{});
    q.addBindValue(snap.viewed ? 1 : 0);
    q.addBindValue(snap.durationSecs);
    q.addBindValue(snap.contentHash);
    q.addBindValue(snap.deliveredAt.isValid()
                   ? toUtcIso(snap.deliveredAt) : QString{});
    // Location — store NULL when not set (qQNaN columns map to SQL NULL via QVariant)
    q.addBindValue(snap.hasLocation() ? QVariant(snap.locLat)  : QVariant{});
    q.addBindValue(snap.hasLocation() ? QVariant(snap.locLon)  : QVariant{});
    q.addBindValue(snap.hasLocation() ? QVariant(snap.locAccM) : QVariant{});
    q.addBindValue(snap.videoDurationMs);
    q.addBindValue(static_cast<int>(Core::Media::tierToWire(snap.tier)));
    q.addBindValue(snap.kept ? 1 : 0);

    if (!q.exec()) {
        qCWarning(lcDb) << "saveSnap failed:" << q.lastError().text();
        return false;
    }
    emit snapSaved(snap.id);
    return true;
}

bool StorageManager::markSnapViewed(const Core::SnapId &id)
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral("UPDATE snaps SET viewed = 1 WHERE id = ?"));
    q.addBindValue(id.value);
    return q.exec();
}

bool StorageManager::markSnapKept(const Core::SnapId &id, bool kept)
{
    QSqlQuery q(m_db);
    if (kept) {
        // Keeping a snap exempts it from expiry: set the flag AND clear
        // expires_at so the existing expiry filters/purge treat it as "no
        // expiry" without any query changes. It now persists until explicitly
        // deleted.
        q.prepare(QStringLiteral(
            "UPDATE snaps SET kept = 1, expires_at = '' WHERE id = ?"));
    } else {
        // Un-keeping just clears the flag; it does NOT resurrect the old
        // expiry (which is gone), so the caller should set a fresh expires_at
        // via saveSnap if it wants the snap to age out again.
        q.prepare(QStringLiteral("UPDATE snaps SET kept = 0 WHERE id = ?"));
    }
    q.addBindValue(id.value);
    if (!q.exec()) {
        qCWarning(lcDb) << "markSnapKept failed:" << q.lastError().text();
        return false;
    }
    return true;
}

int StorageManager::keepAllSnaps()
{
    // Mark every not-yet-kept snap as kept and clear its expiry. Used after a
    // successful monthly export: the user chose to save the batch, so its
    // in-app copies stop auto-expiring. Returns the number of snaps affected.
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "UPDATE snaps SET kept = 1, expires_at = '' WHERE kept = 0"));
    if (!q.exec()) {
        qCWarning(lcDb) << "keepAllSnaps failed:" << q.lastError().text();
        return 0;
    }
    return q.numRowsAffected();
}

bool StorageManager::deleteSnap(const Core::SnapId &id)
{
    // Load the media path before deleting the row.
    QString mediaPath;
    {
        QSqlQuery sel(m_db);
        sel.prepare(QStringLiteral(
            "SELECT local_path FROM snaps WHERE id = ?"));
        sel.addBindValue(id.value);
        if (sel.exec() && sel.next())
            mediaPath = sel.value(0).toString();
    }

    QSqlQuery q(m_db);
    q.prepare(QStringLiteral("DELETE FROM snaps WHERE id = ?"));
    q.addBindValue(id.value);
    if (!q.exec()) {
        qCWarning(lcDb) << "deleteSnap failed:" << q.lastError().text();
        return false;
    }

    // Remove the media file from disk if present.
    if (!mediaPath.isEmpty() && QFile::exists(mediaPath))
        QFile::remove(mediaPath);

    emit snapDeleted(id);
    return true;
}


// Peer address fail threshold: after this many consecutive failures,
// loadPeerAddress() skips the address and forces a DHT lookup.
static constexpr int kAddrFailThreshold = 5;

static Core::Snap snapFromQuery(const QSqlQuery &q)
{
    Core::Snap s;
    s.id           = Core::SnapId{ q.value(0).toString() };
    s.senderId     = Core::UserId{ q.value(1).toString() };
    s.recipientId  = Core::UserId{ q.value(2).toString() };
    s.mediaType    = static_cast<Core::MediaType>(q.value(3).toInt());
    s.localPath    = q.value(4).toString();
    s.sentAt       = fromUtcIso(q.value(5).toString());
    if (!s.sentAt.isValid())
        s.sentAt = QDateTime::fromString(q.value(5).toString(), Qt::ISODateWithMs);
    const QString exp = q.value(6).toString();
    s.expiresAt    = exp.isEmpty() ? QDateTime{} : QDateTime::fromString(exp, Qt::ISODate);
    s.viewed       = q.value(7).toInt() != 0;
    s.durationSecs  = qBound(1, q.value(8).toInt(), 30);
    s.contentHash   = q.value(9).toString();
    const QString dlv = q.value(10).toString();
    s.deliveredAt   = dlv.isEmpty() ? QDateTime{} : QDateTime::fromString(dlv, Qt::ISODate);
    // Location columns (NULL when not set — isNull() guards the read)
    if (!q.value(11).isNull()) {
        s.locLat  = q.value(11).toDouble();
        s.locLon  = q.value(12).toDouble();
        s.locAccM = q.value(13).toDouble();
    }
    // else locLat/locLon stay qQNaN() — hasLocation() returns false

    // video_duration_ms (column 14) — added in migration 11. Older rows
    // without the column return 0 via DEFAULT 0 in the schema.
    s.videoDurationMs = q.value(14).toInt();
    // tier (column 15) — added in migration 12. Older rows return 2
    // (Standard) via DEFAULT 2 in the schema, which is the correct value
    // for any snap predating tiered delivery.
    s.tier = Core::Media::tierFromWire(
        static_cast<quint8>(q.value(15).toInt()));
    // kept (column 16) — added in migration 15. Older rows return 0 via
    // DEFAULT 0. A kept snap is exempt from expiry/purge.
    s.kept = q.value(16).toInt() != 0;
    return s;
}

std::optional<Core::Snap> StorageManager::loadSnap(const Core::SnapId &id) const
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, sender_id, recipient_id, media_type, local_path, "
        "       sent_at, expires_at, viewed, duration_sec, content_hash, delivered_at,"
        "       loc_lat, loc_lon, loc_acc, video_duration_ms, tier, kept "
        "FROM snaps WHERE id = ?"));
    q.addBindValue(id.value);
    if (q.exec() && q.next()) {
        return snapFromQuery(q);
    }
    return std::nullopt;
}

QList<Core::Snap> StorageManager::loadInboxSnaps(const Core::UserId &userId) const
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, sender_id, recipient_id, media_type, local_path, "
        "       sent_at, expires_at, viewed, duration_sec, content_hash, delivered_at,"
        "       loc_lat, loc_lon, loc_acc, video_duration_ms, tier, kept "
        "FROM snaps WHERE recipient_id = ? AND viewed = 0 ORDER BY sent_at ASC LIMIT 500")); // Bug 7 fix: only unviewed snaps
    q.addBindValue(userId.value);
    QList<Core::Snap> results;
    if (q.exec()) {
        while (q.next()) results.append(snapFromQuery(q));
    }
    return results;
}

QList<Core::Snap> StorageManager::loadOutboxSnaps(const Core::UserId &userId) const
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, sender_id, recipient_id, media_type, local_path, "
        "       sent_at, expires_at, viewed, duration_sec, content_hash, delivered_at,"
        "       loc_lat, loc_lon, loc_acc, video_duration_ms, tier, kept "
        "FROM snaps WHERE sender_id = ? AND delivered_at = '' "
        "ORDER BY sent_at DESC LIMIT 500"));
    q.addBindValue(userId.value);
    QList<Core::Snap> results;
    if (q.exec()) {
        while (q.next()) results.append(snapFromQuery(q));
    }
    return results;
}

// ─── Conversation grouping (V32.15) ──────────────────────────────────────────
//
// Both methods compute the "other party" via a CASE expression so a single
// scan of the snaps table can both filter to my conversations AND normalise
// the grouping key regardless of direction:
//
//   conversation_partner = CASE WHEN recipient_id = :self THEN sender_id
//                               WHEN sender_id    = :self THEN recipient_id
//                               END
//
// Snaps that don't touch self at all (transit logs, debug snaps) get a NULL
// partner and are filtered out by the WHERE.
//
// Expired snaps are excluded to match the existing flat inbox behavior; we
// detect expiry inline (expires_at is a sortable ISO-8601 string) rather
// than loading + filtering in C++.

QList<StorageManager::ConversationSummary>
StorageManager::loadConversations(const Core::UserId &selfId) const
{
    QList<ConversationSummary> results;
    QSqlQuery q(m_db);
    // We compute everything in one aggregate query so the result set is
    // <= |contacts| rows even with thousands of historical snaps.
    q.prepare(QStringLiteral(
        "SELECT "
        "  CASE WHEN recipient_id = :self THEN sender_id "
        "       WHEN sender_id    = :self THEN recipient_id "
        "       END AS partner, "
        "  MAX(sent_at) AS last_at, "
        "  SUM(CASE WHEN recipient_id = :self AND viewed = 0 THEN 1 ELSE 0 END) "
        "    AS unread, "
        "  COUNT(*) AS total, "
        "  SUM(CASE WHEN sender_id    = :self THEN 1 ELSE 0 END) AS out_count, "
        "  SUM(CASE WHEN recipient_id = :self THEN 1 ELSE 0 END) AS in_count "
        "FROM snaps "
        "WHERE (recipient_id = :self OR sender_id = :self) "
        "  AND (expires_at = '' OR expires_at >= :now) "
        "GROUP BY partner "
        "HAVING partner IS NOT NULL AND partner != '' "
        "ORDER BY last_at DESC"));
    q.bindValue(QStringLiteral(":self"), selfId.value);
    q.bindValue(QStringLiteral(":now"),
                QDateTime::currentDateTimeUtc().toString(Qt::ISODate));
    if (q.exec()) {
        while (q.next()) {
            ConversationSummary s;
            s.otherParty.value = q.value(0).toString();
            s.lastSnapAt       = QDateTime::fromString(
                q.value(1).toString(), Qt::ISODate);
            s.unreadCount      = q.value(2).toInt();
            s.totalCount       = q.value(3).toInt();
            s.hasOutbound      = q.value(4).toInt() > 0;
            s.hasInbound       = q.value(5).toInt() > 0;
            results.append(std::move(s));
        }
    }
    return results;
}

QList<Core::Snap>
StorageManager::loadConversationSnaps(const Core::UserId &selfId,
                                       const Core::UserId &otherParty) const
{
    QList<Core::Snap> results;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, sender_id, recipient_id, media_type, local_path, "
        "       sent_at, expires_at, viewed, duration_sec, content_hash, delivered_at,"
        "       loc_lat, loc_lon, loc_acc, video_duration_ms, tier, kept "
        "FROM snaps "
        "WHERE ((sender_id = :self AND recipient_id = :other) "
        "    OR (sender_id = :other AND recipient_id = :self)) "
        "  AND (expires_at = '' OR expires_at >= :now) "
        "ORDER BY sent_at ASC LIMIT 1000"));
    q.bindValue(QStringLiteral(":self"),  selfId.value);
    q.bindValue(QStringLiteral(":other"), otherParty.value);
    q.bindValue(QStringLiteral(":now"),
                QDateTime::currentDateTimeUtc().toString(Qt::ISODate));
    if (q.exec()) {
        while (q.next()) results.append(snapFromQuery(q));
    }
    return results;
}

QHash<QString, StorageManager::ConversationLastMeta>
StorageManager::loadConversationLastMeta(const Core::UserId &selfId) const
{
    // V32.34+ EFFICIENCY: per-partner last-snap media-type + direction in ONE
    // query, replacing the inbox's old per-conversation loadConversationSnaps.
    // We pick the row with the greatest sent_at per partner. SQLite resolves
    // bare columns in a GROUP BY against the row that supplied the MAX of an
    // aggregate when that aggregate is in the SELECT (the "bare columns"
    // behavior), which is well-defined here because we MAX(sent_at) and read
    // media_type/sender_id from the same grouped row. Excludes expired snaps
    // to match loadConversations.
    QHash<QString, ConversationLastMeta> out;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT "
        "  CASE WHEN recipient_id = :self THEN sender_id "
        "       WHEN sender_id    = :self THEN recipient_id "
        "       END AS partner, "
        "  MAX(sent_at) AS last_at, "
        "  media_type, "
        "  sender_id "
        "FROM snaps "
        "WHERE (recipient_id = :self OR sender_id = :self) "
        "  AND (expires_at = '' OR expires_at >= :now) "
        "GROUP BY partner "
        "HAVING partner IS NOT NULL AND partner != ''"));
    q.bindValue(QStringLiteral(":self"), selfId.value);
    q.bindValue(QStringLiteral(":now"),
                QDateTime::currentDateTimeUtc().toString(Qt::ISODate));
    if (q.exec()) {
        while (q.next()) {
            ConversationLastMeta m;
            m.mediaType  = static_cast<Core::MediaType>(q.value(2).toInt());
            m.isOutbound = (q.value(3).toString() == selfId.value);
            out.insert(q.value(0).toString(), m);
        }
    }
    return out;
}

QList<Core::Snap>
StorageManager::loadSnapStubs(const Core::UserId &selfId) const
{
    // V32.34+ EFFICIENCY: minimal snap rows for selfId in ONE query — only the
    // fields the inbox's dedup + expiry bookkeeping touches (id, sender,
    // recipient, viewed). Replaces seeding the dedup set by loading whole
    // threads with full structs (media paths, hashes, durations, locations).
    QList<Core::Snap> out;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, sender_id, recipient_id, viewed FROM snaps "
        "WHERE (recipient_id = :self OR sender_id = :self) "
        "  AND (expires_at = '' OR expires_at >= :now)"));
    q.bindValue(QStringLiteral(":self"), selfId.value);
    q.bindValue(QStringLiteral(":now"),
                QDateTime::currentDateTimeUtc().toString(Qt::ISODate));
    if (q.exec()) {
        while (q.next()) {
            Core::Snap s;
            s.id.value          = q.value(0).toString();
            s.senderId.value    = q.value(1).toString();
            s.recipientId.value = q.value(2).toString();
            s.viewed            = q.value(3).toInt() != 0;
            out.append(std::move(s));
        }
    }
    return out;
}

// ─── User CRUD ────────────────────────────────────────────────────────────────

bool StorageManager::saveUser(const Core::UserProfile &profile)
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "INSERT OR REPLACE INTO users"
        " (id, display_name, avatar_path, online, nickname, notes, sc_id, vcard_uid)"
        " VALUES (?, ?, ?, ?, '', '', ?, '')"));
    q.addBindValue(profile.id.value);
    q.addBindValue(profile.displayName);
    q.addBindValue(profile.avatarPath);
    q.addBindValue(profile.online ? 1 : 0);
    q.addBindValue(profile.id.value); // sc_id = stable ID
    return q.exec();
}

bool StorageManager::saveContact(const Core::Contact &c)
{
    if (!m_db.isOpen() || c.id.value.isEmpty()) return false;

    // Preserve existing nickname/notes/vcard if incoming value is empty —
    // a reconnect event should not blank out user-set nicknames.
    QString nickname = c.nickname;
    QString notes    = c.notes;
    QString vcardUid = c.vcardUid;
    if (nickname.isEmpty() || notes.isEmpty() || vcardUid.isEmpty()) {
        QSqlQuery sel(m_db);
        sel.prepare(QStringLiteral(
            "SELECT nickname, notes, vcard_uid FROM users WHERE id=?"));
        sel.addBindValue(c.id.value);
        if (sel.exec() && sel.next()) {
            if (nickname.isEmpty()) nickname = sel.value(0).toString();
            if (notes.isEmpty())    notes    = sel.value(1).toString();
            if (vcardUid.isEmpty()) vcardUid = sel.value(2).toString();
        }
    }

    const QString displayName = c.displayName.isEmpty() ? c.id.value : c.displayName;
    const QString lastSeenStr = c.lastSeen.isValid()
        ? toPrivacyLastSeen(c.lastSeen) : QString{};
    const int stateInt = static_cast<int>(c.state);

    // Single INSERT OR REPLACE — works on every SQLite version ever shipped.
    // Much simpler than the old INSERT OR IGNORE + UPDATE pattern which had
    // multiple code paths and a PASSIVE checkpoint that didn't guarantee fsync.
    QSqlQuery q(m_db);
    bool ok = false;

    q.prepare(QStringLiteral(
        "INSERT OR REPLACE INTO users"
        " (id, display_name, avatar_path, online, nickname, notes,"
        "  sc_id, vcard_uid, contact_state, last_seen_at)"
        " VALUES (?,?,?,?,?,?,?,?,?,?)"));
    q.addBindValue(c.id.value);
    q.addBindValue(displayName);
    q.addBindValue(c.avatarPath);
    q.addBindValue(c.online ? 1 : 0);
    q.addBindValue(nickname);
    q.addBindValue(notes);
    q.addBindValue(c.id.value); // sc_id = same as id (stable sc-XXXXXXXX)
    q.addBindValue(vcardUid);
    q.addBindValue(stateInt);
    q.addBindValue(lastSeenStr);
    ok = q.exec();

    if (!ok) {
        qCWarning(lcDb) << "saveContact full insert failed:"
                        << q.lastError().text() << "— trying base columns";
        // Fallback for very old schemas without extended columns
        QSqlQuery fb(m_db);
        fb.prepare(QStringLiteral(
            "INSERT OR REPLACE INTO users (id, display_name, avatar_path, online)"
            " VALUES (?,?,?,?)"));
        fb.addBindValue(c.id.value);
        fb.addBindValue(displayName);
        fb.addBindValue(c.avatarPath);
        fb.addBindValue(c.online ? 1 : 0);
        ok = fb.exec();
        if (!ok) {
            qCWarning(lcDb) << "saveContact fallback failed:" << fb.lastError().text();
            return false;
        }
    }

    // With DELETE journal + synchronous=FULL, the commit already fsynced.
    // No checkpoint needed.

    qCDebug(lcDb) << "saveContact OK:" << c.id.value
                  << displayName << "state=" << stateInt;
    return true;
}

bool StorageManager::deleteContact(const Core::UserId &id)
{
    if (!m_db.isOpen()) return false;
    if (!m_db.transaction()) {
        qCWarning(lcDb) << "deleteContact: failed to start transaction";
        return false;
    }

    QSqlQuery del(m_db);
    del.prepare(QStringLiteral("DELETE FROM contacts WHERE id = ?"));
    del.addBindValue(id.value);
    if (!del.exec()) {
        qCWarning(lcDb) << "deleteContact failed:" << del.lastError().text();
        m_db.rollback();
        return false;
    }
    const int rows = del.numRowsAffected();

    // Also clear cached peer-addresses so reconnect attempts to a deleted
    // contact don't keep firing in the background.
    QSqlQuery addr(m_db);
    addr.prepare(QStringLiteral("DELETE FROM peer_addresses WHERE peer_id = ?"));
    addr.addBindValue(id.value);
    if (!addr.exec()) {
        // Non-fatal — log and continue
        qCWarning(lcDb) << "deleteContact: peer_addresses cleanup failed:"
                        << addr.lastError().text();
    }

    if (!m_db.commit()) {
        qCWarning(lcDb) << "deleteContact: commit failed:" << m_db.lastError().text();
        m_db.rollback();
        return false;
    }
    qCInfo(lcDb) << "Deleted contact" << id.value << "(rows affected:" << rows << ")";
    return rows > 0;
}

std::optional<Core::UserProfile> StorageManager::loadUser(const Core::UserId &id) const
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, display_name, avatar_path, online FROM users WHERE id = ?"));
    q.addBindValue(id.value);
    if (q.exec() && q.next()) {
        return Core::UserProfile{
            Core::UserId{ q.value(0).toString() },
            q.value(1).toString(),
            q.value(2).toString(),
            q.value(3).toInt() != 0
        };
    }
    return std::nullopt;
}

QList<Core::UserProfile> StorageManager::loadFriends(const Core::UserId &selfId) const
{
    Q_UNUSED(selfId)
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, display_name, avatar_path, online FROM users WHERE id != ?"));
    q.addBindValue(selfId.value);
    QList<Core::UserProfile> results;
    if (q.exec()) {
        while (q.next()) {
            results.append(Core::UserProfile{
                Core::UserId{ q.value(0).toString() },
                q.value(1).toString(),
                q.value(2).toString(),
                q.value(3).toInt() != 0
            });
        }
    }
    return results;
}

std::optional<Core::Contact> StorageManager::loadContact(const Core::UserId &id) const
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, display_name, avatar_path, online, nickname, notes, sc_id, vcard_uid,"
        "       contact_state, last_seen_at"
        " FROM users WHERE id = ?"));
    q.addBindValue(id.value);
    if (!q.exec() || !q.next()) return std::nullopt;
    Core::Contact c;
    c.id          = Core::UserId{ q.value(0).toString() };
    c.displayName = q.value(1).toString();
    c.avatarPath  = q.value(2).toString();
    c.online      = q.value(3).toInt() != 0;
    c.nickname    = q.value(4).toString();
    c.notes       = q.value(5).toString();
    c.vcardUid    = q.value(7).toString();
    c.state       = static_cast<Core::ContactState>(q.value(8).toInt());
    {
        const QString ls = q.value(9).toString();
        if (!ls.isEmpty()) c.lastSeen = fromUtcIso(ls);
    }
    return c;
}

QList<Core::Contact> StorageManager::loadContacts(const Core::UserId &selfId) const
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id, display_name, avatar_path, online, nickname, notes, sc_id, vcard_uid,"
        "       contact_state, last_seen_at"
        " FROM users WHERE id != ?"
        " ORDER BY CASE WHEN nickname != '' THEN nickname ELSE display_name END COLLATE NOCASE"));
    q.addBindValue(selfId.value);
    QList<Core::Contact> results;
    if (q.exec()) {
        while (q.next()) {
            Core::Contact c;
            c.id          = Core::UserId{ q.value(0).toString() };
            c.displayName = q.value(1).toString();
            c.avatarPath  = q.value(2).toString();
            c.online      = q.value(3).toInt() != 0;
            c.nickname    = q.value(4).toString();
            c.notes       = q.value(5).toString();
            c.vcardUid    = q.value(7).toString();
            c.state       = static_cast<Core::ContactState>(q.value(8).toInt());
            {
                const QString ls = q.value(9).toString();
                if (!ls.isEmpty()) c.lastSeen = fromUtcIso(ls);
            }
            results.append(c);
        }
    }
    return results;
}

void StorageManager::touchLastConnected(const Core::UserId &id)
{
    if (!m_db.isOpen() || id.value.isEmpty()) return;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "UPDATE users SET last_connected_at = ? WHERE id = ?"));
    q.addBindValue(QDateTime::currentMSecsSinceEpoch());
    q.addBindValue(id.value);
    if (!q.exec()) {
        qCWarning(lcDb) << "touchLastConnected:" << q.lastError().text();
    }
}

// ─── Landmarks (V32.31) ───────────────────────────────────────────────────────

bool StorageManager::saveLandmark(const Core::Landmark &lm)
{
    if (!m_db.isOpen() || lm.id.isEmpty()) return false;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "INSERT OR REPLACE INTO landmarks ("
        "  id, creator_id, display_name, lat, lon, accuracy_m,"
        "  geohash7, ssid, bssid_hash, photo_blur_hash, photo_edge_hash,"
        "  scope, created_at_ms, signature"
        ") VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"));
    q.addBindValue(lm.id);
    q.addBindValue(lm.creatorId);
    q.addBindValue(lm.displayName);
    q.addBindValue(lm.lat);
    q.addBindValue(lm.lon);
    q.addBindValue(lm.accuracyM);
    q.addBindValue(lm.geohash7);
    q.addBindValue(lm.ssid);
    // Empty QByteArray binds as NULL in SQLite via Qt — perfect, that's
    // our "absent" representation. No need to special-case.
    q.addBindValue(lm.bssidHash);
    q.addBindValue(lm.photoBlurHash);
    q.addBindValue(lm.photoEdgeHash);
    q.addBindValue(static_cast<int>(lm.scope));
    q.addBindValue(lm.createdAtMs == 0
                    ? QDateTime::currentMSecsSinceEpoch()
                    : lm.createdAtMs);
    q.addBindValue(lm.signature);
    if (!q.exec()) {
        qCWarning(lcDb) << "saveLandmark:" << q.lastError().text();
        return false;
    }
    return true;
}

namespace {
// Map a sqlite row into a Landmark struct. Used by all the load
// methods so they share a single column-name layout — if the schema
// changes, fix this once.
Core::Landmark rowToLandmark(const QSqlQuery &q)
{
    Core::Landmark lm;
    lm.id            = q.value(QStringLiteral("id")).toString();
    lm.creatorId     = q.value(QStringLiteral("creator_id")).toString();
    lm.displayName   = q.value(QStringLiteral("display_name")).toString();
    lm.lat           = q.value(QStringLiteral("lat")).toDouble();
    lm.lon           = q.value(QStringLiteral("lon")).toDouble();
    lm.accuracyM     = q.value(QStringLiteral("accuracy_m")).toDouble();
    lm.geohash7      = q.value(QStringLiteral("geohash7")).toString();
    lm.ssid          = q.value(QStringLiteral("ssid")).toString();
    lm.bssidHash     = q.value(QStringLiteral("bssid_hash")).toByteArray();
    lm.photoBlurHash = q.value(QStringLiteral("photo_blur_hash")).toByteArray();
    lm.photoEdgeHash = q.value(QStringLiteral("photo_edge_hash")).toByteArray();
    lm.scope         = static_cast<Core::LandmarkScope>(
                          q.value(QStringLiteral("scope")).toInt());
    lm.createdAtMs   = q.value(QStringLiteral("created_at_ms")).toLongLong();
    lm.signature     = q.value(QStringLiteral("signature")).toByteArray();
    return lm;
}
}

std::optional<Core::Landmark> StorageManager::loadLandmark(const QString &id) const
{
    if (!m_db.isOpen() || id.isEmpty()) return std::nullopt;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral("SELECT * FROM landmarks WHERE id = ?"));
    q.addBindValue(id);
    if (!q.exec() || !q.next()) return std::nullopt;
    return rowToLandmark(q);
}

QList<Core::Landmark> StorageManager::loadAllLandmarks() const
{
    QList<Core::Landmark> out;
    if (!m_db.isOpen()) return out;
    QSqlQuery q(m_db);
    if (!q.exec(QStringLiteral(
            "SELECT * FROM landmarks ORDER BY created_at_ms DESC"))) {
        qCWarning(lcDb) << "loadAllLandmarks:" << q.lastError().text();
        return out;
    }
    while (q.next()) out.append(rowToLandmark(q));
    return out;
}

QList<Core::Landmark> StorageManager::loadLandmarksNear(
    double lat, double lon, double radiusM) const
{
    QList<Core::Landmark> out;
    if (!m_db.isOpen()) return out;

    // Coarse pre-filter: bounding box ±degrees-equiv of radius. At our
    // latitudes 1° lat ≈ 111km. Use radius/111000 as the lat delta;
    // for lon, scale by cos(lat) to account for shrinking parallels
    // near the poles. This is a SQL-side pre-filter ONLY; precise
    // Haversine happens in code below.
    const double latDelta = radiusM / 111000.0;
    const double cosLat   = qCos(qDegreesToRadians(lat));
    // qBound the divisor against 0.01 to avoid blowing up near the
    // poles (acceptable approximation — anyone above 89°N is going to
    // get an overinclusive filter, which is harmless since we run
    // Haversine after).
    const double lonDelta = radiusM / (111000.0 * qMax(0.01, cosLat));

    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT * FROM landmarks"
        " WHERE lat BETWEEN ? AND ?"
        "   AND lon BETWEEN ? AND ?"));
    q.addBindValue(lat - latDelta);
    q.addBindValue(lat + latDelta);
    q.addBindValue(lon - lonDelta);
    q.addBindValue(lon + lonDelta);
    if (!q.exec()) {
        qCWarning(lcDb) << "loadLandmarksNear:" << q.lastError().text();
        return out;
    }

    // Precise filter + collect into a vector for sorting by distance.
    std::vector<std::pair<double, Core::Landmark>> withDist;
    while (q.next()) {
        Core::Landmark lm = rowToLandmark(q);
        const double d = lm.distanceMTo(lat, lon);
        if (d <= radiusM) withDist.emplace_back(d, std::move(lm));
    }
    std::sort(withDist.begin(), withDist.end(),
              [](const auto &a, const auto &b) { return a.first < b.first; });
    out.reserve(static_cast<int>(withDist.size()));
    for (auto &[_, lm] : withDist) out.append(std::move(lm));
    return out;
}

bool StorageManager::deleteLandmark(const QString &id)
{
    if (!m_db.isOpen() || id.isEmpty()) return false;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral("DELETE FROM landmarks WHERE id = ?"));
    q.addBindValue(id);
    if (!q.exec()) {
        qCWarning(lcDb) << "deleteLandmark:" << q.lastError().text();
        return false;
    }
    return q.numRowsAffected() > 0;
}

// ─── Peer addresses ───────────────────────────────────────────────────────────

bool StorageManager::savePeerAddress(const Core::UserId &id, const Core::PeerAddress &addr)
{
    if (addr.host.isEmpty() || addr.port == 0) return false;
    QSqlQuery q(m_db);
    // Upsert: if this (user_id, host, port) already exists, refresh last_seen
    // and reset fail_count (DHT discovery means the address is fresh again).
    q.prepare(QStringLiteral(
        "INSERT INTO peer_addresses (user_id, host, port, last_seen, fail_count)"
        " VALUES (?, ?, ?, datetime('now'), 0)"
        " ON CONFLICT(user_id, host, port) DO UPDATE SET"
        "   last_seen = excluded.last_seen,"
        "   fail_count = 0"));
    q.addBindValue(id.value);
    q.addBindValue(addr.host);
    q.addBindValue(static_cast<int>(addr.port));
    return q.exec();
}

std::optional<Core::PeerAddress> StorageManager::loadPeerAddress(const Core::UserId &id) const
{
    // Return the freshest address that has not exceeded the failure threshold.
    // fail_count >= 5 means we have tried this address 5 times without success
    // — skip it so we fall through to a DHT lookup for a fresh address.
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT host, port FROM peer_addresses"
        " WHERE user_id = ? AND fail_count < ?"
        " ORDER BY last_seen DESC LIMIT 1"));
    q.addBindValue(id.value);
    q.addBindValue(kAddrFailThreshold);
    if (q.exec() && q.next()) {
        return Core::PeerAddress{ q.value(0).toString(),
                                  static_cast<quint16>(q.value(1).toInt()) };
    }
    return std::nullopt; // all addresses exhausted — caller must do DHT lookup
}

QList<Core::PeerAddress> StorageManager::loadAllPeerAddresses(const Core::UserId &id) const
{
    // All known addresses for this peer, newest first, regardless of fail_count.
    // Used for diagnostics and fallback iteration.
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT host, port FROM peer_addresses WHERE user_id = ?"
        " ORDER BY fail_count ASC, last_seen DESC"));
    q.addBindValue(id.value);
    QList<Core::PeerAddress> result;
    if (q.exec()) {
        while (q.next())
            result.append({ q.value(0).toString(),
                            static_cast<quint16>(q.value(1).toInt()) });
    }
    return result;
}

void StorageManager::markPeerAddressFailed(const Core::UserId &id,
                                            const Core::PeerAddress &addr)
{
    // Increment fail_count for this specific address. When fail_count reaches 5,
    // loadPeerAddress() will skip it and return nullopt, forcing a DHT lookup.
    if (!m_db.isOpen()) return;
    // Use exec(QString) directly — prepared statements with 3 positional
    // parameters fail on some Qt SQLite builds with "Parameter count mismatch"
    // even when the bind count matches. Inline values are safe here because
    // user_id and host are validated sc-IDs and IP strings (no SQL injection risk),
    // and port is an integer.
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "UPDATE peer_addresses SET fail_count = fail_count + 1"
        " WHERE user_id = ? AND host = ? AND port = ?"));
    q.addBindValue(id.value);
    q.addBindValue(addr.host);
    q.addBindValue(static_cast<int>(addr.port));
    if (!q.exec())
        qCWarning(lcDb) << "markPeerAddressFailed:" << q.lastError().text();
}

void StorageManager::clearPeerAddress(const Core::UserId &id)
{
    // Mark ALL addresses for this peer as exhausted so the next
    // loadPeerAddress() call returns nullopt and triggers a DHT lookup.
    if (!m_db.isOpen()) return;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "UPDATE peer_addresses SET fail_count = ? WHERE user_id = ?"));
    q.addBindValue(kAddrFailThreshold);
    q.addBindValue(id.value);
    if (!q.exec())
        qCWarning(lcDb) << "clearPeerAddress failed:" << q.lastError().text();
}


// ─── Peer public keys ─────────────────────────────────────────────────────────

bool StorageManager::savePeerPublicKey(const Core::UserId &id, const QByteArray &pubKey)
{
    if (!m_db.isOpen() || id.value.isEmpty() || pubKey.isEmpty()) return false;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "INSERT INTO peer_public_keys (peer_id, public_key, updated_at)"
        " VALUES (?, ?, datetime('now'))"
        " ON CONFLICT(peer_id) DO UPDATE SET"
        "   public_key = excluded.public_key,"
        "   updated_at = excluded.updated_at"));
    q.addBindValue(id.value);
    q.addBindValue(pubKey);
    const bool ok = q.exec();
    if (!ok)
        qCWarning(lcDb) << "savePeerPublicKey failed:" << q.lastError().text();
    return ok;
}

QByteArray StorageManager::loadPeerPublicKey(const Core::UserId &id) const
{
    if (!m_db.isOpen() || id.value.isEmpty()) return {};
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT public_key FROM peer_public_keys WHERE peer_id = ?"));
    q.addBindValue(id.value);
    if (q.exec() && q.next())
        return q.value(0).toByteArray();
    return {};
}

QList<QPair<Core::UserId, QByteArray>> StorageManager::loadAllPeerPublicKeys() const
{
    QList<QPair<Core::UserId, QByteArray>> result;
    if (!m_db.isOpen()) return result;
    QSqlQuery q(m_db);
    q.exec(QStringLiteral("SELECT peer_id, public_key FROM peer_public_keys"));
    while (q.next()) {
        const QString peerId  = q.value(0).toString();
        const QByteArray key  = q.value(1).toByteArray();
        if (!peerId.isEmpty() && !key.isEmpty())
            result.append({ Core::UserId{ peerId }, key });
    }
    return result;
}

// ─── V32.33: Pending key changes (TOFU / CRYPTO_REVIEW.md S-20) ──────────────

bool StorageManager::recordPendingKeyChange(const Core::UserId &id,
                                            const QByteArray &newKey,
                                            const QByteArray &oldKey)
{
    if (!m_db.isOpen() || id.value.isEmpty() || newKey.isEmpty() || oldKey.isEmpty())
        return false;
    QSqlQuery q(m_db);
    // If a row for this peer exists AND its new_public_key is byte-equal
    // to the incoming newKey, bump the attempt counter (same attacker
    // trying again). Otherwise replace — the attempted-replacement key
    // itself changed, which is its own signal worth surfacing.
    q.prepare(QStringLiteral(
        "INSERT INTO pending_key_changes"
        " (peer_id, new_public_key, old_public_key, detected_at, attempt_count)"
        " VALUES (?, ?, ?, datetime('now'), 1)"
        " ON CONFLICT(peer_id) DO UPDATE SET"
        "   attempt_count = CASE WHEN excluded.new_public_key = new_public_key"
        "                        THEN attempt_count + 1"
        "                        ELSE 1 END,"
        "   new_public_key = excluded.new_public_key,"
        "   detected_at    = excluded.detected_at"));
    q.addBindValue(id.value);
    q.addBindValue(newKey);
    q.addBindValue(oldKey);
    const bool ok = q.exec();
    if (!ok) qCWarning(lcDb) << "recordPendingKeyChange failed:" << q.lastError().text();
    return ok;
}

QList<StorageManager::PendingKeyChange>
StorageManager::loadPendingKeyChanges() const
{
    QList<PendingKeyChange> result;
    if (!m_db.isOpen()) return result;
    QSqlQuery q(m_db);
    q.exec(QStringLiteral(
        "SELECT peer_id, new_public_key, old_public_key, detected_at, attempt_count"
        " FROM pending_key_changes ORDER BY detected_at DESC"));
    while (q.next()) {
        PendingKeyChange p;
        p.peerId       = Core::UserId{ q.value(0).toString() };
        p.newKey       = q.value(1).toByteArray();
        p.oldKey       = q.value(2).toByteArray();
        p.detectedAt   = q.value(3).toString();
        p.attemptCount = q.value(4).toInt();
        if (!p.peerId.value.isEmpty()) result.append(std::move(p));
    }
    return result;
}

bool StorageManager::acceptPendingKeyChange(const Core::UserId &id)
{
    if (!m_db.isOpen() || id.value.isEmpty()) return false;
    // Atomic: read pending → write to peer_public_keys → delete pending.
    // Use a transaction so a crash mid-flow can't leave us with the old
    // key replaced but the pending row still present (or vice versa).
    if (!m_db.transaction()) {
        qCWarning(lcDb) << "acceptPendingKeyChange: failed to start transaction:"
                        << m_db.lastError().text();
        return false;
    }
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT new_public_key FROM pending_key_changes WHERE peer_id = ?"));
    q.addBindValue(id.value);
    if (!q.exec() || !q.next()) {
        m_db.rollback();
        return false;
    }
    const QByteArray newKey = q.value(0).toByteArray();
    if (newKey.isEmpty()) {
        m_db.rollback();
        return false;
    }
    // Replace in peer_public_keys (this is the one place that's allowed
    // to silently overwrite, because the user has explicitly approved).
    QSqlQuery upd(m_db);
    upd.prepare(QStringLiteral(
        "INSERT INTO peer_public_keys (peer_id, public_key, updated_at)"
        " VALUES (?, ?, datetime('now'))"
        " ON CONFLICT(peer_id) DO UPDATE SET"
        "   public_key = excluded.public_key,"
        "   updated_at = excluded.updated_at"));
    upd.addBindValue(id.value);
    upd.addBindValue(newKey);
    if (!upd.exec()) {
        qCWarning(lcDb) << "acceptPendingKeyChange: pubkey update failed:"
                        << upd.lastError().text();
        m_db.rollback();
        return false;
    }
    QSqlQuery del(m_db);
    del.prepare(QStringLiteral("DELETE FROM pending_key_changes WHERE peer_id = ?"));
    del.addBindValue(id.value);
    if (!del.exec()) {
        qCWarning(lcDb) << "acceptPendingKeyChange: pending delete failed:"
                        << del.lastError().text();
        m_db.rollback();
        return false;
    }
    return m_db.commit();
}

bool StorageManager::rejectPendingKeyChange(const Core::UserId &id)
{
    if (!m_db.isOpen() || id.value.isEmpty()) return false;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral("DELETE FROM pending_key_changes WHERE peer_id = ?"));
    q.addBindValue(id.value);
    const bool ok = q.exec();
    if (!ok) qCWarning(lcDb) << "rejectPendingKeyChange failed:" << q.lastError().text();
    return ok;
}

bool StorageManager::hasPendingKeyChange(const Core::UserId &id) const
{
    if (!m_db.isOpen() || id.value.isEmpty()) return false;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral("SELECT 1 FROM pending_key_changes WHERE peer_id = ?"));
    q.addBindValue(id.value);
    return q.exec() && q.next();
}

// ─── Sync / dedup helpers ────────────────────────────────────────────────────

/// Find the local_path of a blob we already have for this content hash.
/// Returns empty string if not found.
QString StorageManager::findBlobByHash(const QString &contentHash) const
{
    if (contentHash.isEmpty()) return {};
    QSqlQuery q(m_db);
    // Find any snap that has this hash and a local_path that exists on disk.
    // We query multiple candidates because a previous file may have been deleted
    // by purgeExpiredSnaps or an external process between insert and lookup.
    q.prepare(QStringLiteral(
        "SELECT local_path FROM snaps "
        "WHERE content_hash = ? AND local_path != '' "
        "ORDER BY sent_at DESC LIMIT 5"));
    q.addBindValue(contentHash);
    if (q.exec()) {
        while (q.next()) {
            const QString path = q.value(0).toString();
            if (!path.isEmpty() && QFile::exists(path))
                return path;
        }
    }
    return {};
}

/// Record that a snap was acknowledged by the recipient (delivered_at = now).
bool StorageManager::markSnapDelivered(const Core::SnapId &id)
{
    const QString now = QDateTime::currentDateTimeUtc().toString(Qt::ISODate);
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "UPDATE snaps SET delivered_at = ? WHERE id = ? AND delivered_at = ''"));
    q.addBindValue(now);
    q.addBindValue(id.value);
    return q.exec();
}

/// Return snap IDs sent to recipientId since sinceTime that are unacknowledged.
/// Used on reconnect to find snaps the peer may have missed.
QList<Core::SnapId> StorageManager::undeliveredSnapIds(
    const Core::UserId &recipientId,
    const QDateTime    &sinceTime) const
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT id FROM snaps "
        "WHERE recipient_id = ? AND sent_at > ? AND delivered_at = '' "
        "ORDER BY sent_at ASC LIMIT 1000"));
    q.addBindValue(recipientId.value);
    q.addBindValue(sinceTime.toString(Qt::ISODate));
    QList<Core::SnapId> ids;
    if (q.exec()) {
        while (q.next()) ids.append(Core::SnapId{ q.value(0).toString() });
    }
    return ids;
}

// ─── Maintenance ──────────────────────────────────────────────────────────────

int StorageManager::purgeExpiredSnaps()
{
    // V32.42+: purge grace. Delete snaps that expired more than graceDays ago
    // rather than the instant they lapse, so a snap that lapsed while the app
    // was closed survives long enough for the next session's monthly export
    // reminder to offer it for saving (reminder-before-purge buffer). Default
    // 7 days; configurable. Kept snaps are excluded by the queries regardless.
    const int graceDays = qBound(0,
        QSettings().value(QStringLiteral("storage/purgeGraceDays"), 7).toInt(), 365);
    const QString cutoff = QDateTime::currentDateTimeUtc()
                               .addDays(-graceDays).toString(Qt::ISODate);

    // Collect media file paths before deleting rows so we can clean up disk.
    QSqlQuery select(m_db);
    select.prepare(QStringLiteral(
        "SELECT local_path FROM snaps "
        "WHERE expires_at != '' AND expires_at < ? AND local_path != '' AND kept = 0"));
    select.addBindValue(cutoff);
    QStringList pathsToDelete;
    if (select.exec()) {
        while (select.next()) {
            const QString p = select.value(0).toString();
            if (!p.isEmpty()) pathsToDelete.append(p);
        }
    }

    QSqlQuery del(m_db);
    del.prepare(QStringLiteral(
        "DELETE FROM snaps WHERE expires_at != '' AND expires_at < ? AND kept = 0"));
    del.addBindValue(cutoff);
    if (!del.exec()) {
        qCWarning(lcDb) << "purgeExpiredSnaps failed:" << del.lastError().text();
        return 0;
    }
    const int count = del.numRowsAffected();

    // Remove orphaned media files from disk.
    for (const QString &path : pathsToDelete) {
        // QFile::remove() returns false if the file is already gone — that is fine.
        // Skip the redundant exists() check: it wastes a syscall and introduces
        // a TOCTOU race window between stat() and unlink().
        if (!QFile::remove(path) && QFile::exists(path)) {
            qCWarning(lcDb) << "Could not delete expired media file:" << path;
        }
    }

    if (count > 0) qCInfo(lcDb) << "Purged" << count << "expired snap(s) and"
                                << pathsToDelete.size() << "media file(s)";
    return count;
}


void StorageManager::purgeAllData(const Core::UserId &selfId)
{
    if (!m_db.isOpen()) return;
    qCInfo(lcDb) << "purgeAllData: wiping all peer data, addresses, and snaps";

    // V32.33+ Phase 1 audit: check transaction() return. For a wipe-all
    // operation, consistency between deleted tables matters less than
    // observability — at minimum we want to know if the operation
    // ran in a transaction at all.
    const bool inTx = m_db.transaction();
    if (!inTx) {
        qCWarning(lcDb) << "purgeAllData: failed to start transaction; "
                           "DELETEs will run individually:"
                        << m_db.lastError().text();
    }
    QSqlQuery q(m_db);

    // Wipe address cache entirely.
    q.exec(QStringLiteral("DELETE FROM peer_addresses"));

    // Remove all contacts except self (preserve our own row if present).
    if (selfId.value.isEmpty()) {
        q.exec(QStringLiteral("DELETE FROM users"));
    } else {
        q.prepare(QStringLiteral("DELETE FROM users WHERE id != ?"));
        q.addBindValue(selfId.value);
        q.exec();
    }

    // Remove all snaps, DHT cache, and cryptographic keys.
    q.exec(QStringLiteral("DELETE FROM snaps"));
    q.exec(QStringLiteral("DELETE FROM dht_cache"));
    q.exec(QStringLiteral("DELETE FROM peer_public_keys"));
    // V32.33: also clear any pending TOFU key-change rows (S-20 followup).
    q.exec(QStringLiteral("DELETE FROM pending_key_changes"));

    if (inTx) {
        if (!m_db.commit()) {
            qCWarning(lcDb) << "purgeAllData: commit failed:"
                            << m_db.lastError().text();
            m_db.rollback();
        }
    }

    // Reclaim disk space.
    q.exec(QStringLiteral("VACUUM"));

    // Reset schema version to 0 so all migrations re-run on next launch —
    // this catches any edge case where the schema was partially corrupt.
    q.exec(QStringLiteral("PRAGMA user_version = 0"));

    qCInfo(lcDb) << "purgeAllData: done";
}

// ─── DHT cache ────────────────────────────────────────────────────────────────
// Stores DHT lookup results for up to 1 hour so repeated lookups are instant.
// On cache hit, we skip the DHT round-trip entirely.

void StorageManager::cacheDhtResult(const Core::UserId &id,
                                     const Core::PeerAddress &addr)
{
    if (!m_db.isOpen() || addr.host.isEmpty() || addr.port == 0) return;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "INSERT INTO dht_cache (peer_id, host, port, cached_at)"
        " VALUES (?, ?, ?, datetime('now'))"
        " ON CONFLICT(peer_id, host, port) DO UPDATE SET"
        "   cached_at  = excluded.cached_at,"
        "   fail_count = 0")); // fresh DHT result resets failure count
    q.addBindValue(id.value);
    q.addBindValue(addr.host);
    q.addBindValue(static_cast<int>(addr.port));
    if (!q.exec())
        qCWarning(lcDb) << "cacheDhtResult:" << q.lastError().text();

    // Cap per-peer rows at 15 — delete oldest beyond that.
    QSqlQuery cap(m_db);
    cap.prepare(QStringLiteral(
        "DELETE FROM dht_cache"
        " WHERE peer_id = ?"
        "   AND (host, port) NOT IN ("
        "     SELECT host, port FROM dht_cache"
        "     WHERE peer_id = ?"
        "     ORDER BY fail_count ASC, cached_at DESC LIMIT 15)"));
    cap.addBindValue(id.value);
    cap.addBindValue(id.value);
    cap.exec(); // best-effort — failure is non-critical

    // V32.27: global LRU cap. The per-peer cap above prevents any
    // single peer from blowing up the cache, but with many peers
    // (e.g. a user who's tried connecting to dozens of contacts)
    // the table can still grow unboundedly across peers. Cap total
    // rows at 256 — generous for realistic friend list sizes (~15
    // entries × ~15 friends = 225, just below the cap). When over,
    // delete the worst rows first: high fail_count, then oldest.
    //
    // Run cheap COUNT first; only fire the DELETE if we're over.
    // For typical caches this branch is rarely taken.
    constexpr int kMaxRows = 256;
    QSqlQuery countQ(m_db);
    if (countQ.exec(QStringLiteral("SELECT COUNT(*) FROM dht_cache"))
     && countQ.next()) {
        const int total = countQ.value(0).toInt();
        if (total > kMaxRows) {
            QSqlQuery evict(m_db);
            evict.prepare(QStringLiteral(
                "DELETE FROM dht_cache"
                " WHERE rowid IN ("
                "   SELECT rowid FROM dht_cache"
                "   ORDER BY fail_count DESC, cached_at ASC"
                "   LIMIT ?)"));
            evict.addBindValue(total - kMaxRows);
            if (!evict.exec()) {
                qCWarning(lcDb) << "DHT cache LRU evict:"
                                << evict.lastError().text();
            } else {
                qCInfo(lcDb) << "DHT cache LRU evicted"
                             << (total - kMaxRows)
                             << "rows (total was" << total
                             << ", cap" << kMaxRows << ")";
            }
        }
    }
}

QList<Core::PeerAddress> StorageManager::loadDhtCache(const Core::UserId &id,
                                                        int maxAgeMinutes) const
{
    if (!m_db.isOpen()) return {};
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT host, port FROM dht_cache"
        " WHERE peer_id = ?"
        "   AND cached_at >= datetime('now', ? || ' minutes')"
        " ORDER BY fail_count ASC, cached_at DESC LIMIT 20"));
    q.addBindValue(id.value);
    q.addBindValue(QStringLiteral("-%1").arg(maxAgeMinutes));
    QList<Core::PeerAddress> result;
    if (q.exec()) {
        while (q.next())
            result.append({ q.value(0).toString(),
                            static_cast<quint16>(q.value(1).toInt()) });
    }
    return result;
}

QList<QPair<Core::PeerAddress,int>>
StorageManager::loadDhtCacheWithAge(const Core::UserId &id,
                                     int maxAgeMinutes) const
{
    if (!m_db.isOpen()) return {};
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT host, port,"
        " CAST((julianday('now') - julianday(cached_at)) * 86400 AS INTEGER)"
        " FROM dht_cache"
        " WHERE peer_id = ?"
        "   AND cached_at >= datetime('now', ? || ' minutes')"
        " ORDER BY fail_count ASC, cached_at DESC LIMIT 20")); // prefer unfailed addrs first
    q.addBindValue(id.value);
    q.addBindValue(QStringLiteral("-%1").arg(maxAgeMinutes));
    QList<QPair<Core::PeerAddress,int>> result;
    if (q.exec()) {
        while (q.next())
            result.append({
                { q.value(0).toString(),
                  static_cast<quint16>(q.value(1).toInt()) },
                q.value(2).toInt()
            });
    }
    return result;
}

QStringList StorageManager::listRecentPeers(int maxAgeMinutes) const
{
    if (!m_db.isOpen()) return {};
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT DISTINCT peer_id FROM dht_cache"
        " WHERE cached_at >= datetime('now', ? || ' minutes')"
        " ORDER BY cached_at DESC LIMIT 50"));
    q.addBindValue(QStringLiteral("-%1").arg(maxAgeMinutes));
    if (!q.exec()) return {};
    QStringList result;
    while (q.next())
        result.append(q.value(0).toString());
    return result;
}



void StorageManager::markDhtAddressFailed(const Core::UserId &id,
                                           const Core::PeerAddress &addr)
{
    if (!m_db.isOpen()) return;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "UPDATE dht_cache SET fail_count = fail_count + 1"
        " WHERE peer_id = ? AND host = ? AND port = ?"));
    q.addBindValue(id.value);
    q.addBindValue(addr.host);
    q.addBindValue(static_cast<int>(addr.port));
    if (!q.exec())
        qCWarning(lcDb) << "markDhtAddressFailed:" << q.lastError().text();
}

void StorageManager::purgeDhtCacheForPeer(const Core::UserId &id)
{
    if (!m_db.isOpen()) return;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral("DELETE FROM dht_cache WHERE peer_id = ?"));
    q.addBindValue(id.value);
    if (!q.exec())
        qCWarning(lcDb) << "purgeDhtCacheForPeer:" << q.lastError().text();
}

void StorageManager::purgeDhtCache(int olderThanMinutes)
{
    if (!m_db.isOpen()) return;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "DELETE FROM dht_cache WHERE cached_at < datetime('now', ? || ' minutes')"));
    q.addBindValue(QStringLiteral("-%1").arg(olderThanMinutes));
    if (!q.exec())
        qCWarning(lcDb) << "purgeDhtCache:" << q.lastError().text();
}

// ─── Peer location cache ──────────────────────────────────────────────────────

void StorageManager::upsertPeerLocation(const QString &peerId,
                                         double lat, double lon,
                                         double accuracyM,
                                         const QString &ssid,
                                         qint64 recordedAtMs,
                                         const QString &source)
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "INSERT INTO peer_locations (peer_id, lat, lon, accuracy_m, ssid, recorded_at, source) "
        "VALUES (?, ?, ?, ?, ?, ?, ?) "
        "ON CONFLICT(peer_id) DO UPDATE SET "
        "  lat=excluded.lat, lon=excluded.lon, accuracy_m=excluded.accuracy_m, "
        "  ssid=excluded.ssid, recorded_at=excluded.recorded_at, source=excluded.source "
        "WHERE excluded.recorded_at >= peer_locations.recorded_at")); // never go backwards
    q.addBindValue(peerId);
    q.addBindValue(lat);
    q.addBindValue(lon);
    q.addBindValue(accuracyM);
    q.addBindValue(ssid.isNull() ? QStringLiteral("") : ssid); // NULL → "" for NOT NULL col
    q.addBindValue(recordedAtMs);
    q.addBindValue(source.isNull() ? QStringLiteral("gps") : source);
    if (!q.exec())
        qCWarning(lcDb) << "upsertPeerLocation failed:" << q.lastError().text();
}

std::optional<Core::PeerLocation> StorageManager::loadPeerLocation(
    const QString &peerId, qint64 maxAgeMs) const
{
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "SELECT lat, lon, accuracy_m, ssid, recorded_at, source "
        "FROM peer_locations WHERE peer_id = ?"));
    q.addBindValue(peerId);
    if (!q.exec() || !q.next()) return std::nullopt;

    const qint64 recordedAt = q.value(4).toLongLong();
    const qint64 nowMs      = QDateTime::currentMSecsSinceEpoch();
    if (maxAgeMs > 0 && (nowMs - recordedAt) > maxAgeMs)
        return std::nullopt; // stale — caller treats as unavailable

    Core::PeerLocation loc;
    loc.peerId     = peerId;
    loc.lat        = q.value(0).toDouble();
    loc.lon        = q.value(1).toDouble();
    loc.accuracyM  = q.value(2).toDouble();
    loc.ssid       = q.value(3).toString();
    loc.recordedAt = QDateTime::fromMSecsSinceEpoch(recordedAt, QTimeZone::utc());
    loc.source     = q.value(5).toString();
    return loc;
}

void StorageManager::purgeStaleLocations(qint64 maxAgeMs)
{
    const qint64 cutoffMs = QDateTime::currentMSecsSinceEpoch() - maxAgeMs;
    QSqlQuery q(m_db);
    q.prepare(QStringLiteral(
        "DELETE FROM peer_locations WHERE recorded_at < ?"));
    q.addBindValue(cutoffMs);
    if (!q.exec())
        qCWarning(lcDb) << "purgeStaleLocations failed:" << q.lastError().text();
    else if (q.numRowsAffected() > 0)
        qCDebug(lcDb) << "Purged" << q.numRowsAffected() << "stale location entries";
}


} // namespace SnapClone::Storage
