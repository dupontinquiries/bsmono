//! tadpole — image to text, all Rust.
//!
//! Frog does this well and does it with **pytesseract over a GTK app**: a
//! portal screenshot, a QR decode attempted first, then Tesseract with a
//! per-language `traineddata` file. Its quality comes from Tesseract's mature
//! layout analysis and from Frog's own care around capture — not from anything
//! that has to be Python.
//!
//! ## The conceptual difference from rusto-rs, which decides the design
//!
//! | | Frog / Tesseract | rusto-rs / PP-OCRv5 |
//! |---|---|---|
//! | shape | page layout analysis, then LSTM line recognition | **detection net** finds text boxes, **recognition net** reads each |
//! | languages | one `traineddata` per language, downloaded per language | one model covers many scripts; a dictionary maps class indices |
//! | output | a string, optionally with boxes | **always** boxes: `text`, `score`, four corner points |
//! | failure | a page it cannot segment produces mangled reading order | a box it cannot read produces a low `score` next to good ones |
//!
//! The last row is why this app is built around **regions rather than a blob of
//! text**. Tesseract hands you a page and you trust it or you do not; PP-OCRv5
//! hands you a list of boxes each carrying its own confidence, so the UI can
//! show *which part* it is unsure about and let a user fix or ignore that part
//! alone. Rendering that as one text area would throw away the thing the newer
//! model is better at.
//!
//! ## The engine is a trait
//!
//! `models/PPOCR_v5` is 28MB of MNN weights and cannot live in this repo. More
//! importantly, the OCR engine is exactly the piece that should be swappable —
//! rusto-rs today, something else later, a stub in tests. Everything here is
//! written against `Ocr`, and the app logic is testable with no model at all.

pub mod audio;
pub mod models;
pub mod overview;
pub mod paste;
pub mod speakers;
pub mod table;

use std::collections::BTreeSet;

/// One recognised region.
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    pub text: String,
    /// 0..=1 from the recognition net.
    pub score: f32,
    /// Corners in image pixels: top-left, top-right, bottom-right, bottom-left.
    /// Kept as four points rather than a rect because a photographed page is
    /// not axis-aligned, and squashing it to a rect loses the skew a user can
    /// see — which is exactly the cue that a bad read is bad.
    pub corners: [(f32, f32); 4],
}

impl Region {
    /// Axis-aligned bounds, for hit-testing and layout.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        let xs = self.corners.iter().map(|c| c.0);
        let ys = self.corners.iter().map(|c| c.1);
        (
            xs.clone().fold(f32::MAX, f32::min),
            ys.clone().fold(f32::MAX, f32::min),
            xs.fold(f32::MIN, f32::max),
            ys.fold(f32::MIN, f32::max),
        )
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (x0, y0, x1, y1) = self.bounds();
        x >= x0 && x <= x1 && y >= y0 && y <= y1
    }

    /// Is this read trustworthy enough to present without a warning?
    pub fn is_confident(&self) -> bool { self.score >= CONFIDENT }
}

/// Below this, a region is shown but marked.
///
/// 0.6 is deliberately low. A high bar would hide text the user can plainly
/// read on screen, and an OCR app that silently drops words is worse than one
/// that shows them with a caveat — the user is looking at the image and can
/// judge.
pub const CONFIDENT: f32 = 0.6;

/// What an OCR backend must do. One call, image in, regions out.
pub trait Ocr {
    fn recognise(&self, image: &ImageRef) -> Result<Vec<Region>, OcrError>;
}

/// Borrowed 8-bit RGBA pixels. Deliberately not an owned image type: the app
/// does not decode, and a trait that demanded one would drag an image crate
/// into every consumer including the tests.
pub struct ImageRef<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

#[derive(Clone, Debug, PartialEq)]
pub enum OcrError {
    /// The backend has no model loaded. Distinct from "found nothing", because
    /// a missing model is fixable by the user and an empty page is not.
    NoModel,
    Failed(String),
}

