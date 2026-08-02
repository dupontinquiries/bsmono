//! catalog_source — feed the library view from the app's real catalogue.
//!
//! The previous pass connected the library to `tracks: Vec::new()` and the
//! wiring checker called it done. A view fed a hardcoded empty vector is a
//! connected placeholder and is worse than an unwired module, because the metric
//! now says it is finished.
//!
//! The real source is `bo-controller/src/media/catalog.rs`: a SQLite catalogue,
//! **Qt-free**, with `all_tracks_page_sorted(conn, offset, limit, TrackSort)`
//! and a `Track` row carrying title, album, artists, tags, bpm and key. That is
//! what the QML's `load_all_tracks_page(offset, limit)` calls, so a port reading
//! the same rows shows the same library.
//!
//! ## Why this is a trait rather than a direct call
//!
//! The view must not open a database. `bo-controller` owns the connection, runs
//! the scanner and serialises access through a worker channel — a second reader
//! in the UI thread would block on the writer during a scan, which is precisely
//! when someone is looking at the library.
//!
//! So the frontend declares what it NEEDS and the host supplies it. That also
//! makes the view testable against a fake without a database file, which is how
//! the rules below get checked at all.

/// One row, reduced to what a list draws. Field names match
/// `media::catalog::Track` so the mapping is checkable by eye.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogTrack {
    pub id: i64,
    pub title: String,
    pub album: Option<String>,
    pub artists: Vec<String>,
    pub tags: Vec<String>,
    pub bpm: Option<f32>,
    pub key_text: Option<String>,
}

impl CatalogTrack {
    /// What a row shows. A track with no title falls back to its id rather than
    /// drawing blank — an untitled file is common in a scanned library and a
    /// blank row reads as a broken list.
    pub fn label(&self) -> String {
        if self.title.trim().is_empty() { format!("track {}", self.id) }
        else { self.title.clone() }
    }

    /// "Artist — Album", or whichever exists. Never a dangling separator: a
    /// trailing dash reads as missing data rather than absent data.
    pub fn sub(&self) -> String {
        let a = self.artists.join(", ");
        match (a.is_empty(), self.album.as_deref().unwrap_or("").is_empty()) {
            (false, false) => format!("{a} — {}", self.album.clone().unwrap_or_default()),
            (true, false) => self.album.clone().unwrap_or_default(),
            (false, true) => a,
            (true, true) => String::new(),
        }
    }
}

/// How the host supplies rows. Implemented over the real catalogue on device
/// and over a vector in tests.
pub trait CatalogSource {
    /// One page, already sorted by the backend — sorting in the view would
    /// order only the rows already fetched, which is a different and wrong
    /// order the moment a second page arrives.
    fn page(&self, offset: i64, limit: i64, sort: &str) -> Vec<CatalogTrack>;
    /// Total rows, so the list can say whether it is complete.
    fn total(&self) -> i64;
}

/// The library, fed by a source.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub rows: Vec<CatalogTrack>,
    pub sort: String,
    pub total: i64,
    /// Pages already requested, so a scroll that fires every frame does not
    /// re-request the same page. Kept as the highest offset rather than a set,
    /// because paging only ever moves forward.
    fetched_to: i64,
    pub loaded: bool,
}

/// `pageSize: 100` in the QML.
pub const PAGE: i64 = 100;

impl Catalog {
    /// Load the first page. Also the reset path, since changing sort discards
    /// every fetched row — keeping them would interleave two orderings, which
    /// looks like corruption rather than a sort.
    pub fn reload(&mut self, src: &dyn CatalogSource, sort: &str) {
        self.sort = sort.to_string();
        self.rows = src.page(0, PAGE, sort);
        self.total = src.total();
        self.fetched_to = self.rows.len() as i64;
        self.loaded = true;
    }

    /// Fetch the next page if the view is near the end. Returns true if rows
    /// arrived, so a caller can request a repaint only when something changed.
    pub fn fetch_more(&mut self, src: &dyn CatalogSource, visible_end: usize) -> bool {
        if !self.loaded || self.complete() { return false; }
        if (visible_end as i64) + 20 < self.fetched_to { return false; }
        let next = src.page(self.fetched_to, PAGE, &self.sort);
        if next.is_empty() {
            // An empty page when more were expected means the catalogue shrank
            // under us — a scan deleted rows. Believe the source rather than
            // looping forever asking for a page that no longer exists.
            self.total = self.rows.len() as i64;
            return false;
        }
        self.fetched_to += next.len() as i64;
        self.rows.extend(next);
        true
    }

    pub fn complete(&self) -> bool { self.rows.len() as i64 >= self.total }

