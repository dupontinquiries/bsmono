//! delegateledger.rs — did delegating actually pay?
//!
//! `opsched` says how much work to commit to, `deviceplan` says which machine can take it, `fleet`
//! picks one. **Nothing checks afterwards.** A latency budget that is never compared against a
//! measured round trip is a constant with extra steps — the same shape as
//! `ASAHI_CONCURRENT_BUFFER_GIB`, which was one machine's number applied everywhere for months.
//!
//! ## The thing that makes delegation worth measuring
//!
//! **Delegating work that is slower remotely than locally is worse than not delegating**, and the
//! failure is invisible: the task completes, the result is correct, and the only symptom is that
//! everything took longer. Nobody investigates a success.
//!
//! ## What can and cannot be known
//!
//! The honest measurement is the round trip against the budget that was claimed for it. **What
//! cannot be known is the counterfactual** — how long it would have taken locally, since it was not
//! run locally. So this reports budget accuracy, not speedup, and says so rather than implying a
//! comparison it did not make.

use crate::deviceplan::Task;

/// One completed delegation.
#[derive(Clone, PartialEq, Debug)]
pub struct Record {
    pub device: String,
    pub task: Task,
    /// Seconds the budget allowed.
    pub budget_s: f32,
    /// Seconds it actually took, end to end.
    pub actual_s: f32,
    /// Did it produce a usable result? A fast failure is not a success.
    pub succeeded: bool,
}

impl Record {
    /// Ratio of actual to budget. Above 1 means the budget was optimistic.
    pub fn overrun(&self) -> f32 {
        if self.budget_s <= 0.0 { return f32::INFINITY }
        self.actual_s / self.budget_s
    }
}

/// What the ledger concludes about a device.
#[derive(Clone, PartialEq, Debug)]
pub enum Verdict {
    /// Too few records to say anything.
    Insufficient { have: usize, need: usize },
    /// Budgets are holding.
    Reliable { median_overrun: f32 },
    /// Consistently slower than budgeted.
    Optimistic { median_overrun: f32, worst: f32 },
    /// Failing often enough that speed is not the question.
    Unreliable { success_rate: f32 },
}

impl Verdict {
    pub fn explain(&self) -> String {
        match self {
            Verdict::Insufficient { have, need } => format!(
                "{have} of {need} records. Reporting a trend from fewer would be reading one slow \
network moment as a property of the machine"),
            Verdict::Reliable { median_overrun } => format!(
                "budgets hold — median {median_overrun:.2}x of the allowance. This says the \
BUDGET was accurate, not that delegating was faster than local: the local run did not happen, so \
there is no speedup to report"),
            Verdict::Optimistic { median_overrun, worst } => format!(
                "consistently over budget — median {median_overrun:.2}x, worst {worst:.2}x. The \
budget for this task on this device is wrong, and the fix is the budget rather than the device \
unless local is genuinely faster, which this ledger cannot tell you"),
            Verdict::Unreliable { success_rate } => format!(
                "{:.0}% success rate. Speed is not the question while this is true — a fast failure \
costs the round trip AND the retry", success_rate * 100.0),
        }
    }

    /// Should the fleet keep sending this device work?
    ///
    /// **Optimistic is not disqualifying.** A device that reliably takes 2x its budget is still
    /// useful; the budget is what is wrong. Unreliable is different — it wastes the round trip.
    pub fn keep_using(&self) -> bool { !matches!(self, Verdict::Unreliable { .. }) }
}

/// Records needed before a trend is reported.
pub const MIN_RECORDS: usize = 5;
/// Below this success rate the device is unreliable regardless of speed.
pub const MIN_SUCCESS: f32 = 0.7;

/// Judge a device from its history.
pub fn verdict(records: &[Record]) -> Verdict {
    if records.len() < MIN_RECORDS {
        return Verdict::Insufficient { have: records.len(), need: MIN_RECORDS };
    }
    let ok = records.iter().filter(|r| r.succeeded).count() as f32 / records.len() as f32;
    // Reliability is checked FIRST: a device that fails half its work has no meaningful overrun
    // statistic, because the failures did not run to completion.
    if ok < MIN_SUCCESS { return Verdict::Unreliable { success_rate: ok } }

    let mut over: Vec<f32> = records.iter().filter(|r| r.succeeded).map(|r| r.overrun()).collect();
    over.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // MEDIAN, not mean: one 30-second network stall should not condemn a device that is otherwise
    // inside its budget, and a mean over a handful of samples is exactly that sensitive.
    let median = over[over.len() / 2];
    let worst = *over.last().unwrap_or(&median);
    if median > 1.25 { Verdict::Optimistic { median_overrun: median, worst } }
    else { Verdict::Reliable { median_overrun: median } }
}