/// Reading order for the recognised regions.
///
/// Top-to-bottom, then left-to-right, with a **line tolerance**: two boxes whose
/// vertical centres are within one line height are the same line, however their
/// tops differ. Sorting by raw `y` alone scrambles a line whose words sit a few
/// pixels apart — which is every photographed page.
pub fn reading_order(regions: &[Region]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..regions.len()).collect();
    let heights: Vec<f32> = regions.iter()
        .map(|r| { let (_, y0, _, y1) = r.bounds(); (y1 - y0).max(1.0) })
        .collect();
    let centre = |i: usize| {
        let (x0, y0, _, y1) = regions[i].bounds();
        (x0, (y0 + y1) * 0.5)
    };
    idx.sort_by(|&a, &b| {
        let (ax, ay) = centre(a);
        let (bx, by) = centre(b);
        let tol = heights[a].max(heights[b]) * 0.6;
        if (ay - by).abs() <= tol {
            ax.partial_cmp(&bx).unwrap_or(std::cmp::Ordering::Equal)
        } else {
            ay.partial_cmp(&by).unwrap_or(std::cmp::Ordering::Equal)
        }
    });
    idx
}

/// The app's state.
pub struct Tadpole {
    pub regions: Vec<Region>,
    /// Transcript segments, when the source was audio or pasted text.
    pub segments: Vec<audio::Segment>,
    /// Which regions the user has selected, by index.
    selected: BTreeSet<usize>,
    /// Where the weights are, or what is missing. Re-checked on demand rather
    /// than cached at startup: a user who drops the files in while the app is
    /// open should not have to restart to be believed.
    pub models: models::ModelState,
    /// How to present the page. Defaults to Lines: a page is prose until the
    /// geometry says otherwise, and guessing "table" on a paragraph produces a
    /// two-column mess that looks like a bug.
    pub render: table::Render,
    pub last_error: Option<OcrError>,
    /// Who said what, when the segments came from real audio. `None` for
    /// pasted text: its timings are line indices, and turn detection on fake
    /// gaps would be labels made of noise.
    pub diarised: Option<speakers::Diarised>,
    /// Show only this speaker. Honoured only when the labels are identities
    /// (`Diarised::filterable`) — filtering turn numbers silently loses lines,
    /// which is the exact affordance `speakers::Source` exists to forbid.
    pub speaker_filter: Option<usize>,
}

impl Default for Tadpole {
    fn default() -> Self {
        Tadpole {
            regions: Vec::new(),
            segments: Vec::new(),
            selected: BTreeSet::new(),
            render: table::Render::Lines,
            models: models::ModelState::Missing,
            last_error: None,
            diarised: None,
            speaker_filter: None,
        }
    }
}

impl Tadpole {
    /// Re-scan for weights. Cheap — a handful of `exists` checks — so a caller
    /// can run it whenever the setup page is shown.
    pub fn rescan_models<F>(&mut self, home: &std::path::Path, exists: F)
    where F: Fn(&std::path::Path) -> bool {
        self.models = models::discover(home, exists);
    }

    /// Can a scan actually run?
    ///
    /// Asked before offering the button rather than after pressing it: an OCR
    /// app whose Scan fails with "no model" taught the user nothing they could
    /// not have been told first.
    pub fn can_scan(&self) -> bool { self.models.is_ready() }

    /// The table this page makes, if it makes one.
    pub fn table(&self) -> Option<table::Table> {
        let t = table::detect(&self.regions, 0.6);
        if t.looks_like_table() { Some(t) } else { None }
    }

    /// The page as text, in the current render mode.
    ///
    /// Falls back to lines when the mode asks for a table and the page is not
    /// one — a toggle that produced an empty pane would read as the app
    /// breaking rather than as the page not being tabular.
    pub fn rendered(&self) -> String {
        match self.render {
            table::Render::Lines => self.text(),
            table::Render::Markdown => match self.table() {
                Some(t) => table::to_markdown(&t),
                None => self.text(),
            },
            table::Render::Raw => match self.table() {
                Some(t) => table::to_raw(&t, self.px_per_char()),
                None => self.text(),
            },
        }
    }

    /// Pixels per character, estimated from the regions themselves.
    ///
    /// Measured rather than assumed: a phone photo and a 4K screenshot of the
    /// same table have wildly different pixel scales, and a fixed constant
    /// would make one of them collapse and the other sprawl.
    fn px_per_char(&self) -> f32 {
        let mut total_w = 0.0f32;
        let mut total_c = 0usize;
        for r in &self.regions {
            let (x0, _, x1, _) = r.bounds();
            let n = r.text.chars().count();
            if n > 0 { total_w += x1 - x0; total_c += n; }
        }
        if total_c == 0 { 8.0 } else { (total_w / total_c as f32).max(1.0) }
    }

