// src/media/library.rs
#![allow(dead_code)]   // Functions wired in stages — initial module ships
                       // with QtState dispatch + QML pages to follow.
//
// Higher-level catalog operations: playlists, queue, tag management,
// play history, artist aggregation, and improved search.
//
// Builds on the primitives in catalog.rs (open, upsert, etc) but adds
// the operations the music-player UI actually invokes:
//
//   * Playlist CRUD (create, list, add tracks, remove, reorder, delete)
//   * Queue management (add, peek, pop, clear, list, reorder)
//   * Tag CRUD (create, list, attach/detach, recolor)
//   * Play history (record, list-by-period, recent N)
//   * Artist aggregation (top by play count, sorted alphabetical)
//   * Multi-term search (AND across terms, prefix match, ranked)
//
// All functions are pure-ish: they take a Connection and return
// Result<T>, no I/O outside SQLite. The media_worker dispatches to
// them and packages results as JSON for the QML UI.
//
// Schema invariants:
//   * playlists.id is monotonically increasing
//   * playlist_tracks.ord is dense within a playlist (no gaps), 0-based
//   * play_queue.ord is dense globally per device_url, 0-based
//   * play_history is append-only; we never modify rows, only prune

use rusqlite::{Connection, params, OptionalExtension};
use super::catalog::{TrackSummary, fold_text};

// ── Playlist API ─────────────────────────────────────────────────────

/// Lightweight playlist summary for list views.
#[derive(Debug, Clone)]
pub struct PlaylistSummary {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub track_count: i64,
    pub created_at: i64,
    pub updated_at: Option<i64>,
    pub cover_track_id: Option<i64>,
}

/// Create a new playlist. Name is required; description optional. Returns
/// the new playlist's id.
pub fn create_playlist(conn: &Connection, name: &str, description: &str) -> rusqlite::Result<i64> {
    let now = unix_now();
    conn.execute(
        "INSERT INTO playlists (name, description, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)",
        params![name, description, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Rename + redescribe a playlist. Either field can be passed as empty
/// to leave unchanged.
pub fn update_playlist_meta(conn: &Connection, id: i64, name: Option<&str>, description: Option<&str>) -> rusqlite::Result<()> {
    if let Some(n) = name {
        conn.execute("UPDATE playlists SET name=?1, updated_at=?2 WHERE id=?3",
                     params![n, unix_now(), id])?;
    }
    if let Some(d) = description {
        conn.execute("UPDATE playlists SET description=?1, updated_at=?2 WHERE id=?3",
                     params![d, unix_now(), id])?;
    }
    Ok(())
}

/// Delete a playlist + its tracks (ON DELETE CASCADE).
pub fn delete_playlist(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM playlists WHERE id=?1", params![id])?;
    Ok(())
}

/// List all playlists with track counts. Ordered by most-recently-updated.
pub fn list_playlists(conn: &Connection) -> rusqlite::Result<Vec<PlaylistSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT
            p.id,
            p.name,
            COALESCE(p.description, '') AS description,
            (SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id = p.id) AS track_count,
            p.created_at,
            p.updated_at,
            p.cover_track_id
        FROM playlists p
        ORDER BY COALESCE(p.updated_at, p.created_at) DESC
    "#)?;
    let rows = stmt.query_map([], |r| {
        Ok(PlaylistSummary {
            id: r.get(0)?,
            name: r.get(1)?,
            description: r.get(2)?,
            track_count: r.get(3)?,
            created_at: r.get(4)?,
            updated_at: r.get(5)?,
            cover_track_id: r.get(6)?,
        })
    })?;
    rows.collect()
}

/// Add a single track to the end of a playlist. Idempotent: if the
/// track is already in the playlist, this is a no-op (no duplicates).
/// Returns the ord position the track now occupies.
pub fn add_to_playlist(conn: &Connection, playlist_id: i64, track_id: i64) -> rusqlite::Result<i64> {
    // Already present?
    let existing: Option<i64> = conn.query_row(
        "SELECT ord FROM playlist_tracks WHERE playlist_id=?1 AND track_id=?2",
        params![playlist_id, track_id],
        |r| r.get(0),
    ).optional()?;
    if let Some(ord) = existing { return Ok(ord); }
    // Find the next ord.
    let next_ord: i64 = conn.query_row(
        "SELECT COALESCE(MAX(ord) + 1, 0) FROM playlist_tracks WHERE playlist_id=?1",
        params![playlist_id], |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO playlist_tracks (playlist_id, track_id, ord) VALUES (?1, ?2, ?3)",
        params![playlist_id, track_id, next_ord],
    )?;
    conn.execute("UPDATE playlists SET updated_at=?1 WHERE id=?2",
                 params![unix_now(), playlist_id])?;
    Ok(next_ord)
}

/// Add many tracks in one shot. Skips already-present tracks. Returns
/// the count actually inserted.
pub fn add_many_to_playlist(conn: &mut Connection, playlist_id: i64, track_ids: &[i64]) -> rusqlite::Result<usize> {
    let tx = conn.transaction()?;
    let mut next_ord: i64 = tx.query_row(
        "SELECT COALESCE(MAX(ord) + 1, 0) FROM playlist_tracks WHERE playlist_id=?1",
        params![playlist_id], |r| r.get(0),
    )?;
    let mut added = 0usize;
    {
        let mut chk_stmt = tx.prepare(
            "SELECT 1 FROM playlist_tracks WHERE playlist_id=?1 AND track_id=?2"
        )?;
        let mut ins_stmt = tx.prepare(
            "INSERT INTO playlist_tracks (playlist_id, track_id, ord) VALUES (?1, ?2, ?3)"
        )?;
        for &tid in track_ids {
            let present: Option<i32> = chk_stmt.query_row(
                params![playlist_id, tid], |r| r.get(0),
            ).optional()?;
            if present.is_some() { continue; }
            ins_stmt.execute(params![playlist_id, tid, next_ord])?;
            next_ord += 1;
            added += 1;
        }
    }
    tx.execute("UPDATE playlists SET updated_at=?1 WHERE id=?2",
               params![unix_now(), playlist_id])?;
    tx.commit()?;
    Ok(added)
}

/// Remove a track from a playlist. Re-densifies ord values so there
/// are no gaps (important for reorder operations).
pub fn remove_from_playlist(conn: &mut Connection, playlist_id: i64, track_id: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let removed: i64 = tx.execute(
        "DELETE FROM playlist_tracks WHERE playlist_id=?1 AND track_id=?2",
        params![playlist_id, track_id],
    )? as i64;
    if removed > 0 {
        // Re-densify ords.
        tx.execute(r#"
            WITH ranked AS (
                SELECT track_id,
                       ROW_NUMBER() OVER (ORDER BY ord) - 1 AS new_ord
                FROM playlist_tracks
                WHERE playlist_id = ?1
            )
            UPDATE playlist_tracks
            SET ord = (SELECT new_ord FROM ranked WHERE ranked.track_id = playlist_tracks.track_id)
            WHERE playlist_id = ?1
        "#, params![playlist_id])?;
        tx.execute("UPDATE playlists SET updated_at=?1 WHERE id=?2",
                   params![unix_now(), playlist_id])?;
    }
    tx.commit()?;
    Ok(())
}

/// Move a track to a new position within the playlist. `new_ord` is
/// 0-based; values out of range get clamped. Other tracks shift to
/// fill the gap.
pub fn reorder_playlist(conn: &mut Connection, playlist_id: i64, track_id: i64, new_ord: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let max_ord: i64 = tx.query_row(
        "SELECT COALESCE(MAX(ord), 0) FROM playlist_tracks WHERE playlist_id=?1",
        params![playlist_id], |r| r.get(0),
    )?;
    let new_ord = new_ord.clamp(0, max_ord);
    let current_ord: Option<i64> = tx.query_row(
        "SELECT ord FROM playlist_tracks WHERE playlist_id=?1 AND track_id=?2",
        params![playlist_id, track_id], |r| r.get(0),
    ).optional()?;
    let current_ord = match current_ord {
        Some(o) => o,
        None => { tx.commit()?; return Ok(()); }
    };
    if current_ord == new_ord { tx.commit()?; return Ok(()); }
    if new_ord < current_ord {
        // Moving up: shift items in [new_ord, current_ord) down by 1.
        tx.execute(
            "UPDATE playlist_tracks SET ord = ord + 1
             WHERE playlist_id=?1 AND ord >= ?2 AND ord < ?3",
            params![playlist_id, new_ord, current_ord],
        )?;
    } else {
        // Moving down: shift items in (current_ord, new_ord] up by 1.
        tx.execute(
            "UPDATE playlist_tracks SET ord = ord - 1
             WHERE playlist_id=?1 AND ord > ?2 AND ord <= ?3",
            params![playlist_id, current_ord, new_ord],
        )?;
    }
    tx.execute(
        "UPDATE playlist_tracks SET ord = ?1
         WHERE playlist_id=?2 AND track_id=?3",
        params![new_ord, playlist_id, track_id],
    )?;
    tx.execute("UPDATE playlists SET updated_at=?1 WHERE id=?2",
               params![unix_now(), playlist_id])?;
    tx.commit()?;
    Ok(())
}

/// Return the tracks of a playlist in playlist order, with full
/// TrackSummary metadata so the UI can render rich rows directly.
pub fn tracks_in_playlist(conn: &Connection, playlist_id: i64) -> rusqlite::Result<Vec<TrackSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT
            t.id,
            t.path,
            COALESCE(ov.title, t.title) AS title,
            COALESCE(ov.album, t.album) AS album,
            COALESCE(ov.year,  t.year)  AS year,
            t.bpm,
            t.camelot,
            t.key_text,
            COALESCE(ov.artist,
                (SELECT a.name FROM track_artists ta
                   JOIN artists a ON a.id = ta.artist_id
                  WHERE ta.track_id = t.id AND ta.role = 'primary'
                  ORDER BY ta.ord ASC LIMIT 1),
                ''
            ) AS primary_artist,
            t.rating AS rating
        FROM playlist_tracks pt
        JOIN tracks t ON t.id = pt.track_id
        LEFT JOIN track_overrides ov ON ov.track_id = t.id
        WHERE pt.playlist_id = ?1
        ORDER BY pt.ord ASC
    "#)?;
    let rows = stmt.query_map(params![playlist_id], |r| {
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
            rating: r.get::<_, i64>("rating").unwrap_or(0),
        })
    })?;
    rows.collect()
}

// ── Queue API ────────────────────────────────────────────────────────

/// Push a track to the end of the queue. If `device_url` is empty the
/// track joins the global queue; otherwise the per-device queue.
pub fn queue_push(conn: &Connection, track_id: i64, device_url: &str) -> rusqlite::Result<i64> {
    let next_ord: i64 = conn.query_row(
        "SELECT COALESCE(MAX(ord) + 1, 0) FROM play_queue WHERE device_url=?1",
        params![device_url], |r| r.get(0),
    )?;
    conn.execute(
        "INSERT INTO play_queue (track_id, ord, added_at, device_url)
         VALUES (?1, ?2, ?3, ?4)",
        params![track_id, next_ord, unix_now(), device_url],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insert a track at position N in the queue (0 = play next). Existing
/// tracks at or after N shift down by 1.
pub fn queue_insert(conn: &mut Connection, track_id: i64, device_url: &str, at: i64) -> rusqlite::Result<i64> {
    let tx = conn.transaction()?;
    tx.execute(
        "UPDATE play_queue SET ord = ord + 1 WHERE device_url=?1 AND ord >= ?2",
        params![device_url, at],
    )?;
    tx.execute(
        "INSERT INTO play_queue (track_id, ord, added_at, device_url)
         VALUES (?1, ?2, ?3, ?4)",
        params![track_id, at, unix_now(), device_url],
    )?;
    let id = tx.last_insert_rowid();
    tx.commit()?;
    Ok(id)
}

/// Peek the head of the queue without removing it. Returns None on
/// empty queue.
pub fn queue_peek(conn: &Connection, device_url: &str) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT track_id FROM play_queue WHERE device_url=?1
         ORDER BY ord ASC LIMIT 1",
        params![device_url], |r| r.get(0),
    ).optional()
}

