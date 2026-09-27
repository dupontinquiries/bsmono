//! nav — where you came from, as edges rather than places.
//!
//! F12 jumps to a definition and there is no way back. That is the gap this
//! fills, and the shape of the fix is the whole point of the module.
//!
//! ## A location stack is the obvious answer and it is not enough
//!
//! `Vec<(path, line)>` gets you back. What it cannot tell you is HOW you got
//! there, and that turns out to be the question you actually have. You are
//! three jumps deep in a call chain and the thing you want to know is not
//! "what file was I in" but "which call brought me here" — because the
//! callsite is the context, and there may be forty of them.
//!
//! So a [`NavStep`] records the EDGE traversed: from, relation, the
//! [`Site`](crate::entity::Site) it was observed at, to. Two different routes
//! that land on the same function are two different steps, and going back
//! from either returns you to the right place. A location stack cannot
//! represent that difference at all — it has already thrown away the only
//! field that distinguishes them.
//!
//! ## Forward exists because back is not undo
//!
//! Going back and then forward again is ordinary reading behaviour ("wait,
//! what was that"), so the path keeps a cursor rather than popping. Taking a
//! NEW step from a rewound position truncates the forward tail, exactly like
//! a browser and for the same reason: the tail described a future that no
//! longer follows from where you are.
//!
//! ## Not undo, and not history
//!
//! [`crate::history`] is for edits and this is for attention. They look
//! similar and must not be merged: undoing an edit while three jumps deep
//! should not teleport you, and walking back through where you have been
//! should not change a single character.

use crate::entity::{EntityId, Relation, Site};

/// One traversal. `from` is where you were, and it is what [`NavPath::back`]
/// returns you to — not the previous step's `to`, which is the same thing
/// only when the path is unbroken, and is wrong the moment a jump starts
/// somewhere the last one did not end.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavStep {
    pub from: EntityId,
    pub rel: Relation,
    pub to: EntityId,
    /// Where the relationship was observed — the callsite you would return
    /// to, which is generally NOT the start of `from`.
    pub site: Option<Site>,
    /// Where the jump LANDED — what [`NavPath::forward`] takes you back to.
    /// `to` names the entity; an entity is not a position (a file has no
    /// line, and an item is many lines), so without this forward could only
    /// open the file at its top.
    pub landing: Option<Site>,
}

impl NavStep {
    pub fn new(from: EntityId, rel: Relation, to: EntityId) -> NavStep {
        NavStep { from, rel, to, site: None, landing: None }
    }
    pub fn at(mut self, site: Site) -> NavStep {
        self.site = Some(site);
        self
    }
    pub fn landed(mut self, site: Site) -> NavStep {
        self.landing = Some(site);
        self
    }
    /// A jump that went nowhere: same entity, and no two different lines to
    /// tell it from standing still.
    fn is_self_loop(&self) -> bool {
        if self.from != self.to {
            return false;
        }
        match (&self.site, &self.landing) {
            (Some(a), Some(b)) => a.path == b.path && a.line == b.line,
            _ => true,
        }
    }
    /// Reads as "resolve_collision → calls → apply_impulse".
    pub fn label(&self) -> String {
        format!("{} → {} → {}", self.from.label(), self.rel.label(), self.to.label())
    }
}

/// The trail, with a cursor into it.
///
/// `at` is the number of steps TAKEN: `0` means "at the origin, nothing
/// behind me", `steps.len()` means "at the far end". Storing the count
/// rather than an index into the vec removes the empty-path special case,
/// which is where an off-by-one in this kind of structure always lives.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavPath {
    steps: Vec<NavStep>,
    at: usize,
}

/// How far back the path may remember.
///
/// Not unbounded: a long session is thousands of jumps, every one holding
/// two owned ids, and none of it is reachable through a UI that shows the
/// last handful. Dropping from the FRONT keeps the recent end, which is the
/// end anyone uses.
pub const MAX_STEPS: usize = 256;

