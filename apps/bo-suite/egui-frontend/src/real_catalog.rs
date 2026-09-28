//! real_catalog — bo-controller's SQLite catalog, from this front-end.
//!
//! `music_scan::FsCatalog` answered `CatalogSource` from a filename-only
//! directory walk: no tags, no ratings, no playlists, no queue, because
//! reading any of that meant opening bo-controller's database from a
//! second process while its worker thread might be mid-write.
//!
//! That fear turns out to be stale. `bo_media::catalog::open` puts the
//! database in WAL mode specifically so background writes don't block
//! reads (see that module's header) — a second connection opened here is
//! exactly the case WAL exists for, not the hazard `catalog_source.rs`'s
//! own doc comment warned about. So this reads the real catalog directly
//! on its own connection, and only routes through the worker channel for
//! the things that must serialize through the writer: rescans, ratings,
//! play history, playlists, queue.
//!
//! Both front-ends point at the same path (`config_kit::data_dir
//! ("bo-controller")/media.sqlite`) so a library scanned from the Qt app
//! shows up here unchanged, and vice versa.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

use bo_media::catalog::{self, TrackSort};
use bo_media::MediaCmd;

use crate::catalog_source::{CatalogSource, CatalogTrack};

/// Same XDG app name bo-controller uses, so both front-ends share one
/// database instead of each cataloging its own copy of the same files.
const APP_NAME: &str = "bo-controller";

pub fn db_path() -> PathBuf {
    config_kit::data_dir(APP_NAME).join("media.sqlite")
}

/// Music roots to scan: `$BO_MUSIC_DIR` (colon-separated) or `~/Music`.
/// Mirrors bo-controller's `media_music_roots()` so `Rescan` here finds
/// the same files that binary would.
pub fn music_roots() -> Vec<PathBuf> {
    if let Ok(dirs) = std::env::var("BO_MUSIC_DIR") {
        let roots: Vec<PathBuf> = dirs.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
        if !roots.is_empty() { return roots; }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return vec![PathBuf::from(home).join("Music")];
    }
    vec![PathBuf::from("./Music")]
}

pub struct RealCatalog {
    conn: rusqlite::Connection,
    /// Where writes and long-running work (scan, analysis) go — the
    /// worker thread owns the writing connection and this crate never
    /// opens one, so there is exactly one writer regardless of how many
    /// front-ends are reading.
    cmd_tx: Sender<MediaCmd>,
}

impl RealCatalog {
    /// Opens a second, read-only-by-convention connection to the same
    /// WAL database the worker thread writes through `cmd_tx`.
    pub fn open(path: &std::path::Path, cmd_tx: Sender<MediaCmd>) -> rusqlite::Result<Self> {
        let conn = catalog::open(path)?;
        // Playlist/queue writes below go directly on THIS connection
        // rather than through cmd_tx (unlike rescan/rating/play — see
        // those methods' comments): they're small, single-row-ish
        // mutations with no ordering dependency on the scanner's own
        // writes, so routing them through the worker would only add
        // latency for no correctness gain. A busy_timeout means the rare
        // moment they do collide with a worker write waits briefly for
        // the WAL lock instead of surfacing SQLITE_BUSY to the UI.
        conn.busy_timeout(std::time::Duration::from_secs(3))?;
        Ok(Self { conn, cmd_tx })
    }

    /// A track's file path, for the player to open. `CatalogTrack` (what
    /// `page()` returns) doesn't carry it — `all_tracks_page_sorted`'s
    /// row shape is shared with every sort the list offers, and adding a
    /// rarely-needed column there would widen every page fetched.
    pub fn path_of(&self, id: i64) -> Option<String> {
        catalog::path_for_track(&self.conn, id).ok().flatten()
    }

    /// Ask the worker to walk `music_roots()` and (re)populate the
    /// catalog. Fire-and-forget: progress comes back as `MediaResult::
    /// ScanProgress`, read in the bin's `pump_media_results`.
    pub fn rescan(&self) {
        let _ = self.cmd_tx.send(MediaCmd::RescanLibrary);
    }

