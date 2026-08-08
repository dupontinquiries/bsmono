//! all_tracks — `AllTracksPage`, ported.
//!
//! The whole library, a hundred rows at a time, sorted five ways.
//!
//! ## Paging is the point, not an optimisation
//!
//! A music library is the one list in this app that can be genuinely large —
//! tens of thousands of rows — and the Qt page never holds more than what has
//! been scrolled to. Loading it all would work fine on the developer's five
//! hundred tracks and fall over on someone's real collection, which is exactly
//! the class of bug this port keeps trying not to ship.
//!
//! ## The offset guard is the interesting line
//!
//! `if (p.offset !== root.trackList.length) { loading = false; return }` — an
//! arriving page whose offset does not match what we have is DISCARDED. Two
//! requests can be in flight after a fast scroll or a sort change, and appending
//! a page that starts somewhere else would interleave two orderings into one
//! list: rows duplicated, rows missing, and a scrollbar that lies. Dropping it
//! and clearing the loading flag lets the correct request land.
//!
//! ## Changing the sort resets rather than re-sorting
//!
//! What is held is a window into a server-side ordering, not the library. There
//! is nothing to re-sort locally — the other 40,000 rows are not here — so the
//! list is cleared and page zero is requested again. Sorting the visible
//! hundred would produce a list that is ordered at the top and arbitrary below
//! the fold, which is worse than a brief empty state.

/// Rows per request. The Qt page's `pageSize`.
pub const PAGE_SIZE: usize = 100;

/// How the library is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort { Title, Artist, Album, Recent, Rating }

impl Sort {
    /// The tag sent to the backend.
    pub fn tag(self) -> &'static str {
        match self {
            Sort::Title => "title",
            Sort::Artist => "artist",
            Sort::Album => "album",
            Sort::Recent => "recent",
            Sort::Rating => "rating",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Sort::Title => "Title",
            Sort::Artist => "Artist",
            Sort::Album => "Album",
            Sort::Recent => "Recent",
            Sort::Rating => "Rating",
        }
    }

    /// In the order the chips are shown.
    pub fn all() -> [Sort; 5] {
        [Sort::Title, Sort::Artist, Sort::Album, Sort::Recent, Sort::Rating]
    }

    pub fn parse(tag: &str) -> Option<Sort> {
        Sort::all().into_iter().find(|s| s.tag() == tag)
    }
}

/// One row.
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub primary_artist: String,
}

impl Track {
    /// Never blank. A row with no text cannot be tapped with any confidence,
    /// and a library scan does produce tracks with no tags at all.
    pub fn title_text(&self) -> String {
        if self.title.trim().is_empty() { "(untitled)".into() } else { self.title.clone() }
    }

    /// The Qt page falls back to "Unknown Artist" rather than an empty line —
    /// a blank there reads as a layout fault, and the row would jump height.
    pub fn artist_text(&self) -> String {
        if self.primary_artist.trim().is_empty() { "Unknown Artist".into() }
        else { self.primary_artist.clone() }
    }
}

/// A page arriving from the backend.
#[derive(Debug, Clone, PartialEq)]
pub struct PageResult {
    /// Which list this belongs to. The Qt page checks `context !== "all_tracks"`
    /// because one channel carries pages for several lists.
    pub context: String,
    pub offset: usize,
    pub total: usize,
    pub tracks: Vec<Track>,
}

/// What the page asks the host to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TracksAction {
    /// Fetch `PAGE_SIZE` rows from this offset.
    Load { offset: usize },
    Play(i64),
    Enqueue(i64),
}

/// The page.
#[derive(Debug, Clone, PartialEq)]
pub struct AllTracksPage {
    tracks: Vec<Track>,
    total: usize,
    loading: bool,
    sort: Sort,
}

pub const EMPTY_TEXT: &str = "No tracks yet. Scan a library folder from Settings.";

impl Default for AllTracksPage {
    fn default() -> Self {
        AllTracksPage { tracks: Vec::new(), total: 0, loading: false, sort: Sort::Title }
    }
}

impl AllTracksPage {
    pub fn tracks(&self) -> &[Track] { &self.tracks }
    pub fn total(&self) -> usize { self.total }
    pub fn loading(&self) -> bool { self.loading }
    pub fn sort(&self) -> Sort { self.sort }
    pub fn is_empty(&self) -> bool { self.tracks.is_empty() }

    /// Everything loaded?
    ///
    /// `total == 0` means nothing has arrived yet rather than an empty library,
    /// so it is NOT complete — otherwise the first request would never be made.
    pub fn complete(&self) -> bool {
        self.total > 0 && self.tracks.len() >= self.total
    }

