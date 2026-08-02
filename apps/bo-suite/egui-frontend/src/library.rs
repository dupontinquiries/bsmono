//! library — search, history and playlists, ported from the QML pages.
//!
//! Three pages that share a track model: `SearchPage`, `HistoryPage` (with its
//! hour histogram and relative times) and `PlaylistDetailPage` (with drag
//! reordering).
//!
//! ## The bits with actual behaviour
//!
//! * **`relativeTime(unix_sec)`** — "3 minutes ago", not a timestamp. A
//!   listening history is read as *recency*, and a wall-clock time makes the
//!   reader do the subtraction.
//! * **`hourMax`** — the histogram normalises to its own peak. Normalising to a
//!   fixed ceiling makes a quiet week look like no listening at all.
//! * **Playlist reorder** is a **move**, not a swap. Dragging track 5 to
//!   position 1 should shift 1–4 down; swapping would put track 1 where 5 was,
//!   which is not what the hand did.

/// One track, as the library pages need it.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub tags: Vec<String>,
}

impl Track {
    /// Does this track match a query? Title, artist and album, case-insensitive.
    /// A library search that needs exact case is one nobody uses twice.
    pub fn matches(&self, q: &str) -> bool {
        let q = q.trim().to_lowercase();
        if q.is_empty() { return true; }
        self.title.to_lowercase().contains(&q)
            || self.artist.to_lowercase().contains(&q)
            || self.album.to_lowercase().contains(&q)
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
    }
}

/// A play, for the history page.
#[derive(Debug, Clone, PartialEq)]
pub struct Play {
    pub track: u64,
    /// Unix seconds.
    pub at: i64,
    /// Hour of day 0..23, as the QML's hourStats groups them.
    pub hour: u8,
}

/// `relativeTime(unix_sec)`.
///
/// A listening history is read as recency, so a wall-clock time makes the reader
/// do the subtraction. Coarsens as it goes back, because "43 minutes ago" is
/// useful and "43 days 2 hours ago" is not.
pub fn relative_time(now: i64, then: i64) -> String {
    let d = now - then;
    if d < 0 { return "just now".into(); }   // a clock skew must not read "in -3s"
    match d {
        0..=44 => "just now".into(),
        45..=5399 => {
            let m = (d + 30) / 60;
            format!("{} minute{} ago", m.max(1), if m == 1 { "" } else { "s" })
        }
        5400..=86_399 => {
            let h = (d + 1800) / 3600;
            format!("{} hour{} ago", h.max(1), if h == 1 { "" } else { "s" })
        }
        86_400..=2_591_999 => {
            let dd = d / 86_400;
            format!("{} day{} ago", dd.max(1), if dd == 1 { "" } else { "s" })
        }
        _ => {
            let mo = d / 2_592_000;
            format!("{} month{} ago", mo.max(1), if mo == 1 { "" } else { "s" })
        }
    }
}

/// Plays per hour of day, 24 buckets.
pub fn hour_stats(plays: &[Play]) -> [u32; 24] {
    let mut h = [0u32; 24];
    for p in plays { h[(p.hour % 24) as usize] += 1; }
    h
}

/// The histogram's peak, for normalising the bars.
///
/// Normalising to the peak rather than a fixed ceiling: a quiet week scaled
/// against a busy one's maximum looks like no listening at all, and the shape is
/// what the page is for. Returns at least 1 so a division cannot be by zero.
pub fn hour_max(stats: &[u32; 24]) -> u32 { stats.iter().copied().max().unwrap_or(0).max(1) }

/// A playlist, with the reordering the detail page does.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub tracks: Vec<u64>,
}

impl Playlist {
    /// Move a track from one index to another.
    ///
    /// A MOVE, not a swap: dragging track 5 to position 1 shifts 1–4 down.
    /// Swapping would put track 1 where 5 was, which is not what the hand did
    /// and is the single most common reorder bug.
    pub fn move_track(&mut self, from: usize, to: usize) -> bool {
        if from >= self.tracks.len() || to >= self.tracks.len() || from == to { return false; }
        let t = self.tracks.remove(from);
        self.tracks.insert(to, t);
        true
    }

    /// Add, refusing a duplicate.
    ///
    /// The same track twice in a playlist is almost always a double-tap rather
    /// than an intention, and it breaks "remove this track" — which one?
    pub fn add(&mut self, track: u64) -> bool {
        if self.tracks.contains(&track) { return false; }
        self.tracks.push(track);
        true
    }

    pub fn remove(&mut self, track: u64) -> bool {
        let n = self.tracks.len();
        self.tracks.retain(|t| *t != track);
        self.tracks.len() != n
    }

    /// A playlist needs a name to be saved — an unnamed one cannot be found again.
    pub fn can_save(&self) -> bool { !self.name.trim().is_empty() }
}

/// Search state.
#[derive(Debug, Clone, Default)]
pub struct Search {
    pub query: String,
    /// Restrict to one tag; empty means all.
    pub tag: String,
}

