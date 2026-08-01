//! bars — the bottom bar and the archived-page browser, drawn.
//!
//! Both models are Qt-free (`src/bottom_bar.rs`, `src/archive.rs` have zero
//! qmetaobject references), so the port mounts them by `#[path]` and adds only
//! what a frontend owes: the fan-out gesture and the search surface.
//!
//! ## The fan-out, which is what makes the bar work on a phone
//!
//! A bar button is not just a button — each one carries a list (`Tabs` fans out
//! to the tab list, `Workspaces` to workspaces). Pressing and holding fans the
//! list out above the bar. That gives a phone-sized surface access to four lists
//! without four screens, and it is why the bar is worth porting rather than
//! replacing with a row of tabs.
//!
//! Two rules the gesture needs, and both come from what goes wrong without them:
//!
//! * **A tap is not a hold.** A tap activates the button's primary action; only
//!   a hold fans out. Firing the fan-out on tap makes the primary action
//!   unreachable.
//! * **Releasing outside the fan cancels.** Sliding off the list and letting go
//!   must select nothing, because the alternative is picking whatever your thumb
//!   happened to be over when you gave up.

#[path = "../../src/bottom_bar.rs"]
#[allow(dead_code)]
pub mod bottom_bar;

// archive.rs is NOT mounted, unlike bottom_bar.rs. It reaches for
// `crate::layout_cache` and `crate::util` at the crate root, so mounting it
// drags two more modules and the scraper HTML stack into a widget crate — the
// same chain that stopped mail_proton being mounted. The archived-page browser
// needs a view over a page LIST, and that list is three fields; the engine keeps
// the parsing.
pub use bottom_bar::{BarButton, BarButtonKind, BarOption, BottomBar};

/// How long a press must last to fan out, in seconds.
///
/// Long enough that a tap is unambiguous, short enough not to feel stuck. Below
/// about 0.2s a normal tap starts triggering it; above about 0.6s the bar feels
/// broken.
pub const HOLD_SECONDS: f32 = 0.32;

/// The bar's live gesture state.
#[derive(Debug, Clone, Default)]
pub struct BarState {
    /// Which button is being pressed, and for how long.
    pressing: Option<(usize, f32)>,
    /// Which button is fanned out, if any.
    pub open: Option<usize>,
    /// Highlighted option within the open fan.
    pub hover: Option<usize>,
}

/// What a gesture produced.
#[derive(Debug, Clone, PartialEq)]
pub enum BarEvent {
    /// A tap: run the button's primary action.
    Primary(usize),
    /// An option was chosen from a fan-out.
    Option { button: usize, option: usize },
    /// Fanned out and released with nothing selected.
    Cancelled,
}

impl BarState {
    pub fn press(&mut self, button: usize) {
        self.pressing = Some((button, 0.0));
        self.hover = None;
    }

    /// Advance the press timer. Returns true when the fan-out just opened, so
    /// the caller can play the haptic exactly once rather than every frame.
    pub fn tick(&mut self, dt: f32) -> bool {
        let Some((b, ref mut t)) = self.pressing else { return false };
        *t += dt.max(0.0);
        if *t >= HOLD_SECONDS && self.open != Some(b) {
            self.open = Some(b);
            return true;
        }
        false
    }

    pub fn is_open(&self) -> bool { self.open.is_some() }

    /// Move the highlight while fanned out. Ignored when nothing is open, so a
    /// stray drag across the bar cannot pre-select an option.
    pub fn hover_option(&mut self, option: Option<usize>) {
        if self.open.is_some() { self.hover = option; }
    }

    /// Release. A tap runs the primary; a hold picks the hovered option, or
    /// cancels if the finger left the fan.
    pub fn release(&mut self) -> Option<BarEvent> {
        let (button, held) = self.pressing.take()?;
        let was_open = self.open.take();
        let hover = self.hover.take();
        if was_open.is_some() {
            return Some(match hover {
                Some(o) => BarEvent::Option { button, option: o },
                // Sliding off the list and letting go must select nothing —
                // otherwise you pick whatever your thumb was over when you gave
                // up.
                None => BarEvent::Cancelled,
            });
        }
        // a press that never reached the hold threshold is a tap
        if held < HOLD_SECONDS { Some(BarEvent::Primary(button)) } else { Some(BarEvent::Cancelled) }
    }

