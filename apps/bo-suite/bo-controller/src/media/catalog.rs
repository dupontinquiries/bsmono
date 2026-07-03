// src/catalog.rs
//
// SQLite catalog. One database, one row per file, designed for fast
// incremental scans:
//
//   - First scan of 10k files: ~5–10s for the filesystem walk + parse,
//     then 30–60 min for bliss analysis (deferred, resumable).
//   - Subsequent launches: ~100ms to open the DB and surface everything.
//
// Schema decisions:
//   - `tracks` is the main row. Path is the primary key — fast lookup
//     on filesystem-walk dedup.
//   - `tags` and `track_tags` are a separate join table so we can
//     filter on tags efficiently (a 10k library with 5k unique tags
//     would suck if we kept them as a comma-blob).
//   - `artists` similarly normalized; one track can have many artists.
//   - Indexes on the columns we query most: artist_norm, bpm, camelot,
//     mtime, analyzed_at.
//
// The DB is opened with WAL mode so background writes (analysis
// completing) don't block UI reads.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Current catalog schema version. Informational only — the migrate()
/// function uses PRAGMA user_version directly to drive upgrade steps.
#[allow(dead_code)]
pub const SCHEMA_VERSION: i32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[allow(dead_code)] // Schema struct — fields populated via deserialize
                    // / row-mapping helpers in this crate plus
                    // consumers in mux/. The compiler can't see those
                    // construction paths because they go through the
                    // Deserialize derive (which the dead_code lint
                    // doesn't count as a constructor).
pub struct Track {
    pub id: i64,
    pub path: String,
    pub size: i64,
    pub mtime: i64,           // unix seconds
    pub title: String,
    pub album: Option<String>,
    pub year: Option<u16>,
    pub artists: Vec<String>,
    pub features: Vec<String>,
    pub tags: Vec<String>,
    pub bpm: Option<f32>,
    pub camelot: Option<String>,    // e.g. "8A", "12B"
    pub key_text: Option<String>,   // e.g. "A minor", "C major"
    pub analyzed_at: Option<i64>,
    pub parse_confidence: u8,
}