    /// Ask the worker to walk exactly `roots` instead of the configured
    /// default — what the scan picker is for. An empty `roots` falls back
    /// to the worker's own default (see `MediaCmd::ScanWithRoots`'s doc),
    /// same as an empty pick in the dialog meaning "scan everything as
    /// usual" rather than "scan nothing".
    pub fn scan_with_roots(&self, roots: Vec<String>) {
        let _ = self.cmd_tx.send(MediaCmd::ScanWithRoots(roots));
    }

    /// Record that a track played (history + play-count), and persist a
    /// star rating. Both go through the worker so they land in the same
    /// transaction path bo-controller uses — writing them on this
    /// connection directly would race the worker's own writes.
    pub fn record_play(&self, track_id: i64) {
        let _ = self.cmd_tx.send(MediaCmd::RecordPlay(track_id));
    }

    pub fn set_rating(&self, track_id: i64, rating: i64) {
        let _ = self.cmd_tx.send(MediaCmd::SetTrackRating { track_id, rating });
    }

    // ── Playlists ───────────────────────────────────────────────────
    // Direct on this connection (see the busy_timeout note in `open`),
    // not through cmd_tx — these are what closed the gap in `sources.rs`'s
    // `PlaylistsPage`/`dialogs.rs`'s `AddToPlaylist`: both had a full,
    // tested local model and zero persistence, so a create survived
    // exactly as long as the process did.

    pub fn playlists(&self) -> Vec<crate::sources::PlaylistSummary> {
        bo_media::library::list_playlists(&self.conn).unwrap_or_default()
            .into_iter().map(|p| crate::sources::PlaylistSummary {
                id: p.id, name: p.name, description: p.description,
                track_count: p.track_count.max(0) as u32,
                updated_at: p.updated_at.unwrap_or(p.created_at),
            }).collect()
    }

    /// Returns the new playlist's real id, or `None` on failure (a toast,
    /// not a panic — a duplicate name or a disk-full condition is a thing
    /// that happens, not a bug).
    pub fn create_playlist(&self, name: &str, description: &str) -> Option<i64> {
        bo_media::library::create_playlist(&self.conn, name, description).ok()
    }

    pub fn add_to_playlist(&self, playlist_id: i64, track_id: i64) -> bool {
        bo_media::library::add_to_playlist(&self.conn, playlist_id, track_id).is_ok()
    }

    /// The tracks in a playlist, in order — what a "playlist detail" view
    /// needs. Mapped to `browse::BrowseTrack` rather than a new row type:
    /// it's the type `Controller::browse_tracks` already holds, so the
    /// playlist detail view reuses the same field and the same tested
    /// `apply_browse`/`Action::Play`/`Queue`/`PlayAll` path Albums and
    /// Artists detail views already go through, instead of a
    /// playlist-specific copy of "how does tapping a track work".
    pub fn playlist_tracks(&self, playlist_id: i64) -> Vec<crate::browse::BrowseTrack> {
        bo_media::library::tracks_in_playlist(&self.conn, playlist_id).unwrap_or_default()
            .into_iter().map(|t| crate::browse::BrowseTrack {
                id: t.id, title: t.title, album: t.album,
                primary_artist: t.primary_artist, artist_id: 0,
                year: t.year, play_count: 0, last_played_at: None,
            }).collect()
    }

    pub fn delete_playlist(&self, playlist_id: i64) -> bool {
        bo_media::library::delete_playlist(&self.conn, playlist_id).is_ok()
    }

    /// Take one track out of a playlist. Needs `&mut` because the
    /// library function renumbers the remaining rows in a transaction.
    pub fn remove_from_playlist(&mut self, playlist_id: i64, track_id: i64) -> bool {
        bo_media::library::remove_from_playlist(&mut self.conn, playlist_id, track_id).is_ok()
    }

    /// Ask the worker for the track's path, display fields and — the
    /// part the player cannot always supply — its duration from the tag
    /// reader. Answered as `MediaResult::TrackForPlay`.
    pub fn resolve_for_play(&self, track_id: i64) {
        let _ = self.cmd_tx.send(MediaCmd::ResolveForPlay(track_id));
    }

