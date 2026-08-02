//! backlinks_view — "what links here", over the app's own reverse index.
//!
//! `src/backlinks.rs` builds the inverse of the `[[wiki-link]]` graph and says
//! its own boundary plainly: "the fs scan + `[[ ]]` extraction that feeds it
//! lives in main.rs; this module is the parsing + indexing core." So the port
//! mounts the core and supplies the surface, and does not attempt the scan.
//!
//! ## Why backlinks matter more than links
//!
//! Forward links are already visible — they are in the text you are reading.
//! Backlinks are the ones you cannot see: which notes point *at* this one is
//! exactly the information a wiki hides from you, and it is why "what links
//! here" is a feature rather than a view of data you already have.
//!
//! ## Orphans are the second half
//!
//! A note nothing links to is either the start of a thread or something you
//! wrote and lost. The index can tell them apart from everything else, and a
//! reader that only ever shows backlinks for the current note never surfaces
//! the notes with none — which are precisely the ones worth finding.

#[path = "../../src/backlinks.rs"]
#[allow(dead_code)]
pub mod backlinks;

pub use backlinks::Backlinks;

/// What the panel is listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Listing {
    /// Notes linking to the current one.
    #[default]
    Here,
    /// Notes nothing links to, across the whole set.
    Orphans,
}

// Not Clone: Backlinks is not, and an index is not a thing to copy — a second
// copy is a second thing to keep in step with the notes on disk.
#[derive(Debug, Default)]
pub struct BacklinksView {
    index: Option<Backlinks>,
    /// Every known note path, needed for the orphan query.
    all_paths: Vec<String>,
    pub current: String,
    pub listing: Listing,
    pub query: String,
}

impl BacklinksView {
    /// `notes` is (path, body), as the index expects.
    pub fn rebuild(&mut self, notes: &[(String, String)]) {
        self.all_paths = notes.iter().map(|(p, _)| p.clone()).collect();
        self.index = Some(Backlinks::build(notes));
    }

    pub fn set_current(&mut self, path: &str) { self.current = path.to_string(); }

    /// How many notes point at the current one. Shown as a badge, so a note
    /// with none is obvious without opening the panel.
    pub fn count_here(&self) -> usize {
        self.index.as_ref().map(|i| i.count_for(&self.current)).unwrap_or(0)
    }

    /// The rows, as (path, context).
    pub fn rows(&self) -> Vec<(String, String)> {
        let Some(index) = self.index.as_ref() else { return Vec::new() };
        let q = self.query.trim().to_lowercase();
        let mut v: Vec<(String, String)> = match self.listing {
            Listing::Here => index.for_note(&self.current),
            // Orphans have no linking context by definition, so the second
            // column carries why they are listed rather than an empty string —
            // a blank column reads as missing data.
            Listing::Orphans => index.orphans(&self.all_paths).into_iter()
                .map(|p| (p.clone(), "nothing links here".to_string())).collect(),
        };
        v.retain(|(p, ctx)| q.is_empty()
                 || p.to_lowercase().contains(&q) || ctx.to_lowercase().contains(&q));
        // Stable by path: the panel sits beside an editor and a list that
        // reorders as you type is one you cannot click into.
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// What to say when the list is empty — three different situations that a
    /// single "no results" would collapse.
    pub fn empty_reason(&self) -> Option<&'static str> {
        if !self.rows().is_empty() { return None; }
        if self.index.is_none() { return Some("notes not indexed yet"); }
        if !self.query.trim().is_empty() { return Some("nothing matches"); }
        match self.listing {
            Listing::Here => Some("nothing links to this note"),
            Listing::Orphans => Some("every note is linked from somewhere"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> Vec<(String, String)> {
        vec![
            ("hub.md".into(), "see [[alpha]] and [[beta]]".into()),
            ("alpha.md".into(), "back to [[hub]]".into()),
            ("beta.md".into(), "also [[hub]]".into()),
            ("lost.md".into(), "nothing points here".into()),
        ]
    }

    fn view() -> BacklinksView {
        let mut v = BacklinksView::default();
        v.rebuild(&notes());
        v
    }

    #[test]
    fn a_note_lists_what_points_at_it_not_what_it_points_to() {
        // Forward links are already visible in the text you are reading;
        // backlinks are the information a wiki hides.
        let mut v = view();
        v.set_current("hub.md");
        let paths: Vec<String> = v.rows().into_iter().map(|(p, _)| p).collect();
        assert!(paths.iter().any(|p| p.contains("alpha")), "{paths:?}");
        assert!(paths.iter().any(|p| p.contains("beta")), "{paths:?}");
        assert_eq!(v.count_here(), 2);
    }

    #[test]
    fn orphans_are_findable_because_nothing_else_will_surface_them() {
        // A reader that only shows backlinks for the CURRENT note never
        // surfaces the notes with none — precisely the ones worth finding.
        let mut v = view();
        v.listing = Listing::Orphans;
        let paths: Vec<String> = v.rows().into_iter().map(|(p, _)| p).collect();
        assert!(paths.iter().any(|p| p.contains("lost")), "{paths:?}");
    }

    #[test]
    fn an_orphan_row_says_why_it_is_listed() {
        // An orphan has no linking context by definition, and a blank second
        // column reads as missing data rather than as absent data.
        let mut v = view();
        v.listing = Listing::Orphans;
        assert!(v.rows().iter().all(|(_, ctx)| !ctx.trim().is_empty()));
    }

    #[test]
    fn the_empty_states_are_distinguishable() {
        // Four situations a single "no results" would collapse.
        let mut v = BacklinksView::default();
        assert_eq!(v.empty_reason(), Some("notes not indexed yet"));
        v.rebuild(&notes());
        v.set_current("lost.md");
        assert_eq!(v.empty_reason(), Some("nothing links to this note"));
        v.query = "zzzz".into();
        assert_eq!(v.empty_reason(), Some("nothing matches"));
        v.query.clear();
        v.set_current("hub.md");
        assert_eq!(v.empty_reason(), None);
    }

    #[test]
    fn the_list_does_not_reorder_as_you_type() {
        // The panel sits beside an editor, and a list that reshuffles is one you
        // cannot click into.
        let mut v = view();
        v.set_current("hub.md");
        let a: Vec<String> = v.rows().into_iter().map(|(p, _)| p).collect();
        let b: Vec<String> = v.rows().into_iter().map(|(p, _)| p).collect();
        assert_eq!(a, b);
        let mut sorted = a.clone();
        sorted.sort();
        assert_eq!(a, sorted, "stable by path");
    }

    #[test]
    fn rebuilding_replaces_rather_than_accumulates() {
        // A rescan must not double every backlink, which is what appending to
        // the index would do.
        let mut v = view();
        v.set_current("hub.md");
        let before = v.count_here();
        v.rebuild(&notes());
        assert_eq!(v.count_here(), before);
    }

    #[test]
    fn a_note_with_no_index_reports_zero_rather_than_panicking() {
        let v = BacklinksView::default();
        assert_eq!(v.count_here(), 0);
        assert!(v.rows().is_empty());
    }
}