pub fn open(db_path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(db_path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // Larger page cache speeds up bulk upserts during scan (negative = KB).
    conn.pragma_update(None, "cache_size", -64_000_i64)?;
    migrate(&conn)?;
    // Self-heal any orphaned join rows. A DELETE FROM tracks that ran
    // while foreign_keys was OFF (e.g. an older build, or a connection
    // that didn't set the pragma) leaves track_artists / track_tags rows
    // pointing at deleted track ids. The COUNT queries (which only join
    // the join table) then report inflated counts, while the FETCH
    // queries (which join tracks) return nothing — the "Glorb · 24
    // tracks" list entry that opens to "0 tracks" bug. Prune them here
    // so every catalog open converges to a consistent state.
    let pruned = prune_orphans(&conn)?;
    if pruned > 0 {
        eprintln!("catalog: pruned {} orphaned join row(s) \
                   (stale track_artists/track_tags from a pre-FK delete)", pruned);
    }
    Ok(conn)
}

/// Delete join rows that reference track ids no longer present in the
/// tracks table. Returns the number of rows removed. Idempotent.
pub fn prune_orphans(conn: &Connection) -> rusqlite::Result<usize> {
    let mut total = 0usize;
    total += conn.execute(
        "DELETE FROM track_artists
         WHERE track_id NOT IN (SELECT id FROM tracks)", [])?;
    total += conn.execute(
        "DELETE FROM track_tags
         WHERE track_id NOT IN (SELECT id FROM tracks)", [])?;
    // Tags / artists that now have zero attachments aren't harmful
    // (they just won't appear in the JOIN-based count lists), but the
    // tags list uses a subquery COUNT and would show them at 0. Leave
    // tag rows in place — the user may re-attach; the path-keyed mirror
    // (track_tags_by_path) is the durable source and is replayed by the
    // scanner's relink step.
    Ok(total)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: i32 = conn.query_row(
        "PRAGMA user_version", [], |r| r.get(0),
    ).unwrap_or(0);

    if current < 1 {
        conn.execute_batch(r#"
            CREATE TABLE tracks (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                path            TEXT NOT NULL UNIQUE,
                size            INTEGER NOT NULL,
                mtime           INTEGER NOT NULL,
                title           TEXT NOT NULL DEFAULT '',
                album           TEXT,
                year            INTEGER,
                bpm             REAL,
                camelot         TEXT,
                key_text        TEXT,
                analyzed_at     INTEGER,
                parsed_at       INTEGER,
                parse_confidence INTEGER NOT NULL DEFAULT 0,
                last_played_at  INTEGER,
                play_count      INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX idx_tracks_mtime    ON tracks(mtime);
            CREATE INDEX idx_tracks_bpm      ON tracks(bpm);
            CREATE INDEX idx_tracks_camelot  ON tracks(camelot);
            CREATE INDEX idx_tracks_analyzed ON tracks(analyzed_at);
            CREATE INDEX idx_tracks_lastplay ON tracks(last_played_at);

            CREATE TABLE artists (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                name            TEXT NOT NULL,
                norm            TEXT NOT NULL UNIQUE
            );
            CREATE INDEX idx_artists_norm    ON artists(norm);

            CREATE TABLE track_artists (
                track_id        INTEGER NOT NULL,
                artist_id       INTEGER NOT NULL,
                role            TEXT NOT NULL,  -- 'primary' | 'feature'
                ord             INTEGER NOT NULL,
                PRIMARY KEY (track_id, artist_id, role),
                FOREIGN KEY (track_id)  REFERENCES tracks(id)  ON DELETE CASCADE,
                FOREIGN KEY (artist_id) REFERENCES artists(id) ON DELETE CASCADE
            );
            CREATE INDEX idx_ta_artist ON track_artists(artist_id);

            CREATE TABLE tags (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                name            TEXT NOT NULL,
                norm            TEXT NOT NULL UNIQUE
            );
            CREATE INDEX idx_tags_norm       ON tags(norm);

            CREATE TABLE track_tags (
                track_id        INTEGER NOT NULL,
                tag_id          INTEGER NOT NULL,
                PRIMARY KEY (track_id, tag_id),
                FOREIGN KEY (track_id) REFERENCES tracks(id) ON DELETE CASCADE,
                FOREIGN KEY (tag_id)   REFERENCES tags(id)   ON DELETE CASCADE
            );
            CREATE INDEX idx_tt_tag    ON track_tags(tag_id);

            CREATE TABLE playlists (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                name            TEXT NOT NULL,
                created_at      INTEGER NOT NULL,
                last_used_at    INTEGER
            );
            CREATE TABLE playlist_tracks (
                playlist_id     INTEGER NOT NULL,
                track_id        INTEGER NOT NULL,
                ord             INTEGER NOT NULL,
                PRIMARY KEY (playlist_id, track_id),
                FOREIGN KEY (playlist_id) REFERENCES playlists(id) ON DELETE CASCADE,
                FOREIGN KEY (track_id)    REFERENCES tracks(id)    ON DELETE CASCADE
            );

            -- Singleton kv for "last playing track", scan checkpoints, etc.
            CREATE TABLE app_state (
                key             TEXT PRIMARY KEY,
                value           TEXT NOT NULL
            );

            PRAGMA user_version = 1;
        "#)?;
    }

    if current < 2 {
        conn.execute_batch(r#"
            -- Manual user overrides: per-track edits to title/artist/album/year
            -- that supersede the parser's guesses. Applied at SELECT time so
            -- a re-scan won't overwrite them.
            CREATE TABLE IF NOT EXISTS track_overrides (
                track_id   INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
                title      TEXT,
                artist     TEXT,
                album      TEXT,
                year       INTEGER,
                edited_at  INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            );

            -- Lightweight search index: a normalized text blob per track so
            -- "SELECT ... WHERE search LIKE ?" hits an index. We avoid FTS5
            -- to keep the schema portable across rusqlite builds.
            ALTER TABLE tracks ADD COLUMN search TEXT NOT NULL DEFAULT '';
            CREATE INDEX IF NOT EXISTS idx_tracks_search ON tracks(search);

            PRAGMA user_version = 2;
        "#)?;
    }

    if current < 3 {
        // Schema v3: queue + richer playlist metadata + play_history +
        // genre column. Each piece is independently optional — old
        // installs that skipped v3 still work since the queries below
        // wrap their writes in IF-NOT-EXISTS.
        conn.execute_batch(r#"
            -- The play queue. A single FIFO list (ordered by `ord`); the
            -- player pulls from the head. We persist it so the queue
            -- survives restarts of the controller. `device_url` lets
            -- multi-device users have independent queues per device,
            -- though the default empty string means "global queue".
            CREATE TABLE IF NOT EXISTS play_queue (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                track_id    INTEGER NOT NULL,
                ord         INTEGER NOT NULL,
                added_at    INTEGER NOT NULL,
                device_url  TEXT NOT NULL DEFAULT '',
                FOREIGN KEY (track_id) REFERENCES tracks(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_pq_ord    ON play_queue(ord);
            CREATE INDEX IF NOT EXISTS idx_pq_device ON play_queue(device_url, ord);

            -- A long-form play log. play_count is an aggregate; this is
            -- the per-event history that powers things like "tracks
            -- you've played this week" and the time-of-day biased
            -- prediction priors. The row is small (16 bytes) and we
            -- can prune old entries to bound growth.
            CREATE TABLE IF NOT EXISTS play_history (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                track_id    INTEGER NOT NULL,
                played_at   INTEGER NOT NULL,
                device_url  TEXT NOT NULL DEFAULT '',
                duration_played INTEGER,    -- seconds, may be null if unknown
                FOREIGN KEY (track_id) REFERENCES tracks(id) ON DELETE CASCADE
            );
            CREATE INDEX IF NOT EXISTS idx_ph_played    ON play_history(played_at);
            CREATE INDEX IF NOT EXISTS idx_ph_track     ON play_history(track_id);

            -- Genre column on tracks. ID3 frequently has it, we just
            -- hadn't been using it. Filled by the tag_reader; null
            -- when absent. Indexed because genre-filtered search is
            -- common (queries like "all my ambient tracks").
            ALTER TABLE tracks ADD COLUMN genre TEXT;
            CREATE INDEX IF NOT EXISTS idx_tracks_genre ON tracks(genre);

            -- Playlist metadata: description + cover_track_id (so we can
            -- show a small thumbnail from one of the playlist's tracks
            -- as the playlist's own icon).
            ALTER TABLE playlists ADD COLUMN description TEXT NOT NULL DEFAULT '';
            ALTER TABLE playlists ADD COLUMN cover_track_id INTEGER;
            ALTER TABLE playlists ADD COLUMN updated_at INTEGER;

            -- Tag color: a CSS hex (e.g. '#C8A04B') so tags can render
            -- visually distinct in the UI. Optional — null = use default.
            ALTER TABLE tags ADD COLUMN color TEXT;

            PRAGMA user_version = 3;
        "#)?;
    }

    if current < 4 {
        // ── Path-keyed tag persistence ──────────────────────────────
        //
        // track_tags references tracks.id with ON DELETE CASCADE. When
        // a rescan re-inserts a track (e.g. the tracks table was empty
        // at boot, or a file's row was deleted and re-added), the track
        // gets a NEW id and all its tag attachments cascade away — the
        // user's manually-applied tags vanish. This was the "tags page
        // shows 200 tags but every one has 0 tracks" bug.
        //
        // Fix: a durable, path-keyed mirror of tag attachments that does
        // NOT cascade on track deletion. The scanner re-links these back
        // to track ids after every walk, so tags survive any id churn.
        // Tag NAMES (not ids) are stored too, so even a tags-table reset
        // can be recovered.
        conn.execute_batch(r#"
            CREATE TABLE IF NOT EXISTS track_tags_by_path (
                path        TEXT NOT NULL,
                tag_name    TEXT NOT NULL,
                PRIMARY KEY (path, tag_name)
            );
            CREATE INDEX IF NOT EXISTS idx_ttbp_path ON track_tags_by_path(path);

            -- Backfill from whatever attachments currently exist, so
            -- existing users don't lose what's still linked at upgrade.
            INSERT OR IGNORE INTO track_tags_by_path (path, tag_name)
            SELECT t.path, g.name
            FROM track_tags tt
            JOIN tracks t ON t.id = tt.track_id
            JOIN tags   g ON g.id = tt.tag_id;

            PRAGMA user_version = 4;
        "#)?;
    }

    if current < 5 {
        // ── Track ratings ───────────────────────────────────────────
        //
        // A 0–5 star rating per track (0 = unrated). Stored on the track
        // row; a path-keyed mirror keeps it durable across rescans the
        // same way tags are (a track that gets a new id on re-add must
        // not silently lose its rating). The scanner re-links these after
        // each walk via relink_ratings_by_path.
        conn.execute_batch(r#"
            ALTER TABLE tracks ADD COLUMN rating INTEGER NOT NULL DEFAULT 0;

            CREATE TABLE IF NOT EXISTS track_rating_by_path (
                path    TEXT PRIMARY KEY,
                rating  INTEGER NOT NULL DEFAULT 0
            );

            PRAGMA user_version = 5;
        "#)?;
    }

    Ok(())
}

// Force the helper used below to live before the closing fn migrate brace
// is observed.  Kept as a stub to make subsequent str_replace insertions
// straightforward.
#[allow(dead_code)]
fn _migrate_v3_marker() {}

/// Set a track's star rating (0–5; 0 = unrated). Writes both the track
/// row and the durable path-keyed mirror in one transaction, so the
/// rating survives a rescan that gives the file a new track id. Values
/// outside 0..=5 are clamped.
pub fn set_track_rating(conn: &mut Connection, track_id: i64, rating: i64) -> rusqlite::Result<()> {
    let r = rating.clamp(0, 5);
    let tx = conn.transaction()?;
    tx.execute("UPDATE tracks SET rating = ?1 WHERE id = ?2", params![r, track_id])?;
    // Mirror by path (if the track has one) for durability.
    let path: Option<String> = tx.query_row(
        "SELECT path FROM tracks WHERE id = ?1", params![track_id],
        |row| row.get(0)).optional()?;
    if let Some(p) = path {
        tx.execute(
            "INSERT INTO track_rating_by_path (path, rating) VALUES (?1, ?2)
             ON CONFLICT(path) DO UPDATE SET rating = excluded.rating",
            params![p, r])?;
    }
    tx.commit()?;
    Ok(())
}

/// Read a track's star rating (0 if unrated or unknown).
pub fn track_rating(conn: &Connection, track_id: i64) -> rusqlite::Result<i64> {
    conn.query_row("SELECT rating FROM tracks WHERE id = ?1", params![track_id],
                   |r| r.get(0)).optional().map(|o| o.unwrap_or(0))
}

/// Re-link path-mirrored ratings back onto track rows after a rescan.
/// Mirrors relink_tags_by_path: a file that was re-added with a new id
/// gets its previously-set rating restored from the path mirror. Only
/// overwrites when the mirror has a non-zero rating, so a fresh scan
/// can't wipe a rating the user just set on the new row.
pub fn relink_ratings_by_path(conn: &Connection) -> rusqlite::Result<usize> {
    let n = conn.execute(
        "UPDATE tracks
         SET rating = (SELECT m.rating FROM track_rating_by_path m WHERE m.path = tracks.path)
         WHERE EXISTS (
             SELECT 1 FROM track_rating_by_path m
             WHERE m.path = tracks.path AND m.rating > 0
         )
         AND rating = 0",
        [])?;
    Ok(n)
}

// Inline trailing function from before the schema-expansion patch.
#[allow(dead_code)]
fn _legacy_migrate_close() -> rusqlite::Result<()> {
    Ok(())
}

// ─── Upsert helpers ─────────────────────────────────────────────────────────
//
// Legacy single-row upsert path. The current `scanner::walk_and_upsert`
// preloads all existing rows into a HashMap and batches the writes
// itself — the per-row SQL round-trip that this function performed
// dominated scan time on libraries of more than a few thousand tracks.
//
// Kept around because the API is small, the SQL is well-tested, and
// any future caller that needs the "one path, immediate write"
// shape (e.g. an in-app drag-and-drop import that doesn't go through
// the full walker) can reuse it without re-deriving the schema.

/// Insert or update a row from a filesystem walk. Doesn't touch parsed
/// metadata or analysis — those are filled in by later phases.
#[allow(dead_code)]
pub fn upsert_path(
    conn: &Connection,
    path: &str,
    size: i64,
    mtime: i64,
) -> rusqlite::Result<UpsertOutcome> {
    let existing: Option<(i64, i64, i64)> = conn.query_row(
        "SELECT id, size, mtime FROM tracks WHERE path = ?1",
        params![path],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;

    match existing {
        Some((id, old_size, old_mtime)) if old_size == size && old_mtime == mtime => {
            Ok(UpsertOutcome::Unchanged(id))
        }
        Some((id, _, _)) => {
            conn.execute(
                "UPDATE tracks SET size=?1, mtime=?2, parsed_at=NULL, analyzed_at=NULL WHERE id=?3",
                params![size, mtime, id],
            )?;
            Ok(UpsertOutcome::Changed(id))
        }
        None => {
            conn.execute(
                "INSERT INTO tracks (path, size, mtime) VALUES (?1, ?2, ?3)",
                params![path, size, mtime],
            )?;
            Ok(UpsertOutcome::Inserted(conn.last_insert_rowid()))
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum UpsertOutcome {
    Inserted(i64),
    Changed(i64),
    Unchanged(i64),
}

#[allow(dead_code)]
impl UpsertOutcome {
    pub fn id(&self) -> i64 {
        match *self { Self::Inserted(i) | Self::Changed(i) | Self::Unchanged(i) => i }
    }
    pub fn needs_parse(&self) -> bool {
        matches!(self, Self::Inserted(_) | Self::Changed(_))
    }
}

/// Remove tracks whose paths no longer exist on disk.
pub fn delete_missing(conn: &Connection, existing_paths: &std::collections::HashSet<String>) -> rusqlite::Result<usize> {
    let mut to_delete = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, path FROM tracks")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, path) = row?;
            if !existing_paths.contains(&path) {
                to_delete.push(id);
            }
        }
    }
    let n = to_delete.len();
    let tx = conn.unchecked_transaction()?;
    for id in to_delete {
        tx.execute("DELETE FROM tracks WHERE id = ?1", params![id])?;
    }
    tx.commit()?;
    Ok(n)
}

/// Apply parsed metadata to a track. Replaces existing artist/tag links.
/// Apply parsed metadata to a single track in its own transaction.
/// The scanner's batch path uses `apply_parse_in_tx` instead so all
/// rows commit together; this single-row variant stays around for
/// future ad-hoc uses (e.g. an in-app drag-and-drop import that
/// doesn't go through the walker).
#[allow(dead_code)]
pub fn apply_parse(
    conn: &Connection,
    track_id: i64,
    parsed: &super::parser::ParsedTrack,
    parsed_at: i64,
) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    apply_parse_in_tx(&tx, track_id, parsed, parsed_at)?;
    tx.commit()?;
    Ok(())
}

/// Apply a parsed-metadata result against an outer transaction managed
/// by the caller. Used by the scan-time bulk path, which wraps many
/// rows in one transaction so we get a single fsync at commit time
/// instead of one per track.
///
/// The pre-batch version of this code opened a NEW transaction inside
/// every apply_parse call. On a 50k-track library that meant 50k
/// fsync calls in Phase 2 alone — each one a millisecond-class
/// operation on SSD and tens of milliseconds on rotational/SD-card
/// storage. Batching the writes into a single outer transaction
/// drops Phase 2 from "wall-clock minutes" to "seconds" without
/// changing the schema or write semantics.
pub fn apply_parse_in_tx(
    tx: &rusqlite::Transaction,
    track_id: i64,
    parsed: &super::parser::ParsedTrack,
    parsed_at: i64,
) -> rusqlite::Result<()> {
    // Normalized text blob for the search index: lower-cased, accent-stripped
    // concatenation of title + artists + album. SQLite "LIKE '%foo%'" hits
    // idx_tracks_search and is plenty fast for tens of thousands of rows.
    let search_blob = build_search_blob(&parsed.title, &parsed.artists, parsed.album.as_deref());

    // Genre column: schema v3 adds tracks.genre for fast filtering.
    // The tag_reader puts the file's first genre (or each segment of a
    // multi-genre field) into parsed.tags. We snapshot the first one
    // here for the dedicated column; the tags table still gets the
    // full multi-valued set for richer "browse by genre" experiences.
    let genre = parsed.tags.first().cloned();

    tx.execute(
        "UPDATE tracks SET title=?1, album=?2, year=?3, parsed_at=?4, parse_confidence=?5, search=?6, genre=?7 WHERE id=?8",
        params![
            parsed.title,
            parsed.album,
            parsed.year.map(|y| y as i64),
            parsed_at,
            parsed.confidence as i64,
            search_blob,
            genre,
            track_id,
        ],
    )?;

    tx.execute("DELETE FROM track_artists WHERE track_id=?1", params![track_id])?;
    for (ord, name) in parsed.artists.iter().enumerate() {
        let aid = upsert_artist(tx, name)?;
        tx.execute(
            "INSERT OR IGNORE INTO track_artists (track_id, artist_id, role, ord) VALUES (?1, ?2, 'primary', ?3)",
            params![track_id, aid, ord as i64],
        )?;
    }
    for (ord, name) in parsed.features.iter().enumerate() {
        let aid = upsert_artist(tx, name)?;
        tx.execute(
            "INSERT OR IGNORE INTO track_artists (track_id, artist_id, role, ord) VALUES (?1, ?2, 'feature', ?3)",
            params![track_id, aid, ord as i64],
        )?;
    }

    // Tags: we INSERT OR IGNORE rather than DELETE+INSERT. The old
    // code unconditionally cleared track_tags on every scan, which
    // meant:
    //   1. Files with no genre ID3 tag (very common with YouTube-DL
    //      output — the "9246 .opus files" the user has) ended up
    //      with EMPTY track_tags rows after every scan, even if
    //      tags had been derived from genre metadata in a previous
    //      session with a different encoder.
    //   2. Tags manually attached by the user via the UI (the
    //      attach_tag qt_method, called from QML) were silently
    //      wiped on the next scan.
    //
    // The non-destructive variant means stale tags from a previous
    // scan never get cleaned up — but that's a much smaller harm
    // than wiping user-curated metadata. A future "compact catalog"
    // routine can prune orphans.
    for name in &parsed.tags {
        let tid = upsert_tag(tx, name)?;
        tx.execute(
            "INSERT OR IGNORE INTO track_tags (track_id, tag_id) VALUES (?1, ?2)",
            params![track_id, tid],
        )?;
    }
    Ok(())
}

fn upsert_artist(tx: &rusqlite::Transaction, name: &str) -> rusqlite::Result<i64> {
    let norm = super::parser::normalize_key(name);
    let existing: Option<i64> = tx.query_row(
        "SELECT id FROM artists WHERE norm = ?1",
        params![norm], |r| r.get(0),
    ).optional()?;
    if let Some(id) = existing { return Ok(id); }
    tx.execute(
        "INSERT INTO artists (name, norm) VALUES (?1, ?2)",
        params![name, norm],
    )?;
    Ok(tx.last_insert_rowid())
}

fn upsert_tag(tx: &rusqlite::Transaction, name: &str) -> rusqlite::Result<i64> {
    let norm = super::parser::normalize_key(name);
    let existing: Option<i64> = tx.query_row(
        "SELECT id FROM tags WHERE norm = ?1",
        params![norm], |r| r.get(0),
    ).optional()?;
    if let Some(id) = existing { return Ok(id); }
    tx.execute(
        "INSERT INTO tags (name, norm) VALUES (?1, ?2)",
        params![name, norm],
    )?;
    Ok(tx.last_insert_rowid())
}

/// Apply analysis results (BPM/key) — set after bliss runs. Only invoked
/// when the analysis feature is on (scanner::analyze_pending); under the
/// default no-analysis build this is dead but kept compiled for the API.
#[cfg_attr(not(feature = "analysis"), allow(dead_code))]
pub fn apply_analysis(
    conn: &Connection,
    track_id: i64,
    bpm: f32,
    camelot: &str,
    key_text: &str,
    analyzed_at: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE tracks SET bpm=?1, camelot=?2, key_text=?3, analyzed_at=?4 WHERE id=?5",
        params![bpm, camelot, key_text, analyzed_at, track_id],
    )?;
    Ok(())
}

// ─── Queries for the UI ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackSummary {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub primary_artist: String,
    pub bpm: Option<f32>,
    pub camelot: Option<String>,
    pub key_text: Option<String>,
    pub album: Option<String>,
    pub year: Option<u16>,
    /// 0–5 star rating (0 = unrated).
    #[serde(default)]
    pub rating: i64,
}

pub fn recent(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<TrackSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE((SELECT a.name FROM artists a
                          JOIN track_artists ta ON ta.artist_id = a.id
                         WHERE ta.track_id = t.id AND ta.role = 'primary'
                         ORDER BY ta.ord LIMIT 1), '')
        FROM tracks t
        ORDER BY t.mtime DESC
        LIMIT ?1
    "#)?;
    let mut summaries = rows_to_summaries(&mut stmt, params![limit as i64])?;
    apply_overrides(conn, &mut summaries)?;
    Ok(summaries)
}

/// Every track in the library, paginated, ordered by title. Uses
/// `FROM tracks` with a COALESCE'd artist subquery (NOT an inner join on
/// track_artists), so tracks with no artist link still appear — the
/// belt-and-suspenders guarantee that no track is ever unreachable
/// regardless of how its filename parsed. Returns (rows, total_count).
pub fn all_tracks_page(
    conn: &Connection,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<(Vec<TrackSummary>, i64)> {
    all_tracks_page_sorted(conn, offset, limit, TrackSort::Title)
}

/// Sort modes for the all-tracks browse list. Mapped to a fixed,
/// non-injectable ORDER BY clause by `order_clause` — the value never
/// comes from user text, so there's no SQL-injection surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackSort {
    Title,
    Artist,
    Album,
    RecentlyAdded,
    RatingDesc,
}

impl TrackSort {
    /// Parse the QML tag. Unknown → Title (safe default).
    pub fn from_tag(s: &str) -> Self {
        match s {
            "artist" => TrackSort::Artist,
            "album"  => TrackSort::Album,
            "recent" => TrackSort::RecentlyAdded,
            "rating" => TrackSort::RatingDesc,
            _         => TrackSort::Title,
        }
    }

    /// The stable tag for persistence / the Qt bridge (inverse of from_tag).
    pub fn to_tag(self) -> &'static str {
        match self {
            TrackSort::Title         => "title",
            TrackSort::Artist        => "artist",
            TrackSort::Album         => "album",
            TrackSort::RecentlyAdded => "recent",
            TrackSort::RatingDesc    => "rating",
        }
    }

    /// The literal ORDER BY body. `pa` is the correlated primary-artist
    /// subquery alias available in the SELECT. All branches end with a
    /// stable tiebreak (title, then id) so paging is deterministic.
    fn order_clause(self) -> &'static str {
        match self {
            TrackSort::Title =>
                "t.title COLLATE NOCASE, t.id",
            TrackSort::Artist =>
                "pa COLLATE NOCASE, t.title COLLATE NOCASE, t.id",
            TrackSort::Album =>
                "COALESCE(t.album,'') COLLATE NOCASE, t.title COLLATE NOCASE, t.id",
            // Newest first. tracks.id is monotonic with insertion, a good
            // proxy for "recently added" without needing a timestamp col.
            TrackSort::RecentlyAdded =>
                "t.id DESC",
            // Highest rated first, unrated (0) last; then title.
            TrackSort::RatingDesc =>
                "t.rating DESC, t.title COLLATE NOCASE, t.id",
        }
    }
}

/// Paginated all-tracks list with a chosen sort order. Backs the browse
/// list's sort toggle.
pub fn all_tracks_page_sorted(
    conn: &Connection,
    offset: i64,
    limit: i64,
    sort: TrackSort,
) -> rusqlite::Result<(Vec<TrackSummary>, i64)> {
    // The primary-artist subquery is aliased `pa` so the Artist sort can
    // order by it. ORDER BY is a fixed literal from the enum (never user
    // text) — concatenating it is injection-safe.
    let sql = format!(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE((SELECT a.name FROM artists a
                          JOIN track_artists ta ON ta.artist_id = a.id
                         WHERE ta.track_id = t.id AND ta.role = 'primary'
                         ORDER BY ta.ord LIMIT 1), 'Unknown Artist') AS pa,
               t.rating AS rating
        FROM tracks t
        ORDER BY {}
        LIMIT ?1 OFFSET ?2
    "#, sort.order_clause());
    let mut stmt = conn.prepare(&sql)?;
    let mut summaries = rows_to_summaries(&mut stmt, params![limit, offset])?;
    apply_overrides(conn, &mut summaries)?;
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
    Ok((summaries, total))
}

pub fn by_artist_id(conn: &Connection, artist_id: i64) -> rusqlite::Result<Vec<TrackSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               (SELECT a.name FROM artists a WHERE a.id = ?1)
        FROM tracks t
        JOIN track_artists ta ON ta.track_id = t.id
        WHERE ta.artist_id = ?1
        ORDER BY t.title
    "#)?;
    let mut summaries = rows_to_summaries(&mut stmt, params![artist_id])?;
    apply_overrides(conn, &mut summaries)?;
    Ok(summaries)
}

/// Paginated tracks-by-artist. Same query as `by_artist_id` with
/// LIMIT/OFFSET appended, plus a count query for the total.
///
/// Used by the QML side to render large artist pages (e.g. a
/// prolific artist with 200+ tracks) without instantiating every
/// ListView delegate at once. Total is included so the UI can
/// show "page 3 of 12" or render scrollbar thumb sizing.
pub fn by_artist_id_page(
    conn: &Connection,
    artist_id: i64,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<(Vec<TrackSummary>, i64)> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               (SELECT a.name FROM artists a WHERE a.id = ?1)
        FROM tracks t
        JOIN track_artists ta ON ta.track_id = t.id
        WHERE ta.artist_id = ?1
        ORDER BY t.title
        LIMIT ?2 OFFSET ?3
    "#)?;
    let mut summaries = rows_to_summaries(
        &mut stmt, params![artist_id, limit, offset])?;
    apply_overrides(conn, &mut summaries)?;
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM track_artists WHERE artist_id = ?1",
        params![artist_id],
        |r| r.get(0),
    )?;
    Ok((summaries, total))
}

/// Paginated `recent`. The unpaginated version already takes
/// `limit` so 90% of callers are fine, but for the Library
/// "Recently added" infinite-scroll view we want offset support.
pub fn recent_page(
    conn: &Connection,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<(Vec<TrackSummary>, i64)> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE((SELECT a.name FROM artists a
                          JOIN track_artists ta ON ta.artist_id = a.id
                         WHERE ta.track_id = t.id AND ta.role = 'primary'
                         ORDER BY ta.ord LIMIT 1), '')
        FROM tracks t
        ORDER BY t.mtime DESC
        LIMIT ?1 OFFSET ?2
    "#)?;
    let mut summaries = rows_to_summaries(
        &mut stmt, params![limit, offset])?;
    apply_overrides(conn, &mut summaries)?;
    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
    Ok((summaries, total))
}

pub fn by_bpm_range(conn: &Connection, lo: f32, hi: f32) -> rusqlite::Result<Vec<TrackSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE((SELECT a.name FROM artists a
                          JOIN track_artists ta ON ta.artist_id = a.id
                         WHERE ta.track_id = t.id AND ta.role = 'primary'
                         ORDER BY ta.ord LIMIT 1), '')
        FROM tracks t
        WHERE t.bpm BETWEEN ?1 AND ?2
        ORDER BY t.bpm
    "#)?;
    let mut summaries = rows_to_summaries(&mut stmt, params![lo, hi])?;
    apply_overrides(conn, &mut summaries)?;
    Ok(summaries)
}

pub fn by_camelot(conn: &Connection, camelot: &str) -> rusqlite::Result<Vec<TrackSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE((SELECT a.name FROM artists a
                          JOIN track_artists ta ON ta.artist_id = a.id
                         WHERE ta.track_id = t.id AND ta.role = 'primary'
                         ORDER BY ta.ord LIMIT 1), '')
        FROM tracks t
        WHERE t.camelot = ?1
        ORDER BY t.bpm
    "#)?;
    let mut summaries = rows_to_summaries(&mut stmt, params![camelot])?;
    apply_overrides(conn, &mut summaries)?;
    Ok(summaries)
}

pub fn by_tag_id(conn: &Connection, tag_id: i64) -> rusqlite::Result<Vec<TrackSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE((SELECT a.name FROM artists a
                          JOIN track_artists ta ON ta.artist_id = a.id
                         WHERE ta.track_id = t.id AND ta.role = 'primary'
                         ORDER BY ta.ord LIMIT 1), '')
        FROM tracks t
        JOIN track_tags tt ON tt.track_id = t.id
        WHERE tt.tag_id = ?1
        ORDER BY t.title
    "#)?;
    let mut summaries = rows_to_summaries(&mut stmt, params![tag_id])?;
    apply_overrides(conn, &mut summaries)?;
    Ok(summaries)
}

/// Full-library text search across title, album, and artist names.
/// Case-insensitive substring match (LIKE %q%). Returns at most `limit`
/// results, ranked: title matches first, then artist, then album.
pub fn search(conn: &Connection, query: &str, limit: usize) -> rusqlite::Result<Vec<TrackSummary>> {
    let q = query.trim();
    if q.is_empty() { return Ok(Vec::new()); }
    let like = format!("%{}%", q.to_lowercase());
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE((SELECT a.name FROM artists a
                          JOIN track_artists ta ON ta.artist_id = a.id
                         WHERE ta.track_id = t.id AND ta.role = 'primary'
                         ORDER BY ta.ord LIMIT 1), '')
        FROM tracks t
        WHERE LOWER(t.title) LIKE ?1
           OR LOWER(COALESCE(t.album,'')) LIKE ?1
           OR EXISTS (SELECT 1 FROM track_artists ta
                        JOIN artists a ON a.id = ta.artist_id
                       WHERE ta.track_id = t.id
                         AND LOWER(a.name) LIKE ?1)
        ORDER BY
          CASE
            WHEN LOWER(t.title) LIKE ?1 THEN 0
            WHEN EXISTS (SELECT 1 FROM track_artists ta
                           JOIN artists a ON a.id = ta.artist_id
                          WHERE ta.track_id = t.id
                            AND LOWER(a.name) LIKE ?1) THEN 1
            ELSE 2
          END,
          t.title
        LIMIT ?2
    "#)?;
    let mut summaries = rows_to_summaries(&mut stmt, params![like, limit as i64])?;
    apply_overrides(conn, &mut summaries)?;
    Ok(summaries)
}

pub(crate) fn rows_to_summaries(
    stmt: &mut rusqlite::Statement,
    params: impl rusqlite::Params,
) -> rusqlite::Result<Vec<TrackSummary>> {
    let mut out = Vec::new();
    let rows = stmt.query_map(params, |r| {
        Ok(TrackSummary {
            id: r.get(0)?,
            path: r.get(1)?,
            title: r.get(2)?,
            album: r.get(3)?,
            year: r.get::<_, Option<i64>>(4)?.map(|y| y as u16),
            bpm: r.get(5)?,
            camelot: r.get(6)?,
            key_text: r.get(7)?,
            primary_artist: r.get(8)?,
            // Name-based so this works regardless of column order, and
            // tolerant of queries that don't select it (older/narrower
            // SELECTs default to unrated rather than erroring).
            rating: r.get::<_, i64>("rating").unwrap_or(0),
        })
    })?;
    for row in rows { out.push(row?); }
    Ok(out)
}

// ─── Artist + tag listings (for "By artist" / "By tag" cards) ───────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GroupCount {
    pub id: i64,
    pub name: String,
    /// Number of tracks in this group. QML reads this as `track_count`
    /// (ArtistsPage, AlbumsPage, TagsPage all bind modelData.track_count);
    /// the Rust field stays `count` but serializes under that name so
    /// the group cards stop rendering "undefined tracks". We keep `count`
    /// as a serde alias so any older reader still deserializes.
    #[serde(rename = "track_count", alias = "count")]
    pub count: i64,
}

pub fn artists_with_counts(conn: &Connection) -> rusqlite::Result<Vec<GroupCount>> {
    let mut stmt = conn.prepare(r#"
        SELECT a.id, a.name, COUNT(ta.track_id) AS n
        FROM artists a
        JOIN track_artists ta ON ta.artist_id = a.id
        WHERE ta.role = 'primary'
        GROUP BY a.id
        ORDER BY a.norm
    "#)?;
    let rows = stmt.query_map([], |r| Ok(GroupCount {
        id: r.get(0)?, name: r.get(1)?, count: r.get(2)?,
    }))?;
    rows.collect()
}

/// Page-loaded version of `artists_with_counts`. Returns one page
/// of the alphabetized list plus the total count so the UI knows
/// how to size scrollbars / page indicators.
///
/// Why pagination matters: a 5000-artist library serialized through
/// `serde_json::to_string` produces ~250 KB of JSON. Pushing that
/// to QML via a string property triggers re-parse on every change,
/// and ListView's delegate creation chokes on instantiation cost.
/// Loading 50 at a time keeps each round-trip under 4 KB and the
/// delegate count manageable; the UI can fetch the next page when
/// the user scrolls near the bottom.
///
/// `offset` is 0-indexed. `limit=0` returns nothing (use a positive
/// number like 50). The total count comes from a separate fast
/// COUNT query against the same join — fast because we're not
/// materializing the artist rows, just counting them.
pub fn artists_with_counts_page(
    conn: &Connection,
    offset: i64,
    limit: i64,
) -> rusqlite::Result<(Vec<GroupCount>, i64)> {
    artists_with_counts_page_sorted(conn, offset, limit, ArtistPageSort::Name)
}

/// Sort modes for the paginated artists browse list. Only Name and
/// TrackCount are available here (the paged query aggregates a track
/// count but not play counts); the order clause is a fixed literal, so
/// there's no injection surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtistPageSort {
    Name,
    TrackCount,
}

impl ArtistPageSort {
    pub fn from_tag(s: &str) -> Self {
        match s {
            "tracks" => ArtistPageSort::TrackCount,
            _         => ArtistPageSort::Name,
        }
    }
    fn order_clause(self) -> &'static str {
        match self {
            ArtistPageSort::Name       => "a.norm",
            // Most tracks first, then name for a stable tiebreak.
            ArtistPageSort::TrackCount => "n DESC, a.norm",
        }
    }
}

pub fn artists_with_counts_page_sorted(
    conn: &Connection,
    offset: i64,
    limit: i64,
    sort: ArtistPageSort,
) -> rusqlite::Result<(Vec<GroupCount>, i64)> {
    let sql = format!(r#"
        SELECT a.id, a.name, COUNT(ta.track_id) AS n
        FROM artists a
        JOIN track_artists ta ON ta.artist_id = a.id
        WHERE ta.role = 'primary'
        GROUP BY a.id
        ORDER BY {}
        LIMIT ?1 OFFSET ?2
    "#, sort.order_clause());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params![limit, offset], |r| Ok(GroupCount {
        id: r.get(0)?, name: r.get(1)?, count: r.get(2)?,
    }))?;
    let page: Vec<GroupCount> = rows.collect::<Result<_, _>>()?;
    // Total count — counts artists that have at least one primary
    // track. Cheap because it's a single aggregation against the
    // already-indexed join.
    let total: i64 = conn.query_row(
        r#"SELECT COUNT(DISTINCT a.id)
           FROM artists a
           JOIN track_artists ta ON ta.artist_id = a.id
           WHERE ta.role = 'primary'"#,
        [],
        |r| r.get(0),
    )?;
    Ok((page, total))
}

pub fn tags_with_counts(conn: &Connection) -> rusqlite::Result<Vec<GroupCount>> {
    let mut stmt = conn.prepare(r#"
        SELECT g.id, g.name, COUNT(tt.track_id) AS n
        FROM tags g
        JOIN track_tags tt ON tt.tag_id = g.id
        GROUP BY g.id
        ORDER BY n DESC, g.norm
        LIMIT 200
    "#)?;
    let rows = stmt.query_map([], |r| Ok(GroupCount {
        id: r.get(0)?, name: r.get(1)?, count: r.get(2)?,
    }))?;
    rows.collect()
}

// ─── Statistics for "scan in progress" UI ───────────────────────────────────

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CatalogStats {
    pub total: i64,
    pub parsed: i64,
    pub analyzed: i64,
}

pub fn stats(conn: &Connection) -> rusqlite::Result<CatalogStats> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0))?;
    let parsed: i64 = conn.query_row("SELECT COUNT(*) FROM tracks WHERE parsed_at IS NOT NULL", [], |r| r.get(0))?;
    let analyzed: i64 = conn.query_row("SELECT COUNT(*) FROM tracks WHERE analyzed_at IS NOT NULL", [], |r| r.get(0))?;
    Ok(CatalogStats { total, parsed, analyzed })
}

