//! curve_refs — a curve may read another curve, and the cycle that lets in.
//!
//! Today a `Curve` is a flat `Vec<(Ticks, f32)>` and `eval_lane` is a lookup.
//! The moment a curve can reference another — a riser that follows the filter
//! sweep, a shake whose amplitude rides the music — evaluation becomes a graph
//! and a graph can close on itself.
//!
//! ## Cycles are repaired, not forbidden
//!
//! The obvious move is to reject any reference that would close a loop. That
//! is wrong for the order people actually work in: an editor builds the clip
//! first, gets the motion right by eye, and only then discovers the
//! relationship they have drawn is circular. Refusing the edit at that moment
//! throws away the part they got right and tells them to plan better.
//!
//! The pattern that works, borrowed from animation and VFX rigs: break the
//! cycle with an explicit intermediary — a **dummy** — that holds sampled
//! values instead of a live reference. `A → B → A` becomes `A → B → D`, where
//! `D` is a real curve carrying what `A` was evaluating to. The loop is gone,
//! the shape is kept, and the dummy is visible in the project rather than
//! being an invisible rule the evaluator applies.
//!
//! So the edit is ALLOWED and the cycle is reported. [`promote_to_dummy`] is
//! the repair, and it is the kind of thing a command palette should offer by
//! name at the moment the cycle appears.
//!
//! ## What the dummy holds
//!
//! Sampled values, not a copy of the referenced curve. The artist dialled in a
//! RESULT — what the chain evaluated to — and a copy of one link in that chain
//! is not the same thing. [`sample`] walks the graph with the cycle edge cut
//! and records what came out, so the dummy reproduces what was on screen when
//! the promotion happened.
//!
//! That is also why promotion is lossy in one specific way worth stating: a
//! sampled dummy is FROZEN. It no longer follows the curve it was reading.
//! That is the trade the pattern makes and it is the right one — a live
//! circular reference has no defined value at all — but a UI offering this
//! should say so rather than presenting it as a free fix.

use std::collections::{BTreeMap, BTreeSet};
use studio_core::{CurveId, Ticks};

/// One curve reading another, scaled.
///
/// `scale` because the common case is "follow that, but a third as much", and
/// a reference with no depth control would send every artist to build a second
/// curve immediately.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ref {
    pub from: CurveId,
    pub to: CurveId,
    pub scale: f32,
}

/// The reference graph, alongside the project's curves.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Refs {
    edges: Vec<Ref>,
}

impl Refs {
    pub fn new() -> Refs {
        Refs::default()
    }

    /// Add a reference. Never refuses — see the module header.
    pub fn link(&mut self, from: CurveId, to: CurveId, scale: f32) {
        self.edges.retain(|e| !(e.from == from && e.to == to));
        self.edges.push(Ref { from, to, scale });
    }

    pub fn unlink(&mut self, from: CurveId, to: CurveId) {
        self.edges.retain(|e| !(e.from == from && e.to == to));
    }

    pub fn edges(&self) -> &[Ref] {
        &self.edges
    }

    /// What `from` reads.
    pub fn outgoing(&self, from: CurveId) -> Vec<&Ref> {
        self.edges.iter().filter(|e| e.from == from).collect()
    }

    /// Every cycle reachable from `start`, as the path that closes.
    ///
    /// Returns the path INCLUDING the repeated node at the end, so
    /// `[a, b, a]` reads as "a reads b reads a" — which is the sentence a UI
    /// wants to show. A caller that only needs a yes/no can check `is_empty`.
    pub fn cycle_through(&self, start: CurveId) -> Option<Vec<CurveId>> {
        let mut path = Vec::new();
        let mut on_path = BTreeSet::new();
        self.walk(start, &mut path, &mut on_path)
    }

    fn walk(
        &self,
        at: CurveId,
        path: &mut Vec<CurveId>,
        on_path: &mut BTreeSet<CurveId>,
    ) -> Option<Vec<CurveId>> {
        if on_path.contains(&at) {
            let mut found: Vec<CurveId> =
                path.iter().skip_while(|c| **c != at).copied().collect();
            found.push(at);
            return Some(found);
        }
        path.push(at);
        on_path.insert(at);
        for e in self.outgoing(at) {
            if let Some(c) = self.walk(e.to, path, on_path) {
                return Some(c);
            }
        }
        path.pop();
        on_path.remove(&at);
        None
    }

    /// Does any curve sit on a cycle?
    pub fn any_cycle(&self) -> Option<Vec<CurveId>> {
        let mut seen = BTreeSet::new();
        for e in &self.edges {
            if !seen.insert(e.from) {
                continue;
            }
            if let Some(c) = self.cycle_through(e.from) {
                return Some(c);
            }
        }
        None
    }
}