    /// Accept whatever was dropped, pasted or screenshotted.
    ///
    /// One entry point for all three, because they deliver the same three
    /// things in practice and three handlers would drift. Returns the
    /// classification so the caller can show the message for a refusal without
    /// this function needing to know how messages are drawn.
    pub fn accept(
        &mut self,
        bytes: &[u8],
        name: Option<&str>,
        transcriber: &dyn audio::Transcriber,
    ) -> paste::Incoming {
        let what = paste::classify(bytes, name);
        match &what {
            paste::Incoming::Image(_) => {
                // The decoder lives in the shell, not here — this crate has no
                // image dependency on purpose. A caller that has pixels calls
                // scan() directly; accept() is for routing.
                self.last_error = None;
            }
            paste::Incoming::Audio(f) => {
                match transcriber.transcribe(bytes, *f) {
                    Ok(segs) => { self.take_segments(segs, true); }
                    Err(e) => {
                        self.segments.clear();
                        self.diarised = None;
                        self.speaker_filter = None;
                        self.last_error = Some(e);
                    }
                }
            }
            paste::Incoming::Text(t) => {
                // Text has no positions, so each line becomes a segment at its
                // line index. That is enough for the overview, which only cares
                // about ordering — and it means pasted notes get a mindmap.
                let segs: Vec<audio::Segment> = t.lines()
                    .filter(|l| !l.trim().is_empty())
                    .enumerate()
                    .map(|(i, l)| audio::Segment {
                        start_ms: i as u64, end_ms: i as u64, text: l.trim().into(), score: 1.0,
                    })
                    .collect();
                self.take_segments(segs, false);
            }
            _ => {}
        }
        what
    }

    fn take_segments(&mut self, segs: Vec<audio::Segment>, timed: bool) {
        self.segments = segs;
        // Regions and segments are two views of one document, and a new source
        // replaces the document. Leaving stale regions would put the previous
        // image's text in this recording's overview.
        self.regions.clear();
        self.selected.clear();
        self.last_error = None;
        // Turn labels only where the pauses are real. Pasted text carries
        // line-index timestamps; running the 700ms heuristic over those would
        // produce one giant turn and call it structure.
        self.diarised = if timed && !self.segments.is_empty() {
            Some(speakers::Diarised::from_turns(self.segments.clone()))
        } else {
            None
        };
        self.speaker_filter = None;
    }

    /// Topics for the overview pane, from whichever half has material.
    pub fn overview(&self) -> Vec<overview::Topic> {
        let items = if !self.segments.is_empty() {
            overview::from_segments(&self.segments)
        } else {
            overview::from_regions(&self.regions)
        };
        overview::topics(&items, 4, 12)
    }

    /// Run a backend over an image and take the result.
    pub fn scan(&mut self, ocr: &dyn Ocr, image: &ImageRef) {
        match ocr.recognise(image) {
            Ok(r) => {
                self.regions = r;
                // Same rule in the other direction: a scan replaces the
                // document, so a previous transcript must not linger.
                self.segments.clear();
                self.diarised = None;
                self.speaker_filter = None;
                // Selection is cleared, not remapped: indices from the previous
                // image mean nothing here, and keeping them would copy text
                // from wherever those indices happen to land.
                self.selected.clear();
                self.last_error = None;
            }
            Err(e) => {
                self.regions.clear();
                self.selected.clear();
                self.last_error = Some(e);
            }
        }
    }

    pub fn is_selected(&self, i: usize) -> bool { self.selected.contains(&i) }
    pub fn selection_count(&self) -> usize { self.selected.len() }

    pub fn toggle(&mut self, i: usize) {
        if i >= self.regions.len() { return; }
        if !self.selected.remove(&i) { self.selected.insert(i); }
    }

    pub fn select_all(&mut self) {
        self.selected = (0..self.regions.len()).collect();
    }

    pub fn clear_selection(&mut self) { self.selected.clear(); }

