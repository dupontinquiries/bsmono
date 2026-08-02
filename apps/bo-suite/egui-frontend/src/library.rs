//! library — the paged track list, shared by eight pages.
//!
//! `AllTracksPage`, `AlbumsPage`, `ArtistsPage`, `PlaylistsPage`, `TagsPage`,
//! `SearchPage`, `HistoryPage` and `QueuePage` are the same surface with
//! different sources: a paged list of tracks, sorted, filtered, and enqueued.
//! Porting them as one model rather than eight is not a shortcut — it is what
//! stops the sort behaving differently on the artists page than on the albums
//! page, which is the usual fate of eight separate implementations.
//!
//! ## Paging, and why `maybeFetchMore` is the interesting part
//!
//! `pageSize: 100`, and the QML fetches more as you approach the end. Two rules
//! fall out, and both are about not lying to the user:
//!
//! * **Never request a page already in flight.** Scrolling fast fires the
//!   trigger every frame, and without a guard the same page is requested a
//!   dozen times — which on a speaker over the network is a real stall.
//! * **A total of zero is not the same as "not loaded yet".** An empty library
//!   and an unfetched one look identical if you only track a count, and showing
//!   "no tracks" over a library that is still loading is the difference between
//!   a slow app and a broken one.

/// How the list is ordered. The tags are the QML's, because they are also what
/// the backend takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Title,
    Artist,
    Album,
}

impl Sort {
    pub const ALL: [Sort; 3] = [Sort::Title, Sort::Artist, Sort::Album];
    pub fn tag(self) -> &'static str {
        match self { Sort::Title => "title", Sort::Artist => "artist", Sort::Album => "album" }
    }
    pub fn label(self) -> &'static str {
        match self { Sort::Title => "Title", Sort::Artist => "Artist", Sort::Album => "Album" }
    }
    /// An unknown tag falls back to Title rather than leaving the list in an
    /// undefined order — a list whose order nobody can predict is unusable.
    pub fn from_tag(t: &str) -> Sort {
        Sort::ALL.into_iter().find(|s| s.tag() == t).unwrap_or(Sort::Title)
    }
}

/// Where a chosen track goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Enqueue {
    /// Replace the queue and play now.
    #[default]
    Now,
    /// After the current track.
    Next,
    /// At the end.
    Last,
}

impl Enqueue {
    pub const ALL: [Enqueue; 3] = [Enqueue::Now, Enqueue::Next, Enqueue::Last];
    pub fn tag(self) -> &'static str {
        match self { Enqueue::Now => "now", Enqueue::Next => "next", Enqueue::Last => "last" }
    }
    pub fn label(self) -> &'static str {
        match self { Enqueue::Now => "Play now", Enqueue::Next => "Play next", Enqueue::Last => "Add to end" }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_s: f64,
}

impl Track {
    /// A track with no title shows its id — an empty row reads as a broken list.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() { &self.id } else { &self.title }
    }
    /// The sort key for a field, lowercased so ordering is not case-dependent:
    /// "abba" and "ABBA" belong together.
    pub fn key(&self, s: Sort) -> String {
        match s {
            Sort::Title => self.label().to_lowercase(),
            Sort::Artist => self.artist.to_lowercase(),
            Sort::Album => self.album.to_lowercase(),
        }
    }
}

/// `pageSize: 100`.
pub const PAGE_SIZE: usize = 100;

/// How close to the end triggers a fetch, in rows. Enough that the next page
/// arrives before the user reaches it on a network with real latency.
pub const PREFETCH_ROWS: usize = 20;

/// A paged list, shared by every library page.
#[derive(Debug, Clone, Default)]
pub struct Library {
    pub tracks: Vec<Track>,
    /// Total the backend reports. `None` = not fetched yet, which is NOT the
    /// same as zero — showing "no tracks" over a loading library is the
    /// difference between a slow app and a broken one.
    pub total: Option<usize>,
    pub sort: Sort,
    pub enqueue: Enqueue,
    pub query: String,
    /// A page request currently outstanding.
    in_flight: Option<usize>,
}

impl Library {
    pub fn loading(&self) -> bool { self.in_flight.is_some() }

