//! panes — aperture's split layout, over `shared/pane-kit`.
//!
//! DESIGN.md lists tabs/splits as a v0 non-goal "with a seam waiting for
//! it". The tree itself came from voiceterm (see `shared/pane-kit`);
//! this module is the part that is aperture's: what a pane holds, and
//! how panes relate to open documents.
//!
//! ## The decision this module turns on
//!
//! **Document state is shared; view state is per-pane.**
//!
//! Two panes showing one file must show the same TEXT — a split where
//! the halves disagree about the contents is worse than no split, and
//! the whole reason to open one is to look at two parts of the same
//! thing. But they must keep independent CURSORS and SCROLL, or the
//! split does nothing you could not do by scrolling.
//!
//! So a pane holds a [`DocId`] plus its own [`View`], and the documents
//! themselves live in one map keyed by id. Editing through either pane
//! edits the one document; both panes' views survive it.
//!
//! That split is also what makes multi-user tractable. A collaborator's
//! caret belongs to a DOCUMENT, not to a pane, so it renders in every
//! pane currently showing that document —
//! [`PaneWorkspace::panes_showing`] is the query, and it costs nothing
//! extra because the model already had to know.
//!
//! ## Why the documents are not in here
//!
//! This module stores `DocId`, not `Editor`. Keeping the layout free of
//! the rope means the whole pane/document relationship is testable
//! without loading a file, and it keeps the ownership honest: the shell
//! owns the editors, this owns which pane is looking at which.
//!
//! Pure: no rope, no egui, no I/O.

use pane_kit::{Dir, Tab, Workspace};
use std::collections::{BTreeMap, BTreeSet};

/// A document open in the session. Stable for as long as the document
/// is open; the shell keys its `Editor` on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct DocId(pub u64);

/// What one pane is looking at, and how.
///
/// `Default` is the empty pane, which is what a fresh split produces —
/// splitting should not silently duplicate the document, because the
/// usual reason to split is to put something ELSE beside it. Callers
/// that want a duplicate call [`PaneWorkspace::split_showing_same`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    pub doc: Option<DocId>,
    /// First visible line. Per-pane, so two views of one file scroll
    /// independently.
    pub top_line: usize,
    /// Selection as (anchor, head) char indices, per-pane for the same
    /// reason.
    pub sel: (usize, usize),
}

impl View {
    pub fn showing(doc: DocId) -> Self {
        View { doc: Some(doc), ..Default::default() }
    }
    pub fn caret(&self) -> usize {
        self.sel.1
    }
}

/// Tabs of panes, each looking at a document.
#[derive(Debug, Clone, Default)]
pub struct PaneWorkspace {
    panes: Workspace<View>,
    next_doc: u64,
}

impl PaneWorkspace {
    pub fn new() -> Self {
        PaneWorkspace { panes: Workspace::new(), next_doc: 1 }
    }

    pub fn layout(&self) -> &Workspace<View> {
        &self.panes
    }
    pub fn layout_mut(&mut self) -> &mut Workspace<View> {
        &mut self.panes
    }
    pub fn focused_pane(&self) -> u64 {
        self.panes.focused_pane()
    }

    /// Mint a document id. The shell creates the `Editor`.
    pub fn new_doc(&mut self) -> DocId {
        let d = DocId(self.next_doc);
        self.next_doc += 1;
        d
    }

    /// Show `doc` in the focused pane, resetting that pane's view.
    ///
    /// Resetting is right: the scroll and selection belonged to the
    /// document that was there, and carrying them over lands the caret
    /// at an arbitrary offset in a different file.
    pub fn open_in_focused(&mut self, doc: DocId) {
        let id = self.panes.focused_pane();
        self.panes.set_payload(id, View::showing(doc));
    }

    /// Split the focused pane and show `doc` in the new one.
    pub fn split_open(&mut self, dir: Dir, doc: DocId) -> u64 {
        self.panes.split_with(dir, View::showing(doc))
    }

    /// Split, showing the SAME document in both, with the new pane
    /// inheriting the current scroll and selection.
    ///
    /// Inheriting is right here and wrong in `open_in_focused`, for the
    /// same reason: it is the same document, so the position means the
    /// same thing. This is the "look at two parts of one file" gesture,
    /// and starting the new pane at the top of the file would throw away
    /// exactly the context that motivated it.
    pub fn split_showing_same(&mut self, dir: Dir) -> u64 {
        let current = self.focused_view().cloned().unwrap_or_default();
        self.panes.split_with(dir, current)
    }