// ─── App state singletons ──────────────────────────────────────────────────

pub fn set_state(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_state (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn get_state(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT value FROM app_state WHERE key = ?1",
        params![key],
        |r| r.get(0),
    ).optional()
}

// ─── v2: search index + manual overrides ────────────────────────────────────

/// Lower-case, accent-strip, and concatenate the searchable fields into one
/// blob. We keep it ASCII-folded so case + diacritics don't matter at query
/// time (the QML side lower-cases the user's query the same way).
pub fn build_search_blob(title: &str, artists: &[String], album: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str(&fold_text(title));
    for a in artists { s.push(' '); s.push_str(&fold_text(a)); }
    if let Some(al) = album { s.push(' '); s.push_str(&fold_text(al)); }
    s
}

/// Cheap ASCII folding: lowercase + strip diacritics on common Latin letters.
/// Keeps things dependency-free; "Bjørk" → "bjork", "Café" → "cafe".
pub fn fold_text(s: &str) -> String {
    let lc = s.to_lowercase();
    let mut out = String::with_capacity(lc.len());
    for ch in lc.chars() {
        let mapped = match ch {
            'à'|'á'|'â'|'ã'|'ä'|'å'|'ā' => 'a',
            'ç'|'ć'|'č'                  => 'c',
            'è'|'é'|'ê'|'ë'|'ē'          => 'e',
            'ì'|'í'|'î'|'ï'|'ī'          => 'i',
            'ñ'|'ń'                      => 'n',
            'ò'|'ó'|'ô'|'õ'|'ö'|'ø'|'ō' => 'o',
            'š'|'ś'                      => 's',
            'ù'|'ú'|'û'|'ü'|'ū'          => 'u',
            'ý'|'ÿ'                      => 'y',
            'ž'|'ź'                      => 'z',
            'ß'                          => 's',
            other                        => other,
        };
        out.push(mapped);
    }
    out
}

/// Manual user override for a track's display fields. None fields fall back
/// to the parsed values; setting one to Some replaces the parser's guess in
/// every list the UI shows. Saving a row deletes nothing — to clear, pass
/// all-None (which removes the row entirely so the parser's values reappear).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrackOverride {
    pub track_id: i64,
    pub title:  Option<String>,
    pub artist: Option<String>,
    pub album:  Option<String>,
    pub year:   Option<u16>,
}