    /// Abandon without emitting — for a gesture interrupted by something else,
    /// like the keyboard appearing.
    pub fn abort(&mut self) { self.pressing = None; self.open = None; self.hover = None; }
}

/// One archived page, as the browser needs it. The engine's `ArchivedPage`
/// carries the stored HTML and its extracted text; a list does not.
#[derive(Debug, Clone, PartialEq)]
pub struct PageEntry {
    pub url: String,
    pub title: String,
    /// Seconds since the epoch; only ordering matters here.
    pub at: i64,
}

impl PageEntry {
    /// What to show. A page with no title shows its URL rather than an empty
    /// row, since a blank entry reads as a broken list.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() { &self.url } else { &self.title }
    }
}

/// The archived-page browser's view state.
#[derive(Debug, Clone, Default)]
pub struct ArchiveView {
    pub query: String,
    pub selected: Option<String>,
}

impl ArchiveView {
    /// Pages matching the query, newest first.
    ///
    /// Matched over title AND url, case-insensitively: people remember a domain
    /// as often as a headline, and a search that only reads titles fails on
    /// exactly the pages whose titles were useless.
    pub fn results<'a>(&self, pages: &'a [PageEntry]) -> Vec<&'a PageEntry> {
        let q = self.query.trim().to_lowercase();
        let mut v: Vec<&PageEntry> = pages.iter()
            .filter(|p| q.is_empty() || p.title.to_lowercase().contains(&q)
                        || p.url.to_lowercase().contains(&q))
            .collect();
        // newest first, ties on url so the list cannot shuffle between frames
        v.sort_by(|a, b| b.at.cmp(&a.at).then(a.url.cmp(&b.url)));
        v
    }

    /// Filtering must not strand a selection that is no longer listed.
    pub fn reconcile(&mut self, pages: &[PageEntry]) {
        if let Some(url) = self.selected.clone() {
            if !self.results(pages).iter().any(|p| p.url == url) { self.selected = None; }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> BottomBar {
        let mut b = BottomBar::new();
        // upsert, not a field push:  is private on purpose, and upsert
        // is what keeps a button unique by id when the bar is rebuilt.
        b.upsert(BarButton::new("tabs", "Tabs", BarButtonKind::Tabs)
            .with_options(vec![BarOption::new("t1", "shell"), BarOption::new("t2", "docs")]));
        b.upsert(BarButton::new("ws", "Spaces", BarButtonKind::Workspaces));
        b
    }

    #[test]
    fn a_tap_runs_the_primary_and_does_not_fan_out() {
        // Firing the fan-out on tap makes the primary action unreachable.
        let mut s = BarState::default();
        s.press(0);
        assert!(!s.tick(HOLD_SECONDS * 0.4));
        assert!(!s.is_open());
        assert_eq!(s.release(), Some(BarEvent::Primary(0)));
    }

    #[test]
    fn a_hold_fans_out_and_reports_the_moment_it_opens() {
        // The caller plays a haptic exactly once, not every frame.
        let mut s = BarState::default();
        s.press(0);
        assert!(!s.tick(HOLD_SECONDS * 0.5));
        assert!(s.tick(HOLD_SECONDS * 0.6), "opened on this tick");
        assert!(!s.tick(0.1), "and does not report again");
        assert!(s.is_open());
    }

    #[test]
    fn releasing_outside_the_fan_selects_nothing() {
        // Otherwise you pick whatever your thumb was over when you gave up.
        let mut s = BarState::default();
        s.press(0);
        s.tick(HOLD_SECONDS + 0.01);
        s.hover_option(Some(1));
        s.hover_option(None);          // slid off the list
        assert_eq!(s.release(), Some(BarEvent::Cancelled));
    }

    #[test]
    fn choosing_an_option_reports_both_the_button_and_the_option() {
        let mut s = BarState::default();
        s.press(0);
        s.tick(HOLD_SECONDS + 0.01);
        s.hover_option(Some(1));
        assert_eq!(s.release(), Some(BarEvent::Option { button: 0, option: 1 }));
        assert!(!s.is_open(), "and the fan closes");
    }

    #[test]
    fn a_drag_across_the_bar_cannot_preselect_before_it_opens() {
        // Hovering only counts once the fan is out.
        let mut s = BarState::default();
        s.press(0);
        s.hover_option(Some(1));
        assert_eq!(s.hover, None, "ignored while closed");
        assert_eq!(s.release(), Some(BarEvent::Primary(0)));
    }

    #[test]
    fn a_release_without_a_press_emits_nothing() {
        let mut s = BarState::default();
        assert_eq!(s.release(), None);
    }

    #[test]
    fn an_interrupted_gesture_emits_nothing() {
        // The keyboard appearing mid-press must not send a command.
        let mut s = BarState::default();
        s.press(1);
        s.tick(HOLD_SECONDS + 0.1);
        s.abort();
        assert_eq!(s.release(), None);
        assert!(!s.is_open());
    }

    #[test]
    fn the_bar_model_is_the_apps_own() {
        // Mounted by #[path], so a button kind added to the engine reaches both
        // frontends rather than drifting.
        let b = bar();
        assert_eq!(b.len(), 2);
        assert!(!b.is_empty());
        assert_eq!(b.buttons()[0].kind, BarButtonKind::Tabs);
        assert_eq!(b.options_for("tabs").len(), 2);
        // upsert REPLACES by id rather than appending a duplicate, which is what
        // makes rebuilding the bar every frame safe
        let mut c = bar();
        c.upsert(BarButton::new("tabs", "Tabs", BarButtonKind::Tabs));
        assert_eq!(c.len(), 2);
    }

    fn pages() -> Vec<PageEntry> {
        vec![
            PageEntry { url: "https://rust-lang.org/book".into(), title: "The Book".into(), at: 300 },
            PageEntry { url: "https://example.test/a".into(), title: String::new(), at: 200 },
            PageEntry { url: "https://news.test/x".into(), title: "Headline".into(), at: 100 },
        ]
    }

    #[test]
    fn a_page_with_no_title_shows_its_url() {
        assert_eq!(pages()[1].label(), "https://example.test/a");
        assert_eq!(pages()[0].label(), "The Book");
    }

    #[test]
    fn search_reads_urls_as_well_as_titles() {
        // People remember a domain as often as a headline, and a title-only
        // search fails on exactly the pages whose titles were useless.
        let p = pages();
        let mut v = ArchiveView::default();
        v.query = "rust-lang".into();
        assert_eq!(v.results(&p).len(), 1);
        v.query = "HEADLINE".into();
        assert_eq!(v.results(&p).len(), 1, "case-insensitive");
        v.query = "  ".into();
        assert_eq!(v.results(&p).len(), 3);
    }

    #[test]
    fn the_list_is_newest_first_and_does_not_shuffle() {
        let p = pages();
        let v = ArchiveView::default();
        let a: Vec<&str> = v.results(&p).iter().map(|x| x.url.as_str()).collect();
        let b: Vec<&str> = v.results(&p).iter().map(|x| x.url.as_str()).collect();
        assert_eq!(a, b);
        assert_eq!(a[0], "https://rust-lang.org/book", "newest first");
    }

    #[test]
    fn a_selection_that_leaves_the_results_is_cleared() {
        // Otherwise the reader shows a page the list does not contain.
        let p = pages();
        let mut v = ArchiveView::default();
        v.selected = Some("https://news.test/x".into());
        v.reconcile(&p);
        assert_eq!(v.selected.as_deref(), Some("https://news.test/x"));
        v.query = "rust-lang".into();
        v.reconcile(&p);
        assert!(v.selected.is_none(), "filtered out, so the selection is dropped");
    }
}