    /// Region under a point, or `None`.
    ///
    /// Searches in reverse so the **last drawn wins** — overlapping boxes are
    /// normal when a detector finds a word inside a heading, and picking the
    /// first would always give the larger one, which is never what a tap on a
    /// word means.
    pub fn hit(&self, x: f32, y: f32) -> Option<usize> {
        (0..self.regions.len()).rev().find(|&i| self.regions[i].contains(x, y))
    }

    /// The selected text, or all of it when nothing is selected.
    ///
    /// "Nothing selected means everything" is the behaviour a copy button
    /// needs: the common case is wanting the whole page, and forcing a
    /// select-all first is a step for nothing.
    pub fn text(&self) -> String {
        let order = reading_order(&self.regions);
        let take_all = self.selected.is_empty();
        order.into_iter()
            .filter(|i| take_all || self.selected.contains(i))
            .map(|i| self.regions[i].text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Regions the user should look at before trusting.
    pub fn uncertain(&self) -> Vec<usize> {
        (0..self.regions.len()).filter(|&i| !self.regions[i].is_confident()).collect()
    }

    /// Set (or clear) the speaker filter. Refused — returning false and
    /// clearing — when the labels are not identities: `speakers::Source`
    /// draws that line, and honouring a filter on turn numbers would silently
    /// drop half of what a returning speaker said.
    pub fn set_speaker_filter(&mut self, id: Option<usize>) -> bool {
        let ok = match (&self.diarised, id) {
            (_, None) => true,
            (Some(d), Some(_)) => d.filterable(),
            (None, Some(_)) => false,
        };
        self.speaker_filter = if ok { id } else { None };
        ok
    }

    /// The transcript as plain text, respecting the speaker filter where the
    /// labels can support one. With no diarisation, the joined segments.
    pub fn transcript(&self) -> String {
        match &self.diarised {
            Some(d) => d.raw_text(self.speaker_filter),
            None => self
                .segments
                .iter()
                .map(|s| s.text.trim())
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// The transcript with speaker labels, when there are labels to show.
    pub fn annotated_transcript(&self) -> Option<String> {
        self.diarised.as_ref().map(|d| d.annotated_text())
    }
}

/// What the context menu offers for a region.
///
/// Built here rather than in the draw code so it is testable, and so the same
/// list serves the right-click and the long-press — `uikit_egui::menu` already
/// collapses those into one trigger, which is why this app did not have to
/// invent a menu.
pub fn region_menu(app: &Tadpole, hit: Option<usize>) -> Vec<uikit_egui::menu::MenuItem> {
    use uikit_egui::menu::MenuItem;
    let mut items = Vec::new();
    if let Some(i) = hit {
        items.push(MenuItem::new("copy_region", "Copy this text"));
        items.push(MenuItem::new(
            if app.is_selected(i) { "deselect" } else { "select" },
            if app.is_selected(i) { "Deselect" } else { "Add to selection" },
        ));
    }
    let copy_all = MenuItem::new("copy_all", if app.selection_count() > 0 {
        "Copy selection"
    } else {
        "Copy all text"
    });
    // Disabled rather than absent when there is nothing to copy: an entry that
    // vanishes moves every entry below it, and a menu whose shape changes is a
    // menu you mis-tap.
    items.push(if app.regions.is_empty() { copy_all.disabled() } else { copy_all });
    let clear = MenuItem::new("clear_selection", "Clear selection");
    items.push(if app.selection_count() == 0 { clear.disabled() } else { clear });
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(text: &str, score: f32, x: f32, y: f32, w: f32, h: f32) -> Region {
        Region {
            text: text.into(),
            score,
            corners: [(x, y), (x + w, y), (x + w, y + h), (x, y + h)],
        }
    }

    struct Fake(Vec<Region>);
    impl Ocr for Fake {
        fn recognise(&self, _i: &ImageRef) -> Result<Vec<Region>, OcrError> {
            Ok(self.0.clone())
        }
    }
    struct NoModel;
    impl Ocr for NoModel {
        fn recognise(&self, _i: &ImageRef) -> Result<Vec<Region>, OcrError> {
            Err(OcrError::NoModel)
        }
    }

    fn img() -> ImageRef<'static> {
        static PIX: [u8; 4] = [0, 0, 0, 255];
        ImageRef { width: 1, height: 1, rgba: &PIX }
    }

    #[test]
    fn words_on_one_line_read_left_to_right_even_when_their_tops_differ() {
        // Every photographed page has this: boxes on the same line sitting a
        // few pixels apart vertically. Sorting by raw y scrambles the line.
        let regions = vec![
            r("world", 0.9, 100.0, 12.0, 50.0, 20.0),
            r("hello", 0.9, 10.0, 10.0, 50.0, 20.0),
        ];
        let order = reading_order(&regions);
        assert_eq!(order, vec![1, 0], "same line, so x decides");
    }

    #[test]
    fn separate_lines_read_top_to_bottom() {
        let regions = vec![
            r("second", 0.9, 10.0, 100.0, 50.0, 20.0),
            r("first", 0.9, 200.0, 10.0, 50.0, 20.0),
        ];
        assert_eq!(reading_order(&regions), vec![1, 0], "y decides across lines");
    }

    #[test]
    fn copy_with_nothing_selected_gives_the_whole_page() {
        // The common case is wanting all of it, and forcing a select-all first
        // is a step for nothing.
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("one", 0.9, 0.0, 0.0, 10.0, 10.0),
                            r("two", 0.9, 0.0, 40.0, 10.0, 10.0)]), &img());
        assert_eq!(app.text(), "one\ntwo");
    }

