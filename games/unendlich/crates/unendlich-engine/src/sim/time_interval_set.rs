//! sim::time_interval_set — bounded set algebra over tick intervals.
//!
//! From `12_Deep_Time/timebending_set_algebra`: time-bending is framed as *base
//! rewriting + set operations (union / intersection / difference) over bounded
//! regions in a history graph*, and the concrete structure it names is an
//! interval/range set answering "which span did a branch touch in `[t0, t1)`".
//! The framing doc isn't itself a spec, but that primitive is — a normalised set
//! of half-open tick intervals with the three set operations and overlap queries.
//! It's the datatype the time-bending machinery (and the `skill_constellation`
//! time-window gate) can build region queries on. Pure, deterministic, std-only.
//!
//! Intervals are half-open `[start, end)` in tick space; a `TimeSet` keeps its
//! intervals sorted, disjoint, and non-adjacent (coalesced), so equal sets have
//! equal representations.

/// A half-open tick interval `[start, end)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    pub start: i64,
    pub end: i64,
}

impl Interval {
    /// Construct, normalising an inverted range to empty.
    pub fn new(start: i64, end: i64) -> Interval {
        if end > start {
            Interval { start, end }
        } else {
            Interval { start, end: start }
        }
    }
    pub fn is_empty(self) -> bool {
        self.end <= self.start
    }
    pub fn len(self) -> i64 {
        (self.end - self.start).max(0)
    }
    pub fn contains(self, tick: i64) -> bool {
        tick >= self.start && tick < self.end
    }
    /// Do the two intervals share any tick?
    pub fn overlaps(self, other: Interval) -> bool {
        self.start < other.end && other.start < self.end && !self.is_empty() && !other.is_empty()
    }
    /// Are they overlapping OR touching end-to-start (so they should coalesce)?
    fn touches(self, other: Interval) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// A normalised set of tick intervals (sorted, disjoint, coalesced).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TimeSet {
    intervals: Vec<Interval>,
}

impl TimeSet {
    pub fn empty() -> TimeSet {
        TimeSet { intervals: Vec::new() }
    }

    pub fn from_interval(iv: Interval) -> TimeSet {
        let mut s = TimeSet::empty();
        s.insert(iv);
        s
    }

    /// Build from `(start, end)` pairs.
    pub fn of(ranges: &[(i64, i64)]) -> TimeSet {
        let mut s = TimeSet::empty();
        for &(a, b) in ranges {
            s.insert(Interval::new(a, b));
        }
        s
    }

    pub fn is_empty(&self) -> bool {
        self.intervals.is_empty()
    }

    pub fn intervals(&self) -> &[Interval] {
        &self.intervals
    }

    /// Sum of the lengths of all intervals (total ticks covered).
    pub fn total_span(&self) -> i64 {
        self.intervals.iter().map(|i| i.len()).sum()
    }

    /// Add one interval (set union with a singleton), re-coalescing.
    pub fn insert(&mut self, iv: Interval) {
        if iv.is_empty() {
            return;
        }
        self.intervals.push(iv);
        self.normalize();
    }

    /// Is `tick` covered?
    pub fn contains(&self, tick: i64) -> bool {
        self.intervals.iter().any(|i| i.contains(tick))
    }

    /// Does any interval overlap `iv`?
    pub fn overlaps(&self, iv: Interval) -> bool {
        self.intervals.iter().any(|i| i.overlaps(iv))
    }

    /// Union of two sets.
    pub fn union(&self, other: &TimeSet) -> TimeSet {
        let mut out = self.clone();
        for &iv in &other.intervals {
            out.intervals.push(iv);
        }
        out.normalize();
        out
    }

    /// Intersection of two sets.
    pub fn intersection(&self, other: &TimeSet) -> TimeSet {
        let mut out = TimeSet::empty();
        for &a in &self.intervals {
            for &b in &other.intervals {
                let start = a.start.max(b.start);
                let end = a.end.min(b.end);
                if end > start {
                    out.intervals.push(Interval { start, end });
                }
            }
        }
        out.normalize();
        out
    }