impl NavPath {
    pub fn new() -> NavPath {
        NavPath::default()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
    pub fn len(&self) -> usize {
        self.steps.len()
    }
    /// Every step, oldest first, including any that have been rewound past.
    pub fn all(&self) -> &[NavStep] {
        &self.steps
    }
    pub fn can_go_back(&self) -> bool {
        self.at > 0
    }
    pub fn can_go_forward(&self) -> bool {
        self.at < self.steps.len()
    }

    /// Where you are now, as far as the path knows: the destination of the
    /// last step taken, or the origin of the first if you have rewound to the
    /// start. `None` for an empty path.
    pub fn here(&self) -> Option<&EntityId> {
        if self.at == 0 {
            self.steps.first().map(|s| &s.from)
        } else {
            self.steps.get(self.at - 1).map(|s| &s.to)
        }
    }

    /// Record a traversal.
    ///
    /// Truncates any rewound tail first. A step that goes nowhere is
    /// dropped — several call sites can produce one (accepting the symbol
    /// you are already on), and a path full of self-loops makes back stop
    /// working in a way nobody can see the cause of.
    ///
    /// "Nowhere" is the same entity AND the same line. The same entity alone
    /// was the old rule, and it dropped every jump inside one file when the
    /// only entity the caller could name was the FILE (no outline: a
    /// language without a grammar, or code outside any item) — F12 to a
    /// definition 400 lines down, and no way back.
    pub fn push(&mut self, step: NavStep) {
        if step.is_self_loop() {
            return;
        }
        self.steps.truncate(self.at);
        self.steps.push(step);
        if self.steps.len() > MAX_STEPS {
            let drop = self.steps.len() - MAX_STEPS;
            self.steps.drain(..drop);
        }
        self.at = self.steps.len();
    }

    /// Step back, returning the step that was undone — its `from` is where
    /// the caller should now go, and its `site` is the exact spot.
    pub fn back(&mut self) -> Option<&NavStep> {
        if self.at == 0 {
            return None;
        }
        self.at -= 1;
        self.steps.get(self.at)
    }

    /// Step forward again, returning the step retaken — go to its `to`.
    pub fn forward(&mut self) -> Option<&NavStep> {
        if self.at >= self.steps.len() {
            return None;
        }
        let s = &self.steps[self.at];
        self.at += 1;
        Some(s)
    }

    /// The trail behind you, oldest first — what a breadcrumb chip renders.
    pub fn trail(&self) -> &[NavStep] {
        &self.steps[..self.at]
    }

    /// The trail collapsed to a line of labels, newest last, at most `max`
    /// entries with an ellipsis when it was cut.
    ///
    /// The transcript's "the way back collapses into a visible path". Labels
    /// only — a chip that shows relations too is unreadable at the width a
    /// status bar actually has.
    pub fn breadcrumb(&self, max: usize) -> String {
        let t = self.trail();
        if t.is_empty() {
            return String::new();
        }
        let mut parts: Vec<String> = Vec::new();
        let skipped = t.len().saturating_sub(max.max(1));
        if skipped > 0 {
            parts.push("…".to_string());
        }
        parts.push(t[skipped].from.label());
        for s in &t[skipped..] {
            parts.push(s.to.label());
        }
        parts.join(" › ")
    }

    /// Everything the path has touched, in first-visit order — the bucket a
    /// finished investigation turns into ("keep this").
    pub fn visited(&self) -> Vec<EntityId> {
        let mut out: Vec<EntityId> = Vec::new();
        let mut push = |id: &EntityId| {
            if !out.contains(id) {
                out.push(id.clone());
            }
        };
        for s in &self.steps {
            push(&s.from);
            push(&s.to);
        }
        out
    }

    /// Forget everything. What closing a workspace does; not bound to any
    /// key, because "I did not mean to go back that far" is what `forward`
    /// is for.
    pub fn clear(&mut self) {
        self.steps.clear();
        self.at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str) -> EntityId {
        EntityId::item("src/a.rs", "fn", name)
    }
    #[test]
    fn a_jump_within_one_entity_is_kept_when_it_moved_lines() {
        let file = EntityId::file("src/a.rs");
        let mut p = NavPath::new();
        let far = NavStep::new(file.clone(), Relation::References, file.clone())
            .at(Site::new("src/a.rs", 10, 4))
            .landed(Site::new("src/a.rs", 410, 0));
        p.push(far.clone());
        assert_eq!(p.len(), 1, "a 400-line jump inside one file is a jump");
        let still = NavStep::new(file.clone(), Relation::References, file)
            .at(Site::new("src/a.rs", 10, 4))
            .landed(Site::new("src/a.rs", 10, 9));
        p.push(still);
        assert_eq!(p.len(), 1, "same line is standing still");
        assert_eq!(p.forward(), None);
        assert_eq!(p.back().unwrap().site, far.site);
        assert_eq!(p.forward().unwrap().landing, far.landing, "forward knows where it landed");
    }

    fn step(a: &str, b: &str, line: u32) -> NavStep {
        NavStep::new(f(a), Relation::Calls, f(b)).at(Site::new("src/a.rs", line, 0))
    }

    #[test]
    fn back_returns_the_step_not_the_place_so_the_callsite_survives() {
        let mut p = NavPath::new();
        p.push(step("main", "run", 12));
        assert_eq!(p.here(), Some(&f("run")));

        let undone = p.back().expect("something to undo").clone();
        assert_eq!(undone.from, f("main"), "back lands on where the step STARTED");
        assert_eq!(
            undone.site,
            Some(Site::new("src/a.rs", 12, 0)),
            "and at the exact line, not the top of the function"
        );
        assert_eq!(p.here(), Some(&f("main")));
        assert!(!p.can_go_back());
        assert!(p.can_go_forward());
    }

    #[test]
    fn back_from_deep_in_a_chain_returns_the_step_it_undid() {
        // A one-step path cannot tell an off-by-one here from correct code —
        // both readings return `steps[0]`. Found by breaking `back` to
        // return the PREVIOUS step and watching every test still pass. Three
        // steps and an assertion on the returned step, not on `here()`,
        // which reads the cursor and so agrees with either version.
        let mut p = NavPath::new();
        p.push(step("a", "b", 1));
        p.push(step("b", "c", 2));
        p.push(step("c", "d", 3));

        let undone = p.back().expect("a step").clone();
        assert_eq!(undone.from, f("c"), "undid c→d");
        assert_eq!(undone.to, f("d"));
        assert_eq!(undone.site, Some(Site::new("src/a.rs", 3, 0)), "and its callsite, not another's");

        let undone = p.back().expect("another").clone();
        assert_eq!((undone.from, undone.to), (f("b"), f("c")));
    }

    /// The property a location stack cannot have.
    #[test]
    fn two_routes_to_the_same_target_are_different_steps() {
        let mut p = NavPath::new();
        p.push(step("alpha", "shared", 10));
        let via_alpha = p.back().unwrap().clone();
        p.forward();
        p.push(step("beta", "shared", 99));
        let via_beta = p.back().unwrap().clone();

        assert_eq!(via_alpha.to, via_beta.to, "same destination");
        assert_ne!(via_alpha, via_beta, "different journeys");
        assert_ne!(via_alpha.from, via_beta.from, "and back goes to different places");
        assert_ne!(via_alpha.site, via_beta.site);
    }

    #[test]
    fn forward_retraces_and_a_new_step_truncates_the_tail() {
        let mut p = NavPath::new();
        p.push(step("a", "b", 1));
        p.push(step("b", "c", 2));
        p.push(step("c", "d", 3));
        assert_eq!(p.len(), 3);

        p.back();
        p.back();
        assert_eq!(p.here(), Some(&f("b")));
        assert_eq!(p.forward().unwrap().to, f("c"), "and forward again");
        assert_eq!(p.here(), Some(&f("c")));

        p.back();
        p.push(step("b", "z", 9));
        assert_eq!(p.len(), 2, "the c→d future no longer follows from here");
        assert_eq!(p.here(), Some(&f("z")));
        assert!(!p.can_go_forward());
    }

    #[test]
    fn the_ends_are_not_special_cases() {
        let mut p = NavPath::new();
        assert!(p.back().is_none());
        assert!(p.forward().is_none());
        assert_eq!(p.here(), None);
        assert_eq!(p.breadcrumb(5), "");
        assert!(p.visited().is_empty());

        p.push(step("a", "b", 1));
        assert!(p.forward().is_none(), "already at the far end");
        p.back();
        assert!(p.back().is_none(), "already at the origin");
        assert_eq!(p.here(), Some(&f("a")), "…and still knows where that is");
    }

    #[test]
    fn a_step_that_goes_nowhere_is_not_recorded() {
        // Accepting a symbol you are already on. Left in, `back` appears to
        // do nothing and the cause is invisible.
        let mut p = NavPath::new();
        p.push(NavStep::new(f("a"), Relation::Calls, f("a")));
        assert!(p.is_empty());
        assert!(!p.can_go_back());
    }

    #[test]
    fn the_trail_is_what_is_behind_you_not_the_whole_vec() {
        let mut p = NavPath::new();
        p.push(step("a", "b", 1));
        p.push(step("b", "c", 2));
        p.back();
        assert_eq!(p.trail().len(), 1, "the rewound step is not behind you");
        assert_eq!(p.all().len(), 2, "but it is still there to go forward into");
        assert_eq!(p.breadcrumb(9), "a › b");
        p.forward();
        assert_eq!(p.breadcrumb(9), "a › b › c");
    }

    #[test]
    fn the_breadcrumb_keeps_the_recent_end_and_marks_the_cut() {
        let mut p = NavPath::new();
        for (a, b) in [("a", "b"), ("b", "c"), ("c", "d"), ("d", "e")] {
            p.push(step(a, b, 1));
        }
        assert_eq!(p.breadcrumb(99), "a › b › c › d › e");
        assert_eq!(p.breadcrumb(2), "… › c › d › e", "the near end is the useful end");
        assert_eq!(p.breadcrumb(1), "… › d › e");
        assert_eq!(p.breadcrumb(0), "… › d › e", "zero is treated as one, not as a panic");
    }

    #[test]
    fn the_path_is_bounded_and_drops_from_the_old_end() {
        let mut p = NavPath::new();
        for i in 0..(MAX_STEPS + 50) {
            p.push(step(&format!("n{i}"), &format!("n{}", i + 1), i as u32));
        }
        assert_eq!(p.len(), MAX_STEPS);
        assert_eq!(p.here(), Some(&f(&format!("n{}", MAX_STEPS + 50))), "the recent end survives");
        assert_eq!(p.all()[0].from, f("n50"), "the old end went");
        // And back still walks the whole remaining path without underflow.
        let mut n = 0;
        while p.back().is_some() {
            n += 1;
        }
        assert_eq!(n, MAX_STEPS);
    }

    #[test]
    fn visited_is_first_visit_order_and_deduplicated() {
        let mut p = NavPath::new();
        p.push(step("a", "b", 1));
        p.push(step("b", "c", 2));
        p.push(step("c", "a", 3)); // back round to the start
        let v: Vec<String> = p.visited().iter().map(|e| e.label()).collect();
        assert_eq!(v, ["a", "b", "c"], "a bucket made from this has three entries, not six");
    }

    #[test]
    fn a_step_reads_as_a_sentence() {
        assert_eq!(step("main", "run", 3).label(), "main → calls → run");
    }
}