    #[test]
    fn a_selection_narrows_the_copy_and_keeps_reading_order() {
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("a", 0.9, 0.0, 80.0, 10.0, 10.0),
                            r("b", 0.9, 0.0, 0.0, 10.0, 10.0),
                            r("c", 0.9, 0.0, 40.0, 10.0, 10.0)]), &img());
        app.toggle(0);
        app.toggle(1);
        assert_eq!(app.text(), "b\na", "selected, in reading order — not click order");
    }

    #[test]
    fn a_new_scan_clears_the_selection_rather_than_remapping_it() {
        // Indices from the previous image mean nothing in this one, and keeping
        // them copies text from wherever those indices happen to land.
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("old", 0.9, 0.0, 0.0, 10.0, 10.0)]), &img());
        app.select_all();
        assert_eq!(app.selection_count(), 1);
        app.scan(&Fake(vec![r("new", 0.9, 0.0, 0.0, 10.0, 10.0),
                            r("also", 0.9, 0.0, 40.0, 10.0, 10.0)]), &img());
        assert_eq!(app.selection_count(), 0);
        assert_eq!(app.text(), "new\nalso");
    }

    #[test]
    fn a_tap_picks_the_innermost_overlapping_region() {
        // A detector finding a word inside a heading is normal. Picking the
        // first match always gives the larger box, which is never what a tap on
        // a word means.
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("HEADING", 0.9, 0.0, 0.0, 200.0, 40.0),
                            r("word", 0.9, 10.0, 5.0, 40.0, 20.0)]), &img());
        assert_eq!(app.hit(20.0, 10.0), Some(1));
        assert_eq!(app.hit(150.0, 10.0), Some(0), "outside the word, inside the heading");
        assert_eq!(app.hit(500.0, 500.0), None);
    }

    #[test]
    fn low_confidence_regions_are_shown_and_flagged_rather_than_dropped() {
        // An OCR app that silently drops words is worse than one that shows
        // them with a caveat — the user is looking at the image and can judge.
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("sure", 0.95, 0.0, 0.0, 10.0, 10.0),
                            r("maybe", 0.31, 0.0, 40.0, 10.0, 10.0)]), &img());
        assert_eq!(app.regions.len(), 2, "nothing was dropped");
        assert_eq!(app.uncertain(), vec![1]);
        assert!(app.text().contains("maybe"));
    }

    #[test]
    fn a_missing_model_is_distinct_from_an_empty_page() {
        // One is fixable by the user and the other is not, so a UI that showed
        // "no text found" for both would send people looking at their image
        // instead of their install.
        let mut app = Tadpole::default();
        app.scan(&NoModel, &img());
        assert_eq!(app.last_error, Some(OcrError::NoModel));
        assert!(app.regions.is_empty());

        app.scan(&Fake(vec![]), &img());
        assert_eq!(app.last_error, None, "an empty page is not an error");
    }

    #[test]
    fn the_context_menu_offers_region_actions_only_over_a_region() {
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("x", 0.9, 0.0, 0.0, 10.0, 10.0)]), &img());
        let over = region_menu(&app, Some(0));
        assert!(over.iter().any(|i| i.id == "copy_region"));
        let empty = region_menu(&app, None);
        assert!(!empty.iter().any(|i| i.id == "copy_region"),
                "nothing under the cursor, so no per-region action");
    }

    #[test]
    fn menu_entries_are_disabled_rather_than_removed_when_unavailable() {
        // An entry that vanishes moves every entry below it, and a menu whose
        // shape changes is a menu you mis-tap.
        let app = Tadpole::default();
        let m = region_menu(&app, None);
        let copy = m.iter().find(|i| i.id == "copy_all").expect("always present");
        assert!(!copy.enabled, "nothing to copy yet");
        let clear = m.iter().find(|i| i.id == "clear_selection").expect("always present");
        assert!(!clear.enabled);
    }

    #[test]
    fn the_copy_label_says_which_copy_it_means() {
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("a", 0.9, 0.0, 0.0, 10.0, 10.0),
                            r("b", 0.9, 0.0, 40.0, 10.0, 10.0)]), &img());
        let all = region_menu(&app, None);
        assert_eq!(all.iter().find(|i| i.id == "copy_all").unwrap().label, "Copy all text");
        app.toggle(0);
        let some = region_menu(&app, None);
        assert_eq!(some.iter().find(|i| i.id == "copy_all").unwrap().label, "Copy selection");
    }

    #[test]
    fn toggling_a_region_that_does_not_exist_is_ignored() {
        let mut app = Tadpole::default();
        app.toggle(7);
        assert_eq!(app.selection_count(), 0);
    }

    #[test]
    fn the_render_toggle_switches_presentation_without_changing_the_data() {
        // Raw exists because reflowing a receipt is corruption dressed as
        // tidiness; markdown exists because a wall of cells is unreadable. Both
        // read the same regions.
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![
            r("Item", 0.9, 10.0, 10.0, 40.0, 20.0), r("Price", 0.9, 300.0, 11.0, 45.0, 20.0),
            r("Tea", 0.9, 10.0, 50.0, 30.0, 20.0), r("2.50", 0.9, 300.0, 49.0, 38.0, 20.0),
        ]), &img());

        app.render = table::Render::Markdown;
        let md = app.rendered();
        assert!(md.contains("| Item | Price |"), "markdown: {md}");
        assert!(md.lines().nth(1).unwrap().contains("---"));

        app.render = table::Render::Raw;
        let raw = app.rendered();
        assert!(!raw.contains('|'), "raw keeps spacing, not pipes: {raw}");
        assert!(raw.lines().next().unwrap().contains("Item"));

        app.render = table::Render::Lines;
        assert_eq!(app.rendered(), "Item\nPrice\nTea\n2.50");
    }

    #[test]
    fn a_table_mode_on_a_page_that_is_not_a_table_falls_back_to_lines() {
        // A toggle that produced an empty pane would read as the app breaking
        // rather than as the page not being tabular.
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("just prose here", 0.9, 10.0, 10.0, 200.0, 20.0)]), &img());
        app.render = table::Render::Markdown;
        assert_eq!(app.rendered(), "just prose here");
        app.render = table::Render::Raw;
        assert_eq!(app.rendered(), "just prose here");
    }

    #[test]
    fn a_rescan_notices_weights_that_arrived_while_the_app_was_open() {
        // The whole reason discovery is re-run rather than cached: a user who
        // drops the files in should not have to restart to be believed.
        let home = std::path::PathBuf::from("/home/kd");
        let mut app = Tadpole::default();
        app.rescan_models(&home, |_| false);
        assert!(!app.can_scan());

        app.rescan_models(&home, |p| {
            let s = p.display().to_string();
            s.starts_with("/home/kd/models/") && !s.contains("PPOCR")
        });
        assert!(app.can_scan(), "the same app instance now sees them");
    }

    #[test]
    fn scanning_is_offered_only_when_it_would_work() {
        // A Scan button that fails with "no model" taught the user nothing they
        // could not have been told first.
        let app = Tadpole::default();
        assert!(!app.can_scan(), "a fresh app has not found weights yet");
    }

    struct FakeVoice(Vec<audio::Segment>);
    impl audio::Transcriber for FakeVoice {
        fn transcribe(&self, _b: &[u8], _f: audio::Format) -> Result<Vec<audio::Segment>, OcrError> {
            Ok(self.0.clone())
        }
    }
    struct NoVoice;
    impl audio::Transcriber for NoVoice {
        fn transcribe(&self, _b: &[u8], _f: audio::Format) -> Result<Vec<audio::Segment>, OcrError> {
            Err(OcrError::NoModel)
        }
    }
    fn wav_bytes() -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(b"WAVE");
        v.extend_from_slice(&[0; 8]);
        v
    }

    #[test]
    fn a_dropped_wav_transcribes_and_fills_the_overview() {
        let mut app = Tadpole::default();
        let voice = FakeVoice(vec![
            audio::Segment { start_ms: 0, end_ms: 1, text: "routing table basics".into(), score: 0.9 },
            audio::Segment { start_ms: 30_000, end_ms: 1, text: "more routing table talk".into(), score: 0.9 },
        ]);
        let what = app.accept(&wav_bytes(), Some("meeting.wav"), &voice);
        assert_eq!(what, paste::Incoming::Audio(audio::Format::Wav));
        assert_eq!(app.segments.len(), 2);
        assert!(app.overview().iter().any(|t| t.term == "routing"));
    }

    #[test]
    fn pasted_text_gets_an_overview_without_any_model() {
        // The overview works on ordering alone, so notes off the clipboard earn
        // a mindmap with nothing installed.
        let mut app = Tadpole::default();
        let what = app.accept(b"routing tables
something else
routing again", None, &NoVoice);
        assert!(matches!(what, paste::Incoming::Text(_)));
        assert_eq!(app.segments.len(), 3);
        assert!(app.overview().iter().any(|t| t.term == "routing"));
    }

    #[test]
    fn a_new_source_replaces_the_document_in_both_directions() {
        // Regions and segments are two views of one document. Leaving stale
        // ones puts the previous image's text in this recording's overview.
        let mut app = Tadpole::default();
        app.scan(&Fake(vec![r("from the page", 0.9, 0.0, 0.0, 10.0, 10.0)]), &img());
        assert_eq!(app.regions.len(), 1);

        app.accept(&wav_bytes(), None,
                   &FakeVoice(vec![audio::Segment { start_ms: 0, end_ms: 1,
                       text: "from the tape".into(), score: 0.9 }]));
        assert!(app.regions.is_empty(), "the page went away");
        assert_eq!(app.segments.len(), 1);

        app.scan(&Fake(vec![r("a new page", 0.9, 0.0, 0.0, 10.0, 10.0)]), &img());
        assert!(app.segments.is_empty(), "and the tape went away");
    }

    #[test]
    fn a_failed_transcription_reports_the_missing_model() {
        let mut app = Tadpole::default();
        app.accept(&wav_bytes(), None, &NoVoice);
        assert_eq!(app.last_error, Some(OcrError::NoModel));
        assert!(app.segments.is_empty());
    }

    #[test]
    fn an_unsupported_drop_leaves_the_current_work_alone() {
        // Dropping a PDF by accident must not clear the transcript you were
        // reading — a refusal is not a reset.
        let mut app = Tadpole::default();
        app.accept(b"notes about routing
and more routing", None, &NoVoice);
        assert_eq!(app.segments.len(), 2);

        let mut pdf = b"%PDF-1.7".to_vec();
        pdf.extend_from_slice(&[0; 16]);
        let what = app.accept(&pdf, Some("paper.pdf"), &NoVoice);
        assert!(what.message().unwrap().contains("PNG"));
        assert_eq!(app.segments.len(), 2, "the transcript survived the refusal");
    }

    #[test]
    fn the_default_is_lines_because_a_page_is_prose_until_proven_otherwise() {
        // Guessing "table" on a paragraph produces a two-column mess that looks
        // like a bug.
        assert_eq!(Tadpole::default().render, table::Render::Lines);
    }

    #[test]
    fn character_width_is_measured_from_the_page_not_assumed() {
        // A phone photo and a 4K screenshot of the same table have wildly
        // different pixel scales; a fixed constant collapses one and sprawls
        // the other.
        let mut small = Tadpole::default();
        small.scan(&Fake(vec![r("abcd", 0.9, 0.0, 0.0, 40.0, 12.0)]), &img());
        let mut large = Tadpole::default();
        large.scan(&Fake(vec![r("abcd", 0.9, 0.0, 0.0, 400.0, 120.0)]), &img());
        assert!(large.px_per_char() > small.px_per_char() * 5.0);
    }

    #[test]
    fn skewed_corners_still_produce_usable_bounds() {
        // A photographed page is not axis-aligned. Keeping four points rather
        // than a rect preserves the skew a user can see, and bounds() is the
        // flattening done only where hit-testing needs it.
        let skew = Region {
            text: "tilted".into(), score: 0.9,
            corners: [(10.0, 12.0), (60.0, 8.0), (62.0, 30.0), (12.0, 34.0)],
        };
        let (x0, y0, x1, y1) = skew.bounds();
        assert_eq!((x0, y0, x1, y1), (10.0, 8.0, 62.0, 34.0));
        assert!(skew.contains(30.0, 20.0));
    }

    // ── speakers wiring: diarisation is part of the document lifecycle ──

    fn tseg(start: u64, end: u64, text: &str) -> audio::Segment {
        audio::Segment { start_ms: start, end_ms: end, text: text.into(), score: 0.9 }
    }

    #[test]
    fn real_audio_gets_turn_labels_but_refuses_a_speaker_filter() {
        let mut app = Tadpole::default();
        let voice = FakeVoice(vec![
            tseg(0, 1_000, "hello there"),
            tseg(2_000, 3_000, "and back again"), // 1s pause ≥ TURN_GAP_MS
        ]);
        app.accept(&wav_bytes(), Some("m.wav"), &voice);
        let d = app.diarised.as_ref().expect("timed audio is diarised");
        assert_eq!(d.roster().len(), 2, "the pause is a turn boundary");
        assert_eq!(d.roster()[0].label(), "Turn 1", "turns say turn, not speaker");
        assert!(
            !app.set_speaker_filter(Some(0)),
            "turn labels are alternation, not identity — the filter is refused"
        );
        assert_eq!(app.speaker_filter, None);
        assert_eq!(app.transcript(), "hello there\nand back again", "refused filter hides nothing");
        assert!(app.annotated_transcript().unwrap().starts_with("Turn 1:"));
    }

    #[test]
    fn pasted_text_is_never_diarised() {
        let mut app = Tadpole::default();
        app.accept(b"line one\nline two", None, &NoVoice);
        assert_eq!(app.segments.len(), 2);
        assert!(app.diarised.is_none(), "line-index timings are not pauses");
        assert_eq!(app.transcript(), "line one\nline two");
        assert!(app.annotated_transcript().is_none());
    }

    #[test]
    fn a_scan_clears_the_previous_transcripts_labels() {
        let mut app = Tadpole::default();
        let voice = FakeVoice(vec![tseg(0, 500, "before")]);
        app.accept(&wav_bytes(), Some("m.wav"), &voice);
        assert!(app.diarised.is_some());
        app.scan(&Fake(vec![r("abcd", 0.9, 0.0, 0.0, 40.0, 12.0)]), &img());
        assert!(app.diarised.is_none(), "a scan replaces the document, labels included");
        assert_eq!(app.speaker_filter, None);
    }

    #[test]
    fn identity_labels_filter_and_rename_through_the_app() {
        // A Model-source Diarised — the shape a diarisation backend will hand
        // over. Installed directly since no backend ships yet; the app-level
        // filter path is what this pins.
        let segs = vec![tseg(0, 1, "alpha"), tseg(10_000, 10_001, "beta"), tseg(20_000, 20_001, "gamma")];
        let sp = |id| Some(speakers::Speaker::new(id, speakers::Source::Model));
        let mut app = Tadpole::default();
        app.segments = segs.clone();
        app.diarised = Some(speakers::Diarised::new(segs, vec![sp(0), sp(1), sp(0)]));
        assert!(app.set_speaker_filter(Some(0)), "identities may filter");
        assert_eq!(app.transcript(), "alpha\ngamma", "the returning speaker keeps both lines");
        assert!(app.set_speaker_filter(None));
        assert_eq!(app.transcript(), "alpha\nbeta\ngamma");
        if let Some(d) = &mut app.diarised { d.rename(0, "Ada"); }
        assert!(app.annotated_transcript().unwrap().contains("Ada:"));
    }
}