    /// This set minus `other` (punch out the other set's coverage).
    pub fn difference(&self, other: &TimeSet) -> TimeSet {
        let mut pieces: Vec<Interval> = self.intervals.clone();
        for &cut in &other.intervals {
            let mut next = Vec::new();
            for iv in pieces {
                // left remnant
                if iv.start < cut.start {
                    next.push(Interval { start: iv.start, end: iv.end.min(cut.start) });
                }
                // right remnant
                if iv.end > cut.end {
                    next.push(Interval { start: iv.start.max(cut.end), end: iv.end });
                }
                // fully-uncut interval (no overlap): keep as-is
                if !iv.overlaps(cut) {
                    next.push(iv);
                }
            }
            pieces = next.into_iter().filter(|i| !i.is_empty()).collect();
        }
        let mut out = TimeSet { intervals: pieces };
        out.normalize();
        out
    }

    /// Sort, drop empties, and coalesce overlapping/adjacent intervals.
    fn normalize(&mut self) {
        self.intervals.retain(|i| !i.is_empty());
        self.intervals.sort_by_key(|i| (i.start, i.end));
        let mut merged: Vec<Interval> = Vec::with_capacity(self.intervals.len());
        for iv in self.intervals.drain(..) {
            match merged.last_mut() {
                Some(last) if last.touches(iv) => {
                    last.end = last.end.max(iv.end);
                }
                _ => merged.push(iv),
            }
        }
        self.intervals = merged;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iv(a: i64, b: i64) -> Interval {
        Interval::new(a, b)
    }

    #[test]
    fn interval_basics() {
        let i = iv(10, 20);
        assert!(i.contains(10) && i.contains(19) && !i.contains(20));
        assert_eq!(i.len(), 10);
        assert!(iv(5, 5).is_empty());
        assert!(iv(9, 1).is_empty(), "inverted ⇒ empty");
        assert!(i.overlaps(iv(15, 25)));
        assert!(!i.overlaps(iv(20, 30)), "half-open: touching is not overlapping");
    }

    #[test]
    fn insert_coalesces_overlapping_and_adjacent() {
        let mut s = TimeSet::empty();
        s.insert(iv(0, 10));
        s.insert(iv(5, 15)); // overlaps ⇒ merge
        s.insert(iv(15, 20)); // adjacent ⇒ merge
        assert_eq!(s.intervals(), &[iv(0, 20)]);
        s.insert(iv(30, 40)); // disjoint ⇒ separate
        assert_eq!(s.intervals(), &[iv(0, 20), iv(30, 40)]);
        assert_eq!(s.total_span(), 20 + 10);
    }

    #[test]
    fn union_merges_two_sets() {
        let a = TimeSet::of(&[(0, 10), (20, 30)]);
        let b = TimeSet::of(&[(8, 22)]);
        assert_eq!(a.union(&b).intervals(), &[iv(0, 30)]);
    }

    #[test]
    fn intersection_keeps_shared_span() {
        let a = TimeSet::of(&[(0, 10), (20, 30)]);
        let b = TimeSet::of(&[(5, 25)]);
        assert_eq!(a.intersection(&b).intervals(), &[iv(5, 10), iv(20, 25)]);
        // disjoint sets ⇒ empty
        assert!(TimeSet::of(&[(0, 5)]).intersection(&TimeSet::of(&[(10, 15)])).is_empty());
    }

    #[test]
    fn difference_punches_holes() {
        let a = TimeSet::of(&[(0, 100)]);
        let b = TimeSet::of(&[(20, 40), (60, 70)]);
        assert_eq!(a.difference(&b).intervals(), &[iv(0, 20), iv(40, 60), iv(70, 100)]);
        // removing everything ⇒ empty
        assert!(a.difference(&TimeSet::of(&[(0, 100)])).is_empty());
        // removing a disjoint set ⇒ unchanged
        assert_eq!(a.difference(&TimeSet::of(&[(200, 300)])), a);
    }

    #[test]
    fn contains_and_overlaps_queries() {
        let s = TimeSet::of(&[(0, 10), (20, 30)]);
        assert!(s.contains(5) && s.contains(25));
        assert!(!s.contains(15));
        assert!(s.overlaps(iv(9, 21)));
        assert!(!s.overlaps(iv(10, 20)), "the gap between branch touches");
    }

    #[test]
    fn set_ops_are_deterministic_and_canonical() {
        // Same coverage built two different ways ⇒ identical representation.
        let x = TimeSet::of(&[(0, 5), (5, 10), (3, 8)]);
        let y = TimeSet::of(&[(0, 10)]);
        assert_eq!(x, y);
        assert_eq!(x.union(&y), y.union(&x));
    }
}