impl Search {
    pub fn results<'a>(&self, all: &'a [Track]) -> Vec<&'a Track> {
        all.iter()
            .filter(|t| self.tag.is_empty() || t.tags.iter().any(|x| x == &self.tag))
            .filter(|t| t.matches(&self.query))
            .collect()
    }

    /// Every tag in the library with a count, most-used first.
    /// Alphabetical ties, so the tag cloud does not reshuffle between frames.
    pub fn tags(all: &[Track]) -> Vec<(String, usize)> {
        use std::collections::BTreeMap;
        let mut m: BTreeMap<&str, usize> = BTreeMap::new();
        for t in all { for g in &t.tags { *m.entry(g.as_str()).or_insert(0) += 1; } }
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.into(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: u64, title: &str, artist: &str, tags: &[&str]) -> Track {
        Track { id, title: title.into(), artist: artist.into(), album: "Album".into(),
                tags: tags.iter().map(|s| s.to_string()).collect() }
    }

    fn lib() -> Vec<Track> {
        vec![t(1, "Rain", "Ambient Co", &["calm", "rain"]),
             t(2, "Thunder", "Ambient Co", &["storm"]),
             t(3, "Morning", "Dawn", &["calm"])]
    }

    #[test]
    fn relative_time_coarsens_as_it_goes_back() {
        // "43 minutes ago" is useful; "43 days 2 hours ago" is not.
        let now = 1_700_000_000;
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now, now - 30), "just now");
        assert_eq!(relative_time(now, now - 60), "1 minute ago");
        assert_eq!(relative_time(now, now - 180), "3 minutes ago");
        assert_eq!(relative_time(now, now - 7200), "2 hours ago");
        assert_eq!(relative_time(now, now - 86_400 * 3), "3 days ago");
        assert!(relative_time(now, now - 86_400 * 90).contains("month"));
    }

    #[test]
    fn a_clock_skew_does_not_produce_a_negative_age() {
        // A play stamped in the future - clock skew across devices, which a
        // multi-room system has by definition - must not read "in -3 seconds".
        let now = 1_700_000_000;
        assert_eq!(relative_time(now, now + 500), "just now");
    }

    #[test]
    fn the_histogram_normalises_to_its_own_peak() {
        // A quiet week scaled against a busy one's maximum looks like no
        // listening at all, and the shape is what the page is for.
        let plays: Vec<Play> = [7u8, 7, 7, 22, 22, 9].iter().enumerate()
            .map(|(i, h)| Play { track: i as u64, at: 0, hour: *h }).collect();
        let s = hour_stats(&plays);
        assert_eq!(s[7], 3);
        assert_eq!(s[22], 2);
        assert_eq!(hour_max(&s), 3);
        // an empty history divides by one rather than zero
        assert_eq!(hour_max(&hour_stats(&[])), 1);
    }

    #[test]
    fn an_out_of_range_hour_cannot_panic() {
        // Hours arrive from a device clock and 24 is a real off-by-one.
        let s = hour_stats(&[Play { track: 1, at: 0, hour: 24 },
                             Play { track: 2, at: 0, hour: 255 }]);
        assert_eq!(s[0], 1, "24 wraps to 0");
        assert_eq!(s.iter().sum::<u32>(), 2);
    }

    #[test]
    fn reordering_a_playlist_moves_rather_than_swaps() {
        // Dragging track 5 to position 1 shifts 1-4 down. Swapping would put
        // track 1 where 5 was, which is not what the hand did.
        let mut p = Playlist { tracks: vec![1, 2, 3, 4, 5], ..Default::default() };
        assert!(p.move_track(4, 0));
        assert_eq!(p.tracks, vec![5, 1, 2, 3, 4]);
        // a no-op drag costs nothing and is refused rather than churning
        assert!(!p.move_track(2, 2));
        assert!(!p.move_track(9, 0));
        assert_eq!(p.tracks.len(), 5, "nothing lost or duplicated");
    }

    #[test]
    fn a_track_cannot_be_added_to_a_playlist_twice() {
        // The same track twice is almost always a double-tap, and it breaks
        // "remove this track" - which one?
        let mut p = Playlist::default();
        assert!(p.add(7));
        assert!(!p.add(7));
        assert_eq!(p.tracks, vec![7]);
        assert!(p.remove(7));
        assert!(!p.remove(7), "removing twice is false, not an error");
    }

    #[test]
    fn an_unnamed_playlist_cannot_be_saved() {
        // It cannot be found again.
        let mut p = Playlist::default();
        assert!(!p.can_save());
        p.name = "   ".into();
        assert!(!p.can_save(), "whitespace is not a name");
        p.name = "Evening".into();
        assert!(p.can_save());
    }

    #[test]
    fn search_covers_tags_as_well_as_text() {
        let all = lib();
        let mut s = Search::default();
        s.query = "calm".into();
        assert_eq!(s.results(&all).len(), 2, "matched by tag");
        s.query = "ambient".into();
        assert_eq!(s.results(&all).len(), 2, "matched by artist");
        s.query = String::new();
        s.tag = "storm".into();
        assert_eq!(s.results(&all).len(), 1, "the tag filter alone");
    }

    #[test]
    fn the_tag_cloud_is_counted_and_stably_ordered() {
        let all = lib();
        let a = Search::tags(&all);
        assert_eq!(a, Search::tags(&all));
        assert_eq!(a[0], ("calm".to_string(), 2), "most used first");
    }
}