/// Evaluate `id` at `at`, with one edge cut.
///
/// `cut` is the edge the promotion is about to replace, so the walk that
/// produces the dummy's values is the walk that WILL happen once the dummy
/// exists — not the broken circular one. Without cutting, sampling a cyclic
/// graph either recurses forever or returns whatever the recursion guard
/// invented.
///
/// A missing curve contributes nothing rather than failing: a reference to a
/// deleted curve is a stale edge, and a sample is not the place to discover it.
pub fn sample(
    curves: &BTreeMap<CurveId, studio_core::Curve>,
    refs: &Refs,
    id: CurveId,
    at: Ticks,
    cut: Option<(CurveId, CurveId)>,
) -> f32 {
    fn go(
        curves: &BTreeMap<CurveId, studio_core::Curve>,
        refs: &Refs,
        id: CurveId,
        at: Ticks,
        cut: Option<(CurveId, CurveId)>,
        depth: usize,
    ) -> f32 {
        // A depth stop as well as the cut: `sample` is public and a caller can
        // hand it a graph with a cycle this cut does not break. Returning the
        // node's own value at the limit is wrong in a way that is at least
        // bounded and visible, where a stack overflow takes the app.
        if depth > 64 {
            return 1.0;
        }
        let own = curves
            .get(&id)
            .map(|c| studio_core::eval_lane(&c.points, at))
            .unwrap_or(1.0);
        let mut v = own;
        for e in refs.outgoing(id) {
            if cut == Some((e.from, e.to)) {
                continue;
            }
            v *= 1.0 + (go(curves, refs, e.to, at, cut, depth + 1) - 1.0) * e.scale;
        }
        v
    }
    go(curves, refs, id, at, cut, 0)
}

/// What a promotion would do, without doing it.
#[derive(Debug, Clone, PartialEq)]
pub struct Promotion {
    /// The edge that gets cut.
    pub cut: (CurveId, CurveId),
    /// The dummy's sampled points.
    pub points: Vec<(Ticks, f32)>,
    /// A name a person will recognise in a curve list.
    pub name: String,
    /// True when the dummy is frozen — always, today. Carried explicitly so a
    /// UI has something to warn from rather than the caller remembering.
    pub frozen: bool,
}

/// Default sampling resolution: one point per this many ticks.
///
/// Dense enough that a curve with per-beat detail survives, coarse enough that
/// a four-minute track does not become a hundred thousand points. A promotion
/// is a repair, not a bake — if someone needs sample-accurate they should say
/// so, which is why this is a parameter on [`promote_to_dummy`] and not a
/// constant buried in it.
pub const SAMPLE_EVERY: i64 = 120;