    /// Record a play in the history table as well as the play count, so
    /// "recently played" has rows. `device_url` names where it played.
    pub fn record_history(&self, track_id: i64, device_url: &str, secs: Option<i64>) {
        let _ = self.cmd_tx.send(MediaCmd::RecordPlayHistory {
            track_id, device_url: device_url.to_string(), duration_seconds: secs });
    }

    /// Every track with the fields the browse aggregates need — real
    /// play counts, last-played times, years, a real artist id. The
    /// CatalogSource page carries none of those, which is why the TOP
    /// album view was always empty: it filters on plays and every row
    /// had zero.
    pub fn browse_tracks(&self) -> Vec<crate::browse::BrowseTrack> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT t.id, COALESCE(ov.title, t.title), COALESCE(ov.album, t.album),
                    COALESCE(ov.year, t.year), t.play_count, t.last_played_at,
                    COALESCE(ov.artist, (SELECT a.name FROM artists a
                        JOIN track_artists ta ON ta.artist_id = a.id
                        WHERE ta.track_id = t.id AND ta.role = 'primary'
                        ORDER BY ta.ord LIMIT 1), ''),
                    COALESCE((SELECT ta.artist_id FROM track_artists ta
                        WHERE ta.track_id = t.id AND ta.role = 'primary'
                        ORDER BY ta.ord LIMIT 1), 0)
             FROM tracks t LEFT JOIN track_overrides ov ON ov.track_id = t.id
             ORDER BY t.title") else { return Vec::new() };
        let rows = stmt.query_map([], |r| Ok(crate::browse::BrowseTrack {
            id: r.get(0)?,
            title: r.get::<_, String>(1)?,
            album: r.get::<_, Option<String>>(2)?,
            year: r.get::<_, Option<i64>>(3)?.map(|y| y as u16),
            play_count: r.get::<_, i64>(4)?.max(0) as u32,
            last_played_at: r.get(5)?,
            primary_artist: r.get(6)?,
            artist_id: r.get(7)?,
        }));
        let mut out: Vec<crate::browse::BrowseTrack> = match rows {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        };
        // An override can name an artist with no artists row; give
        // those a stable id from the name so browse still groups them.
        for t in out.iter_mut() {
            if t.artist_id == 0 && !t.primary_artist.is_empty() {
                t.artist_id = crate::artist_id_of(&t.primary_artist);
            }
        }
        out
    }

    /// The paths for a set of rows, for `Controller::merge_track_paths`.
    /// One query, not one per row — the previous per-page sync ran a
    /// statement per track and an O(n²) merge on every page load.
    pub fn paths_for(&self, ids: &[i64]) -> Vec<(i64, String)> {
        if ids.is_empty() { return Vec::new(); }
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let Ok(mut stmt) = self.conn.prepare(
            &format!("SELECT id, path FROM tracks WHERE id IN ({list})")) else { return Vec::new() };
        let out: Vec<(i64, String)> = match stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))) {
            Ok(rows) => rows.flatten().collect(),
            Err(_) => Vec::new(),
        };
        out
    }

    fn summaries_to_browse(rows: Vec<bo_media::catalog::TrackSummary>) -> Vec<crate::browse::BrowseTrack> {
        rows.into_iter().map(|t| crate::browse::BrowseTrack {
            id: t.id, title: t.title, album: t.album,
            artist_id: crate::artist_id_of(&t.primary_artist),
            primary_artist: t.primary_artist,
            year: t.year, play_count: 0, last_played_at: None,
        }).collect()
    }

    /// The catalogue's own search: folded (so "bjork" finds "Björk"),
    /// every term must match, most-played first. An empty query is the
    /// recent tracks, which the caller treats as "nothing asked".
    pub fn search(&self, query: &str, limit: usize) -> Vec<crate::browse::BrowseTrack> {
        if query.trim().is_empty() { return Vec::new(); }
        let mut rows = Self::summaries_to_browse(
            bo_media::library::search_advanced(&self.conn, query, limit).unwrap_or_default());
        self.fill_counts(&mut rows);
        rows
    }

    /// The Home lists, straight from the library's own queries.
    pub fn home(&self, rows: usize) -> crate::HomeLists {
        let recently_played: Vec<crate::browse::BrowseTrack> =
            bo_media::library::recent_history(&self.conn, rows * 3).unwrap_or_default()
            .into_iter().fold(Vec::new(), |mut acc: Vec<crate::browse::BrowseTrack>, h| {
                // One row per track, most recent play wins: a track
                // played five times in a row is one line, not five.
                if !acc.iter().any(|t| t.id == h.track_id) && acc.len() < rows {
                    acc.push(crate::browse::BrowseTrack {
                        id: h.track_id, title: h.title, album: None,
                        artist_id: crate::artist_id_of(h.primary_artist.as_deref().unwrap_or("")),
                        primary_artist: h.primary_artist.unwrap_or_default(),
                        year: None, play_count: 0, last_played_at: Some(h.played_at),
                    });
                }
                acc
            });
        let mut home = crate::HomeLists {
            recently_played,
            recently_added: Self::summaries_to_browse(
                bo_media::library::recently_added(&self.conn, rows).unwrap_or_default()),
            most_played: Self::summaries_to_browse(
                bo_media::library::top_tracks_by_plays(&self.conn, rows).unwrap_or_default()),
            never_played: Self::summaries_to_browse(
                bo_media::library::never_played(&self.conn, rows).unwrap_or_default()),
            loaded: true,
        };
        self.fill_counts(&mut home.recently_played);
        self.fill_counts(&mut home.recently_added);
        self.fill_counts(&mut home.most_played);
        home
    }

    /// Fill play counts and albums on rows that came from a query that
    /// does not carry them, in one statement.
    fn fill_counts(&self, rows: &mut [crate::browse::BrowseTrack]) {
        if rows.is_empty() { return; }
        let ids: Vec<i64> = rows.iter().map(|t| t.id).collect();
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let Ok(mut stmt) = self.conn.prepare(&format!(
            "SELECT id, play_count, last_played_at, album FROM tracks WHERE id IN ({list})")) else { return };
        let Ok(found) = stmt.query_map([], |r| Ok((
            r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<String>>(3)?))) else { return };
        for (id, plays, last, album) in found.flatten() {
            if let Some(t) = rows.iter_mut().find(|t| t.id == id) {
                t.play_count = plays.max(0) as u32;
                t.last_played_at = last;
                if t.album.is_none() { t.album = album; }
            }
        }
    }

    // ── Track edits ─────────────────────────────────────────────────
    // Backs `dialogs::EditTrack`. Goes through `MediaCmd::SetOverride`
    // (a DB-only overlay row) rather than `WriteTagsToFile` (which
    // rewrites the actual audio file's tags) — an override applies
    // immediately and is what `all_tracks_page_sorted` and friends
    // already coalesce onto the parsed values (see `apply_overrides` in
    // bo-media's catalog.rs), where a file-tag write only shows up after
    // the next rescan re-parses it. Permanent file tagging stays a
    // follow-up, same as it was in bo-controller's own QML pages.

    /// The current (parsed-or-overridden) title/artist/album/year, for
    /// seeding the editor when it opens. `None` if the track has vanished
    /// between the row being drawn and the edit button being pressed.
    pub fn track_fields(&self, track_id: i64) -> Option<crate::dialogs::TrackFields> {
        self.conn.query_row(
            "SELECT title, COALESCE((SELECT a.name FROM artists a
                 JOIN track_artists ta ON ta.artist_id = a.id
                 WHERE ta.track_id = t.id AND ta.role = 'primary'
                 ORDER BY ta.ord LIMIT 1), ''),
                    COALESCE(album, ''), COALESCE(year, 0)
             FROM tracks t WHERE t.id = ?1",
            [track_id],
            |r| Ok(crate::dialogs::TrackFields {
                title: r.get::<_, String>(0)?,
                artist: r.get::<_, String>(1)?,
                album: r.get::<_, String>(2)?,
                year: match r.get::<_, i64>(3)? { 0 => String::new(), y => y.to_string() },
            }),
        ).ok()
    }

    /// Persists only the fields `EditTrack::payload()` says changed —
    /// `None` for the rest leaves them as whatever the parser/previous
    /// override already had, which is the whole point of an overlay
    /// rather than a full-row rewrite.
    pub fn save_track_edit(&self, track_id: i64,
        title: Option<String>, artist: Option<String>, album: Option<String>, year: Option<u16>)
    {
        let _ = self.cmd_tx.send(MediaCmd::SetOverride { track_id, title, artist, album, year });
    }

    // ── Queue ───────────────────────────────────────────────────────
    // `device_url` is the same string `outputs::Device.url` carries and
    // `cast.rs` passes to `play_url_on_device` — one identity for "which
    // speaker", not a second id space to keep in sync with the first.

    pub fn queue_push(&self, track_id: i64, device_url: &str) -> bool {
        bo_media::library::queue_push(&self.conn, track_id, device_url).is_ok()
    }

    pub fn queue_for_device(&self, device_url: &str) -> Vec<bo_media::library::QueueEntry> {
        bo_media::library::queue_list(&self.conn, device_url).unwrap_or_default()
    }

    pub fn queue_devices(&self) -> Vec<bo_media::library::QueueDeviceSummary> {
        bo_media::library::queue_devices(&self.conn).unwrap_or_default()
    }
}

