//! perfverdict.rs — turn setup/sync/compute into ONE named bottleneck, or say the split does not matter.
//!
//! A profile reporting "38% compute, 31% submit, 31% sync" is unactionable. The instinctive response is
//! to attack the largest, which is wrong twice over: the largest may be a symptom of another term, and
//! if the run is already at the memory-bandwidth roofline then none of them is the problem and every
//! hour spent on the split is wasted.
//!
//! ## The check that comes first, and is usually skipped
//!
//! Compute the bandwidth bound: active bytes per token divided by measured bandwidth. If observed token
//! time is near it, the run is at the physical limit and the profile split is IRRELEVANT — there is no
//! arrangement of setup and sync that beats the memory system. This is the roofline discipline applied
//! to our own profile, and it is the answer that stops work rather than directing it.
//!
//! ## Why sync time cannot be read directly
//!
//! Sync is time spent waiting, and waiting has three causes that need opposite responses:
//!
//! * the GPU is genuinely busy — this is compute wearing a different label, and reducing it means
//!   reducing work, not waiting differently;
//! * the driver stalled — a bug, and batching harder makes it worse;
//! * the GPU went idle because we submitted too rarely — pacing, and the fix is the opposite of the
//!   fix for a stall.
//!
//! `sync_per_submit` discriminates: divide sync by submits and compare against how long one batch of
//! work should take. Near it, the waiting is healthy. Far above, something is stalling.

/// One generation's measured profile.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub tokens: u64,
    pub wall_ms: f64,
    /// Encoder/bind-group construction on the host.
    pub setup_ms: f64,
    /// Blocking waits.
    pub sync_ms: f64,
    pub submits: u64,
    /// Weight bytes the model actually reads per token — for an MoE, dense plus routed experts.
    pub active_bytes: u64,
    /// Measured device bandwidth, from `--gate-perf`.
    pub gib_per_s: f64,
}

impl Sample {
    pub fn ms_per_token(&self) -> f64 {
        if self.tokens == 0 { 0.0 } else { self.wall_ms / self.tokens as f64 }
    }
    /// The floor: how fast a token could possibly be, given the bytes it must read.
    pub fn bandwidth_bound_ms(&self) -> f64 {
        if self.gib_per_s <= 0.0 { return 0.0; }
        (self.active_bytes as f64 / (1u64 << 30) as f64) / self.gib_per_s * 1000.0
    }
    /// How close the run is to the physical limit. 1.0 means at it.
    pub fn efficiency(&self) -> f64 {
        let b = self.bandwidth_bound_ms();
        if b <= 0.0 || self.ms_per_token() <= 0.0 { return 0.0; }
        b / self.ms_per_token()
    }
    pub fn submits_per_token(&self) -> f64 {
        if self.tokens == 0 { 0.0 } else { self.submits as f64 / self.tokens as f64 }
    }
    /// Waiting per submit. Healthy when it approximates the time one batch of work takes.
    pub fn sync_per_submit_ms(&self) -> f64 {
        if self.submits == 0 { 0.0 } else { self.sync_ms / self.submits as f64 }
    }
    /// Wall time neither setup nor sync accounts for.
    ///
    /// Went dead when `Underfed` became `FewSubmitsStillSlow` — the old branch tested
    /// `compute_ms() < sync_ms` and the new one does not. Kept and made public rather than deleted,
    /// because it is the term that matters most and the one the profile CANNOT see into: on every run
    /// measured it is 99%+ of wall time, and calling that "GPU execution" was the overstatement fixed
    /// in _399.
    ///
    /// Naming it `unaccounted_ms` rather than `compute_ms` is the point. The old name asserted what the
    /// time was spent on; the new one says only that we do not know, which is the truth and is what
    /// stops the next reader concluding the GPU was busy.
    pub fn unaccounted_ms(&self) -> f64 { (self.wall_ms - self.setup_ms - self.sync_ms).max(0.0) }

    /// Fraction of wall time nothing accounts for, 0..1.
    pub fn unaccounted_share(&self) -> f64 {
        if self.wall_ms <= 0.0 { return 0.0; }
        self.unaccounted_ms() / self.wall_ms
    }
}

/// What is actually limiting this run.
#[derive(Clone, PartialEq, Debug)]
pub enum Verdict {
    /// At the memory roofline. The profile split does not matter and optimizing it will not help.
    AtBandwidthLimit { efficiency: f64 },
    /// Host-side dispatch overhead. Many submits, much setup.
    DispatchBound { submits_per_token: f64, setup_share: f64 },
    /// Waiting far longer per submit than the work should take.
    Stalling { sync_per_submit_ms: f64 },
    /// Few submits and still slow. The GPU is either running slow kernels or idling between batches,
    /// and THESE COUNTERS CANNOT TELL WHICH — naming one would be a guess wearing a verdict's clothes.
    FewSubmitsStillSlow { submits_per_token: f64, efficiency: f64 },
    /// Executing, and below the roofline for a reason the profile cannot see.
    ComputeBoundBelowRoofline { efficiency: f64 },
}

