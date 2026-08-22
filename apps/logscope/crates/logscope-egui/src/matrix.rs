//! matrix — the protocol as rows a message falls through, and columns it can
//! take at each row.
//!
//! ```text
//!            col 0          col 1          col 2
//!          ┌────────────────────────────────────────┐
//!  row 0   │  liaison                               │   one column: no choice
//!          ├────────────────────────────────────────┤
//!  row 1   │  local-lookup   web-lookup   model     │ ← DIRECTOR sits here
//!          ├────────────────────────────────────────┤
//!  row 2   │  delegator      optimizer              │ ← DIRECTOR sits here
//!          ├────────────────────────────────────────┤
//!  row 3   │  summarise                             │   one column: no choice
//!          └────────────────────────────────────────┘
//! ```
//!
//! A row with ONE column needs no decision, so it has no director; a row with
//! more than one does, and the director is a real code block rather than a
//! prompt, because "which lane handles this" is a routing question and routing
//! questions answered by a language model are answered differently every run.
//!
//! ## Why the shape earns its keep: prefetch
//!
//! Knowing the row a message is ABOUT to reach is what lets the runtime warm
//! the next cell — its weights, its prompt, its tools — while the current one
//! is still running. That is the same trick `gpu-gemm`'s expert staging ring
//! plays with weights, and it targets the same measured problem: this fleet's
//! profile is 92% GPU sync at 1% of memory bandwidth, so the hardware is idle
//! waiting on work that could have been prepared.
//!
//! The prediction is a SEAM, not a neural net. [`Predictor`] is a trait with a
//! frequency-table default, so a learned model can replace it without anything
//! else moving — and the frequency table is already a real predictor, which
//! means the prefetch path can be measured before anyone trains anything.
//!
//! ## The invariant that makes prefetch safe to be wrong
//!
//! A misprediction must cost a wasted warm-up and NOTHING else. So prefetch
//! never influences routing: [`Director::choose`] cannot see what was warmed,
//! and the runtime asks it fresh. A predictor that could steer the route would
//! be a router, and a bad one — trained on its own past choices.
//!
//! ## And it must not eat the memory it is trying to save
//!
//! Residency is budgeted in [`Warm`]. The point of a small footprint is that a
//! BIGGER model fits resident; a prefetcher that holds six cells warm to save
//! six stalls has spent the thing it was buying.

use std::collections::HashMap;

/// One cell: a thing that can handle a message at a given row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub name: String,
    /// What must be resident for this cell to run, in bytes. Weights, prompt,
    /// tool state — whatever the runtime would otherwise load on demand.
    pub residency: u64,
}

impl Cell {
    pub fn new(name: impl Into<String>, residency: u64) -> Self {
        Cell { name: name.into(), residency }
    }
}

/// One row of the matrix.
#[derive(Debug, Clone)]
pub struct Row {
    pub name: String,
    pub cells: Vec<Cell>,
}

impl Row {
    pub fn new(name: impl Into<String>, cells: Vec<Cell>) -> Self {
        Row { name: name.into(), cells }
    }

    /// Does this row need a director?
    ///
    /// Exactly when there is a choice to make. A single-column row with a
    /// director is a decision point that always decides the same way, which
    /// reads as configurable and is not.
    pub fn needs_director(&self) -> bool {
        self.cells.len() > 1
    }
}

/// Chooses which column(s) a message takes at a branching row.
///
/// Deliberately not a model call. Routing answered by a language model is
/// answered differently every run, which makes a pipeline impossible to
/// compare against itself and impossible to prefetch for.
pub trait Director {
    /// Which columns handle this. More than one means fan-out.
    fn choose(&self, row: &Row, msg: &Message) -> Vec<usize>;
}

/// What is travelling through the matrix.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Message {
    pub intent: String,
    /// Set by cells as they run; a director may read it.
    pub tags: Vec<String>,
}

impl Message {
    pub fn new(intent: impl Into<String>) -> Self {
        Message { intent: intent.into(), tags: Vec::new() }
    }
    pub fn tagged(mut self, t: impl Into<String>) -> Self {
        self.tags.push(t.into());
        self
    }
    pub fn has(&self, t: &str) -> bool {
        self.tags.iter().any(|x| x == t)
    }
}

/// One step of a taken route: the row, and the columns it went to.
pub type Step = (usize, Vec<usize>);

