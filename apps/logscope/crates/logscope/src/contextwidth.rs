//! contextwidth.rs — how much context a read needs, decided rather than fixed.
//!
//! `filewindow::CONTEXT` is a default and defaults are where arbitrary numbers hide. A width of 5 was
//! briefly adopted here on the belief that `rg -C 5` was a considered choice; it was an upload-size
//! limit for a different tool. **A constant with a plausible story attached is harder to question
//! than one with none.**
//!
//! ## Width follows purpose
//!
//! The two questions want opposite answers:
//!
//! * *How many places does this touch?* — many narrow windows. Distinct SITES are the answer, and a
//!   wide window spends the budget proving one site is still there.
//! * *What does this code do?* — few wide windows. A function split across a boundary is worse than
//!   not reading it.
//!
//! One number cannot serve both, and picking the average serves neither.
//!
//! ## And it adapts
//!
//! Two signals, both observable without judging the content:
//!
//! * **A re-read of the same file** means the window missed what was needed — too narrow.
//! * **A window whose lines went unused** means the budget bought nothing — too wide.
//!
//! Widening on a re-read is the important one, because a re-read is invisible as a failure: it looks
//! like diligence.

use crate::dwell::{Dwelling, Verdict};

/// What the read is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Purpose {
    /// Counting or locating sites. Narrow, many.
    Survey,
    /// Understanding one region. Wide, few.
    Comprehend,
    /// Editing: enough to place an anchor confidently and verify it is unique.
    Anchor,
}

impl Purpose {
    pub fn default_width(self) -> usize {
        match self {
            // Enough to see the signature and a line either side, no more. The question is WHERE.
            Purpose::Survey => 4,
            // A small function fits; a large one at least shows its shape.
            Purpose::Comprehend => 24,
            // An anchor needs its surroundings to be sure the match is unique — the failure mode is
            // a str_replace hitting a different occurrence, which this project has done.
            Purpose::Anchor => 12,
        }
    }

    pub fn explain(self) -> &'static str {
        match self {
            Purpose::Survey => "many narrow windows: distinct sites are the answer, and a wide \
window spends the budget proving one site is still there",
            Purpose::Comprehend => "few wide windows: a function split across a boundary is worse \
than not reading it",
            Purpose::Anchor => "enough surroundings to be sure the match is unique, because the \
failure is an edit landing on a different occurrence",
        }
    }
}

/// Bounds. Below the floor a window shows a line with no surroundings; above the ceiling it is
/// cheaper to read the file.
pub const MIN_WIDTH: usize = 2;
pub const MAX_WIDTH: usize = 60;

/// A width that learns.
pub struct Tuned {
    width: usize,
    purpose: Purpose,
    dwell: Dwelling,
    rereads: u32,
    wasted: u32,
}

impl Tuned {
    pub fn new(purpose: Purpose) -> Self {
        Tuned {
            width: purpose.default_width(),
            purpose,
            // Hysteresis from `dwell`, so a width does not oscillate on one bad read. Short dwell —
            // reads are cheap and frequent, unlike the model reloads dwell was written for.
            dwell: Dwelling::new(std::time::Duration::from_secs(20), 0.10),
            rereads: 0,
            wasted: 0,
        }
    }

    pub fn width(&self) -> usize { self.width }

    /// Record how a read went.
    ///
    /// `reread` — the same file had to be opened again for the same question.
    /// `useful_lines` / `shown_lines` — how much of what was shown was actually referenced.
    pub fn observe(&mut self, reread: bool, useful_lines: usize, shown_lines: usize) -> Verdict {
        if reread { self.rereads += 1 } else if shown_lines > 0 && useful_lines * 3 < shown_lines {
            self.wasted += 1;
        }
        // Score: fewer re-reads and less waste is better. Expressed so `Dwelling` can compare it.
        let score = 1.0 / (1.0 + self.rereads as f32 * 2.0 + self.wasted as f32);
        // A re-read counts double because it is the invisible failure — it looks like diligence,
        // where a wasted window at least looks like a wasted window.
        let candidate = if self.rereads > self.wasted {
            (self.width * 3 / 2).min(MAX_WIDTH)
        } else {
            (self.width * 2 / 3).max(MIN_WIDTH)
        };
        if candidate == self.width { return Verdict::HeldByMargin { challenger_gain: 0 } }
        let v = self.dwell.consider(score, score * 1.5, false);
        if v.switches() {
            self.width = candidate;
            self.rereads = 0;
            self.wasted = 0;
        }
        v
    }