impl Verdict {
    /// The one line an operator should act on.
    pub fn advice(&self) -> String {
        match self {
            Verdict::AtBandwidthLimit { efficiency } => format!(
                "at {:.0}% of the memory-bandwidth limit — the compute/submit/sync split is IRRELEVANT \
here. Going faster needs fewer bytes per token (smaller quant, fewer active experts), not a different \
arrangement of the same work", efficiency * 100.0),
            Verdict::DispatchBound { submits_per_token, setup_share } => format!(
                "{submits_per_token:.0} submits/token with {:.0}% of wall time in encoder setup — \
host-side dispatch is the limit. Batch more passes per submit; the GPU is waiting on us",
                setup_share * 100.0),
            Verdict::Stalling { sync_per_submit_ms } => format!(
                "{sync_per_submit_ms:.1} ms of waiting per submit, far more than a batch of work should \
take — something is stalling. Batching HARDER makes this worse, which is the opposite of the \
dispatch-bound fix"),
            Verdict::FewSubmitsStillSlow { submits_per_token, efficiency } => format!(
                "{submits_per_token:.1} submits/token at {:.0}% of the bandwidth limit, with no stall \
signature. Either the kernels are slow or the GPU idles between batches, and THIS PROFILE CANNOT TELL \
WHICH. A per-shape --gate-perf separates them: kernels at full bandwidth in isolation means the gaps \
are the problem", efficiency * 100.0),
            Verdict::ComputeBoundBelowRoofline { efficiency } => format!(
                "executing at {:.0}% of the bandwidth limit with no dispatch or stall signature — the \
gap is in the kernels themselves, and the profile cannot see which. --gate-perf per shape is the next \
measurement", efficiency * 100.0),
        }
    }
}

/// Near enough to the roofline that the split stops mattering.
pub const AT_LIMIT: f64 = 0.75;
/// Submits per token above which host dispatch dominates.
pub const DISPATCH_HEAVY: f64 = 200.0;
/// Submits per token below which the GPU may be starving.
pub const TOO_FEW: f64 = 2.0;