/// Pop the head of the queue. Returns the track_id (or None if empty)
/// and re-densifies the remaining ords.
pub fn queue_pop(conn: &mut Connection, device_url: &str) -> rusqlite::Result<Option<i64>> {
    let tx = conn.transaction()?;
    let head: Option<(i64, i64)> = tx.query_row(
        "SELECT id, track_id FROM play_queue WHERE device_url=?1
         ORDER BY ord ASC LIMIT 1",
        params![device_url], |r| Ok((r.get(0)?, r.get(1)?)),
    ).optional()?;
    let (qid, tid) = match head { Some(v) => v, None => { tx.commit()?; return Ok(None); } };
    tx.execute("DELETE FROM play_queue WHERE id=?1", params![qid])?;
    tx.execute(r#"
        WITH ranked AS (
            SELECT id, ROW_NUMBER() OVER (ORDER BY ord) - 1 AS new_ord
            FROM play_queue WHERE device_url = ?1
        )
        UPDATE play_queue
        SET ord = (SELECT new_ord FROM ranked WHERE ranked.id = play_queue.id)
        WHERE device_url = ?1
    "#, params![device_url])?;
    tx.commit()?;
    Ok(Some(tid))
}

/// Remove a specific queue entry by its id (not track id — a track can
/// appear multiple times in the queue).
pub fn queue_remove(conn: &mut Connection, queue_id: i64) -> rusqlite::Result<()> {
    let tx = conn.transaction()?;
    let device_url: Option<String> = tx.query_row(
        "SELECT device_url FROM play_queue WHERE id=?1",
        params![queue_id], |r| r.get(0),
    ).optional()?;
    tx.execute("DELETE FROM play_queue WHERE id=?1", params![queue_id])?;
    if let Some(dev) = device_url {
        tx.execute(r#"
            WITH ranked AS (
                SELECT id, ROW_NUMBER() OVER (ORDER BY ord) - 1 AS new_ord
                FROM play_queue WHERE device_url = ?1
            )
            UPDATE play_queue
            SET ord = (SELECT new_ord FROM ranked WHERE ranked.id = play_queue.id)
            WHERE device_url = ?1
        "#, params![dev])?;
    }
    tx.commit()?;
    Ok(())
}

/// Clear the entire queue for a device (empty string = global queue).
pub fn queue_clear(conn: &Connection, device_url: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM play_queue WHERE device_url=?1", params![device_url])?;
    Ok(())
}

/// Queue entry with display metadata. `queue_id` is the row's own id
/// (for removal), `ord` is the position in the queue.
#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub queue_id: i64,
    pub track_id: i64,
    pub ord: i64,
    pub title: String,
    pub primary_artist: Option<String>,
    pub bpm: Option<f32>,
}

/// List all queue entries with display metadata, ordered by position.
pub fn queue_list(conn: &Connection, device_url: &str) -> rusqlite::Result<Vec<QueueEntry>> {
    let mut stmt = conn.prepare(r#"
        SELECT
            q.id, q.track_id, q.ord,
            COALESCE(ov.title, t.title) AS title,
            COALESCE(ov.artist,
                (SELECT a.name FROM track_artists ta
                   JOIN artists a ON a.id = ta.artist_id
                  WHERE ta.track_id = t.id AND ta.role = 'primary'
                  ORDER BY ta.ord ASC LIMIT 1)
            ) AS primary_artist,
            t.bpm
        FROM play_queue q
        JOIN tracks t ON t.id = q.track_id
        LEFT JOIN track_overrides ov ON ov.track_id = t.id
        WHERE q.device_url = ?1
        ORDER BY q.ord ASC
    "#)?;
    let rows = stmt.query_map(params![device_url], |r| {
        Ok(QueueEntry {
            queue_id: r.get(0)?,
            track_id: r.get(1)?,
            ord: r.get(2)?,
            title: r.get(3)?,
            primary_artist: r.get(4)?,
            bpm: r.get(5)?,
        })
    })?;
    rows.collect()
}

/// Summary of one device's queued items. Used by QueuePage's
/// per-device picker — when multiple devices have queued tracks,
/// the user can switch between them without changing which device
/// is currently active (which would re-route volume commands).
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct QueueDeviceSummary {
    pub device_url: String,
    pub item_count: u32,
    /// Title of the track at the head of this device's queue (what it's
    /// about to play / is playing). Lets the per-device strip show WHAT
    /// each speaker is set to, not just how many items are queued — the
    /// multi-speaker "different streams per device" view. Empty if the
    /// head track has no resolvable title.
    pub head_title: String,
}

/// All devices that currently have queued items, ordered by item
/// count descending then by URL alphabetically (stable picker order).
/// Empty queues are omitted from the list.
pub fn queue_devices(conn: &Connection) -> rusqlite::Result<Vec<QueueDeviceSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT q.device_url,
               COUNT(*) AS n,
               COALESCE((
                   SELECT t.title
                   FROM play_queue q2
                   JOIN tracks t ON t.id = q2.track_id
                   WHERE q2.device_url = q.device_url
                   ORDER BY q2.ord ASC
                   LIMIT 1
               ), '') AS head_title
        FROM play_queue q
        GROUP BY q.device_url
        HAVING n > 0
        ORDER BY n DESC, q.device_url ASC
    "#)?;
    let rows = stmt.query_map([], |r| {
        Ok(QueueDeviceSummary {
            device_url: r.get(0)?,
            item_count: r.get::<_, i64>(1)? as u32,
            head_title: r.get(2)?,
        })
    })?;
    rows.collect()
}

// ── Tag API ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TagInfo {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub track_count: i64,
}

/// Create or look up a tag by name. Tag names are normalized for
/// dedup but the original casing is preserved in the display name.
pub fn upsert_tag(conn: &Connection, name: &str, color: Option<&str>) -> rusqlite::Result<i64> {
    let norm = fold_text(name);
    if let Some(id) = conn.query_row(
        "SELECT id FROM tags WHERE norm=?1", params![norm], |r| r.get(0),
    ).optional()? {
        if let Some(c) = color {
            conn.execute("UPDATE tags SET color=?1 WHERE id=?2", params![c, id])?;
        }
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO tags (name, norm, color) VALUES (?1, ?2, ?3)",
        params![name, norm, color],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Attach a tag to a track. Idempotent. Also records the attachment in
/// the path-keyed durable mirror so it survives track-id churn on rescan.
pub fn attach_tag(conn: &Connection, track_id: i64, tag_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO track_tags (track_id, tag_id) VALUES (?1, ?2)",
        params![track_id, tag_id],
    )?;
    // Durable mirror keyed by (path, tag_name). Best-effort: if either
    // lookup fails the id-keyed row above still stands for this session.
    let _ = conn.execute(
        "INSERT OR IGNORE INTO track_tags_by_path (path, tag_name)
         SELECT t.path, g.name FROM tracks t, tags g
         WHERE t.id = ?1 AND g.id = ?2",
        params![track_id, tag_id],
    );
    Ok(())
}

/// Detach a tag from a track. Removes both the id-keyed row and the
/// durable path-keyed mirror so a re-link doesn't resurrect it.
pub fn detach_tag(conn: &Connection, track_id: i64, tag_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM track_tags WHERE track_id=?1 AND tag_id=?2",
        params![track_id, tag_id],
    )?;
    let _ = conn.execute(
        "DELETE FROM track_tags_by_path
         WHERE path = (SELECT path FROM tracks WHERE id = ?1)
           AND tag_name = (SELECT name FROM tags WHERE id = ?2)",
        params![track_id, tag_id],
    );
    Ok(())
}

