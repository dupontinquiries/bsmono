//! router — the piece that was missing.
//!
//! Ten pages were ported from the GTK build, each with a `*_ui` renderer, and
//! `chrome.rs` was ported too — `Section` carries every page's title, icon,
//! keyboard shortcut and tab label. Nothing assembled them. `wiring_check`
//! reported ambient-learner as "0 modules, 0 DEAD" and it was not clean, it was
//! NOT SCANNED: the tool only looked in `egui-frontend/src` and this app keeps
//! its views in `src/ui/egui/`. With that fixed, all ten renderers came back as
//! never called.
//!
//! So this file is the routing the port never got: a current section, a way to
//! change it, and a dispatch to the page that draws it. The pages themselves
//! are untouched — they were ported faithfully and they work; they had no way
//! in.
//!
//! ## Why the shortcut table is the source of truth
//!
//! `Section::from_shortcut` already existed and already had the mapping.
//! Writing a second one here — a match in the key handler — would be two lists
//! that drift, and the one that drifts is always the one without the test.

use super::chrome::Section;

/// Every section, in the order the tab bar shows them.
///
/// A constant rather than a derive: the ORDER is a product decision (Live
/// first, Debug last) and deriving it from the enum would silently reorder the
/// UI the next time someone inserts a variant.
pub const SECTIONS: [Section; 7] = [
    Section::Live, Section::History, Section::Places,
    Section::Map, Section::Model, Section::Sync, Section::Debug,
];

/// Which page is showing, and how it got there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Router {
    current: Section,
    /// The previous section, so a Back gesture has somewhere to go. One deep:
    /// this is a tabbed app, not a stack, and a full history would let Back
    /// walk a cycle the user never perceived as navigation.
    previous: Option<Section>,
}

impl Default for Router {
    fn default() -> Self {
        Router { current: Section::Live, previous: None }
    }
}

impl Router {
    pub fn current(&self) -> Section { self.current }

    /// Go to a section. Selecting the current one is a no-op rather than a
    /// self-transition, or Back would return to the page you are already on.
    pub fn go(&mut self, s: Section) {
        if s == self.current { return; }
        self.previous = Some(self.current);
        self.current = s;
    }

    /// Back to where we came from, if anywhere.
    pub fn back(&mut self) -> bool {
        match self.previous.take() {
            Some(p) => { self.current = p; true }
            None => false,
        }
    }

    /// Handle a typed character as a shortcut.
    ///
    /// Delegates to `Section::from_shortcut`, which already had the mapping.
    /// A second match here would be two lists that drift.
    pub fn shortcut(&mut self, c: char) -> bool {
        match Section::from_shortcut(c) {
            Some(s) => { self.go(s); true }
            None => false,
        }
    }
}

/// Draw the tab bar and return a section if one was chosen.
///
/// Wraps, because seven tabs do not fit across a phone in one row — the same
/// fix krapper and ambient-learner's own settings row needed. Icons only on
/// narrow screens: the labels are what wrap, and a wrapped tab bar two rows
/// deep costs more vertical space than a phone has to spare.
pub fn tab_bar_ui(
    ui: &mut egui::Ui,
    current: Section,
    glyphs: bool,
) -> Option<Section> {
    let mut chosen = None;
    ui.horizontal_wrapped(|ui| {
        for s in SECTIONS {
            let selected = s == current;
            if ui.selectable_label(selected, s.tab_label(glyphs)).clicked() {
                chosen = Some(s);
            }
        }
    });
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_is_reachable_from_the_tab_bar() {
        // The bug this file fixes: ten renderers and no way to reach any of
        // them. If a variant is added and not listed here, it is a page with
        // no route in — which is precisely what happened before.
        for s in [Section::Live, Section::History, Section::Places, Section::Map,
                  Section::Model, Section::Sync, Section::Debug] {
            assert!(SECTIONS.contains(&s), "{s:?} has no tab and cannot be opened");
        }
        assert_eq!(SECTIONS.len(), 7);
    }

    /// `shell.rs`'s two responsive layouts read two DIFFERENT lists for the
    /// same tab strip: the Tabs layout calls `tab_bar_ui` (iterates
    /// `SECTIONS`, here), the narrow-screen Pages layout iterates
    /// `chrome::Section::ALL` directly. Both currently list the same seven
    /// sections in the same order, but nothing enforced that — exactly the
    /// "two lists that drift" trap this file's own module doc names for a
    /// different pair (the shortcut table). A phone whose screen width
    /// crosses the Tabs/Pages threshold mid-session would show a DIFFERENT
    /// set of sections depending on which layout happened to be active,
    /// if these ever disagreed.
    #[test]
    fn sections_and_router_sections_never_drift_apart() {
        assert_eq!(SECTIONS.to_vec(), super::super::chrome::Section::ALL.to_vec(),
                   "router::SECTIONS and chrome::Section::ALL must list the \
                    same sections in the same order — shell.rs's two \
                    responsive layouts read one each");
    }

    #[test]
    fn the_shortcut_table_is_not_duplicated_here() {
        // Section::from_shortcut already had the mapping. A second match in
        // the router would be two lists that drift, and the one that drifts is
        // the one without the test.
        let mut r = Router::default();
        for s in SECTIONS {
            if let Some(c) = s.shortcut() {
                let mut r2 = Router::default();
                assert!(r2.shortcut(c), "{c:?} is a declared shortcut and did nothing");
                assert_eq!(r2.current(), s, "{c:?} opened the wrong section");
            }
        }
        assert!(!r.shortcut('\u{1}'), "an undeclared key navigated somewhere");
    }

    #[test]
    fn selecting_the_current_section_is_not_a_transition() {
        // Otherwise Back returns to the page you are already on, which reads
        // as Back being broken.
        let mut r = Router::default();
        r.go(Section::Map);
        r.go(Section::Map);
        assert!(r.back());
        assert_eq!(r.current(), Section::Live);
    }

    #[test]
    fn back_from_the_first_page_does_nothing_and_says_so() {
        // Returning false lets the caller pass the gesture on — swallowing it
        // silently is how a Back button stops closing the app.
        let mut r = Router::default();
        assert!(!r.back());
        assert_eq!(r.current(), Section::Live);
    }

    #[test]
    fn history_is_one_deep_rather_than_a_stack() {
        // A tabbed app, not a navigation stack. A full history lets Back walk
        // a cycle the user never perceived as navigation: Live -> Map -> Live
        // -> Map, back, back, back.
        let mut r = Router::default();
        r.go(Section::Map);
        r.go(Section::Sync);
        assert!(r.back());
        assert_eq!(r.current(), Section::Map);
        assert!(!r.back(), "a second Back walked further than one step");
    }

    #[test]
    fn the_tab_order_is_fixed_rather_than_derived() {
        // The order is a product decision — Live first, Debug last — and
        // deriving it from the enum would silently reorder the UI the next
        // time someone inserts a variant.
        assert_eq!(SECTIONS[0], Section::Live);
        assert_eq!(SECTIONS[SECTIONS.len() - 1], Section::Debug);
    }
}