    /// The header count, as the Qt page phrases it.
    pub fn count_text(&self) -> String {
        if self.total == 0 { return "—".into(); }
        format!("{} of {}", self.tracks.len(), self.total)
    }

    /// Ask for the next page, if there is one and we are not mid-flight.
    ///
    /// Both guards matter. Without the loading check a fast scroll fires the
    /// same request repeatedly; without the completeness check the page asks
    /// forever at the end of the list.
    pub fn load_more(&mut self) -> Option<TracksAction> {
        if self.loading || self.complete() { return None; }
        self.loading = true;
        Some(TracksAction::Load { offset: self.tracks.len() })
    }

    /// Start again from the top.
    pub fn reload(&mut self) -> TracksAction {
        self.tracks.clear();
        self.total = 0;
        self.loading = true;
        TracksAction::Load { offset: 0 }
    }

    /// Accept a page from the backend. Returns false when it was discarded.
    pub fn accept(&mut self, p: &PageResult) -> bool {
        // One channel carries pages for several lists.
        if p.context != "all_tracks" { return false; }
        // THE guard. Two requests can be in flight after a fast scroll or a
        // sort change, and appending a page that starts somewhere else would
        // interleave two orderings: rows duplicated, rows missing, a scrollbar
        // that lies. Clearing `loading` lets the correct request land.
        if p.offset != self.tracks.len() {
            self.loading = false;
            return false;
        }
        self.tracks.extend(p.tracks.iter().cloned());
        self.total = p.total;
        self.loading = false;
        true
    }

    /// Change the ordering.
    ///
    /// Resets rather than re-sorting: what is held is a WINDOW into a
    /// server-side ordering, and the other rows are not here. Sorting the
    /// visible hundred would give a list ordered at the top and arbitrary below
    /// the fold, which is worse than a brief empty state.
    pub fn set_sort(&mut self, s: Sort) -> Option<TracksAction> {
        if s == self.sort { return None; }
        self.sort = s;
        Some(self.reload())
    }
}