/// Name the bottleneck.
///
/// Ordered deliberately: the roofline check runs FIRST, because a run at the physical limit has no
/// bottleneck to fix and every later branch would name one anyway.
pub fn verdict(s: &Sample, expected_batch_ms: f64) -> Verdict {
    let eff = s.efficiency();
    if eff >= AT_LIMIT { return Verdict::AtBandwidthLimit { efficiency: eff }; }

    let spt = s.submits_per_token();
    let setup_share = if s.wall_ms > 0.0 { s.setup_ms / s.wall_ms } else { 0.0 };

    // Stall before dispatch: a stalling run also shows many submits, and calling it dispatch-bound
    // would prescribe batching, which makes a stall worse.
    let sps = s.sync_per_submit_ms();
    if s.submits > 0 && expected_batch_ms > 0.0 && sps > expected_batch_ms * 4.0 {
        return Verdict::Stalling { sync_per_submit_ms: sps };
    }
    if spt >= DISPATCH_HEAVY && setup_share > 0.2 {
        return Verdict::DispatchBound { submits_per_token: spt, setup_share };
    }
    // Few submits, no stall signature, still below the roofline. Two causes, indistinguishable here.
    if spt > 0.0 && spt < TOO_FEW {
        return Verdict::FewSubmitsStillSlow { submits_per_token: spt, efficiency: eff };
    }
    Verdict::ComputeBoundBelowRoofline { efficiency: eff }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(tokens: u64, wall: f64, setup: f64, sync: f64, submits: u64, active_gib: f64) -> Sample {
        Sample { tokens, wall_ms: wall, setup_ms: setup, sync_ms: sync, submits,
                 active_bytes: (active_gib * (1u64 << 30) as f64) as u64, gib_per_s: 258.1 }
    }

    #[test]
    fn the_unaccounted_share_is_the_number_that_MATTERS() {
        // Every run measured this session put 99%+ of wall time here — setup 19ms and sync 0ms of
        // 18292ms on DeepSeek-8B. Calling that "GPU-execution-bound" was the overstatement fixed in
        // _399, and the honest reading is that nothing in the profile can see where the time went.
        let s = Sample { tokens: 81, wall_ms: 18292.0, setup_ms: 19.0, sync_ms: 0.0,
                         submits: 610, active_bytes: 0, gib_per_s: 0.0 };
        assert!(s.unaccounted_share() > 0.99, "got {:.4}", s.unaccounted_share());
        assert!((s.unaccounted_ms() - 18273.0).abs() < 1.0);
    }

    #[test]
    fn a_zero_length_run_reports_no_share_rather_than_dividing() {
        let s = Sample { tokens: 0, wall_ms: 0.0, setup_ms: 0.0, sync_ms: 0.0,
                         submits: 0, active_bytes: 0, gib_per_s: 0.0 };
        assert_eq!(s.unaccounted_share(), 0.0);
    }

    #[test]
    fn a_run_at_the_roofline_is_told_the_split_DOES_NOT_MATTER() {
        // The check that comes first and is usually skipped. 10.5 GiB at 258 GiB/s is ~40.7 ms, so a
        // 45 ms token is near the limit and no rearrangement of setup and sync beats the memory system.
        let v = verdict(&s(100, 4500.0, 1400.0, 1400.0, 5000, 10.5), 1.0);
        assert!(matches!(v, Verdict::AtBandwidthLimit { .. }), "{v:?}");
        assert!(v.advice().contains("IRRELEVANT"));
        assert!(v.advice().contains("fewer bytes per token"), "and says what WOULD help");
    }

    #[test]
    fn a_stall_is_named_before_dispatch_because_the_fixes_are_opposite() {
        // A stalling run also shows many submits. Calling it dispatch-bound would prescribe batching,
        // and batching harder makes a stall worse — so the order of these branches is load-bearing.
        let v = verdict(&s(10, 20000.0, 500.0, 18000.0, 3000, 10.5), 0.5);
        assert!(matches!(v, Verdict::Stalling { .. }), "{v:?}");
        assert!(v.advice().contains("opposite of the"));
    }

    #[test]
    fn many_submits_with_heavy_setup_is_dispatch_bound() {
        // The classic: the GPU is waiting on us. 1100 submits/token with a third of wall time in
        // encoder construction.
        let v = verdict(&s(10, 22000.0, 8000.0, 2000.0, 11000, 10.5), 5.0);
        assert!(matches!(v, Verdict::DispatchBound { .. }), "{v:?}");
        assert!(v.advice().contains("Batch more passes"));
    }

    #[test]
    fn few_submits_and_slow_ADMITS_the_profile_cannot_tell_which() {
        // Written as Underfed first, and the test caught a real ambiguity: an underfed run and a
        // stalling one look IDENTICAL from sync-per-submit. Rather than invent a discriminator these
        // counters cannot support, the verdict names both causes and the measurement that separates
        // them. A guess wearing a verdict's clothes is worse than an admitted gap, because it directs
        // work confidently in a direction nothing tested.
        let v = verdict(&s(100, 8000.0, 100.0, 4000.0, 100, 10.5), 45.0);
        assert!(matches!(v, Verdict::FewSubmitsStillSlow { .. }), "{v:?}");
        assert!(v.advice().contains("CANNOT TELL WHICH"));
        assert!(v.advice().contains("--gate-perf separates them"), "and names what would");
    }

    #[test]
    fn a_run_below_the_roofline_with_no_signature_says_the_profile_cannot_see_it() {
        // The honest verdict. Admitting the profile cannot answer is more useful than naming whichever
        // term happened to be largest.
        let v = verdict(&s(100, 20000.0, 1000.0, 1000.0, 500, 1.0), 20.0);
        assert!(matches!(v, Verdict::ComputeBoundBelowRoofline { .. }), "{v:?}");
        assert!(v.advice().contains("profile cannot see which"));
    }

    #[test]
    fn the_REAL_deepseek_run_gets_an_actionable_verdict_without_bandwidth() {
        // Measured on an M2 Max: 81 tokens in 18.3s, 610 submits, setup 19ms, sync 0ms. The profile
        // line called it "GPU-execution-bound, 0.1% accounted", which says where the time ISN'T.
        //
        // With active_bytes and gib_per_s unavailable the roofline cannot be evaluated, so
        // AtBandwidthLimit is unreachable — the verdict that would say STOP. What remains is still
        // useful: 7.5 submits/token is neither dispatch-heavy nor starving, and sync is zero, so the
        // honest answer is that the profile cannot see it and --gate-perf is the next measurement.
        let s = Sample { tokens: 81, wall_ms: 18_300.0, setup_ms: 19.0, sync_ms: 0.0,
                         submits: 610, active_bytes: 0, gib_per_s: 0.0 };
        assert!((s.submits_per_token() - 7.5).abs() < 0.1);
        let v = verdict(&s, 30.0);
        assert!(matches!(v, Verdict::ComputeBoundBelowRoofline { .. }), "{v:?}");
        assert!(v.advice().contains("profile cannot see which"));
        assert!(v.advice().contains("--gate-perf"), "and names the measurement that would");
    }

    #[test]
    fn efficiency_is_zero_rather_than_infinite_when_bandwidth_is_unknown() {
        // gib_per_s = 0 means --gate-perf never ran. Dividing by it would produce a confident verdict
        // from no measurement, which is the shape this project keeps getting caught by.
        let mut x = s(10, 1000.0, 100.0, 100.0, 100, 10.0);
        x.gib_per_s = 0.0;
        assert_eq!(x.bandwidth_bound_ms(), 0.0);
        assert_eq!(x.efficiency(), 0.0);
    }

    #[test]
    fn a_zero_token_sample_does_not_divide_by_zero() {
        let x = s(0, 0.0, 0.0, 0.0, 0, 1.0);
        assert_eq!(x.ms_per_token(), 0.0);
        assert_eq!(x.submits_per_token(), 0.0);
        assert_eq!(x.sync_per_submit_ms(), 0.0);
    }
}