/// Re-link path-keyed tag attachments back onto current track ids.
/// Called after every scan so manually-applied tags survive track-id
/// churn (re-inserts give new ids; the id-keyed track_tags rows cascade
/// away, but the path-keyed mirror persists and is replayed here).
///
/// Creates any missing tag rows by name, then re-inserts the id-keyed
/// attachment. Returns the number of (re)linked attachments.
pub fn relink_tags_by_path(conn: &Connection) -> rusqlite::Result<usize> {
    // For each durable (path, tag_name) whose track exists, ensure the
    // tag row exists and the id-keyed attachment is present.
    let mut stmt = conn.prepare(
        "SELECT m.path, m.tag_name, t.id
         FROM track_tags_by_path m
         JOIN tracks t ON t.path = m.path")?;
    let rows: Vec<(String, String, i64)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .filter_map(|r| r.ok())
        .collect();
    let mut linked = 0usize;
    // Wrap all the upserts/inserts in ONE transaction. Without this each
    // statement is its own implicit transaction (an fsync per row), which
    // on a library with hundreds of tagged tracks turns the post-scan
    // relink into hundreds of disk syncs — slow, and non-atomic if the
    // process dies mid-way. unchecked_transaction() takes &Connection so
    // it composes with upsert_tag(&Connection, ...) without a borrow clash.
    let txn = conn.unchecked_transaction()?;
    for (_path, tag_name, track_id) in rows {
        // Reuse upsert_tag so the tag is normalized identically to how
        // it was created originally (fold_text: lowercase + accent fold).
        // Using a different normalization here would create duplicate
        // tag rows or fail to match the existing one.
        let tag_id = upsert_tag(conn, &tag_name, None)?;
        let n = conn.execute(
            "INSERT OR IGNORE INTO track_tags (track_id, tag_id) VALUES (?1, ?2)",
            params![track_id, tag_id],
        )?;
        linked += n;
    }
    txn.commit()?;
    Ok(linked)
}

/// List all tags with their use counts.
pub fn list_tags(conn: &Connection) -> rusqlite::Result<Vec<TagInfo>> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.name, t.color,
               (SELECT COUNT(*) FROM track_tags tt WHERE tt.tag_id = t.id) AS track_count
        FROM tags t
        ORDER BY track_count DESC, t.name ASC
    "#)?;
    let rows = stmt.query_map([], |r| {
        Ok(TagInfo {
            id: r.get(0)?,
            name: r.get(1)?,
            color: r.get(2)?,
            track_count: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// All tags attached to a specific track.
pub fn track_tags(conn: &Connection, track_id: i64) -> rusqlite::Result<Vec<TagInfo>> {
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.name, t.color,
               (SELECT COUNT(*) FROM track_tags tt WHERE tt.tag_id = t.id) AS track_count
        FROM tags t
        JOIN track_tags tt ON tt.tag_id = t.id
        WHERE tt.track_id = ?1
        ORDER BY t.name ASC
    "#)?;
    let rows = stmt.query_map(params![track_id], |r| {
        Ok(TagInfo {
            id: r.get(0)?, name: r.get(1)?, color: r.get(2)?, track_count: r.get(3)?,
        })
    })?;
    rows.collect()
}

/// All tracks attached to a specific tag, with full TrackSummary
/// metadata so the UI can render full rows directly. Tracks ordered
/// by play_count DESC, most-played first.
pub fn tracks_by_tag(conn: &Connection, tag_id: i64, limit: usize) -> rusqlite::Result<Vec<TrackSummary>> {
    let mut stmt = conn.prepare(r#"
        SELECT
            t.id,
            t.path,
            COALESCE(ov.title, t.title) AS title,
            COALESCE(ov.album, t.album) AS album,
            COALESCE(ov.year,  t.year)  AS year,
            t.bpm,
            t.camelot,
            t.key_text,
            COALESCE(ov.artist,
                (SELECT a.name FROM track_artists ta
                   JOIN artists a ON a.id = ta.artist_id
                  WHERE ta.track_id = t.id AND ta.role = 'primary'
                  ORDER BY ta.ord ASC LIMIT 1),
                ''
            ) AS primary_artist
        FROM track_tags tt
        JOIN tracks t ON t.id = tt.track_id
        LEFT JOIN track_overrides ov ON ov.track_id = t.id
        WHERE tt.tag_id = ?1
        ORDER BY t.play_count DESC, t.last_played_at DESC, t.mtime DESC
        LIMIT ?2
    "#)?;
    let rows = stmt.query_map(params![tag_id, limit as i64], |r| {
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
            rating: r.get::<_, i64>("rating").unwrap_or(0),
        })
    })?;
    rows.collect()
}

// ── Play history ─────────────────────────────────────────────────────

/// Record a play event (in addition to incrementing tracks.play_count).
/// `duration_played` is optional — pass None when unknown.
pub fn record_history(conn: &Connection, track_id: i64, device_url: &str, duration_played: Option<i64>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
         VALUES (?1, ?2, ?3, ?4)",
        params![track_id, unix_now(), device_url, duration_played],
    )?;
    Ok(())
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub track_id: i64,
    pub played_at: i64,
    pub device_url: String,
    pub title: String,
    pub primary_artist: Option<String>,
}

/// Most recent N play history entries.
pub fn recent_history(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<HistoryEntry>> {
    let mut stmt = conn.prepare(r#"
        SELECT h.track_id, h.played_at, h.device_url,
               COALESCE(ov.title, t.title) AS title,
               COALESCE(ov.artist,
                   (SELECT a.name FROM track_artists ta
                      JOIN artists a ON a.id = ta.artist_id
                     WHERE ta.track_id = t.id AND ta.role = 'primary'
                     ORDER BY ta.ord ASC LIMIT 1)
               ) AS primary_artist
        FROM play_history h
        JOIN tracks t ON t.id = h.track_id
        LEFT JOIN track_overrides ov ON ov.track_id = t.id
        ORDER BY h.played_at DESC
        LIMIT ?1
    "#)?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        Ok(HistoryEntry {
            track_id: r.get(0)?,
            played_at: r.get(1)?,
            device_url: r.get(2)?,
            title: r.get(3)?,
            primary_artist: r.get(4)?,
        })
    })?;
    rows.collect()
}

/// Prune history older than `cutoff_unix`. Returns the number of rows removed.
pub fn prune_history(conn: &Connection, cutoff_unix: i64) -> rusqlite::Result<usize> {
    Ok(conn.execute("DELETE FROM play_history WHERE played_at < ?1", params![cutoff_unix])?)
}

/// Aggregate play counts grouped by hour-of-day (0–23). Returns
/// exactly 24 entries (zero-filling hours with no plays) so the
/// renderer can lay out a fixed-width bar chart without conditional
/// logic. Limited to plays within `lookback_days` so the chart
/// reflects recent listening habits rather than ancient history.
pub fn plays_by_hour_of_day(conn: &Connection, lookback_days: i64)
    -> rusqlite::Result<Vec<(u8, u32)>>
{
    let cutoff = chrono_now_unix() - (lookback_days * 86_400);
    let mut counts = [0u32; 24];
    let mut stmt = conn.prepare(r#"
        SELECT CAST(strftime('%H', datetime(played_at, 'unixepoch', 'localtime'))
                    AS INTEGER) AS hour,
               COUNT(*) AS n
        FROM play_history
        WHERE played_at >= ?1
        GROUP BY hour
        ORDER BY hour ASC
    "#)?;
    let rows = stmt.query_map(params![cutoff], |r| {
        let hour: i64 = r.get(0)?;
        let n: i64 = r.get(1)?;
        Ok((hour as u8, n as u32))
    })?;
    for row in rows {
        let (h, n) = row?;
        if h < 24 { counts[h as usize] = n; }
    }
    Ok((0u8..24).map(|h| (h, counts[h as usize])).collect())
}

/// Wall-clock now() as Unix seconds. Library code uses `std::time`
/// rather than chrono to keep the dependency surface small.
fn chrono_now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Rolling history summary — total plays, distinct tracks heard, the
/// single most-played track, and the most-played primary artist, all
/// over the last N days. Returned as a serializable shape (the QML
/// side renders KPI cards from this). Each "top" field is None when
/// no plays exist in the window.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct HistorySummary {
    pub total_plays: u32,
    pub distinct_tracks: u32,
    pub top_track: Option<HistoryTopTrack>,
    pub top_artist: Option<HistoryTopArtist>,
    pub lookback_days: u32,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct HistoryTopTrack {
    pub track_id: i64,
    pub title: String,
    pub primary_artist: String,
    pub play_count: u32,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct HistoryTopArtist {
    pub name: String,
    pub play_count: u32,
}

pub fn history_summary(conn: &Connection, lookback_days: i64)
    -> rusqlite::Result<HistorySummary>
{
    let cutoff = chrono_now_unix() - (lookback_days * 86_400);

    // Total plays + distinct tracks in one query.
    let (total_plays, distinct_tracks): (u32, u32) = conn.query_row(
        "SELECT COUNT(*), COUNT(DISTINCT track_id)
         FROM play_history WHERE played_at >= ?1",
        params![cutoff],
        |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u32)),
    ).unwrap_or((0, 0));

    // Top track by play count in the window. We join the track table
    // for title and the artist table (via track_artists primary role)
    // for the byline. LIMIT 1 because we only want the leader.
    let top_track = conn.query_row(r#"
        SELECT h.track_id,
               COALESCE(ov.title, t.title) AS title,
               COALESCE(ov.artist,
                   (SELECT a.name FROM track_artists ta
                      JOIN artists a ON a.id = ta.artist_id
                     WHERE ta.track_id = t.id AND ta.role = 'primary'
                     ORDER BY ta.ord ASC LIMIT 1),
                   ''
               ) AS primary_artist,
               COUNT(*) AS plays
        FROM play_history h
        JOIN tracks t ON t.id = h.track_id
        LEFT JOIN track_overrides ov ON ov.track_id = t.id
        WHERE h.played_at >= ?1
        GROUP BY h.track_id
        ORDER BY plays DESC, h.played_at DESC
        LIMIT 1
    "#, params![cutoff], |r| {
        Ok(HistoryTopTrack {
            track_id:       r.get(0)?,
            title:          r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            primary_artist: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
            play_count:     r.get::<_, i64>(3)? as u32,
        })
    }).ok();

    // Top artist by aggregated play count. We aggregate across tracks
    // by joining track_artists (filtering for primary role only — a
    // featured artist on one track doesn't get credited as if they
    // were the headline).
    let top_artist = conn.query_row(r#"
        SELECT a.name, COUNT(*) AS plays
        FROM play_history h
        JOIN tracks t   ON t.id = h.track_id
        JOIN track_artists ta ON ta.track_id = t.id AND ta.role = 'primary'
        JOIN artists a ON a.id = ta.artist_id
        WHERE h.played_at >= ?1
        GROUP BY a.id
        ORDER BY plays DESC, a.name ASC
        LIMIT 1
    "#, params![cutoff], |r| {
        Ok(HistoryTopArtist {
            name:       r.get(0)?,
            play_count: r.get::<_, i64>(1)? as u32,
        })
    }).ok();

    Ok(HistorySummary {
        total_plays,
        distinct_tracks,
        top_track,
        top_artist,
        lookback_days: lookback_days as u32,
    })
}

// ── Artist aggregation ───────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ArtistDetail {
    pub id: i64,
    pub name: String,
    pub track_count: i64,
    pub total_plays: i64,
    pub last_played_at: Option<i64>,
}

/// Top artists ordered by total play count. Useful for the home page's
/// "most listened to" section.
pub fn top_artists_by_plays(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<ArtistDetail>> {
    let mut stmt = conn.prepare(r#"
        SELECT a.id, a.name,
               COUNT(DISTINCT ta.track_id) AS track_count,
               COALESCE(SUM(t.play_count), 0) AS total_plays,
               MAX(t.last_played_at) AS last_played_at
        FROM artists a
        JOIN track_artists ta ON ta.artist_id = a.id AND ta.role = 'primary'
        JOIN tracks t ON t.id = ta.track_id
        GROUP BY a.id
        ORDER BY total_plays DESC, track_count DESC, a.name ASC
        LIMIT ?1
    "#)?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        Ok(ArtistDetail {
            id: r.get(0)?,
            name: r.get(1)?,
            track_count: r.get(2)?,
            total_plays: r.get(3)?,
            last_played_at: r.get(4)?,
        })
    })?;
    rows.collect()
}

/// All artists alphabetically.
pub fn all_artists_alpha(conn: &Connection) -> rusqlite::Result<Vec<ArtistDetail>> {
    all_artists_alpha_base(conn)
}

/// Base (name-ordered) artist aggregation with play counts. Used by the
/// unpaginated alpha list + the top-artists fallback. (The paginated
/// ArtistsPage uses catalog::artists_with_counts_page_sorted, which sorts
/// in SQL; this richer aggregation is for callers that want play totals.)
fn all_artists_alpha_base(conn: &Connection) -> rusqlite::Result<Vec<ArtistDetail>> {
    let mut stmt = conn.prepare(r#"
        SELECT a.id, a.name,
               COUNT(DISTINCT ta.track_id) AS track_count,
               COALESCE(SUM(t.play_count), 0) AS total_plays,
               MAX(t.last_played_at) AS last_played_at
        FROM artists a
        JOIN track_artists ta ON ta.artist_id = a.id
        JOIN tracks t ON t.id = ta.track_id
        GROUP BY a.id
        ORDER BY a.name COLLATE NOCASE ASC
    "#)?;
    let rows = stmt.query_map([], |r| {
        Ok(ArtistDetail {
            id: r.get(0)?,
            name: r.get(1)?,
            track_count: r.get(2)?,
            total_plays: r.get(3)?,
            last_played_at: r.get(4)?,
        })
    })?;
    rows.collect()
}

// ── Improved search ──────────────────────────────────────────────────

/// Multi-term AND search with prefix matching + ranking.
/// Lightweight album record for the QML side. An album is identified
/// by the (album_title, primary_artist) pair — we don't keep a
/// separate `albums` table because metadata already lives on each
/// track row and the natural unit of mutation is "edit a track tag",
/// not "edit an album". Aggregations are computed on demand.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AlbumSummary {
    pub title:          String,
    pub primary_artist: String,
    /// Most-common year across tracks in the album. Sometimes albums
    /// have inconsistent year tags (re-releases, compilations); we
    /// just pick the most-frequent value.
    pub year:           Option<u16>,
    pub track_count:    u32,
    pub total_plays:    u32,
    pub last_played_at: Option<i64>,
}