    pub fn focused_view(&self) -> Option<&View> {
        self.panes.payload(self.panes.focused_pane())
    }
    pub fn focused_view_mut(&mut self) -> Option<&mut View> {
        let id = self.panes.focused_pane();
        self.panes.payload_mut(id)
    }
    pub fn view(&self, pane: u64) -> Option<&View> {
        self.panes.payload(pane)
    }
    pub fn view_mut(&mut self, pane: u64) -> Option<&mut View> {
        self.panes.payload_mut(pane)
    }

    /// The document the focused pane is showing, if any.
    pub fn focused_doc(&self) -> Option<DocId> {
        self.focused_view().and_then(|v| v.doc)
    }

    /// Every pane showing `doc`, across all tabs.
    ///
    /// The query the renderer needs for a collaborator's caret: a remote
    /// caret belongs to a DOCUMENT, so it draws in every pane looking at
    /// that document, including ones in tabs you cannot see.
    pub fn panes_showing(&self, doc: DocId) -> Vec<u64> {
        self.panes
            .tabs
            .iter()
            .flat_map(|t| t.root.panes())
            .filter(|(_, v)| v.doc == Some(doc))
            .map(|(id, _)| id)
            .collect()
    }

    /// Panes in the ACTIVE tab showing `doc` — the ones actually on
    /// screen, for a renderer that only draws what is visible.
    pub fn visible_panes_showing(&self, doc: DocId) -> Vec<u64> {
        self.panes
            .active()
            .root
            .panes()
            .into_iter()
            .filter(|(_, v)| v.doc == Some(doc))
            .map(|(id, _)| id)
            .collect()
    }

    /// Every document open in some pane. What the shell keeps `Editor`s
    /// for; anything else can be dropped.
    pub fn open_docs(&self) -> BTreeSet<DocId> {
        self.panes
            .tabs
            .iter()
            .flat_map(|t| t.root.panes())
            .filter_map(|(_, v)| v.doc)
            .collect()
    }

    /// Stop showing `doc` anywhere, emptying the panes that had it.
    /// Returns those pane ids.
    ///
    /// Empties rather than closes: closing panes out from under someone
    /// because a file was closed elsewhere rearranges a layout they
    /// arranged deliberately.
    pub fn close_doc(&mut self, doc: DocId) -> Vec<u64> {
        let hit = self.panes_showing(doc);
        for id in &hit {
            self.panes.set_payload(*id, View::default());
        }
        hit
    }

    /// Move focus to a pane showing `doc`, preferring one in the active
    /// tab. True if one was found.
    pub fn focus_doc(&mut self, doc: DocId) -> bool {
        if let Some(&id) = self.visible_panes_showing(doc).first() {
            return self.panes.focus_id(id);
        }
        match self.panes_showing(doc).first() {
            Some(&id) => self.panes.focus_id(id),
            None => false,
        }
    }

    /// Set the focused pane's selection. Per-pane, so the other view of
    /// the same document keeps its own.
    pub fn set_selection(&mut self, anchor: usize, head: usize) {
        if let Some(v) = self.focused_view_mut() {
            v.sel = (anchor, head);
        }
    }

    /// Clamp every view of `doc` to `len` after an edit shortened it.
    ///
    /// Necessary because a remote edit — or an edit through the OTHER
    /// pane — can delete text a view's selection was pointing past, and
    /// a selection past the end of the rope panics on the next slice.
    pub fn clamp_views(&mut self, doc: DocId, len: usize) {
        for id in self.panes_showing(doc) {
            if let Some(v) = self.panes.payload_mut(id) {
                v.sel = (v.sel.0.min(len), v.sel.1.min(len));
            }
        }
    }

    /// Shift every OTHER pane's view of `doc` by an edit that happened
    /// at `at`, so a second view of the same file does not drift when
    /// the first one is typed in.
    ///
    /// `delta` is positive for an insert, negative for a delete. The
    /// pane that made the edit is excluded — its own editor already
    /// moved its caret, and applying the shift twice would double it.
    pub fn shift_other_views(&mut self, doc: DocId, except: u64, at: usize, delta: isize) {
        for id in self.panes_showing(doc) {
            if id == except {
                continue;
            }
            if let Some(v) = self.panes.payload_mut(id) {
                v.sel = (shift(v.sel.0, at, delta), shift(v.sel.1, at, delta));
            }
        }
    }