/// Draw the page.
pub fn all_tracks_ui(ui: &mut egui::Ui, p: &mut AllTracksPage,
                     c: &uikit_egui::PaletteColors) -> Option<TracksAction> {
    let mut act = None;
    ui.horizontal(|ui| {
        ui.colored_label(c.text_weak, "SORT");
        for s in Sort::all() {
            let sel = p.sort() == s;
            if ui.selectable_label(sel, s.label()).clicked() {
                if let Some(a) = p.set_sort(s) { act = Some(a); }
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.colored_label(c.text_weak, p.count_text());
        });
    });
    ui.separator();

    if p.is_empty() && !p.loading() {
        ui.colored_label(c.text_weak, EMPTY_TEXT);
        return act;
    }

    let rows: Vec<Track> = p.tracks().to_vec();
    egui::ScrollArea::vertical().id_salt("all_tracks").show(ui, |ui| {
        for t in &rows {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    if ui.add(egui::Button::new(t.title_text()).frame(false)).clicked() {
                        act = Some(TracksAction::Play(t.id));
                    }
                    ui.colored_label(c.text_weak,
                        egui::RichText::new(t.artist_text()).small());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("+").on_hover_text("Add to queue").clicked() {
                        act = Some(TracksAction::Enqueue(t.id));
                    }
                });
            });
        }
        // Asking at the bottom of what is drawn, rather than on a scroll
        // fraction: the fraction is meaningless while the list is still growing.
        if !p.complete() {
            ui.colored_label(c.text_weak, if p.loading() { "Loading…" } else { "" });
            if !p.loading() {
                if let Some(a) = p.load_more() { act = Some(a); }
            }
        }
    });
    act
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: i64) -> Track {
        Track { id, title: format!("Track {id}"), primary_artist: "Ada".into() }
    }

    fn page(offset: usize, total: usize, n: usize) -> PageResult {
        PageResult { context: "all_tracks".into(), offset, total,
                     tracks: (0..n).map(|i| track((offset + i) as i64)).collect() }
    }

    #[test]
    fn the_first_request_is_made_from_an_empty_page() {
        // total == 0 means nothing has arrived yet rather than an empty
        // library; treating it as complete would mean never asking at all.
        let mut p = AllTracksPage::default();
        assert!(!p.complete());
        assert_eq!(p.load_more(), Some(TracksAction::Load { offset: 0 }));
    }

    #[test]
    fn a_second_request_is_refused_while_one_is_in_flight() {
        // Without this a fast scroll fires the same request repeatedly.
        let mut p = AllTracksPage::default();
        p.load_more();
        assert_eq!(p.load_more(), None);
    }

    #[test]
    fn pages_append_in_order() {
        let mut p = AllTracksPage::default();
        p.load_more();
        assert!(p.accept(&page(0, 250, 100)));
        assert_eq!(p.tracks().len(), 100);
        assert_eq!(p.load_more(), Some(TracksAction::Load { offset: 100 }));
        assert!(p.accept(&page(100, 250, 100)));
        assert_eq!(p.tracks().len(), 200);
    }

    #[test]
    fn a_page_with_the_wrong_offset_is_discarded() {
        // THE guard. Two requests in flight after a fast scroll, and appending
        // a page that starts elsewhere interleaves two orderings: rows
        // duplicated, rows missing, a scrollbar that lies.
        let mut p = AllTracksPage::default();
        p.load_more();
        p.accept(&page(0, 250, 100));
        p.load_more();
        assert!(!p.accept(&page(200, 250, 100)), "offset 200 but we hold 100");
        assert_eq!(p.tracks().len(), 100);
    }

    #[test]
    fn discarding_a_page_clears_loading_so_the_right_one_can_land() {
        // Otherwise the page is stuck: it refuses new requests forever because
        // it believes one is still in flight.
        let mut p = AllTracksPage::default();
        p.load_more();
        p.accept(&page(50, 250, 100));
        assert!(!p.loading());
        assert_eq!(p.load_more(), Some(TracksAction::Load { offset: 0 }));
    }

    #[test]
    fn a_page_for_another_list_is_ignored() {
        // One channel carries pages for several lists.
        let mut p = AllTracksPage::default();
        p.load_more();
        let mut other = page(0, 250, 100);
        other.context = "search".into();
        assert!(!p.accept(&other));
        assert_eq!(p.tracks().len(), 0);
    }

    #[test]
    fn the_page_stops_asking_at_the_end_of_the_library() {
        // Without the completeness check it asks forever.
        let mut p = AllTracksPage::default();
        p.load_more();
        p.accept(&page(0, 150, 100));
        p.load_more();
        p.accept(&page(100, 150, 50));
        assert!(p.complete());
        assert_eq!(p.load_more(), None);
    }

    #[test]
    fn changing_the_sort_resets_rather_than_reordering_what_is_held() {
        // What is held is a WINDOW into a server-side ordering; the other rows
        // are not here. Sorting the visible hundred gives a list ordered at the
        // top and arbitrary below the fold.
        let mut p = AllTracksPage::default();
        p.load_more();
        p.accept(&page(0, 250, 100));
        assert_eq!(p.set_sort(Sort::Artist), Some(TracksAction::Load { offset: 0 }));
        assert_eq!(p.tracks().len(), 0);
        assert_eq!(p.total(), 0);
        assert_eq!(p.sort(), Sort::Artist);
    }

    #[test]
    fn choosing_the_sort_already_in_use_does_nothing() {
        // Otherwise tapping the highlighted chip throws away a loaded library
        // and reloads exactly the same rows.
        let mut p = AllTracksPage::default();
        p.load_more();
        p.accept(&page(0, 250, 100));
        assert_eq!(p.set_sort(Sort::Title), None);
        assert_eq!(p.tracks().len(), 100);
    }

    #[test]
    fn every_sort_has_a_distinct_tag_and_label() {
        // A duplicate tag would silently select the wrong ordering.
        let tags: Vec<&str> = Sort::all().iter().map(|s| s.tag()).collect();
        let mut t = tags.clone(); t.sort_unstable(); t.dedup();
        assert_eq!(t.len(), tags.len());
        for s in Sort::all() { assert_eq!(Sort::parse(s.tag()), Some(s)); }
        assert_eq!(Sort::parse("nonsense"), None);
    }

    #[test]
    fn an_untagged_track_still_renders_something_tappable() {
        // A library scan does produce tracks with no tags, and a row with no
        // text cannot be tapped with any confidence.
        let t = Track { id: 1, title: "  ".into(), primary_artist: String::new() };
        assert_eq!(t.title_text(), "(untitled)");
        assert_eq!(t.artist_text(), "Unknown Artist");
    }

    #[test]
    fn the_count_reads_as_a_fraction_of_the_whole() {
        // "100" alone would look like the library has a hundred tracks.
        let mut p = AllTracksPage::default();
        assert_eq!(p.count_text(), "—", "nothing known yet");
        p.load_more();
        p.accept(&page(0, 250, 100));
        assert_eq!(p.count_text(), "100 of 250");
    }

    #[test]
    fn an_empty_library_says_how_to_fill_it() {
        let p = AllTracksPage::default();
        assert!(p.is_empty());
        assert!(EMPTY_TEXT.contains("Scan"));
    }
}