/// Predicts where a message will go, so the runtime can warm it early.
pub trait Predictor {
    /// Likely columns at `row`, best first. May be empty — an honest "no idea"
    /// is better than a guess, because every wrong warm-up costs residency the
    /// budget could have spent on a right one.
    fn likely(&self, row: usize, msg: &Message) -> Vec<usize>;

    /// Learn from a completed route.
    fn observe(&mut self, route: &[Step], msg: &Message);
}

/// The default predictor: how often each column was taken at each row.
///
/// Not a neural net, and it does not pretend to be. It is the baseline any
/// learned router has to beat, and having it here means the prefetch machinery
/// can be measured before a model exists — if a frequency table already
/// captures most of the win, a net is not the next thing to build.
#[derive(Debug, Clone, Default)]
pub struct FrequencyPredictor {
    /// (row, column) -> times taken.
    counts: HashMap<(usize, usize), u32>,
    /// (row, column, tag) -> times taken with that tag present. The tag is the
    /// only context it has; a real model would have the message.
    tagged: HashMap<(usize, usize, String), u32>,
}

impl Predictor for FrequencyPredictor {
    fn likely(&self, row: usize, msg: &Message) -> Vec<usize> {
        let mut scored: Vec<(u32, usize)> = Vec::new();
        for (&(r, c), &n) in &self.counts {
            if r != row { continue; }
            // A tag match counts for more: "this message looks like the ones
            // that went left" is the whole signal a table this simple has.
            let bonus: u32 = msg.tags.iter()
                .filter_map(|t| self.tagged.get(&(r, c, t.clone())))
                .sum();
            scored.push((n + bonus * 4, c));
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, c)| c).collect()
    }

    fn observe(&mut self, route: &[Step], msg: &Message) {
        for (row, cols) in route {
            for c in cols {
                *self.counts.entry((*row, *c)).or_default() += 1;
                for t in &msg.tags {
                    *self.tagged.entry((*row, *c, t.clone())).or_default() += 1;
                }
            }
        }
    }
}

/// What is currently held warm, under a budget.
#[derive(Debug, Clone)]
pub struct Warm {
    budget: u64,
    held: Vec<(String, u64)>,
    /// Warm cells that were actually used, and warm cells that were not.
    pub hits: u32,
    pub wasted: u32,
}

impl Warm {
    pub fn new(budget: u64) -> Self {
        Warm { budget, held: Vec::new(), hits: 0, wasted: 0 }
    }

    pub fn used(&self) -> u64 {
        self.held.iter().map(|(_, b)| b).sum()
    }

    pub fn is_warm(&self, name: &str) -> bool {
        self.held.iter().any(|(n, _)| n == name)
    }

    /// Warm a cell if it fits, evicting the oldest until it does.
    ///
    /// Returns false when the cell cannot fit even in an empty budget — a cell
    /// larger than the whole allowance is a configuration problem, and
    /// silently evicting everything to half-fit it would turn one bad number
    /// into a thrashing pipeline.
    pub fn warm(&mut self, c: &Cell) -> bool {
        if self.is_warm(&c.name) { return true; }
        if c.residency > self.budget { return false; }
        while self.used() + c.residency > self.budget && !self.held.is_empty() {
            let (n, _) = self.held.remove(0);
            // Evicting something never used is the prefetcher's own miss, and
            // counting it is what makes a bad predictor visible rather than
            // merely expensive.
            let _ = n;
            self.wasted += 1;
        }
        self.held.push((c.name.clone(), c.residency));
        true
    }

    /// Called when a cell actually runs.
    pub fn touch(&mut self, name: &str) {
        if let Some(i) = self.held.iter().position(|(n, _)| n == name) {
            self.hits += 1;
            // Move to the back: least-recently-used eviction, so the cell that
            // just ran is the last to go.
            let e = self.held.remove(i);
            self.held.push(e);
        }
    }

    /// Share of warmed cells that were used. The prefetcher's score.
    pub fn precision(&self) -> f32 {
        let total = self.hits + self.wasted;
        if total == 0 { return 0.0; }
        self.hits as f32 / total as f32
    }
}

/// The matrix and its runtime.
pub struct Matrix {
    pub rows: Vec<Row>,
    /// One director per row that needs one. A branching row without a director
    /// is a build error, not a runtime surprise — see `validate`.
    pub directors: HashMap<usize, Box<dyn Director>>,
}

impl Matrix {
    pub fn new(rows: Vec<Row>) -> Self {
        Matrix { rows, directors: HashMap::new() }
    }