    /// Reset to the purpose's default — for a new branch, where the previous branch's tuning is
    /// evidence about a different file.
    pub fn reset(&mut self) {
        self.width = self.purpose.default_width();
        self.rereads = 0;
        self.wasted = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn SURVEY_and_COMPREHEND_want_opposite_widths() {
        // One number cannot serve both and the average serves neither.
        assert!(Purpose::Survey.default_width() < Purpose::Anchor.default_width());
        assert!(Purpose::Anchor.default_width() < Purpose::Comprehend.default_width());
    }

    #[test]
    fn repeated_REREADS_widen_the_window() {
        // The important direction: a re-read is invisible as a failure because it looks like
        // diligence, so nothing else will report it.
        let mut t = Tuned::new(Purpose::Survey);
        let w0 = t.width();
        t.dwell = crate::dwell::Dwelling::new(std::time::Duration::from_secs(0), 0.0);
        for _ in 0..3 { t.observe(true, 10, 10); }
        assert!(t.width() > w0, "{} -> {}", w0, t.width());
    }

    #[test]
    fn WASTED_lines_narrow_it() {
        let mut t = Tuned::new(Purpose::Comprehend);
        let w0 = t.width();
        t.dwell = crate::dwell::Dwelling::new(std::time::Duration::from_secs(0), 0.0);
        for _ in 0..3 { t.observe(false, 1, 40); }
        assert!(t.width() < w0);
    }

    #[test]
    fn a_width_does_not_OSCILLATE_on_one_bad_read() {
        // dwell's hysteresis, reused. A single miss should not resize the window.
        let mut t = Tuned::new(Purpose::Anchor);
        let w0 = t.width();
        t.observe(true, 10, 10);
        assert_eq!(t.width(), w0, "held by the dwell");
    }

    #[test]
    fn the_width_stays_within_BOUNDS() {
        // Below the floor a window shows a line with no surroundings; above the ceiling it is cheaper
        // to read the whole file.
        let mut t = Tuned::new(Purpose::Survey);
        t.dwell = crate::dwell::Dwelling::new(std::time::Duration::from_secs(0), 0.0);
        for _ in 0..40 { t.observe(true, 10, 10); }
        assert!(t.width() <= MAX_WIDTH);
        for _ in 0..40 { t.observe(false, 1, 100); }
        assert!(t.width() >= MIN_WIDTH);
    }

    #[test]
    fn a_BRANCH_resets_to_the_purpose_default() {
        // The previous branch's tuning is evidence about a different file.
        let mut t = Tuned::new(Purpose::Comprehend);
        t.dwell = crate::dwell::Dwelling::new(std::time::Duration::from_secs(0), 0.0);
        for _ in 0..3 { t.observe(false, 1, 40); }
        assert_ne!(t.width(), Purpose::Comprehend.default_width());
        t.reset();
        assert_eq!(t.width(), Purpose::Comprehend.default_width());
    }
}

/// Planning a parallel pre-read.
///
/// **Summarising windows is the ideal fan workload**, and it is worth saying why rather than just
/// asserting it: each window is independent, so there is no ordering constraint; each is short, so a
/// small model suffices; and the work runs AHEAD of the main loop, so its latency hides entirely.
///
/// That is the opposite of the main decode loop, which is sequential, long, and latency-critical —
/// the fan has never had a workload that fits it this well.
///
/// ## The reduction is the point
///
/// A survey read of a large file can produce forty windows and four thousand lines. Sent whole, that
/// spends the thinker's context on material most of which it will discard. Sent as forty
/// two-line summaries with pointers, it costs two hundred and the thinker asks for the three it
/// wants in full.
///
/// **A summary that loses the line numbers is useless**, because the follow-up read is the whole
/// point of summarising.
/// **Named `ReadFanPlan`, not `FanPlan`, and that is not a style choice.**
///
/// `fanplan.rs` already has a `FanPlan` — with `kind`, `width`, `gib` and a reason line — and I built
/// this one without noticing. Rust permitted both because they live in different modules, which is
/// why the collision survived until `dupscan` was written and immediately found it.
///
/// They are genuinely different: that one plans INFERENCE lanes and their memory, this one splits
/// file windows across seats. Two different things wanting one name is the better half of the
/// duplicate problem — the worse half is two implementations of one thing, and telling them apart is
/// the point of looking.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ReadFanPlan {
    /// How many windows each seat summarises.
    pub per_seat: usize,
    /// Seats used. May be fewer than available — starting a seat costs more than it saves for one
    /// window.
    pub seats: usize,
    /// Windows handled inline rather than farmed out.
    pub inline: usize,
}