    /// Snapshot for session persistence, with the caller supplying the
    /// path for each document — this module holds ids, not filenames.
    pub fn snapshot(&self, path_of: impl Fn(DocId) -> Option<String>) -> String {
        self.panes.snapshot_with(|v| match v.doc.and_then(&path_of) {
            Some(p) => format!("{}:{}:{}:{}", p, v.top_line, v.sel.0, v.sel.1),
            None => String::new(),
        })
    }

    /// Inverse of [`snapshot`], with the caller resolving paths back to
    /// ids (it reopens the files).
    ///
    /// A path that no longer resolves leaves an EMPTY pane rather than
    /// failing the whole restore: a deleted file should cost you one
    /// pane's contents, not your entire layout.
    pub fn restore(text: &str, mut id_of: impl FnMut(&str) -> Option<DocId>) -> Option<Self> {
        let mut max_doc = 0u64;
        let panes = Workspace::restore_with(text, |s| {
            if s.is_empty() {
                return Some(View::default());
            }
            let mut parts = s.rsplitn(4, ':');
            let head: usize = parts.next()?.parse().ok()?;
            let anchor: usize = parts.next()?.parse().ok()?;
            let top: usize = parts.next()?.parse().ok()?;
            let path = parts.next()?;
            match id_of(path) {
                Some(d) => {
                    max_doc = max_doc.max(d.0);
                    Some(View { doc: Some(d), top_line: top, sel: (anchor, head) })
                }
                None => Some(View::default()),
            }
        })?;
        Some(PaneWorkspace { panes, next_doc: max_doc + 1 })
    }
}

fn shift(pos: usize, at: usize, delta: isize) -> usize {
    if pos < at {
        return pos;
    }
    if delta >= 0 {
        pos + delta as usize
    } else {
        // A delete that spans the position collapses it to the edit
        // point rather than underflowing.
        pos.saturating_sub(delta.unsigned_abs()).max(at)
    }
}

/// Where a collaborator's caret should be drawn: every pane showing the
/// document it belongs to, paired with the index it resolved to.
///
/// Takes the resolved index rather than an `aperture_collab::Resolved`
/// so aperture-core keeps no dependency on the collab crate — the shell
/// resolves and calls this.
pub fn caret_targets(
    ws: &PaneWorkspace,
    doc: DocId,
    carets: &[(u64, usize, usize)],
) -> BTreeMap<u64, Vec<(u64, usize, usize)>> {
    let mut out = BTreeMap::new();
    for pane in ws.visible_panes_showing(doc) {
        out.insert(pane, carets.to_vec());
    }
    out
}