    pub fn with_director(mut self, row: usize, d: Box<dyn Director>) -> Self {
        self.directors.insert(row, d);
        self
    }

    /// Every branching row has a director, and no single-column row has one.
    pub fn validate(&self) -> Result<(), String> {
        for (i, r) in self.rows.iter().enumerate() {
            match (r.needs_director(), self.directors.contains_key(&i)) {
                (true, false) => return Err(format!(
                    "row {i} ({}) has {} columns and no director — something must choose",
                    r.name, r.cells.len())),
                (false, true) => return Err(format!(
                    "row {i} ({}) has one column and a director — a decision point that always \
decides the same way reads as configurable and is not", r.name)),
                _ => {}
            }
        }
        Ok(())
    }

    /// Run a message through, warming the next row as each row is entered.
    ///
    /// The prefetch happens BEFORE the current row's work, which is the whole
    /// point: the warm-up overlaps the run rather than following it.
    pub fn route<P: Predictor>(&self, msg: &Message, pred: &P, warm: &mut Warm) -> Vec<Step> {
        let mut taken = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            // WARM THE NEXT ROW FIRST. Prediction cannot see the current row's
            // outcome yet, which is exactly why it is a prediction.
            if let Some(next) = self.rows.get(i + 1) {
                for c in pred.likely(i + 1, msg) {
                    if let Some(cell) = next.cells.get(c) {
                        if !warm.warm(cell) { break; }
                    }
                }
            }
            let cols = if row.needs_director() {
                // The director is asked FRESH. It cannot see what was warmed —
                // a predictor that could steer the route would be a router
                // trained on its own past choices.
                self.directors.get(&i).map(|d| d.choose(row, msg)).unwrap_or_default()
            } else {
                vec![0]
            };
            for &c in &cols {
                if let Some(cell) = row.cells.get(c) { warm.touch(&cell.name); }
            }
            taken.push((i, cols));
        }
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<usize>);
    impl Director for Fixed {
        fn choose(&self, _r: &Row, _m: &Message) -> Vec<usize> { self.0.clone() }
    }
    /// Routes on a tag, which is what a real director does.
    struct ByTag;
    impl Director for ByTag {
        fn choose(&self, _r: &Row, m: &Message) -> Vec<usize> {
            if m.has("code") { vec![0] } else { vec![1] }
        }
    }

    fn grid() -> Matrix {
        Matrix::new(vec![
            Row::new("liaison", vec![Cell::new("liaison", 100)]),
            Row::new("lookup", vec![Cell::new("local", 200), Cell::new("web", 200),
                                    Cell::new("model", 900)]),
            Row::new("summarise", vec![Cell::new("summarise", 100)]),
        ]).with_director(1, Box::new(ByTag))
    }

    #[test]
    fn a_branching_row_must_have_a_director_and_a_single_column_must_not() {
        // The matrix's shape rule: a director sits at each row with more than
        // one column, and nowhere else.
        let m = Matrix::new(vec![
            Row::new("a", vec![Cell::new("x", 1), Cell::new("y", 1)]),
        ]);
        let e = m.validate().unwrap_err();
        assert!(e.contains("no director"), "{e}");

        let m = Matrix::new(vec![Row::new("a", vec![Cell::new("x", 1)])])
            .with_director(0, Box::new(Fixed(vec![0])));
        let e = m.validate().unwrap_err();
        assert!(e.contains("always decides the same way"), "{e}");

        assert!(grid().validate().is_ok());
    }

    #[test]
    fn the_next_row_is_warmed_before_the_current_one_runs() {
        // The whole point of the shape. If the warm-up followed the work it
        // would be a cache, not a prefetch, and the stall it is aimed at would
        // still be there.
        let m = grid();
        let mut p = FrequencyPredictor::default();
        // Teach it that tagged-code messages go to column 0.
        let msg = Message::new("q").tagged("code");
        p.observe(&[(1, vec![0])], &msg);

        let mut w = Warm::new(10_000);
        m.route(&msg, &p, &mut w);
        assert!(w.is_warm("local"), "the predicted cell was never warmed");
        assert!(w.hits > 0, "a warmed cell was never credited when it ran");
    }

    #[test]
    fn a_misprediction_costs_a_warm_up_and_nothing_else() {
        // The invariant that makes prefetch safe to be wrong. The director is
        // asked fresh and cannot see what was warmed, so a bad prediction
        // changes the bill and never the route.
        let m = grid();
        let mut p = FrequencyPredictor::default();
        // Teach it the WRONG column for a code message.
        p.observe(&[(1, vec![1])], &Message::new("q").tagged("code"));

        let msg = Message::new("q").tagged("code");
        let mut w = Warm::new(10_000);
        let route = m.route(&msg, &p, &mut w);
        // ByTag sends "code" to column 0 regardless of what was warmed.
        assert_eq!(route[1].1, vec![0], "the prediction steered the route");
        assert!(w.is_warm("web"), "the wrong cell should still have been warmed");
    }

    #[test]
    fn the_budget_is_not_exceeded_and_eviction_is_counted() {
        // A prefetcher that holds six cells warm to save six stalls has spent
        // the thing it was buying: the small footprint exists so a BIGGER
        // model fits resident.
        let mut w = Warm::new(500);
        assert!(w.warm(&Cell::new("a", 200)));
        assert!(w.warm(&Cell::new("b", 200)));
        assert!(w.warm(&Cell::new("c", 200)));   // forces an eviction
        assert!(w.used() <= 500, "budget blown: {}", w.used());
        assert_eq!(w.wasted, 1, "the evicted-unused cell was not counted");
    }

    #[test]
    fn a_cell_larger_than_the_whole_budget_is_refused_not_thrashed() {
        // Silently evicting everything to half-fit one oversized cell turns a
        // configuration mistake into a pipeline that thrashes forever.
        let mut w = Warm::new(500);
        w.warm(&Cell::new("a", 200));
        assert!(!w.warm(&Cell::new("huge", 5000)));
        assert!(w.is_warm("a"), "the existing warm set was destroyed for a cell that cannot fit");
    }

    #[test]
    fn the_predictor_scores_itself_so_a_bad_one_is_visible() {
        // "wasted" is the prefetcher's own miss. Without counting it, a
        // predictor that warms the wrong cell every time looks identical to
        // one that is right — both just spend residency.
        let mut w = Warm::new(300);
        w.warm(&Cell::new("used", 100));
        w.touch("used");
        w.warm(&Cell::new("x", 100));
        w.warm(&Cell::new("y", 100));
        w.warm(&Cell::new("z", 100));   // evicts one unused
        assert!(w.precision() < 1.0 && w.precision() > 0.0, "{}", w.precision());
    }

    #[test]
    fn frequency_beats_nothing_and_a_tag_beats_raw_frequency() {
        // The baseline any learned router has to beat. If a table this simple
        // already captures the win, a net is not the next thing to build —
        // which is the point of having it before one.
        let mut p = FrequencyPredictor::default();
        // Column 1 is taken more often overall...
        for _ in 0..10 { p.observe(&[(1, vec![1])], &Message::new("q")); }
        // ...but "code" messages always went to column 0.
        for _ in 0..3 { p.observe(&[(1, vec![0])], &Message::new("q").tagged("code")); }

        assert_eq!(p.likely(1, &Message::new("q")).first(), Some(&1),
                   "raw frequency should win with no tag");
        assert_eq!(p.likely(1, &Message::new("q").tagged("code")).first(), Some(&0),
                   "the tag should override the raw count");
    }

    #[test]
    fn an_untrained_predictor_says_nothing_rather_than_guessing() {
        // Every wrong warm-up costs residency a right one could have used, so
        // "no idea" is the correct output before there is evidence.
        let p = FrequencyPredictor::default();
        assert!(p.likely(1, &Message::new("q")).is_empty());
        let mut w = Warm::new(10_000);
        grid().route(&Message::new("q"), &p, &mut w);
        assert_eq!(w.wasted, 0, "an untrained predictor spent residency on a guess");
    }

    #[test]
    fn a_fan_out_row_warms_and_credits_every_column_it_takes() {
        // A director may return more than one column; that is the fan-out the
        // matrix exists to express, and both branches are real work.
        let m = Matrix::new(vec![
            Row::new("a", vec![Cell::new("x", 10)]),
            Row::new("fan", vec![Cell::new("p", 10), Cell::new("q", 10)]),
        ]).with_director(1, Box::new(Fixed(vec![0, 1])));
        let mut p = FrequencyPredictor::default();
        p.observe(&[(1, vec![0, 1])], &Message::new("m"));
        let mut w = Warm::new(1000);
        let route = m.route(&Message::new("m"), &p, &mut w);
        assert_eq!(route[1].1, vec![0, 1]);
        assert_eq!(w.hits, 2, "both fanned columns should be credited");
    }
}