    /// What to show when the list is empty, distinguishing the three cases a
    /// single message would collapse — and the reason this type exists at all:
    /// an unloaded library and an empty one are not the same, and saying "no
    /// tracks" over a library that has not been read yet is the placeholder
    /// failure in a different costume.
    pub fn empty_reason(&self) -> Option<&'static str> {
        if !self.rows.is_empty() { return None; }
        if !self.loaded { return Some("catalogue not opened"); }
        if self.total == 0 { return Some("no tracks — run a scan"); }
        Some("loading…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake { rows: Vec<CatalogTrack> }

    impl Fake {
        fn with(n: i64) -> Fake {
            Fake { rows: (0..n).map(|i| CatalogTrack {
                id: i, title: format!("T{i}"), album: Some("A".into()),
                artists: vec!["Ada".into()], tags: vec![], bpm: None, key_text: None,
            }).collect() }
        }
    }

    impl CatalogSource for Fake {
        fn page(&self, offset: i64, limit: i64, _sort: &str) -> Vec<CatalogTrack> {
            self.rows.iter().skip(offset.max(0) as usize).take(limit.max(0) as usize)
                .cloned().collect()
        }
        fn total(&self) -> i64 { self.rows.len() as i64 }
    }

    #[test]
    fn a_view_with_no_source_says_so_rather_than_no_tracks() {
        // THE failure this module exists to fix: a hardcoded empty vector shows
        // "no tracks" over a library nobody has read yet.
        let c = Catalog::default();
        assert_eq!(c.empty_reason(), Some("catalogue not opened"));
        let mut c2 = Catalog::default();
        c2.reload(&Fake::with(0), "title");
        assert_eq!(c2.empty_reason(), Some("no tracks — run a scan"),
                   "an empty catalogue is a different message from an unread one");
    }

    #[test]
    fn the_first_page_arrives_and_reports_the_real_total() {
        let mut c = Catalog::default();
        c.reload(&Fake::with(250), "title");
        assert_eq!(c.rows.len(), 100);
        assert_eq!(c.total, 250);
        assert!(!c.complete());
        assert_eq!(c.empty_reason(), None);
    }

    #[test]
    fn scrolling_fetches_the_next_page_once() {
        let src = Fake::with(250);
        let mut c = Catalog::default();
        c.reload(&src, "title");
        assert!(!c.fetch_more(&src, 0), "not near the end yet");
        assert!(c.fetch_more(&src, 95));
        assert_eq!(c.rows.len(), 200);
        assert!(!c.fetch_more(&src, 95), "already fetched past here");
    }

    #[test]
    fn a_complete_list_stops_asking() {
        let src = Fake::with(40);
        let mut c = Catalog::default();
        c.reload(&src, "title");
        assert!(c.complete());
        assert!(!c.fetch_more(&src, 39));
    }

    #[test]
    fn a_catalogue_that_shrinks_mid_scroll_does_not_loop_forever() {
        // A scan can delete rows while the list is open. Believing a stale
        // total would ask for a page that no longer exists, every frame.
        struct Shrinking;
        impl CatalogSource for Shrinking {
            fn page(&self, offset: i64, _l: i64, _s: &str) -> Vec<CatalogTrack> {
                if offset == 0 { Fake::with(100).rows } else { Vec::new() }
            }
            fn total(&self) -> i64 { 500 }      // claims more than it has
        }
        let s = Shrinking;
        let mut c = Catalog::default();
        c.reload(&s, "title");
        assert!(!c.fetch_more(&s, 99), "the empty page is believed");
        assert!(c.complete(), "and the total is corrected down");
        assert!(!c.fetch_more(&s, 99));
    }

    #[test]
    fn changing_sort_discards_the_fetched_rows() {
        // Keeping them interleaves two orderings, which looks like corruption
        // rather than a sort.
        let src = Fake::with(250);
        let mut c = Catalog::default();
        c.reload(&src, "title");
        c.fetch_more(&src, 95);
        assert_eq!(c.rows.len(), 200);
        c.reload(&src, "artist");
        assert_eq!(c.rows.len(), 100, "back to one page");
        assert_eq!(c.sort, "artist");
    }

    #[test]
    fn a_row_never_draws_blank_or_dangles_a_separator() {
        let t = CatalogTrack { id: 7, title: "  ".into(), album: None,
                               artists: vec![], tags: vec![], bpm: None, key_text: None };
        assert_eq!(t.label(), "track 7", "an untitled file is common in a scan");
        assert_eq!(t.sub(), "", "no dash with nothing on either side");
        let u = CatalogTrack { artists: vec!["Ada".into()], ..t.clone() };
        assert_eq!(u.sub(), "Ada", "artist alone, no trailing dash");
        let v = CatalogTrack { album: Some("Blue".into()), ..u.clone() };
        assert_eq!(v.sub(), "Ada — Blue");
    }
}