/// The tab a fresh workspace starts with, for a caller building one by
/// hand.
pub fn empty_tab() -> Tab<View> {
    Tab {
        name: "1".into(),
        root: pane_kit::Node::Pane { id: 0, payload: View::default() },
        focused_pane: 0,
        pinned: false,
        last_used: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> (PaneWorkspace, DocId) {
        let mut w = PaneWorkspace::new();
        let d = w.new_doc();
        w.open_in_focused(d);
        (w, d)
    }

    #[test]
    fn a_fresh_split_is_empty_not_a_duplicate() {
        // splitting usually means "put something else here"
        let (mut w, d) = ws();
        let d2 = w.new_doc();
        let new = w.split_open(Dir::Right, d2);
        assert_ne!(w.view(new).unwrap().doc, Some(d));
        let blank = w.layout_mut().split(Dir::Down);
        assert_eq!(w.view(blank).unwrap().doc, None);
    }

    #[test]
    fn split_showing_same_carries_the_position_over() {
        // the "two parts of one file" gesture: starting at the top would
        // throw away the context that motivated the split
        let (mut w, d) = ws();
        w.focused_view_mut().unwrap().top_line = 420;
        w.set_selection(1000, 1000);
        let new = w.split_showing_same(Dir::Down);
        let v = w.view(new).unwrap();
        assert_eq!(v.doc, Some(d));
        assert_eq!(v.top_line, 420);
        assert_eq!(v.sel, (1000, 1000));
    }

    #[test]
    fn two_panes_on_one_document_keep_separate_views() {
        let (mut w, d) = ws();
        let first = w.focused_pane();
        let second = w.split_showing_same(Dir::Right);
        w.set_selection(50, 50); // focus is on the new pane
        w.layout_mut().focus_id(first);
        w.set_selection(5, 5);
        assert_eq!(w.view(first).unwrap().sel, (5, 5));
        assert_eq!(w.view(second).unwrap().sel, (50, 50));
        // ...but both are looking at the same document
        assert_eq!(w.view(first).unwrap().doc, w.view(second).unwrap().doc);
        assert_eq!(w.panes_showing(d).len(), 2);
    }

    #[test]
    fn opening_a_different_document_resets_the_view() {
        // carrying scroll and selection over lands the caret at an
        // arbitrary offset in an unrelated file
        let (mut w, _) = ws();
        w.focused_view_mut().unwrap().top_line = 300;
        w.set_selection(900, 950);
        let other = w.new_doc();
        w.open_in_focused(other);
        let v = w.focused_view().unwrap();
        assert_eq!(v.doc, Some(other));
        assert_eq!(v.top_line, 0);
        assert_eq!(v.sel, (0, 0));
    }

    #[test]
    fn panes_showing_spans_tabs_but_visible_does_not() {
        let (mut w, d) = ws();
        w.layout_mut().new_tab();
        w.open_in_focused(d); // same doc, second tab
        assert_eq!(w.panes_showing(d).len(), 2);
        assert_eq!(w.visible_panes_showing(d).len(), 1, "only the active tab is on screen");
    }

    #[test]
    fn open_docs_is_what_the_shell_must_keep_editors_for() {
        let (mut w, d) = ws();
        let other = w.new_doc();
        w.split_open(Dir::Right, other);
        assert_eq!(w.open_docs(), [d, other].into_iter().collect());
        w.close_doc(other);
        assert_eq!(w.open_docs(), [d].into_iter().collect());
    }

    #[test]
    fn closing_a_document_empties_panes_rather_than_removing_them() {
        // rearranging a layout someone arranged deliberately, because a
        // file was closed elsewhere, is a worse surprise than a blank
        let (mut w, d) = ws();
        w.split_showing_same(Dir::Right);
        let before = w.layout().total_panes();
        let emptied = w.close_doc(d);
        assert_eq!(emptied.len(), 2);
        assert_eq!(w.layout().total_panes(), before);
        assert!(w.open_docs().is_empty());
    }

    #[test]
    fn focus_doc_prefers_the_visible_pane() {
        let (mut w, d) = ws();
        let home = w.focused_pane();
        w.layout_mut().new_tab();
        w.open_in_focused(d);
        // from tab 1, a pane in tab 1 wins over the one in tab 0
        let here = w.focused_pane();
        assert!(w.focus_doc(d));
        assert_eq!(w.focused_pane(), here);
        // and from a tab with no view of it, we cross over
        w.layout_mut().new_tab();
        assert!(w.focus_doc(d));
        assert!(w.focused_pane() == here || w.focused_pane() == home);
        assert!(!w.focus_doc(DocId(999)));
    }

    #[test]
    fn clamp_stops_a_selection_pointing_past_a_shortened_document() {
        // a remote edit, or an edit through the other pane, can delete
        // text this view was pointing at; slicing past the rope panics
        let (mut w, d) = ws();
        let other = w.split_showing_same(Dir::Right);
        w.set_selection(500, 600);
        w.clamp_views(d, 100);
        assert_eq!(w.view(other).unwrap().sel, (100, 100));
        assert!(w.view(w.focused_pane()).unwrap().sel.1 <= 100);
    }

    #[test]
    fn an_edit_shifts_the_other_view_but_not_its_own() {
        let (mut w, d) = ws();
        let first = w.focused_pane();
        let second = w.split_showing_same(Dir::Right);
        w.layout_mut().focus_id(first);
        w.set_selection(10, 10);
        w.layout_mut().focus_id(second);
        w.set_selection(20, 20);

        // pane `first` inserts 3 chars at index 0
        w.shift_other_views(d, first, 0, 3);
        assert_eq!(w.view(first).unwrap().sel, (10, 10), "its own editor already moved it");
        assert_eq!(w.view(second).unwrap().sel, (23, 23));
    }

    #[test]
    fn a_delete_before_a_view_pulls_it_back_without_underflow() {
        let (mut w, d) = ws();
        let first = w.focused_pane();
        let second = w.split_showing_same(Dir::Right);
        w.layout_mut().focus_id(second);
        w.set_selection(5, 5);
        // delete 100 chars at 0 — far more than sits before the caret
        w.shift_other_views(d, first, 0, -100);
        assert_eq!(w.view(second).unwrap().sel, (0, 0), "collapses to the edit point");
    }

    #[test]
    fn an_edit_after_a_view_leaves_it_alone() {
        let (mut w, d) = ws();
        let first = w.focused_pane();
        let second = w.split_showing_same(Dir::Right);
        w.layout_mut().focus_id(second);
        w.set_selection(5, 5);
        w.shift_other_views(d, first, 50, 10);
        assert_eq!(w.view(second).unwrap().sel, (5, 5));
    }

    #[test]
    fn caret_targets_hits_every_visible_pane_on_the_document() {
        // a collaborator's caret belongs to a document, so it draws in
        // both halves of a split on that document
        let (mut w, d) = ws();
        w.split_showing_same(Dir::Right);
        let carets = vec![(7u64, 3usize, 9usize)];
        let t = caret_targets(&w, d, &carets);
        assert_eq!(t.len(), 2);
        assert!(t.values().all(|v| v == &carets));
        assert!(caret_targets(&w, DocId(999), &carets).is_empty());
    }

    #[test]
    fn snapshot_round_trips_paths_and_positions() {
        let (mut w, d) = ws();
        w.focused_view_mut().unwrap().top_line = 12;
        w.set_selection(3, 8);
        let other = w.new_doc();
        w.split_open(Dir::Right, other);
        w.layout_mut().rename_tab("work");

        let paths = |id: DocId| Some(format!("src/f{}.rs", id.0));
        let text = w.snapshot(paths);
        let back = PaneWorkspace::restore(&text, |p| {
            p.strip_prefix("src/f")
                .and_then(|r| r.strip_suffix(".rs"))
                .and_then(|n| n.parse().ok())
                .map(DocId)
        })
        .expect("restores");

        assert_eq!(back.layout().tabs[0].name, "work");
        assert_eq!(back.layout().total_panes(), 2);
        let v = back.view(0).unwrap();
        assert_eq!(v.doc, Some(d));
        assert_eq!(v.top_line, 12);
        assert_eq!(v.sel, (3, 8));
    }

    #[test]
    fn a_path_that_no_longer_resolves_costs_one_pane_not_the_layout() {
        let (mut w, _) = ws();
        let d2 = w.new_doc();
        w.split_open(Dir::Right, d2);
        let text = w.snapshot(|id| Some(format!("gone{}.rs", id.0)));
        let back = PaneWorkspace::restore(&text, |_| None).expect("still restores");
        assert_eq!(back.layout().total_panes(), 2, "the layout survived");
        assert!(back.open_docs().is_empty(), "the panes are empty");
    }

    #[test]
    fn restored_doc_ids_do_not_collide_with_new_ones() {
        let (mut w, _) = ws();
        let d2 = w.new_doc();
        w.split_open(Dir::Right, d2);
        let text = w.snapshot(|id| Some(format!("f{}", id.0)));
        let mut back = PaneWorkspace::restore(&text, |p| {
            p.strip_prefix('f').and_then(|n| n.parse().ok()).map(DocId)
        })
        .unwrap();
        let fresh = back.new_doc();
        assert!(!back.open_docs().contains(&fresh));
    }

    #[test]
    fn a_path_containing_a_colon_still_round_trips() {
        // the payload encoding splits from the RIGHT for exactly this
        let mut w = PaneWorkspace::new();
        let d = w.new_doc();
        w.open_in_focused(d);
        w.set_selection(1, 2);
        let text = w.snapshot(|_| Some("C:/proj/main.rs".into()));
        let back = PaneWorkspace::restore(&text, |p| {
            assert_eq!(p, "C:/proj/main.rs");
            Some(DocId(1))
        })
        .unwrap();
        assert_eq!(back.view(0).unwrap().sel, (1, 2));
    }
}