/// Below this, farming out costs more than doing it.
pub const MIN_WINDOWS_TO_FAN: usize = 6;

/// Split windows across fan seats.
pub fn fan_plan(windows: usize, seats_available: usize) -> ReadFanPlan {
    if windows < MIN_WINDOWS_TO_FAN || seats_available == 0 {
        // Dispatch, prompt and collect cost more than reading six windows directly. A plan that fans
        // everything looks parallel and is slower.
        return ReadFanPlan { per_seat: 0, seats: 0, inline: windows };
    }
    let seats = seats_available.min(windows);
    ReadFanPlan { per_seat: windows.div_ceil(seats), seats, inline: 0 }
}

impl ReadFanPlan {
    /// Is the work actually spread, or is this one seat wearing a plan?
    pub fn is_parallel(&self) -> bool { self.seats > 1 }

    pub fn explain(&self) -> String {
        if self.seats == 0 {
            return format!("{} windows read inline — below {MIN_WINDOWS_TO_FAN}, dispatch and \
collection cost more than the reading", self.inline);
        }
        format!("{} windows across {} seats, {} each — independent, short and ahead of the main \
loop, so the latency hides", self.per_seat * self.seats, self.seats, self.per_seat)
    }
}

#[cfg(test)]
mod fan_tests {
    use super::*;

    #[test]
    fn a_FEW_windows_are_read_inline() {
        // Dispatch, prompt and collect cost more than reading them. A plan that fans everything looks
        // parallel and is slower.
        let p = fan_plan(4, 5);
        assert!(!p.is_parallel());
        assert_eq!(p.inline, 4);
        assert!(p.explain().contains("cost more than the reading"));
    }

    #[test]
    fn MANY_windows_spread_across_seats() {
        let p = fan_plan(40, 5);
        assert!(p.is_parallel());
        assert_eq!((p.seats, p.per_seat), (5, 8));
    }

    #[test]
    fn seats_never_exceed_WINDOWS() {
        // Five seats for six windows is one window each and four idle dispatches.
        let p = fan_plan(6, 20);
        assert_eq!(p.seats, 6);
        assert_eq!(p.per_seat, 1);
    }

    #[test]
    fn with_NO_SEATS_everything_is_inline() {
        assert_eq!(fan_plan(100, 0).inline, 100);
    }

    #[test]
    fn the_explanation_says_why_the_latency_is_free() {
        // The reason this workload fits the fan where decode does not: independent, short, and ahead
        // of the loop that is waiting.
        assert!(fan_plan(40, 5).explain().contains("ahead of the main"));
    }
}