    /// Has everything been fetched?
    pub fn complete(&self) -> bool {
        matches!(self.total, Some(t) if self.tracks.len() >= t)
    }

    /// What to show when the list is empty, distinguishing the three cases a
    /// single "no results" message would collapse.
    pub fn empty_reason(&self) -> Option<&'static str> {
        if !self.tracks.is_empty() { return None; }
        if self.total.is_none() { return Some("loading…"); }
        if !self.query.trim().is_empty() { return Some("nothing matches"); }
        Some("no tracks")
    }

    /// `maybeFetchMore(visible_end)` — the page index to request, or `None`.
    ///
    /// Returns `None` while a request is outstanding: scrolling fast fires this
    /// every frame, and without the guard the same page is requested a dozen
    /// times, which on a speaker over the network is a real stall.
    pub fn maybe_fetch(&self, visible_end: usize) -> Option<usize> {
        if self.in_flight.is_some() || self.complete() { return None; }
        if self.total.is_some() && visible_end + PREFETCH_ROWS < self.tracks.len() { return None; }
        Some(self.tracks.len() / PAGE_SIZE)
    }

    /// Mark a request as sent.
    pub fn begin_fetch(&mut self, page: usize) { self.in_flight = Some(page); }

    /// A page arrived. Appends and clears the guard.
    ///
    /// A page for an index we did not ask for is IGNORED rather than appended:
    /// a late response from a cancelled request would otherwise splice stale
    /// rows into the middle of the list.
    pub fn receive(&mut self, page: usize, tracks: Vec<Track>, total: usize) -> bool {
        if self.in_flight != Some(page) { return false; }
        self.in_flight = None;
        self.total = Some(total);
        self.tracks.extend(tracks);
        true
    }

    /// Changing the sort or query invalidates every fetched page.
    ///
    /// Keeping them would show the first hundred titles sorted by artist, which
    /// is neither ordering and looks like corruption.
    pub fn reset(&mut self) {
        self.tracks.clear();
        self.total = None;
        self.in_flight = None;
    }

    pub fn set_sort(&mut self, s: Sort) {
        if self.sort != s { self.sort = s; self.reset(); }
    }

    pub fn set_query(&mut self, q: &str) {
        if self.query != q { self.query = q.to_string(); self.reset(); }
    }

    /// Rows to draw, sorted locally within what has been fetched.
    ///
    /// Ties break on id so the list cannot shuffle between frames — a list that
    /// reorders under a scrolling finger is the fastest way to lose someone's
    /// place.
    pub fn rows(&self) -> Vec<&Track> {
        let mut v: Vec<&Track> = self.tracks.iter().collect();
        let s = self.sort;
        v.sort_by(|a, b| a.key(s).cmp(&b.key(s)).then(a.id.cmp(&b.id)));
        v
    }
}

/// Total running time, for the header a detail page shows.
pub fn total_duration(tracks: &[Track]) -> f64 {
    tracks.iter().map(|t| t.duration_s.max(0.0)).sum()
}

