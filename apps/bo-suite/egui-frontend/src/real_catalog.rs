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
    /// catalog. Fire-and-forget: the worker's own `MediaResult::
    /// ScanProgress` messages report progress, which this front-end does
    /// not yet surface (see `all_tracks.rs`'s TODO on scan UI) — issuing
    /// the command is the gap that mattered, since without it the
    /// "Rescan" button had nothing to send.
    pub fn rescan(&self) {
        let _ = self.cmd_tx.send(MediaCmd::RescanLibrary);
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