/// A corrected budget, from what actually happened.
///
/// Returns None when the current budget is holding — **changing a budget that works is churn**, and
/// a system that retunes on every sample stops being predictable.
pub fn suggest_budget(records: &[Record], current_s: f32) -> Option<f32> {
    match verdict(records) {
        Verdict::Optimistic { median_overrun, .. } => Some(current_s * median_overrun * 1.1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(actual: f32, budget: f32, ok: bool) -> Record {
        Record { device: "peer".into(), task: Task::BuildCheck,
                 budget_s: budget, actual_s: actual, succeeded: ok }
    }

    #[test]
    fn FEW_records_say_nothing_rather_than_guessing() {
        // One slow network moment is not a property of the machine.
        let v = verdict(&[rec(10.0, 5.0, true)]);
        assert!(matches!(v, Verdict::Insufficient { .. }));
        assert!(v.explain().contains("one slow network moment"));
    }

    #[test]
    fn RELIABILITY_is_judged_before_speed() {
        // A device failing half its work has no meaningful overrun statistic — the failures did not
        // run to completion.
        let rs: Vec<_> = (0..6).map(|i| rec(1.0, 10.0, i % 2 == 0)).collect();
        assert!(matches!(verdict(&rs), Verdict::Unreliable { .. }));
    }

    #[test]
    fn a_consistently_SLOW_device_is_kept_and_its_BUDGET_is_blamed() {
        // 2x its budget every time is still a useful machine; the budget is what is wrong.
        let rs: Vec<_> = (0..6).map(|_| rec(20.0, 10.0, true)).collect();
        let v = verdict(&rs);
        assert!(matches!(v, Verdict::Optimistic { .. }));
        assert!(v.keep_using());
        assert!(v.explain().contains("the fix is the budget rather than the device"));
    }

    #[test]
    fn an_UNRELIABLE_device_is_dropped_and_speed_stops_mattering() {
        let rs: Vec<_> = (0..6).map(|i| rec(1.0, 10.0, i == 0)).collect();
        let v = verdict(&rs);
        assert!(!v.keep_using());
        assert!(v.explain().contains("a fast failure costs the round trip AND the retry"));
    }

    #[test]
    fn the_MEDIAN_survives_one_outlier() {
        // A mean over six samples would be dragged over the threshold by a single stall.
        let mut rs: Vec<_> = (0..5).map(|_| rec(9.0, 10.0, true)).collect();
        rs.push(rec(300.0, 10.0, true));
        assert!(matches!(verdict(&rs), Verdict::Reliable { .. }));
    }

    #[test]
    fn a_RELIABLE_verdict_refuses_to_claim_a_SPEEDUP() {
        // The counterfactual is unavailable: the local run did not happen. Claiming one would be
        // inventing a comparison.
        let rs: Vec<_> = (0..6).map(|_| rec(8.0, 10.0, true)).collect();
        assert!(verdict(&rs).explain().contains("no speedup to report"));
    }

    #[test]
    fn a_budget_that_HOLDS_is_left_alone() {
        // Retuning on every sample stops the system being predictable.
        let rs: Vec<_> = (0..6).map(|_| rec(8.0, 10.0, true)).collect();
        assert_eq!(suggest_budget(&rs, 10.0), None);
    }

    #[test]
    fn an_OPTIMISTIC_budget_is_raised_with_headroom() {
        let rs: Vec<_> = (0..6).map(|_| rec(20.0, 10.0, true)).collect();
        let s = suggest_budget(&rs, 10.0).unwrap();
        assert!(s > 20.0 && s < 25.0, "got {s}");
    }

    #[test]
    fn a_ZERO_budget_does_not_divide_by_it() {
        assert!(rec(1.0, 0.0, true).overrun().is_infinite());
    }
}