pub fn set_override(conn: &Connection, ov: &TrackOverride) -> rusqlite::Result<()> {
    let all_empty = ov.title.is_none() && ov.artist.is_none()
                 && ov.album.is_none() && ov.year.is_none();
    if all_empty {
        conn.execute("DELETE FROM track_overrides WHERE track_id=?1", params![ov.track_id])?;
        return Ok(());
    }
    conn.execute(
        r#"INSERT INTO track_overrides (track_id, title, artist, album, year)
           VALUES (?1, ?2, ?3, ?4, ?5)
           ON CONFLICT(track_id) DO UPDATE SET
             title  = excluded.title,
             artist = excluded.artist,
             album  = excluded.album,
             year   = excluded.year,
             edited_at = strftime('%s','now')"#,
        params![ov.track_id, ov.title, ov.artist, ov.album, ov.year.map(|y| y as i64)],
    )?;
    Ok(())
}

/// Read the current override row for a track (None if no override is set).
/// Reserved for the override-display UI; the dialog currently writes only.
#[allow(dead_code)]
pub fn get_override(conn: &Connection, track_id: i64) -> rusqlite::Result<Option<TrackOverride>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT title, artist, album, year FROM track_overrides WHERE track_id=?1",
        params![track_id],
        |r| Ok(TrackOverride {
            track_id,
            title: r.get(0)?,
            artist: r.get(1)?,
            album: r.get(2)?,
            year: r.get::<_, Option<i64>>(3)?.map(|y| y as u16),
        }),
    ).optional()
}

