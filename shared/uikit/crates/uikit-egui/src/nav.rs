//! nav — page + subpage navigation, extracted from snapclone into the shared kit.
//!
//! A two-level model that fits mobile and desktop from one source: a set of top-level **pages**
//! (a tab/section), each of which can drill into a stack of **subpages**. On a phone you see one
//! page (or its current subpage) full-screen with a back affordance; on a desktop the same model
//! renders master–detail. localweather (Today → hourly/details subpages) and av1tuner on mobile
//! (presets → per-encode subpages) are the motivating cases.
//!
//! The [`Nav`] state machine is pure and unit-tested; [`Nav::title_bar`] is the thin egui bit.

/// Page + subpage navigation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nav {
    /// Current top-level page index.
    page: usize,
    page_count: usize,
    /// Drill-down stack *within* the current page (indices are app-defined subpage ids).
    subpages: Vec<usize>,
}

impl Nav {
    /// Create with `page_count` top-level pages, starting on page 0.
    pub fn new(page_count: usize) -> Nav {
        Nav { page: 0, page_count: page_count.max(1), subpages: Vec::new() }
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// Switch top-level page. Clears any drill-down (each page keeps its own root when
    /// re-entered; callers that want persistent per-page stacks can hold their own).
    pub fn go_page(&mut self, page: usize) {
        if page < self.page_count {
            self.page = page;
            self.subpages.clear();
        }
    }

    /// Drill into a subpage of the current page.
    pub fn push_subpage(&mut self, id: usize) {
        self.subpages.push(id);
    }

    /// The current subpage id, or `None` when at a page root.
    pub fn subpage(&self) -> Option<usize> {
        self.subpages.last().copied()
    }

    /// How deep the current drill-down is (0 = at the page root).
    pub fn depth(&self) -> usize {
        self.subpages.len()
    }

    /// Is there anywhere to go back to (a subpage to pop)?
    pub fn can_go_back(&self) -> bool {
        !self.subpages.is_empty()
    }

    /// Go back one level: pop a subpage. Returns true if it moved. (At a page root, returns
    /// false — the app decides whether that means "exit" or nothing.)
    pub fn back(&mut self) -> bool {
        self.subpages.pop().is_some()
    }

    /// The full path of subpage ids from the page root to the current view.
    pub fn path(&self) -> &[usize] {
        &self.subpages
    }

    /// Render a standard title bar: a back button when drilled in, plus `title`. Returns true
    /// if the back button was pressed this frame (equivalent to having called `back`).
    pub fn title_bar(&mut self, ui: &mut egui::Ui, title: &str) -> bool {
        let mut went_back = false;
        ui.horizontal(|ui| {
            if self.can_go_back() && ui.button("‹ back").clicked() {
                self.back();
                went_back = true;
            }
            ui.heading(title);
        });
        went_back
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_switch_and_clamp() {
        let mut nav = Nav::new(3);
        assert_eq!(nav.page(), 0);
        nav.go_page(2);
        assert_eq!(nav.page(), 2);
        nav.go_page(9); // out of range → ignored
        assert_eq!(nav.page(), 2);
    }

    #[test]
    fn subpage_drilldown_and_back() {
        let mut nav = Nav::new(2);
        assert!(!nav.can_go_back());
        assert_eq!(nav.subpage(), None);
        nav.push_subpage(10);
        nav.push_subpage(11);
        assert_eq!(nav.subpage(), Some(11));
        assert_eq!(nav.depth(), 2);
        assert_eq!(nav.path(), &[10, 11]);
        assert!(nav.can_go_back());
        assert!(nav.back());
        assert_eq!(nav.subpage(), Some(10));
        assert!(nav.back());
        assert!(!nav.can_go_back());
        assert!(!nav.back(), "back at root does nothing");
    }

    #[test]
    fn switching_page_clears_drilldown() {
        let mut nav = Nav::new(3);
        nav.push_subpage(5);
        assert_eq!(nav.depth(), 1);
        nav.go_page(1);
        assert_eq!(nav.depth(), 0, "changing top-level page resets the drill-down");
    }
}