/// Plan the repair for a cycle: cut its last edge, freeze what it evaluated to.
///
/// Cuts the edge that CLOSES the loop — the last hop in the reported path —
/// because that is the reference the person most recently drew, and the one
/// they will recognise as the thing they just did.
pub fn promote_to_dummy(
    curves: &BTreeMap<CurveId, studio_core::Curve>,
    refs: &Refs,
    cycle: &[CurveId],
    span: (Ticks, Ticks),
    every: i64,
) -> Option<Promotion> {
    if cycle.len() < 2 {
        return None;
    }
    let from = cycle[cycle.len() - 2];
    let to = cycle[cycle.len() - 1];
    let every = every.max(1);
    let mut points = Vec::new();
    let mut t = span.0 .0;
    while t <= span.1 .0 {
        points.push((Ticks(t), sample(curves, refs, to, Ticks(t), Some((from, to)))));
        t += every;
    }
    // The end of the span, always: a promotion that stopped one step short
    // would let the dummy hold flat over the tail, which is exactly where a
    // riser's peak lives.
    if points.last().map(|(t, _)| *t) != Some(span.1) {
        points.push((span.1, sample(curves, refs, to, span.1, Some((from, to)))));
    }
    let base = curves.get(&to).map(|c| c.name.as_str()).unwrap_or("curve");
    Some(Promotion {
        cut: (from, to),
        points,
        name: format!("{base} (frozen)"),
        frozen: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::Curve;

    fn c(name: &str, pts: &[(i64, f32)]) -> Curve {
        Curve {
            name: name.into(),
            points: pts.iter().map(|(t, v)| (Ticks(*t), *v)).collect(),
        }
    }

    fn graph() -> BTreeMap<CurveId, Curve> {
        let mut m = BTreeMap::new();
        m.insert(CurveId(1), c("a", &[(0, 1.0), (1000, 2.0)]));
        m.insert(CurveId(2), c("b", &[(0, 1.0), (1000, 1.0)]));
        m
    }

    /// A REFERENCE IS NEVER REFUSED. The editor builds the clip first and
    /// discovers the loop after; rejecting the edit throws away the part they
    /// got right.
    #[test]
    fn a_circular_reference_is_accepted_and_reported() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(2), 1.0);
        r.link(CurveId(2), CurveId(1), 1.0);
        let cycle = r.cycle_through(CurveId(1)).expect("the cycle is found");
        assert_eq!(cycle, vec![CurveId(1), CurveId(2), CurveId(1)],
                   "reads as 'a reads b reads a'");
        assert!(r.any_cycle().is_some());
    }

    /// A chain that does not close is not a cycle, however long.
    #[test]
    fn a_long_chain_is_not_a_cycle() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(2), 1.0);
        r.link(CurveId(2), CurveId(3), 1.0);
        r.link(CurveId(3), CurveId(4), 1.0);
        assert_eq!(r.cycle_through(CurveId(1)), None);
        assert!(r.any_cycle().is_none());
    }

    /// A curve reading itself is the shortest cycle and must not be special.
    #[test]
    fn a_self_reference_is_a_cycle() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(1), 1.0);
        assert_eq!(r.cycle_through(CurveId(1)), Some(vec![CurveId(1), CurveId(1)]));
    }

    /// THE REPAIR CUTS THE CLOSING EDGE -- the reference most recently drawn,
    /// which is the one the person will recognise as what they just did.
    #[test]
    fn promotion_cuts_the_edge_that_closes_the_loop() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(2), 1.0);
        r.link(CurveId(2), CurveId(1), 1.0);
        let cycle = r.cycle_through(CurveId(1)).unwrap();
        let p = promote_to_dummy(&graph(), &r, &cycle, (Ticks(0), Ticks(1000)), SAMPLE_EVERY)
            .expect("a plan");
        assert_eq!(p.cut, (CurveId(2), CurveId(1)));
    }

    /// The dummy holds what the chain EVALUATED TO, not a copy of one link.
    /// With the cycle edge cut, curve 1 rises 1.0 -> 2.0, so the dummy must.
    #[test]
    fn the_dummy_freezes_the_evaluated_result() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(2), 1.0);
        r.link(CurveId(2), CurveId(1), 1.0);
        let cycle = r.cycle_through(CurveId(1)).unwrap();
        let p = promote_to_dummy(&graph(), &r, &cycle, (Ticks(0), Ticks(1000)), SAMPLE_EVERY)
            .unwrap();
        assert!(p.points.len() > 2);
        assert!((p.points[0].1 - 1.0).abs() < 1e-3, "starts where the curve starts");
        let last = p.points.last().unwrap();
        assert_eq!(last.0, Ticks(1000), "and the span's end is always sampled");
        assert!((last.1 - 2.0).abs() < 1e-3, "ends where it ends: {}", last.1);
    }

    /// FROZEN, and the plan says so. A sampled dummy no longer follows what it
    /// was reading -- the right trade, since a live cycle has no defined value
    /// at all, but a UI must be able to warn rather than present a free fix.
    #[test]
    fn the_plan_admits_the_dummy_is_frozen() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(2), 1.0);
        r.link(CurveId(2), CurveId(1), 1.0);
        let cycle = r.cycle_through(CurveId(1)).unwrap();
        let p = promote_to_dummy(&graph(), &r, &cycle, (Ticks(0), Ticks(1000)), SAMPLE_EVERY)
            .unwrap();
        assert!(p.frozen);
        assert!(p.name.contains("frozen"), "and is named so in a curve list: {}", p.name);
    }

    /// `scale` is why a reference is usable: "follow that, a third as much".
    #[test]
    fn a_reference_is_scaled_rather_than_all_or_nothing() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(2), 0.5);
        let mut m = graph();
        m.insert(CurveId(2), c("b", &[(0, 3.0), (1000, 3.0)]));
        let full = {
            let mut r2 = Refs::new();
            r2.link(CurveId(1), CurveId(2), 1.0);
            sample(&m, &r2, CurveId(1), Ticks(0), None)
        };
        let half = sample(&m, &r, CurveId(1), Ticks(0), None);
        assert!(half < full, "half depth must move it less: {half} vs {full}");
        assert!(half > 1.0, "but still move it");
    }

    /// `sample` is public and a caller can hand it a cycle the cut does not
    /// break. Bounded and visible beats a stack overflow that takes the app.
    #[test]
    fn sampling_an_unbroken_cycle_terminates() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(2), 1.0);
        r.link(CurveId(2), CurveId(1), 1.0);
        let v = sample(&graph(), &r, CurveId(1), Ticks(500), None);
        assert!(v.is_finite(), "got {v}");
    }

    /// A stale edge to a deleted curve contributes nothing. A sample is not
    /// the place to discover a dangling reference.
    #[test]
    fn a_reference_to_a_missing_curve_is_transparent() {
        let mut r = Refs::new();
        r.link(CurveId(1), CurveId(99), 1.0);
        let with = sample(&graph(), &r, CurveId(1), Ticks(0), None);
        let without = sample(&graph(), &Refs::new(), CurveId(1), Ticks(0), None);
        assert!((with - without).abs() < 1e-6, "{with} vs {without}");
    }

    /// Nothing to promote when there is no cycle.
    #[test]
    fn a_degenerate_cycle_has_no_promotion() {
        assert!(promote_to_dummy(&graph(), &Refs::new(), &[], (Ticks(0), Ticks(1)), 120).is_none());
        assert!(promote_to_dummy(&graph(), &Refs::new(), &[CurveId(1)], (Ticks(0), Ticks(1)), 120)
            .is_none());
    }
}