/// Overlay overrides onto a Vec<TrackSummary>. One round-trip to fetch all
/// overrides for the given track_ids, then a linear merge. Cheap.
fn apply_overrides(conn: &Connection, summaries: &mut [TrackSummary]) -> rusqlite::Result<()> {
    if summaries.is_empty() { return Ok(()); }
    let ids: Vec<String> = summaries.iter().map(|s| s.id.to_string()).collect();
    let in_list = ids.join(",");
    // SAFETY: ids are integers from our own rows; no injection risk.
    let sql = format!(
        "SELECT track_id, title, artist, album, year FROM track_overrides WHERE track_id IN ({})",
        in_list
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |r| Ok((
        r.get::<_, i64>(0)?,
        r.get::<_, Option<String>>(1)?,
        r.get::<_, Option<String>>(2)?,
        r.get::<_, Option<String>>(3)?,
        r.get::<_, Option<i64>>(4)?,
    )))?;
    let mut by_id: std::collections::HashMap<i64, (Option<String>, Option<String>, Option<String>, Option<i64>)>
        = std::collections::HashMap::new();
    for row in rows {
        let (id, t, ar, al, y) = row?;
        by_id.insert(id, (t, ar, al, y));
    }
    for s in summaries.iter_mut() {
        if let Some((t, ar, al, y)) = by_id.get(&s.id) {
            if let Some(v) = t  { s.title = v.clone(); }
            if let Some(v) = ar { s.primary_artist = v.clone(); }
            if let Some(v) = al { s.album = Some(v.clone()); }
            if let Some(v) = y  { s.year = Some(*v as u16); }
        }
    }
    Ok(())
}

/// Read a value from the app_state KV table. Returns None when the key
/// is absent. Used by the anticipator to load its persisted transition
/// counts on startup.
pub fn get_app_state(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT value FROM app_state WHERE key = ?1")?;
    let rows = stmt.query_map(rusqlite::params![key], |r| r.get::<_, String>(0))?;
    for r in rows { return Ok(Some(r?)); }
    Ok(None)
}

/// Upsert a value into the app_state KV table. Used by the anticipator
/// to persist its transition counts so learning survives restarts.
pub fn set_app_state(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_state(key, value) VALUES(?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        rusqlite::params![key, value],
    )?;
    Ok(())
}