/// Top albums by total play count across their constituent tracks.
/// Excludes tracks with empty album field and albums whose total
/// play count is zero — those are surfaced via `all_albums_alpha`
/// instead, since "top played" implies actual listening history.
pub fn top_albums_by_plays(conn: &Connection, limit: usize)
    -> rusqlite::Result<Vec<AlbumSummary>>
{
    let mut stmt = conn.prepare(r#"
        SELECT
            COALESCE(t.album, '') AS album_title,
            COALESCE(
                (SELECT a.name FROM artists a
                  JOIN track_artists ta ON ta.artist_id = a.id
                 WHERE ta.track_id = t.id AND ta.role = 'primary'
                 ORDER BY ta.ord LIMIT 1), '') AS primary_artist,
            COUNT(*) AS track_count,
            COALESCE(SUM(t.play_count), 0) AS total_plays,
            MAX(t.last_played_at) AS last_played_at,
            t.year AS year_sample
        FROM tracks t
        WHERE t.album IS NOT NULL AND t.album != ''
        GROUP BY t.album, primary_artist
        HAVING total_plays > 0
        ORDER BY total_plays DESC, last_played_at DESC
        LIMIT ?1
    "#)?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        Ok(AlbumSummary {
            title:          r.get(0)?,
            primary_artist: r.get(1)?,
            track_count:    r.get::<_, i64>(2)? as u32,
            total_plays:    r.get::<_, i64>(3)? as u32,
            last_played_at: r.get(4)?,
            // year_sample is just SOME track's year — we'd prefer
            // the mode, but the mode-by-group query is gnarly in
            // pure SQLite. As a compromise we use the sample plus
            // a second pass below to grab the most-common year.
            year:           r.get::<_, Option<i64>>(5)?
                              .map(|y| y.clamp(0, 9999) as u16),
        })
    })?;
    let mut albums: Vec<AlbumSummary> = rows.collect::<rusqlite::Result<_>>()?;
    // Backfill year from the most-common year for each (album, artist).
    refine_album_years(conn, &mut albums)?;
    Ok(albums)
}

/// All albums sorted alphabetically by title, then by primary artist.
/// Includes albums with zero plays (useful for "I haven't listened to
/// X in a while" browsing). Matches `all_artists_alpha`'s ergonomics.
pub fn all_albums_alpha(conn: &Connection)
    -> rusqlite::Result<Vec<AlbumSummary>>
{
    all_albums_sorted(conn, AlbumSort::Title)
}

/// Sort modes for the albums browse list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumSort {
    Title,
    Artist,
    Year,
    MostPlayed,
}

impl AlbumSort {
    pub fn from_tag(s: &str) -> Self {
        match s {
            "artist" => AlbumSort::Artist,
            "year"   => AlbumSort::Year,
            "plays"  => AlbumSort::MostPlayed,
            _         => AlbumSort::Title,
        }
    }
    pub fn to_tag(self) -> &'static str {
        match self {
            AlbumSort::Title      => "title",
            AlbumSort::Artist     => "artist",
            AlbumSort::Year       => "year",
            AlbumSort::MostPlayed => "plays",
        }
    }
}