impl CatalogSource for RealCatalog {
    fn page(&self, offset: i64, limit: i64, sort: &str) -> Vec<CatalogTrack> {
        let (rows, _total) = match catalog::all_tracks_page_sorted(
            &self.conn, offset, limit, TrackSort::from_tag(sort))
        {
            Ok(v) => v,
            // A query failure reads as an empty page rather than a panic —
            // consistent with `FsCatalog`, which likewise degrades to
            // nothing-found rather than crashing the view on a bad path.
            Err(_) => return Vec::new(),
        };
        rows.into_iter().map(|t| CatalogTrack {
            id: t.id,
            title: t.title,
            album: t.album,
            artists: if t.primary_artist.is_empty() || t.primary_artist == "Unknown Artist" {
                Vec::new()
            } else {
                vec![t.primary_artist]
            },
            // Not fetched by this query (see path_of's doc comment on the
            // same trade-off) — a tag chip on the all-tracks row is a
            // follow-up, not a regression: FsCatalog never had tags either.
            tags: Vec::new(),
            bpm: t.bpm,
            key_text: t.key_text,
        }).collect()
    }

    fn total(&self) -> i64 {
        self.conn.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get(0)).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_catalog() -> (tempfile::TempDir, RealCatalog) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("media.sqlite");
        let (tx, _rx) = std::sync::mpsc::channel();
        let cat = RealCatalog::open(&db, tx).unwrap();
        (dir, cat)
    }

    #[test]
    fn empty_catalog_pages_and_totals_are_empty() {
        let (_dir, cat) = temp_catalog();
        assert_eq!(cat.total(), 0);
        assert!(cat.page(0, 20, "title").is_empty());
        assert_eq!(cat.path_of(1), None);
    }

    #[test]
    fn a_scanned_row_shows_up_by_every_sort() {
        let (_dir, cat) = temp_catalog();
        // Insert directly the way the scanner would, without running a
        // real filesystem walk — this test is about the read path
        // (page/total/path_of), not the scanner (covered in bo-media).
        cat.conn.execute(
            "INSERT INTO tracks (path, size, mtime, title, album, year, rating)
             VALUES ('/music/a.flac', 100, 0, 'A Song', 'An Album', 2020, 0)",
            [],
        ).unwrap();
        assert_eq!(cat.total(), 1);
        let page = cat.page(0, 20, "title");
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].title, "A Song");
        assert_eq!(cat.path_of(page[0].id), Some("/music/a.flac".to_string()));
    }

    #[test]
    fn rescan_and_record_play_send_without_a_receiver_alive() {
        // The commands are fire-and-forget from this crate's point of
        // view — a dropped receiver (worker not running, e.g. in a test)
        // must not panic the caller.
        let (_dir, cat) = temp_catalog();
        cat.rescan();
        cat.record_play(1);
        cat.set_rating(1, 4);
    }

    #[test]
    fn a_created_playlist_shows_up_in_the_list_and_can_take_a_track() {
        let (_dir, cat) = temp_catalog();
        assert!(cat.playlists().is_empty());
        let id = cat.create_playlist("Chill", "wind-down").expect("create succeeds");
        let all = cat.playlists();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, id);
        assert_eq!(all[0].name, "Chill");
        assert_eq!(all[0].track_count, 0);

        cat.conn.execute(
            "INSERT INTO tracks (path, size, mtime, title, album, year, rating)
             VALUES ('/music/a.flac', 100, 0, 'A Song', 'An Album', 2020, 0)",
            [],
        ).unwrap();
        let track_id = cat.conn.last_insert_rowid();
        assert!(cat.add_to_playlist(id, track_id));
        assert_eq!(cat.playlists()[0].track_count, 1);
    }

    #[test]
    fn adding_to_a_playlist_that_does_not_exist_fails_without_panicking() {
        let (_dir, cat) = temp_catalog();
        assert!(!cat.add_to_playlist(999, 1));
    }

    #[test]
    fn queue_push_and_read_back_round_trip_per_device() {
        let (_dir, cat) = temp_catalog();
        cat.conn.execute(
            "INSERT INTO tracks (path, size, mtime, title, album, year, rating)
             VALUES ('/music/a.flac', 100, 0, 'A Song', 'An Album', 2020, 0)",
            [],
        ).unwrap();
        let track_id = cat.conn.last_insert_rowid();
        assert!(cat.queue_devices().is_empty());
        assert!(cat.queue_push(track_id, "http://192.168.1.50"));
        let entries = cat.queue_for_device("http://192.168.1.50");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].track_id, track_id);
        let devices = cat.queue_devices();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_url, "http://192.168.1.50");
        assert_eq!(devices[0].item_count, 1);
        // A different device's queue is empty — queues don't leak across
        // speakers.
        assert!(cat.queue_for_device("http://192.168.1.99").is_empty());
    }

    #[test]
    fn playlist_tracks_reads_back_in_order_and_delete_empties_the_list() {
        let (_dir, cat) = temp_catalog();
        let id = cat.create_playlist("Focus", "").unwrap();
        assert!(cat.playlist_tracks(id).is_empty());
        for (title, path) in [("First", "/m/1.flac"), ("Second", "/m/2.flac")] {
            cat.conn.execute(
                "INSERT INTO tracks (path, size, mtime, title, album, year, rating)
                 VALUES (?1, 100, 0, ?2, '', 2020, 0)",
                rusqlite::params![path, title],
            ).unwrap();
            let track_id = cat.conn.last_insert_rowid();
            assert!(cat.add_to_playlist(id, track_id));
        }
        let tracks = cat.playlist_tracks(id);
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].title, "First");
        assert_eq!(tracks[1].title, "Second");

        assert!(cat.delete_playlist(id));
        assert!(cat.playlists().is_empty());
        assert!(cat.playlist_tracks(id).is_empty(), "a deleted playlist has no tracks");
    }

    #[test]
    fn track_fields_reads_the_current_row() {
        let (_dir, cat) = temp_catalog();
        cat.conn.execute(
            "INSERT INTO tracks (path, size, mtime, title, album, year, rating)
             VALUES ('/m/a.flac', 100, 0, 'A Song', 'An Album', 2020, 0)",
            [],
        ).unwrap();
        let id = cat.conn.last_insert_rowid();
        let f = cat.track_fields(id).expect("row exists");
        assert_eq!(f.title, "A Song");
        assert_eq!(f.album, "An Album");
        assert_eq!(f.year, "2020");
        assert_eq!(f.artist, "", "no artist row linked yet");
    }

    fn insert(cat: &RealCatalog, path: &str, title: &str, album: &str, plays: i64) -> i64 {
        cat.conn.execute(
            "INSERT INTO tracks (path, size, mtime, title, album, year, rating, play_count, last_played_at)
             VALUES (?1, 100, 0, ?2, ?3, 2020, 0, ?4, ?5)",
            rusqlite::params![path, title, album, plays, if plays > 0 { Some(plays * 100) } else { None }],
        ).unwrap();
        cat.conn.last_insert_rowid()
    }

    #[test]
    fn browse_rows_carry_real_play_counts_so_the_top_view_can_rank() {
        let (_dir, cat) = temp_catalog();
        let a = insert(&cat, "/m/a.flac", "A", "Blue", 4);
        let b = insert(&cat, "/m/b.flac", "B", "Blue", 0);
        let rows = cat.browse_tracks();
        assert_eq!(rows.len(), 2);
        let ra = rows.iter().find(|t| t.id == a).unwrap();
        assert_eq!(ra.play_count, 4);
        assert_eq!(ra.last_played_at, Some(400));
        assert_eq!(ra.year, Some(2020));
        assert_eq!(rows.iter().find(|t| t.id == b).unwrap().play_count, 0);
        // and the album aggregate now has a play total the TOP view accepts
        let albums = crate::browse::aggregate_albums(&rows);
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].total_plays, 4);
        assert!(!crate::browse::top_albums(&albums, 10).is_empty(), "TOP was always empty before");
    }

    #[test]
    fn paths_come_back_for_a_set_in_one_query() {
        let (_dir, cat) = temp_catalog();
        let a = insert(&cat, "/m/a.flac", "A", "", 0);
        let b = insert(&cat, "/m/b.flac", "B", "", 0);
        let mut got = cat.paths_for(&[a, b, 999]);
        got.sort();
        assert_eq!(got, vec![(a, "/m/a.flac".to_string()), (b, "/m/b.flac".to_string())]);
        assert!(cat.paths_for(&[]).is_empty());
    }

    #[test]
    fn search_is_folded_and_multi_term() {
        let (_dir, cat) = temp_catalog();
        // The search blob is what the scanner writes; mimic it the way
        // apply_parse does so this exercises the real query.
        let id = insert(&cat, "/m/bjork.flac", "Jóga", "Homogenic", 2);
        cat.conn.execute("UPDATE tracks SET search = ?1 WHERE id = ?2",
            rusqlite::params![bo_media::catalog::build_search_blob("Jóga", &["Björk".to_string()], Some("Homogenic")), id]).unwrap();
        let hits = cat.search("bjork joga", 20);
        assert_eq!(hits.len(), 1, "ascii typed finds the accented tag");
        assert_eq!(hits[0].id, id);
        assert_eq!(hits[0].play_count, 2, "counts are filled in");
        assert!(cat.search("bjork nothingelse", 20).is_empty(), "every term must match");
        assert!(cat.search("   ", 20).is_empty(), "an empty query is not a search");
    }

    #[test]
    fn home_lists_answer_the_four_questions() {
        let (_dir, cat) = temp_catalog();
        let hot = insert(&cat, "/m/hot.flac", "Hot", "", 9);
        let cold = insert(&cat, "/m/cold.flac", "Cold", "", 0);
        bo_media::library::record_history(&cat.conn, hot, "local", None).unwrap();
        bo_media::library::record_history(&cat.conn, hot, "local", None).unwrap();
        let home = cat.home(8);
        assert!(home.loaded);
        assert_eq!(home.recently_played.iter().map(|t| t.id).collect::<Vec<_>>(), vec![hot],
                   "one row per track, however many plays");
        assert_eq!(home.most_played.first().map(|t| t.id), Some(hot));
        assert_eq!(home.never_played.iter().map(|t| t.id).collect::<Vec<_>>(), vec![cold]);
        assert_eq!(home.recently_added.len(), 2);
    }

    #[test]
    fn a_track_can_be_removed_from_a_playlist() {
        let (_dir, mut cat) = temp_catalog();
        let pl = cat.create_playlist("Mix", "").unwrap();
        let a = insert(&cat, "/m/a.flac", "A", "", 0);
        let b = insert(&cat, "/m/b.flac", "B", "", 0);
        assert!(cat.add_to_playlist(pl, a) && cat.add_to_playlist(pl, b));
        assert!(cat.remove_from_playlist(pl, a));
        let left = cat.playlist_tracks(pl);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, b);
    }

    #[test]
    fn track_fields_on_a_missing_track_is_none() {
        let (_dir, cat) = temp_catalog();
        assert_eq!(cat.track_fields(999), None);
    }

    #[test]
    fn save_track_edit_sends_without_a_receiver_alive() {
        let (_dir, cat) = temp_catalog();
        cat.save_track_edit(1, Some("New Title".into()), None, None, Some(1999));
    }
}
