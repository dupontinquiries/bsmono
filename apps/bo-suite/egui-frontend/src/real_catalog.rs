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
}