/// `h:mm:ss`, or `m:ss` under an hour — an album shows minutes, a playlist may
/// show hours, and padding a three-minute album to `0:03:12` is noise.
pub fn duration_label(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, sec) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, title: &str, artist: &str, album: &str) -> Track {
        Track { id: id.into(), title: title.into(), artist: artist.into(),
                album: album.into(), duration_s: 180.0 }
    }

    fn page(n: usize) -> Vec<Track> {
        (0..n).map(|i| t(&format!("id{i:03}"), &format!("T{i}"), "A", "B")).collect()
    }

    #[test]
    fn an_empty_list_says_which_empty_it_is() {
        // "No tracks" over a library that is still loading is the difference
        // between a slow app and a broken one.
        let mut l = Library::default();
        assert_eq!(l.empty_reason(), Some("loading…"), "total unknown");
        l.total = Some(0);
        assert_eq!(l.empty_reason(), Some("no tracks"));
        l.query = "zzz".into();
        assert_eq!(l.empty_reason(), Some("nothing matches"));
        l.tracks.push(t("1", "x", "y", "z"));
        assert_eq!(l.empty_reason(), None);
    }

    #[test]
    fn a_page_already_in_flight_is_not_requested_again() {
        // Scrolling fast fires the trigger every frame; without the guard the
        // same page is requested a dozen times, which stalls a speaker.
        let mut l = Library::default();
        let p = l.maybe_fetch(0).expect("first page");
        l.begin_fetch(p);
        assert_eq!(l.maybe_fetch(0), None, "guarded while outstanding");
        assert!(l.loading());
        l.receive(p, page(100), 250);
        assert!(!l.loading());
        assert_eq!(l.maybe_fetch(95), Some(1), "and the next page is asked for");
    }

    #[test]
    fn a_late_page_from_a_cancelled_request_is_ignored() {
        // Otherwise stale rows splice into the middle of the list.
        let mut l = Library::default();
        l.begin_fetch(0);
        assert!(!l.receive(3, page(10), 100), "not the page we asked for");
        assert!(l.tracks.is_empty());
        assert!(l.receive(0, page(10), 100));
        assert_eq!(l.tracks.len(), 10);
    }

    #[test]
    fn a_complete_list_stops_asking() {
        let mut l = Library::default();
        l.begin_fetch(0);
        l.receive(0, page(40), 40);
        assert!(l.complete());
        assert_eq!(l.maybe_fetch(39), None);
    }

    #[test]
    fn changing_the_sort_discards_the_fetched_pages() {
        // Keeping them shows the first hundred titles sorted by artist, which is
        // neither ordering and looks like corruption.
        let mut l = Library::default();
        l.begin_fetch(0);
        l.receive(0, page(100), 500);
        assert_eq!(l.tracks.len(), 100);
        l.set_sort(Sort::Artist);
        assert!(l.tracks.is_empty());
        assert_eq!(l.total, None, "and the total is unknown again");
        // setting the SAME sort is a no-op, so a redraw does not wipe the list
        l.begin_fetch(0);
        l.receive(0, page(100), 500);
        l.set_sort(Sort::Artist);
        assert_eq!(l.tracks.len(), 100);
    }

    #[test]
    fn changing_the_query_also_resets() {
        let mut l = Library::default();
        l.begin_fetch(0);
        l.receive(0, page(20), 20);
        l.set_query("abba");
        assert!(l.tracks.is_empty());
        l.set_query("abba");
        assert!(l.tracks.is_empty(), "and repeating it is harmless");
    }

    #[test]
    fn sorting_is_case_insensitive_and_stable() {
        // "abba" and "ABBA" belong together, and a list that reorders under a
        // scrolling finger loses your place.
        let mut l = Library::default();
        l.tracks = vec![t("c", "beta", "ABBA", "x"), t("a", "Alpha", "abba", "x"),
                        t("b", "alpha", "zz", "x")];
        let ids: Vec<&str> = l.rows().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"], "Alpha and alpha adjacent, tie on id");
        let again: Vec<&str> = l.rows().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, again);
    }

    #[test]
    fn an_unknown_sort_tag_falls_back_rather_than_leaving_it_undefined() {
        assert_eq!(Sort::from_tag("artist"), Sort::Artist);
        assert_eq!(Sort::from_tag("nonsense"), Sort::Title);
        for s in Sort::ALL { assert_eq!(Sort::from_tag(s.tag()), s); }
    }

    #[test]
    fn a_track_with_no_title_shows_its_id() {
        assert_eq!(t("id7", "", "a", "b").label(), "id7");
        assert_eq!(t("id7", "  ", "a", "b").label(), "id7");
    }

    #[test]
    fn durations_show_hours_only_when_there_are_hours() {
        assert_eq!(duration_label(192.0), "3:12");
        assert_eq!(duration_label(3612.0), "1:00:12");
        assert_eq!(duration_label(-5.0), "0:00");
        assert_eq!(duration_label(total_duration(&page(3))), "9:00");
        assert_eq!(total_duration(&[]), 0.0);
    }

    #[test]
    fn the_enqueue_modes_are_the_three_the_qml_offers() {
        assert_eq!(Enqueue::default(), Enqueue::Now);
        let tags: Vec<&str> = Enqueue::ALL.iter().map(|e| e.tag()).collect();
        assert_eq!(tags, vec!["now", "next", "last"]);
    }
}