/// All albums in a chosen order. The base query already returns every
/// album with the fields we sort on, so we order in Rust (the album
/// cardinality is small — a few hundred — so this is cheap and keeps the
/// SQL single-purpose).
pub fn all_albums_sorted(conn: &Connection, sort: AlbumSort)
    -> rusqlite::Result<Vec<AlbumSummary>>
{
    let mut albums = all_albums_alpha_base(conn)?;
    match sort {
        AlbumSort::Title => {
            // base query already title-ordered; keep as-is.
        }
        AlbumSort::Artist => albums.sort_by(|a, b| {
            a.primary_artist.to_lowercase().cmp(&b.primary_artist.to_lowercase())
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
        AlbumSort::Year => albums.sort_by(|a, b| {
            // Newest first; albums with no year sink to the bottom.
            b.year.unwrap_or(0).cmp(&a.year.unwrap_or(0))
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
        AlbumSort::MostPlayed => albums.sort_by(|a, b| {
            b.total_plays.cmp(&a.total_plays)
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
    }
    Ok(albums)
}

/// The base (title-ordered) album aggregation. Factored out so the sorted
/// wrapper can reorder without duplicating the query.
fn all_albums_alpha_base(conn: &Connection)
    -> rusqlite::Result<Vec<AlbumSummary>>
{
    let mut stmt = conn.prepare(r#"
        SELECT
            COALESCE(t.album, '') AS album_title,
            COALESCE(
                (SELECT a.name FROM artists a
                  JOIN track_artists ta ON ta.artist_id = a.id
                 WHERE ta.track_id = t.id AND ta.role = 'primary'
                 ORDER BY ta.ord LIMIT 1), '') AS primary_artist,
            COUNT(*) AS track_count,
            COALESCE(SUM(t.play_count), 0) AS total_plays,
            MAX(t.last_played_at) AS last_played_at,
            t.year AS year_sample
        FROM tracks t
        WHERE t.album IS NOT NULL AND t.album != ''
        GROUP BY t.album, primary_artist
        ORDER BY album_title COLLATE NOCASE, primary_artist COLLATE NOCASE
    "#)?;
    let rows = stmt.query_map([], |r| {
        Ok(AlbumSummary {
            title:          r.get(0)?,
            primary_artist: r.get(1)?,
            track_count:    r.get::<_, i64>(2)? as u32,
            total_plays:    r.get::<_, i64>(3)? as u32,
            last_played_at: r.get(4)?,
            year:           r.get::<_, Option<i64>>(5)?
                              .map(|y| y.clamp(0, 9999) as u16),
        })
    })?;
    let mut albums: Vec<AlbumSummary> = rows.collect::<rusqlite::Result<_>>()?;
    refine_album_years(conn, &mut albums)?;
    Ok(albums)
}

/// For each album, replace the year-sample with the most-common year
/// across the album's tracks. Compilations and rereleases often have
/// inconsistent year tags; "most common" is the right summary.
///
/// Done as a per-album second query rather than a giant join: simpler
/// to read, and N+1 isn't a problem at the cardinalities we deal with
/// (a typical user library is a few hundred albums).
fn refine_album_years(conn: &Connection, albums: &mut [AlbumSummary])
    -> rusqlite::Result<()>
{
    let mut stmt = conn.prepare(r#"
        SELECT t.year, COUNT(*) as cnt
        FROM tracks t
        WHERE t.album = ?1 AND t.year IS NOT NULL
          AND COALESCE(
                (SELECT a.name FROM artists a
                  JOIN track_artists ta ON ta.artist_id = a.id
                 WHERE ta.track_id = t.id AND ta.role = 'primary'
                 ORDER BY ta.ord LIMIT 1), '') = ?2
        GROUP BY t.year
        ORDER BY cnt DESC, t.year ASC
        LIMIT 1
    "#)?;
    for a in albums.iter_mut() {
        let mut rows = stmt.query(params![a.title, a.primary_artist])?;
        if let Some(row) = rows.next()? {
            let y: Option<i64> = row.get(0)?;
            if let Some(yy) = y {
                a.year = Some(yy.clamp(0, 9999) as u16);
            }
        }
    }
    Ok(())
}

/// Tracks in a specific album, ordered by track title (we don't have
/// a track-number field, so alphabetical is the best stable order).
/// Filters by both album title AND primary artist to disambiguate
/// the common case where the same album title appears under multiple
/// artists (e.g. "Greatest Hits" by 47 different bands).
pub fn tracks_in_album(conn: &Connection, album_title: &str, primary_artist: &str)
    -> rusqlite::Result<Vec<crate::media::catalog::TrackSummary>>
{
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE(
                   (SELECT a.name FROM artists a
                     JOIN track_artists ta ON ta.artist_id = a.id
                    WHERE ta.track_id = t.id AND ta.role = 'primary'
                    ORDER BY ta.ord LIMIT 1), '') AS primary_artist
        FROM tracks t
        WHERE t.album = ?1
        ORDER BY t.title COLLATE NOCASE
    "#)?;
    let all = crate::media::catalog::rows_to_summaries(&mut stmt, params![album_title])?;
    // Filter by primary_artist in Rust — the SQL would need a
    // subquery-on-correlated-result to do it in one shot, and the
    // typical bag of tracks-for-an-album is tiny.
    let filtered: Vec<_> = all.into_iter()
        .filter(|t| t.primary_artist == primary_artist)
        .collect();
    Ok(filtered)
}


/// Tracks added to the library most recently. Ordered by file
/// mtime descending — this matches "what did I just import?" better
/// than parsed_at or analyzed_at would, since file mtime is set the
/// moment the user drops a file in their music dir.
///
/// Limit clamps at 200 to keep the result list reasonable for UI
/// rendering — beyond that the user should be using Search.
pub fn recently_added(conn: &Connection, limit: usize)
    -> rusqlite::Result<Vec<TrackSummary>>
{
    super::catalog::recent(conn, limit.min(200))
}

/// Top individual tracks by play_count. Complements top_artists +
/// top_albums for the "what do I actually listen to?" question
/// drilled down to the song level.
///
/// Excludes tracks with zero plays — those belong in Library /
/// recently_added, not a "top played" list.
pub fn top_tracks_by_plays(conn: &Connection, limit: usize)
    -> rusqlite::Result<Vec<TrackSummary>>
{
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE(
                   (SELECT a.name FROM artists a
                     JOIN track_artists ta ON ta.artist_id = a.id
                    WHERE ta.track_id = t.id AND ta.role = 'primary'
                    ORDER BY ta.ord LIMIT 1), '') AS primary_artist
        FROM tracks t
        WHERE t.play_count > 0
        ORDER BY t.play_count DESC, t.last_played_at DESC
        LIMIT ?1
    "#)?;
    super::catalog::rows_to_summaries(&mut stmt, params![limit as i64])
}

/// Tracks that have never been played. Useful for "rediscover what
/// I imported and forgot about". Ordered by mtime descending so
/// recent imports appear first.
pub fn never_played(conn: &Connection, limit: usize)
    -> rusqlite::Result<Vec<TrackSummary>>
{
    let mut stmt = conn.prepare(r#"
        SELECT t.id, t.path, t.title, t.album, t.year, t.bpm, t.camelot, t.key_text,
               COALESCE(
                   (SELECT a.name FROM artists a
                     JOIN track_artists ta ON ta.artist_id = a.id
                    WHERE ta.track_id = t.id AND ta.role = 'primary'
                    ORDER BY ta.ord LIMIT 1), '') AS primary_artist
        FROM tracks t
        WHERE t.play_count = 0
        ORDER BY t.mtime DESC
        LIMIT ?1
    "#)?;
    super::catalog::rows_to_summaries(&mut stmt, params![limit as i64])
}


/// Full-text search. Tokenizes on whitespace, every term must appear
/// in the track's normalized search blob (title + artist + album).
pub fn search_advanced(conn: &Connection, query: &str, limit: usize) -> rusqlite::Result<Vec<TrackSummary>> {
    let folded = fold_text(query);
    let terms: Vec<&str> = folded.split_whitespace().collect();
    if terms.is_empty() {
        // Empty query → return recents.
        return super::catalog::recent(conn, limit);
    }

    // Build "search LIKE ? AND search LIKE ? ..." pattern.
    let where_clauses: Vec<String> = terms.iter().map(|_| "search LIKE ?".to_string()).collect();
    let where_sql = where_clauses.join(" AND ");
    let sql = format!(r#"
        SELECT
            t.id,
            t.path,
            COALESCE(ov.title, t.title) AS title,
            COALESCE(ov.album, t.album) AS album,
            COALESCE(ov.year,  t.year)  AS year,
            t.bpm,
            t.camelot,
            t.key_text,
            COALESCE(ov.artist,
                (SELECT a.name FROM track_artists ta
                   JOIN artists a ON a.id = ta.artist_id
                  WHERE ta.track_id = t.id AND ta.role = 'primary'
                  ORDER BY ta.ord ASC LIMIT 1),
                ''
            ) AS primary_artist
        FROM tracks t
        LEFT JOIN track_overrides ov ON ov.track_id = t.id
        WHERE {}
        ORDER BY t.play_count DESC, t.last_played_at DESC, t.mtime DESC
        LIMIT ?
    "#, where_sql);

    let mut stmt = conn.prepare(&sql)?;
    let bind_strings: Vec<String> = terms.iter().map(|t| format!("%{}%", t)).collect();
    let mut params_dyn: Vec<&dyn rusqlite::ToSql> = bind_strings.iter()
        .map(|s| s as &dyn rusqlite::ToSql).collect();
    let lim = limit as i64;
    params_dyn.push(&lim);

    let rows = stmt.query_map(params_dyn.as_slice(), |r| {
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
            rating: r.get::<_, i64>("rating").unwrap_or(0),
        })
    })?;
    let mut all: Vec<TrackSummary> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    // ── Dedup by (case-folded title, case-folded primary_artist) ──
    //
    // The user's library has many tracks with auto-generated titles
    // (e.g. "track 7947", "track 7946" from YouTube-DL output) and
    // the resulting search list looks duplicate-heavy: same title
    // text repeated across slightly different files. The path column
    // is unique (UNIQUE constraint on tracks.path) so SQL doesn't
    // emit literal dupes, but the *visual* dedup the user expects
    // is by what they read, not by what's distinct in the DB.
    //
    // Strategy: group by (lower(title), lower(artist)). Keep the
    // first occurrence — already in the order returned by the SQL
    // (play_count DESC, last_played_at DESC, mtime DESC), so the
    // representative is the most-played / most-recent file. A
    // future "show variants" UI can fetch hidden rows by querying
    // tracks where lower(title)=? and lower(artist)=?.
    let mut seen: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();
    let before = all.len();
    all.retain(|t| {
        let k = (t.title.to_lowercase(), t.primary_artist.to_lowercase());
        seen.insert(k)
    });
    if before != all.len() {
        eprintln!("search_advanced: dedup collapsed {} → {} rows \
                   (same title + primary_artist)", before, all.len());
    }
    Ok(all)
}

// ── Utilities ────────────────────────────────────────────────────────

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::catalog;
    use std::path::PathBuf;

    fn fresh_conn() -> Connection {
        // Use an in-memory DB but we need to invoke the same migrations.
        // catalog::open takes a path; for tests we use a tempfile path.
        let tmp = std::env::temp_dir().join(format!(
            "bo_test_lib_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        // Best-effort: remove if it exists from a prior partial test.
        let _ = std::fs::remove_file(&tmp);
        let conn = catalog::open(&tmp).expect("open db");
        conn
    }

    fn insert_test_track(conn: &Connection, title: &str, artist: &str) -> i64 {
        catalog::upsert_path(conn, &format!("/test/{}.mp3", title),
                             1234, 0).expect("upsert");
        let tid: i64 = conn.query_row(
            "SELECT id FROM tracks WHERE path=?1",
            params![format!("/test/{}.mp3", title)],
            |r| r.get(0),
        ).expect("get id");
        // Manually set title + create an artist link
        conn.execute("UPDATE tracks SET title=?1 WHERE id=?2",
                     params![title, tid]).expect("update");
        let norm = fold_text(artist);
        conn.execute("INSERT OR IGNORE INTO artists (name, norm) VALUES (?1, ?2)",
                     params![artist, norm]).expect("ins artist");
        let aid: i64 = conn.query_row(
            "SELECT id FROM artists WHERE norm=?1", params![norm], |r| r.get(0),
        ).expect("get aid");
        conn.execute(
            "INSERT INTO track_artists (track_id, artist_id, role, ord)
             VALUES (?1, ?2, 'primary', 0)",
            params![tid, aid],
        ).expect("link");
        // Search blob
        let blob = catalog::build_search_blob(title, &[artist.to_string()], None);
        conn.execute("UPDATE tracks SET search=?1 WHERE id=?2",
                     params![blob, tid]).expect("blob");
        tid
    }

    #[test]
    fn rating_set_get_clamp_and_persist_by_path() {
        let mut conn = fresh_conn();
        let tid = insert_test_track(&conn, "RatedSong", "Artist");
        assert_eq!(catalog::track_rating(&conn, tid).unwrap(), 0);
        catalog::set_track_rating(&mut conn, tid, 4).unwrap();
        assert_eq!(catalog::track_rating(&conn, tid).unwrap(), 4);
        catalog::set_track_rating(&mut conn, tid, 99).unwrap();
        assert_eq!(catalog::track_rating(&conn, tid).unwrap(), 5);
        catalog::set_track_rating(&mut conn, tid, -3).unwrap();
        assert_eq!(catalog::track_rating(&conn, tid).unwrap(), 0);
        catalog::set_track_rating(&mut conn, tid, 3).unwrap();
        let mirrored: i64 = conn.query_row(
            "SELECT rating FROM track_rating_by_path WHERE path = '/test/RatedSong.mp3'",
            [], |r| r.get(0)).unwrap();
        assert_eq!(mirrored, 3);
    }

    #[test]
    fn rating_relinks_after_id_change() {
        let mut conn = fresh_conn();
        let tid = insert_test_track(&conn, "Persistent", "Artist");
        catalog::set_track_rating(&mut conn, tid, 5).unwrap();
        conn.execute("UPDATE tracks SET rating = 0 WHERE id = ?1", params![tid]).unwrap();
        assert_eq!(catalog::track_rating(&conn, tid).unwrap(), 0);
        let n = catalog::relink_ratings_by_path(&conn).unwrap();
        assert_eq!(n, 1);
        assert_eq!(catalog::track_rating(&conn, tid).unwrap(), 5);
        let n2 = catalog::relink_ratings_by_path(&conn).unwrap();
        assert_eq!(n2, 0);
    }


    #[test]
    fn all_tracks_includes_tracks_with_no_artist() {
        // A track with NO track_artists link must still appear in
        // all_tracks_page — the belt-and-suspenders guarantee that no
        // track is ever unreachable (the "missing tracks" bug).
        let conn = fresh_conn();
        // One track WITH an artist (via the helper)...
        insert_test_track(&conn, "Has Artist", "Some Artist");
        // ...and one WITHOUT any artist link at all.
        catalog::upsert_path(&conn, "/test/orphan.mp3", 100, 100).unwrap();
        let orphan_id: i64 = conn.query_row(
            "SELECT id FROM tracks WHERE path=?1",
            params!["/test/orphan.mp3"], |r| r.get(0)).unwrap();
        conn.execute("UPDATE tracks SET title=?1 WHERE id=?2",
                     params!["Orphan Track", orphan_id]).unwrap();

        let (rows, total) = catalog::all_tracks_page(&conn, 0, 100).unwrap();
        assert_eq!(total, 2, "both tracks counted");
        let titles: Vec<&str> = rows.iter().map(|t| t.title.as_str()).collect();
        assert!(titles.contains(&"Has Artist"));
        assert!(titles.contains(&"Orphan Track"),
            "a track with no artist link must still be reachable");
        // The orphan's artist should COALESCE to the sentinel.
        let orphan = rows.iter().find(|t| t.title == "Orphan Track").unwrap();
        assert_eq!(orphan.primary_artist, "Unknown Artist");
        // all_tracks_page must surface the rating column (defaults 0).
        let has = rows.iter().find(|t| t.title == "Has Artist").unwrap();
        assert_eq!(has.rating, 0, "unrated track reads 0 through all_tracks_page");
    }

    #[test]
    fn all_tracks_page_surfaces_set_rating() {
        let mut conn = fresh_conn();
        let tid = insert_test_track(&conn, "Star Track", "Artist");
        catalog::set_track_rating(&mut conn, tid, 4).unwrap();
        let (rows, _) = catalog::all_tracks_page(&conn, 0, 100).unwrap();
        let row = rows.iter().find(|t| t.id == tid).unwrap();
        assert_eq!(row.rating, 4, "rating set should surface in the all-tracks list");
    }

    #[test]
    fn artist_page_sort_modes_order_correctly() {
        use catalog::ArtistPageSort;
        let conn = fresh_conn();
        // Beta: 2 tracks. Alpha: 1 track. Gamma: 3 tracks.
        insert_test_track(&conn, "b1", "Beta");
        insert_test_track(&conn, "b2", "Beta");
        insert_test_track(&conn, "a1", "Alpha");
        insert_test_track(&conn, "g1", "Gamma");
        insert_test_track(&conn, "g2", "Gamma");
        insert_test_track(&conn, "g3", "Gamma");

        let names = |v: &[catalog::GroupCount]| v.iter().map(|a| a.name.clone()).collect::<Vec<_>>();
        // Name: Alpha, Beta, Gamma.
        let (n, total) = catalog::artists_with_counts_page_sorted(
            &conn, 0, 100, ArtistPageSort::Name).unwrap();
        assert_eq!(names(&n), vec!["Alpha", "Beta", "Gamma"]);
        assert_eq!(total, 3);
        // Track count desc: Gamma(3), Beta(2), Alpha(1).
        let (t, _) = catalog::artists_with_counts_page_sorted(
            &conn, 0, 100, ArtistPageSort::TrackCount).unwrap();
        assert_eq!(names(&t), vec!["Gamma", "Beta", "Alpha"]);
        // Tag fallback.
        assert_eq!(ArtistPageSort::from_tag("zzz"), ArtistPageSort::Name);
        assert_eq!(ArtistPageSort::from_tag("tracks"), ArtistPageSort::TrackCount);
    }

    #[test]
    fn album_sort_modes_order_correctly() {
        use catalog as _cat; // silence unused if any
        let conn = fresh_conn();
        // Three tracks → three albums with distinct year/plays/artist.
        let t1 = insert_test_track(&conn, "s1", "Beta");
        let t2 = insert_test_track(&conn, "s2", "Alpha");
        let t3 = insert_test_track(&conn, "s3", "Gamma");
        conn.execute("UPDATE tracks SET album='Zed', year=1999, play_count=1 WHERE id=?1", params![t1]).unwrap();
        conn.execute("UPDATE tracks SET album='Mid', year=2020, play_count=9 WHERE id=?1", params![t2]).unwrap();
        conn.execute("UPDATE tracks SET album='Ace', year=2010, play_count=5 WHERE id=?1", params![t3]).unwrap();

        let titles = |v: &[AlbumSummary]| v.iter().map(|a| a.title.clone()).collect::<Vec<_>>();
        // Title: Ace, Mid, Zed.
        let t = all_albums_sorted(&conn, AlbumSort::Title).unwrap();
        assert_eq!(titles(&t), vec!["Ace", "Mid", "Zed"]);
        // Year (newest first): Mid(2020), Ace(2010), Zed(1999).
        let y = all_albums_sorted(&conn, AlbumSort::Year).unwrap();
        assert_eq!(titles(&y), vec!["Mid", "Ace", "Zed"]);
        // Most played: Mid(9), Ace(5), Zed(1).
        let p = all_albums_sorted(&conn, AlbumSort::MostPlayed).unwrap();
        assert_eq!(titles(&p), vec!["Mid", "Ace", "Zed"]);
        // Artist: Alpha(Mid), Beta(Zed), Gamma(Ace).
        let ar = all_albums_sorted(&conn, AlbumSort::Artist).unwrap();
        assert_eq!(titles(&ar), vec!["Mid", "Zed", "Ace"]);
        // Tag fallback.
        assert_eq!(AlbumSort::from_tag("zzz"), AlbumSort::Title);
        assert_eq!(AlbumSort::from_tag("plays"), AlbumSort::MostPlayed);
    }

    #[test]
    fn all_tracks_sort_modes_order_correctly() {
        use catalog::TrackSort;
        let mut conn = fresh_conn();
        // Titles deliberately out of insertion order.
        let _c = insert_test_track(&conn, "Charlie", "Zeta");
        let a = insert_test_track(&conn, "Alpha", "Yankee");
        let _b = insert_test_track(&conn, "Bravo", "Xray");
        catalog::set_track_rating(&mut conn, a, 5).unwrap();

        // Title sort: Alpha, Bravo, Charlie.
        let (t, _) = catalog::all_tracks_page_sorted(&conn, 0, 100, TrackSort::Title).unwrap();
        assert_eq!(t.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(),
                   vec!["Alpha", "Bravo", "Charlie"]);
        // Artist sort: Xray(Bravo), Yankee(Alpha), Zeta(Charlie).
        let (ar, _) = catalog::all_tracks_page_sorted(&conn, 0, 100, TrackSort::Artist).unwrap();
        assert_eq!(ar.iter().map(|x| x.title.as_str()).collect::<Vec<_>>(),
                   vec!["Bravo", "Alpha", "Charlie"]);
        // Recently added: newest id first → Bravo, Alpha, Charlie.
        let (rc, _) = catalog::all_tracks_page_sorted(&conn, 0, 100, TrackSort::RecentlyAdded).unwrap();
        assert_eq!(rc.first().unwrap().title, "Bravo", "newest inserted first");
        // Rating desc: Alpha (5) first.
        let (rt, _) = catalog::all_tracks_page_sorted(&conn, 0, 100, TrackSort::RatingDesc).unwrap();
        assert_eq!(rt.first().unwrap().title, "Alpha", "highest rated first");
        // Tag parsing falls back to Title for garbage.
        assert_eq!(TrackSort::from_tag("nonsense"), TrackSort::Title);
        assert_eq!(TrackSort::from_tag("rating"), TrackSort::RatingDesc);
    }

    #[test]
    fn all_tracks_pagination_bounds() {
        let conn = fresh_conn();
        for i in 0..5 { insert_test_track(&conn, &format!("T{}", i), "A"); }
        let (page1, total) = catalog::all_tracks_page(&conn, 0, 2).unwrap();
        assert_eq!(total, 5);
        assert_eq!(page1.len(), 2);
        let (page3, _) = catalog::all_tracks_page(&conn, 4, 2).unwrap();
        assert_eq!(page3.len(), 1, "last page has the remainder");
    }

    #[test]
    fn create_and_list_playlists() {
        let conn = fresh_conn();
        let id1 = create_playlist(&conn, "Morning", "wake up vibes").unwrap();
        let id2 = create_playlist(&conn, "Workout", "fast").unwrap();
        assert_ne!(id1, id2);
        let pls = list_playlists(&conn).unwrap();
        assert_eq!(pls.len(), 2);
        let names: Vec<&str> = pls.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"Morning"));
        assert!(names.contains(&"Workout"));
    }

    #[test]
    fn add_to_playlist_no_dupes() {
        let conn = fresh_conn();
        let pid = create_playlist(&conn, "Test", "").unwrap();
        let tid = insert_test_track(&conn, "Track A", "Artist A");
        let ord1 = add_to_playlist(&conn, pid, tid).unwrap();
        let ord2 = add_to_playlist(&conn, pid, tid).unwrap();
        assert_eq!(ord1, ord2, "duplicate add should be idempotent");
        let tracks = tracks_in_playlist(&conn, pid).unwrap();
        assert_eq!(tracks.len(), 1);
    }

    #[test]
    fn add_many_skips_duplicates() {
        let mut conn = fresh_conn();
        let pid = create_playlist(&conn, "X", "").unwrap();
        let t1 = insert_test_track(&conn, "A", "Artist A");
        let t2 = insert_test_track(&conn, "B", "Artist B");
        let t3 = insert_test_track(&conn, "C", "Artist C");
        let n = add_many_to_playlist(&mut conn, pid, &[t1, t2, t3]).unwrap();
        assert_eq!(n, 3);
        // Second call adds nothing new.
        let n2 = add_many_to_playlist(&mut conn, pid, &[t1, t2]).unwrap();
        assert_eq!(n2, 0);
    }

    #[test]
    fn playlist_order_preserved() {
        let conn = fresh_conn();
        let pid = create_playlist(&conn, "P", "").unwrap();
        let t1 = insert_test_track(&conn, "A", "Artist1");
        let t2 = insert_test_track(&conn, "B", "Artist2");
        let t3 = insert_test_track(&conn, "C", "Artist3");
        add_to_playlist(&conn, pid, t1).unwrap();
        add_to_playlist(&conn, pid, t2).unwrap();
        add_to_playlist(&conn, pid, t3).unwrap();
        let tracks = tracks_in_playlist(&conn, pid).unwrap();
        let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![t1, t2, t3]);
    }

    #[test]
    fn reorder_moves_track_up() {
        let mut conn = fresh_conn();
        let pid = create_playlist(&conn, "P", "").unwrap();
        let t1 = insert_test_track(&conn, "A", "A1");
        let t2 = insert_test_track(&conn, "B", "B1");
        let t3 = insert_test_track(&conn, "C", "C1");
        add_to_playlist(&conn, pid, t1).unwrap();
        add_to_playlist(&conn, pid, t2).unwrap();
        add_to_playlist(&conn, pid, t3).unwrap();
        // Move t3 (currently at ord 2) to ord 0.
        reorder_playlist(&mut conn, pid, t3, 0).unwrap();
        let ids: Vec<i64> = tracks_in_playlist(&conn, pid).unwrap()
            .iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![t3, t1, t2]);
    }

    #[test]
    fn reorder_moves_track_down() {
        let mut conn = fresh_conn();
        let pid = create_playlist(&conn, "P", "").unwrap();
        let t1 = insert_test_track(&conn, "A", "A1");
        let t2 = insert_test_track(&conn, "B", "B1");
        let t3 = insert_test_track(&conn, "C", "C1");
        add_to_playlist(&conn, pid, t1).unwrap();
        add_to_playlist(&conn, pid, t2).unwrap();
        add_to_playlist(&conn, pid, t3).unwrap();
        // Move t1 (ord 0) to ord 2.
        reorder_playlist(&mut conn, pid, t1, 2).unwrap();
        let ids: Vec<i64> = tracks_in_playlist(&conn, pid).unwrap()
            .iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![t2, t3, t1]);
    }

    #[test]
    fn remove_densifies_ord() {
        let mut conn = fresh_conn();
        let pid = create_playlist(&conn, "P", "").unwrap();
        let t1 = insert_test_track(&conn, "A", "A1");
        let t2 = insert_test_track(&conn, "B", "B1");
        let t3 = insert_test_track(&conn, "C", "C1");
        add_to_playlist(&conn, pid, t1).unwrap();
        add_to_playlist(&conn, pid, t2).unwrap();
        add_to_playlist(&conn, pid, t3).unwrap();
        remove_from_playlist(&mut conn, pid, t2).unwrap();
        let tracks = tracks_in_playlist(&conn, pid).unwrap();
        assert_eq!(tracks.len(), 2);
        let ords: Vec<i64> = conn.prepare("SELECT ord FROM playlist_tracks WHERE playlist_id=?1 ORDER BY ord")
            .unwrap().query_map(params![pid], |r| r.get(0))
            .unwrap().filter_map(|x| x.ok()).collect();
        assert_eq!(ords, vec![0, 1], "ord values densified");
    }

    #[test]
    fn delete_playlist_removes_tracks() {
        let conn = fresh_conn();
        let pid = create_playlist(&conn, "P", "").unwrap();
        let t1 = insert_test_track(&conn, "A", "A1");
        add_to_playlist(&conn, pid, t1).unwrap();
        delete_playlist(&conn, pid).unwrap();
        assert_eq!(list_playlists(&conn).unwrap().len(), 0);
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM playlist_tracks WHERE playlist_id=?1",
            params![pid], |r| r.get(0)).unwrap();
        assert_eq!(n, 0);
    }

    // ── Queue tests ──

    #[test]
    fn queue_push_and_peek() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "A1");
        let t2 = insert_test_track(&conn, "B", "B1");
        queue_push(&conn, t1, "").unwrap();
        queue_push(&conn, t2, "").unwrap();
        assert_eq!(queue_peek(&conn, "").unwrap(), Some(t1));
    }

    #[test]
    fn queue_pop_returns_head_and_densifies() {
        let mut conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "A1");
        let t2 = insert_test_track(&conn, "B", "B1");
        let t3 = insert_test_track(&conn, "C", "C1");
        queue_push(&conn, t1, "").unwrap();
        queue_push(&conn, t2, "").unwrap();
        queue_push(&conn, t3, "").unwrap();
        assert_eq!(queue_pop(&mut conn, "").unwrap(), Some(t1));
        assert_eq!(queue_peek(&conn, "").unwrap(), Some(t2));
        let entries = queue_list(&conn, "").unwrap();
        let ords: Vec<i64> = entries.iter().map(|e| e.ord).collect();
        assert_eq!(ords, vec![0, 1], "queue ords densified");
    }

    #[test]
    fn queue_devices_lists_only_non_empty() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "Ax");
        let t2 = insert_test_track(&conn, "B", "Bx");
        let t3 = insert_test_track(&conn, "C", "Cx");
        // Stage has 3 items, Emerge has 1, Mobile has 0 (nothing pushed).
        queue_push(&conn, t1, "http://stage").unwrap();
        queue_push(&conn, t2, "http://stage").unwrap();
        queue_push(&conn, t3, "http://stage").unwrap();
        queue_push(&conn, t1, "http://emerge").unwrap();
        let devs = queue_devices(&conn).unwrap();
        // Mobile shouldn't appear; Stage should be first (3 > 1).
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[0].device_url, "http://stage");
        assert_eq!(devs[0].item_count, 3);
        assert_eq!(devs[1].device_url, "http://emerge");
        assert_eq!(devs[1].item_count, 1);
        // head_title reflects the track at the head (lowest ord) of each
        // device's queue — so the per-device strip can show WHAT each
        // speaker is set to play, not just a count. Stage's head is t1
        // ("A"), Emerge's head is also t1 ("A").
        assert_eq!(devs[0].head_title, "A", "stage head should be first pushed");
        assert_eq!(devs[1].head_title, "A", "emerge head is its only track");
    }

    #[test]
    fn queue_devices_empty_returns_empty() {
        let conn = fresh_conn();
        let devs = queue_devices(&conn).unwrap();
        assert!(devs.is_empty());
    }

    #[test]
    fn queue_insert_at_head() {
        let mut conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "A1");
        let t2 = insert_test_track(&conn, "B", "B1");
        let t3 = insert_test_track(&conn, "C", "C1");
        queue_push(&conn, t1, "").unwrap();
        queue_push(&conn, t2, "").unwrap();
        queue_insert(&mut conn, t3, "", 0).unwrap();
        assert_eq!(queue_peek(&conn, "").unwrap(), Some(t3));
    }

    #[test]
    fn queue_independent_per_device() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "A1");
        let t2 = insert_test_track(&conn, "B", "B1");
        queue_push(&conn, t1, "http://stage").unwrap();
        queue_push(&conn, t2, "http://emerge").unwrap();
        assert_eq!(queue_peek(&conn, "http://stage").unwrap(), Some(t1));
        assert_eq!(queue_peek(&conn, "http://emerge").unwrap(), Some(t2));
        assert_eq!(queue_peek(&conn, "").unwrap(), None, "global queue empty");
    }

    #[test]
    fn queue_clear_removes_all() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "A1");
        queue_push(&conn, t1, "").unwrap();
        queue_push(&conn, t1, "").unwrap();
        queue_clear(&conn, "").unwrap();
        assert!(queue_list(&conn, "").unwrap().is_empty());
    }

    // ── Tag tests ──

    #[test]
    fn upsert_tag_idempotent_and_lists() {
        let conn = fresh_conn();
        let id1 = upsert_tag(&conn, "ambient", Some("#456789")).unwrap();
        let id2 = upsert_tag(&conn, "ambient", None).unwrap();
        assert_eq!(id1, id2);
        let tags = list_tags(&conn).unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].color.as_deref(), Some("#456789"));
    }

    #[test]
    fn attach_detach_tag() {
        let conn = fresh_conn();
        let tid = insert_test_track(&conn, "A", "X");
        let tag = upsert_tag(&conn, "chill", None).unwrap();
        attach_tag(&conn, tid, tag).unwrap();
        let attached = track_tags(&conn, tid).unwrap();
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0].name, "chill");
        detach_tag(&conn, tid, tag).unwrap();
        let attached = track_tags(&conn, tid).unwrap();
        assert!(attached.is_empty());
    }

    #[test]
    fn tags_survive_track_id_churn() {
        // Regression: rescans re-insert tracks with new ids; the
        // id-keyed track_tags rows cascade away. The path-keyed mirror
        // + relink must restore the user's tag after the churn.
        let conn = fresh_conn();
        let path = "/test/churn.mp3";
        catalog::upsert_path(&conn, path, 1234, 0).unwrap();
        let old_id: i64 = conn.query_row(
            "SELECT id FROM tracks WHERE path=?1", params![path], |r| r.get(0)
        ).unwrap();
        let tag = upsert_tag(&conn, "Favorites", None).unwrap();
        attach_tag(&conn, old_id, tag).unwrap();
        assert_eq!(track_tags(&conn, old_id).unwrap().len(), 1);

        // Simulate a rescan that re-inserts the track with a NEW id:
        // delete the row (cascades track_tags away), then re-insert the
        // same path. AUTOINCREMENT guarantees a different id.
        conn.execute("DELETE FROM tracks WHERE id=?1", params![old_id]).unwrap();
        // The id-keyed attachment is gone...
        catalog::upsert_path(&conn, path, 1234, 0).unwrap();
        let new_id: i64 = conn.query_row(
            "SELECT id FROM tracks WHERE path=?1", params![path], |r| r.get(0)
        ).unwrap();
        assert_ne!(old_id, new_id, "expected a fresh id after re-insert");
        // ...but the durable path-keyed mirror still has it.
        assert!(track_tags(&conn, new_id).unwrap().is_empty(),
                "id-keyed attachment should have cascaded away");

        // Relink restores it onto the new id.
        let n = relink_tags_by_path(&conn).unwrap();
        assert_eq!(n, 1, "relink should restore exactly one attachment");
        let restored = track_tags(&conn, new_id).unwrap();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].name, "Favorites");
    }

    #[test]
    fn detach_clears_path_mirror() {
        // After detach, a relink must NOT resurrect the tag.
        let conn = fresh_conn();
        let path = "/test/detach.mp3";
        catalog::upsert_path(&conn, path, 1, 0).unwrap();
        let id: i64 = conn.query_row(
            "SELECT id FROM tracks WHERE path=?1", params![path], |r| r.get(0)
        ).unwrap();
        let tag = upsert_tag(&conn, "temp", None).unwrap();
        attach_tag(&conn, id, tag).unwrap();
        detach_tag(&conn, id, tag).unwrap();
        let n = relink_tags_by_path(&conn).unwrap();
        assert_eq!(n, 0, "detached tag must not be resurrected by relink");
        assert!(track_tags(&conn, id).unwrap().is_empty());
    }

    #[test]
    fn prune_orphans_makes_counts_consistent() {
        // The bug: a DELETE FROM tracks with foreign_keys OFF leaves
        // orphaned track_artists rows. The count query (joins only
        // track_artists) sees them; the fetch query (joins tracks)
        // returns 0. prune_orphans must remove them so both agree.
        let conn = fresh_conn();
        let tid = insert_test_track(&conn, "song", "Glorb");
        // Sanity: artist has 1 track via both paths.
        let counts = catalog::artists_with_counts(&conn).unwrap();
        let glorb = counts.iter().find(|g| g.name == "Glorb").unwrap();
        assert_eq!(glorb.count, 1);

        // Force the orphan: disable FKs, delete the track. The
        // track_artists row survives pointing at a dead id.
        conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
        conn.execute("DELETE FROM tracks WHERE id=?1", params![tid]).unwrap();
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();

        // Count is now inflated (sees the orphan)...
        let counts = catalog::artists_with_counts(&conn).unwrap();
        let inflated = counts.iter().find(|g| g.name == "Glorb").map(|g| g.count);
        assert_eq!(inflated, Some(1), "orphan should inflate the count");
        // ...but the fetch returns nothing.
        assert!(catalog::by_artist_id(&conn, glorb.id).unwrap().is_empty());

        // Prune fixes it: the orphan is gone, count drops to 0.
        let pruned = catalog::prune_orphans(&conn).unwrap();
        assert!(pruned >= 1, "expected at least one orphan pruned");
        let counts = catalog::artists_with_counts(&conn).unwrap();
        assert!(counts.iter().find(|g| g.name == "Glorb").is_none(),
                "artist with only orphaned links should drop out of counts");
    }

    #[test]
    fn tracks_by_tag_returns_attached_tracks_only() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "morning1", "A");
        let t2 = insert_test_track(&conn, "morning2", "B");
        let _t3 = insert_test_track(&conn, "evening1", "C");
        let tag_morning = upsert_tag(&conn, "morning", None).unwrap();
        attach_tag(&conn, t1, tag_morning).unwrap();
        attach_tag(&conn, t2, tag_morning).unwrap();
        let results = tracks_by_tag(&conn, tag_morning, 100).unwrap();
        assert_eq!(results.len(), 2);
        let ids: Vec<i64> = results.iter().map(|t| t.id).collect();
        assert!(ids.contains(&t1));
        assert!(ids.contains(&t2));
    }

    // ── Search tests ──

    #[test]
    fn search_finds_by_title() {
        let conn = fresh_conn();
        insert_test_track(&conn, "ocean drift", "Aether");
        insert_test_track(&conn, "morning glow", "Sunflower");
        let r = search_advanced(&conn, "ocean", 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "ocean drift");
    }

    #[test]
    #[allow(non_snake_case)]   // AND is the SQL operator — keep it readable
    fn search_multi_term_AND() {
        let conn = fresh_conn();
        insert_test_track(&conn, "ocean drift", "Aether");
        insert_test_track(&conn, "ocean rise", "Wavebreaker");
        let r = search_advanced(&conn, "ocean aether", 10).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title, "ocean drift");
    }

    #[test]
    fn search_empty_returns_recents() {
        let conn = fresh_conn();
        insert_test_track(&conn, "A", "X");
        let r = search_advanced(&conn, "", 10).unwrap();
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn search_case_insensitive() {
        let conn = fresh_conn();
        insert_test_track(&conn, "Björk Song", "Björk");
        let r = search_advanced(&conn, "BJORK", 10).unwrap();
        assert_eq!(r.len(), 1);
    }

    // ── History tests ──

    #[test]
    fn record_and_list_history() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "X");
        let t2 = insert_test_track(&conn, "B", "Y");
        record_history(&conn, t1, "http://x", None).unwrap();
        record_history(&conn, t2, "http://x", Some(120)).unwrap();
        let hist = recent_history(&conn, 10).unwrap();
        assert_eq!(hist.len(), 2);
        // Most-recent first
        assert_eq!(hist[0].track_id, t2);
        assert_eq!(hist[1].track_id, t1);
    }

    #[test]
    fn prune_history_removes_old_rows() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "X");
        // Insert a record with a manually-set old timestamp.
        conn.execute(
            "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
             VALUES (?1, ?2, '', null)",
            params![t1, 1_000_000_i64],
        ).unwrap();
        let _ = PathBuf::new(); // silence unused import on certain platforms
        let n = prune_history(&conn, 2_000_000_i64).unwrap();
        assert_eq!(n, 1);
        assert_eq!(recent_history(&conn, 10).unwrap().len(), 0);
    }

    #[test]
    fn plays_by_hour_of_day_returns_24_entries_zero_filled() {
        let conn = fresh_conn();
        // No plays yet — should still return 24 entries, all zero.
        let stats = plays_by_hour_of_day(&conn, 30).unwrap();
        assert_eq!(stats.len(), 24);
        assert!(stats.iter().all(|(_h, n)| *n == 0));
        // Hours are 0..23 in order.
        for (i, (h, _)) in stats.iter().enumerate() {
            assert_eq!(*h as usize, i);
        }
    }

    #[test]
    fn plays_by_hour_of_day_aggregates_recent_history() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "X");
        // Insert plays at known timestamps. Use recent times so the
        // 30-day lookback includes them.
        let now = chrono_now_unix();
        // Three plays at "an hour ago"
        for _ in 0..3 {
            conn.execute(
                "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
                 VALUES (?1, ?2, '', null)",
                params![t1, now - 3600],
            ).unwrap();
        }
        // Two at "two hours ago"
        for _ in 0..2 {
            conn.execute(
                "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
                 VALUES (?1, ?2, '', null)",
                params![t1, now - 7200],
            ).unwrap();
        }
        let stats = plays_by_hour_of_day(&conn, 30).unwrap();
        // Total plays across the histogram should equal what we inserted.
        let total: u32 = stats.iter().map(|(_, n)| n).sum();
        assert_eq!(total, 5);
    }

    #[test]
    fn plays_by_hour_of_day_excludes_outside_lookback() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "A", "X");
        // Insert a play 100 days ago — beyond a 30-day lookback.
        conn.execute(
            "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
             VALUES (?1, ?2, '', null)",
            params![t1, chrono_now_unix() - 100 * 86_400],
        ).unwrap();
        let stats = plays_by_hour_of_day(&conn, 30).unwrap();
        let total: u32 = stats.iter().map(|(_, n)| n).sum();
        assert_eq!(total, 0);
    }

    #[test]
    fn history_summary_empty_returns_zero() {
        let conn = fresh_conn();
        let s = history_summary(&conn, 30).unwrap();
        assert_eq!(s.total_plays, 0);
        assert_eq!(s.distinct_tracks, 0);
        assert!(s.top_track.is_none());
        assert!(s.top_artist.is_none());
        assert_eq!(s.lookback_days, 30);
    }

    #[test]
    fn history_summary_picks_most_played_track() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "Hit", "ArtistA");
        let t2 = insert_test_track(&conn, "B-side", "ArtistB");
        let now = chrono_now_unix();
        for _ in 0..5 {
            conn.execute(
                "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
                 VALUES (?1, ?2, '', null)",
                params![t1, now - 3600],
            ).unwrap();
        }
        for _ in 0..2 {
            conn.execute(
                "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
                 VALUES (?1, ?2, '', null)",
                params![t2, now - 3600],
            ).unwrap();
        }
        let s = history_summary(&conn, 30).unwrap();
        assert_eq!(s.total_plays, 7);
        assert_eq!(s.distinct_tracks, 2);
        let top = s.top_track.unwrap();
        assert_eq!(top.track_id, t1);
        assert_eq!(top.title, "Hit");
        assert_eq!(top.play_count, 5);
    }

    #[test]
    fn history_summary_aggregates_artist_across_tracks() {
        let conn = fresh_conn();
        // Two tracks by the same primary artist.
        let t1 = insert_test_track(&conn, "Track1", "SharedArtist");
        let t2 = insert_test_track(&conn, "Track2", "SharedArtist");
        // One track by a different artist.
        let t3 = insert_test_track(&conn, "Other", "OtherArtist");
        let now = chrono_now_unix();
        // 2 plays of t1 + 2 plays of t2 = 4 plays for SharedArtist.
        // 3 plays of t3 = 3 plays for OtherArtist.
        // SharedArtist wins despite no single track having the most plays.
        for _ in 0..2 {
            conn.execute(
                "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
                 VALUES (?1, ?2, '', null)",
                params![t1, now - 100],
            ).unwrap();
        }
        for _ in 0..2 {
            conn.execute(
                "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
                 VALUES (?1, ?2, '', null)",
                params![t2, now - 100],
            ).unwrap();
        }
        for _ in 0..3 {
            conn.execute(
                "INSERT INTO play_history (track_id, played_at, device_url, duration_played)
                 VALUES (?1, ?2, '', null)",
                params![t3, now - 100],
            ).unwrap();
        }
        let s = history_summary(&conn, 30).unwrap();
        let top_artist = s.top_artist.unwrap();
        assert_eq!(top_artist.name, "SharedArtist");
        assert_eq!(top_artist.play_count, 4);
    }

    // ── Artist aggregation tests ──

    #[test]
    fn top_artists_orders_by_plays() {
        let conn = fresh_conn();
        let t_a = insert_test_track(&conn, "A1", "Alpha");
        let t_b = insert_test_track(&conn, "B1", "Beta");
        // Alpha gets 5 plays, Beta gets 2.
        conn.execute("UPDATE tracks SET play_count=5 WHERE id=?1", params![t_a]).unwrap();
        conn.execute("UPDATE tracks SET play_count=2 WHERE id=?1", params![t_b]).unwrap();
        let top = top_artists_by_plays(&conn, 10).unwrap();
        assert_eq!(top[0].name, "Alpha");
        assert_eq!(top[0].total_plays, 5);
        assert_eq!(top[1].name, "Beta");
        assert_eq!(top[1].total_plays, 2);
    }

    #[test]
    fn all_artists_alphabetical() {
        let conn = fresh_conn();
        insert_test_track(&conn, "z", "Zeta");
        insert_test_track(&conn, "a", "Alpha");
        insert_test_track(&conn, "m", "Mu");
        let arts = all_artists_alpha(&conn).unwrap();
        let names: Vec<&str> = arts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Mu", "Zeta"]);
    }

    fn insert_test_track_with_album(conn: &Connection, title: &str, artist: &str,
                                    album: &str, year: Option<i64>) -> i64 {
        let tid = insert_test_track(conn, title, artist);
        conn.execute("UPDATE tracks SET album = ?1, year = ?2 WHERE id = ?3",
                     params![album, year, tid]).unwrap();
        tid
    }

    #[test]
    fn all_albums_alpha_groups_tracks_and_sorts() {
        let conn = fresh_conn();
        insert_test_track_with_album(&conn, "Track A1", "ArtistA",
                                     "Zebra Album", Some(2024));
        insert_test_track_with_album(&conn, "Track A2", "ArtistA",
                                     "Zebra Album", Some(2024));
        insert_test_track_with_album(&conn, "Track B1", "ArtistB",
                                     "Alpaca Album", Some(2020));
        // No-album track should not show up.
        insert_test_track(&conn, "Loose track", "ArtistC");

        let albums = all_albums_alpha(&conn).unwrap();
        let names: Vec<&str> = albums.iter().map(|a| a.title.as_str()).collect();
        // Alpaca before Zebra (alphabetical).
        assert_eq!(names, vec!["Alpaca Album", "Zebra Album"]);
        // Track counts roll up.
        assert_eq!(albums[0].track_count, 1);
        assert_eq!(albums[1].track_count, 2);
        // Years come through.
        assert_eq!(albums[0].year, Some(2020));
        assert_eq!(albums[1].year, Some(2024));
        // Zero plays at this point.
        assert_eq!(albums[0].total_plays, 0);
        assert_eq!(albums[1].total_plays, 0);
    }

    #[test]
    fn top_albums_orders_by_total_plays_excludes_zero() {
        let conn = fresh_conn();
        let t1 = insert_test_track_with_album(&conn, "a", "X", "Quiet", None);
        let t2 = insert_test_track_with_album(&conn, "b", "X", "Loud", None);
        let t3 = insert_test_track_with_album(&conn, "c", "X", "Loud", None);
        // Quiet has 0 plays; Loud has 5+3=8 plays.
        conn.execute("UPDATE tracks SET play_count = 5 WHERE id=?1",
                     params![t2]).unwrap();
        conn.execute("UPDATE tracks SET play_count = 3 WHERE id=?1",
                     params![t3]).unwrap();
        let _ = t1; // touched for clarity
        let albums = top_albums_by_plays(&conn, 10).unwrap();
        // Only Loud appears (Quiet has 0 plays).
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].title, "Loud");
        assert_eq!(albums[0].total_plays, 8);
        assert_eq!(albums[0].track_count, 2);
    }

    #[test]
    fn tracks_in_album_filters_by_artist() {
        // The "Greatest Hits" disambiguation case: same album title
        // under two different artists.
        let conn = fresh_conn();
        insert_test_track_with_album(&conn, "Hit A", "Artist1",
                                     "Greatest Hits", None);
        insert_test_track_with_album(&conn, "Hit B", "Artist1",
                                     "Greatest Hits", None);
        insert_test_track_with_album(&conn, "Smash 1", "Artist2",
                                     "Greatest Hits", None);

        let a1_tracks = tracks_in_album(&conn, "Greatest Hits", "Artist1").unwrap();
        let titles: Vec<&str> = a1_tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["Hit A", "Hit B"],
                   "Artist1's album should have its own tracks only");

        let a2_tracks = tracks_in_album(&conn, "Greatest Hits", "Artist2").unwrap();
        let titles2: Vec<&str> = a2_tracks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles2, vec!["Smash 1"]);
    }

    #[test]
    fn tracks_in_album_alpha_order() {
        let conn = fresh_conn();
        insert_test_track_with_album(&conn, "Z is for Zebra", "X", "Letters", None);
        insert_test_track_with_album(&conn, "A is for Aardvark", "X", "Letters", None);
        insert_test_track_with_album(&conn, "M is for Mango", "X", "Letters", None);
        let ts = tracks_in_album(&conn, "Letters", "X").unwrap();
        let titles: Vec<&str> = ts.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec![
            "A is for Aardvark", "M is for Mango", "Z is for Zebra"
        ]);
    }

    #[test]
    fn album_year_uses_most_common_year() {
        // Compilation case: mixed years on track tags, want the mode.
        let conn = fresh_conn();
        insert_test_track_with_album(&conn, "1", "X", "Mixed", Some(2020));
        insert_test_track_with_album(&conn, "2", "X", "Mixed", Some(2021));
        insert_test_track_with_album(&conn, "3", "X", "Mixed", Some(2021));
        insert_test_track_with_album(&conn, "4", "X", "Mixed", Some(2021));
        let albums = all_albums_alpha(&conn).unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].year, Some(2021),
                   "expected mode-year (2021), got {:?}", albums[0].year);
    }

    #[test]
    fn top_tracks_orders_by_play_count_excludes_zero() {
        let conn = fresh_conn();
        let _t1 = insert_test_track(&conn, "Silent", "X");
        let t2 = insert_test_track(&conn, "Popular", "X");
        let t3 = insert_test_track(&conn, "MidPlay", "X");
        // play_counts: Silent=0, Popular=10, MidPlay=3
        conn.execute("UPDATE tracks SET play_count = 10 WHERE id=?1",
                     params![t2]).unwrap();
        conn.execute("UPDATE tracks SET play_count = 3 WHERE id=?1",
                     params![t3]).unwrap();
        let tops = top_tracks_by_plays(&conn, 5).unwrap();
        let titles: Vec<&str> = tops.iter().map(|t| t.title.as_str()).collect();
        // Popular (10), MidPlay (3); Silent excluded.
        assert_eq!(titles, vec!["Popular", "MidPlay"]);
    }

    #[test]
    fn never_played_returns_only_zero_plays() {
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "Untouched1", "X");
        let t2 = insert_test_track(&conn, "Played", "X");
        let t3 = insert_test_track(&conn, "Untouched2", "X");
        conn.execute("UPDATE tracks SET play_count = 5 WHERE id=?1",
                     params![t2]).unwrap();
        let _ = (t1, t3);  // touched for clarity
        let never = never_played(&conn, 10).unwrap();
        let titles: Vec<&str> = never.iter().map(|t| t.title.as_str()).collect();
        // Both untouched tracks, no Played.
        assert_eq!(titles.len(), 2);
        assert!(titles.contains(&"Untouched1"));
        assert!(titles.contains(&"Untouched2"));
        assert!(!titles.contains(&"Played"));
    }

    #[test]
    fn recently_added_respects_limit() {
        let conn = fresh_conn();
        for i in 0..10 {
            insert_test_track(&conn, &format!("track-{}", i), "X");
        }
        let recent = recently_added(&conn, 5).unwrap();
        assert_eq!(recent.len(), 5);
    }
    #[test]
    fn tag_and_artist_detail_queries_populate() {
        // Regression for "tags/artists pages don't populate": both the
        // by-artist and by-tag drill-down queries must return the attached
        // tracks (the UI bug was a QML binding conflict, but lock the data
        // layer down too).
        let conn = fresh_conn();
        let t1 = insert_test_track(&conn, "Song One", "The Band");
        let t2 = insert_test_track(&conn, "Song Two", "The Band");
        let aid: i64 = conn.query_row(
            "SELECT artist_id FROM track_artists WHERE track_id=?1 LIMIT 1",
            params![t1], |r| r.get(0)).unwrap();
        assert_eq!(catalog::by_artist_id(&conn, aid).unwrap().len(), 2,
                   "artist drill-down should return both tracks");

        conn.execute("INSERT INTO tags (name, norm) VALUES ('chill','chill')", []).unwrap();
        let tag_id: i64 = conn.query_row("SELECT id FROM tags WHERE name='chill'", [], |r| r.get(0)).unwrap();
        conn.execute("INSERT INTO track_tags (track_id, tag_id) VALUES (?1,?2)", params![t1, tag_id]).unwrap();
        conn.execute("INSERT INTO track_tags (track_id, tag_id) VALUES (?1,?2)", params![t2, tag_id]).unwrap();
        assert_eq!(tracks_by_tag(&conn, tag_id, 500).unwrap().len(), 2,
                   "tag drill-down should return both tracks");
    }

}
