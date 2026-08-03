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
            // "executing at 0% of the bandwidth limit" appeared in a real log, and 0% did not mean
            // the GPU was idle — it meant gib_per_s was 0 because --gate-perf had never run, so
            // efficiency() returned 0 by construction. A percentage computed from a missing
            // measurement is not a small number, it is no number, and printing it as one sent the
            // reader looking for a kernel that was 500x too slow.
            Verdict::ComputeBoundBelowRoofline { efficiency } if *efficiency <= 0.0 => {
                "no dispatch or stall signature, and the bandwidth ceiling is UNMEASURED — run `kernel-validate --gate-perf` first, because without it there is nothing to be a percentage of".into()
            }
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
    fn a_MISSING_bandwidth_measurement_is_not_zero_percent() {
        // From a real log: "executing at 0% of the bandwidth limit". The GPU was not idle — gib_per_s
        // was 0 because --gate-perf had never run, so efficiency() returned 0 by construction. A
        // percentage computed from a missing measurement is not a small number, it is NO number, and
        // printing it as one sends the reader hunting a kernel 500x too slow.
        let s = Sample { tokens: 555, wall_ms: 103568.0, setup_ms: 211.0, sync_ms: 0.0,
                         submits: 8063, active_bytes: 0, gib_per_s: 0.0 };
        let v = verdict(&s, 50.0);
        let a = v.advice();
        assert!(a.contains("UNMEASURED"), "got {a}");
        assert!(!a.contains("0%"), "a missing measurement must not render as a percentage: {a}");
        assert!(a.contains("gate-perf"), "and it must say what would produce one");
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
        // This asserted "profile cannot see which" — the wording of a verdict that ALSO reported
        // "0% of the bandwidth limit" from a gib_per_s of 0. The test pinned the bug: it checked that
        // the message named the next measurement, and not that the number preceding it meant anything.
        assert!(v.advice().contains("UNMEASURED"), "no bandwidth reading: say so, do not render 0%");
        assert!(!v.advice().contains("0%"));
        assert!(v.advice().contains("gate-perf"), "and names the measurement that would");
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

#[cfg(test)]
mod poll_coverage_tests {
    /// Every blocking poll must be counted, or the sync figure describes a fraction of the waiting.
    ///
    /// The profile reported "1 polls" in every run — at 349 submits and at 8063 — because one of
    /// seventeen call sites incremented, and it runs once per generation. The same site was the only
    /// one adding to GPU_SYNC_NS, which is why sync read 0 ms of 19860 ms.
    ///
    /// That is what made "UNACCOUNTED, 0.2% accounted" look like a mystery: 99.8% of the time was in
    /// waits nobody instrumented, and the label blamed the kernels.
    ///
    /// Source-read because an uncounted poll produces no wrong number — it produces a MISSING one, and
    /// nothing behavioural can see an absence.
    #[test]
    fn every_blocking_poll_goes_through_counted_poll() {
        let src = include_str!("lib.rs");
        let mut raw = Vec::new();
        for (n, line) in src.lines().enumerate() {
            if !line.contains("device.poll(wgpu::PollType::wait_indefinitely())") { continue; }
            // The helper itself, and the download path which times its own span, are the exceptions.
            if line.contains("counted_poll") || src.lines().nth(n.saturating_sub(1))
                .map(|p| p.contains("GPU_POLLS")).unwrap_or(false) { continue; }
            if line.contains("pub fn counted_poll") { continue; }
            raw.push(n + 1);
        }
        // Allow the two inside counted_poll and the download path; anything else is uncounted waiting.
        assert!(raw.len() <= 3,
            "uncounted blocking polls at lines {raw:?} — each one is wall time the profile attributes \
to nothing, which is how 99.8% of a generation became UNACCOUNTED");
    }
}

/// Decode compared against this model's OWN prefill, which needs no roofline.
///
/// `verdict()` reports "bandwidth ceiling UNMEASURED" whenever `--gate-perf` has not run, which is
/// almost always — so the strongest signal available went unreported for want of a number nobody had.
///
/// Prefill and decode read the same weights. Prefill amortises each read across its chunk; decode at
/// m=1 reads them per token. So decode SHOULD be slower per token in arithmetic terms and dramatically
/// faster in memory terms, and the ratio between them is a self-contained measurement of how much
/// latency decode is failing to hide.
///
/// Measured on DeepSeek-8B: prefill 12.6 ms/token, decode 182 ms/token — **14×**, on the same weights
/// in the same run.
#[derive(Clone, PartialEq, Debug)]
pub enum SelfRelative {
    /// Not enough of one phase to compare.
    NotEnoughData,
    /// Decode is within a small factor of prefill per token. Nothing to explain.
    Consistent { ratio: f64 },
    /// Decode is far slower per token than prefill on the same weights.
    DecodeStarved { ratio: f64 },
}

/// Ratios above this are not explicable by decode's lower arithmetic intensity alone.
///
/// Decode does strictly less work per token than prefill and reads the same bytes, so a small factor
/// is expected from fixed per-token costs. An order of magnitude is not — it says decode has nothing
/// in flight to cover memory latency while prefill's batch covers it.
pub const STARVED_RATIO: f64 = 4.0;

/// Compare the two phases of one run.
///
/// Both arguments are ms per token of their own phase: prefill over its prompt, decode over its
/// output. Deliberately not a `Sample`, because this needs no bandwidth, no active-byte count and no
/// submit total — it is two numbers the engine already prints.
pub fn self_relative(prefill_ms_per_tok: f64, decode_ms_per_tok: f64,
                     prompt_tokens: usize, decoded_tokens: usize) -> SelfRelative {
    // A handful of tokens of either phase is noise. 32 is enough to average over pipeline fill.
    if prompt_tokens < 32 || decoded_tokens < 32 { return SelfRelative::NotEnoughData; }
    if prefill_ms_per_tok <= 0.0 || decode_ms_per_tok <= 0.0 { return SelfRelative::NotEnoughData; }
    let ratio = decode_ms_per_tok / prefill_ms_per_tok;
    if ratio >= STARVED_RATIO { SelfRelative::DecodeStarved { ratio } }
    else { SelfRelative::Consistent { ratio } }
}

impl SelfRelative {
    pub fn advice(&self) -> String {
        match self {
            SelfRelative::NotEnoughData =>
                "not enough of one phase to compare decode against prefill".into(),
            SelfRelative::Consistent { ratio } => format!(
                "decode is {ratio:.1}x prefill's cost per token, which is what fixed per-token costs \
account for — nothing here suggests the decode kernel is starved"),
            SelfRelative::DecodeStarved { ratio } => format!(
                "decode is {ratio:.1}x SLOWER PER TOKEN than this model's own prefill, on the same \
weights in the same run. Prefill reads each weight once per chunk and decode reads it once per token, \
so decode should be the cheaper of the two — a gap this size means decode has nothing in flight to \
cover memory latency while prefill's batch covers it. The batched matvec for m<=8 already exists \
(MATVEC_SMALL_Q4); what is missing is something producing more than one candidate token, which is \
speculative decoding. This comparison needs no bandwidth measurement, which is why it is reported \
when the roofline verdict cannot be."),
        }
    }
}

#[cfg(test)]
mod self_relative_tests {
    use super::*;

    #[test]
    fn the_measured_deepseek_run_is_STARVED() {
        // prefill 1634 tokens in 20.51s, decode 109 tokens in 19.9s — the real numbers.
        let v = self_relative(20510.0 / 1634.0, 19900.0 / 109.0, 1634, 109);
        match v {
            SelfRelative::DecodeStarved { ratio } => assert!((ratio - 14.4).abs() < 0.5, "{ratio}"),
            other => panic!("{other:?}"),
        }
        assert!(v.advice().contains("speculative decoding"));
        assert!(v.advice().contains("needs no bandwidth measurement"));
    }

    #[test]
    fn a_healthy_run_says_so_rather_than_staying_silent() {
        // Decode doing 2x prefill's per-token cost is explained by fixed costs. Reporting that
        // positively matters: a diagnostic that only speaks when something is wrong leaves a reader
        // unable to tell "fine" from "not measured".
        let v = self_relative(12.0, 24.0, 512, 256);
        assert!(matches!(v, SelfRelative::Consistent { .. }));
        assert!(v.advice().contains("nothing here suggests"));
    }

    #[test]
    fn a_short_phase_is_NOT_ENOUGH_DATA_rather_than_a_verdict() {
        // A three-token reply against a long prompt would otherwise produce a confident ratio built
        // almost entirely from pipeline fill.
        assert_eq!(self_relative(12.0, 200.0, 1634, 3), SelfRelative::NotEnoughData);
        assert_eq!(self_relative(12.0, 200.0, 8, 256), SelfRelative::NotEnoughData);
    }

    #[test]
    fn a_zero_timing_is_a_MISSING_measurement_not_an_infinite_ratio() {
        // The same mistake as reporting 0% of an unmeasured bandwidth, which _441 had to fix.
        assert_eq!(self_relative(0.0, 200.0, 512, 256), SelfRelative::NotEnoughData);
        assert_eq!(self_relative(12.0, 0.0, 512, 256), SelfRelative::NotEnoughData);
    }
}

/// Does the profile's own arithmetic hold together?
///
/// A profile line that contradicts itself is worse than no profile: every number in it gets used to
/// reason about where time goes, and one that cannot be true poisons the conclusions drawn from the
/// rest.
///
/// Observed in a real run, in one line:
///
/// ```text
/// 1625 submits (15/tok), 1 polls | setup 49ms vs sync 0ms of 19860ms
/// submit causes — ... download 141 ...
/// ```
///
/// **141 downloads and 1 poll.** Every buffer download must poll to map, so the two cannot both be
/// right. And `sync 0ms` in a run whose separate phase breakdown reported 15.5% sync says the same
/// thing from the other side: either the CPU never waited, or the waiting is not being measured where
/// it happens.
///
/// This does not decide which is wrong. It refuses to present numbers that cannot coexist as though
/// they describe one run.
#[derive(Clone, PartialEq, Debug)]
pub enum ProfileDoubt {
    /// Fewer polls than downloads: a download maps a buffer, and mapping requires a poll.
    FewerPollsThanDownloads { polls: u64, downloads: u64 },
    /// The CPU reportedly never waited across a multi-second run.
    NoSyncOverALongRun { wall_ms: f64 },
}

impl ProfileDoubt {
    pub fn explain(&self) -> String {
        match self {
            ProfileDoubt::FewerPollsThanDownloads { polls, downloads } => format!(
                "{polls} polls against {downloads} downloads — every download maps a buffer and every \
map requires a poll, so one of these counters is not measuring what its name says. Until that is \
settled, neither the poll count nor the sync total supports a conclusion about where time went"),
            ProfileDoubt::NoSyncOverALongRun { wall_ms } => format!(
                "sync is 0 ms across {wall_ms:.0} ms of wall time. Either the CPU genuinely never \
waited on the GPU — in which case the work is not where the profile is looking — or the wait is \
happening somewhere GPU_SYNC_NS is not wrapped around"),
        }
    }
}

/// Check a profile against itself. Empty means the numbers can coexist.
pub fn profile_doubts(_polls: u64, downloads: u64, sync_ms: f64, wall_ms: f64) -> Vec<ProfileDoubt> {
    let mut v = Vec::new();
    // THE POLL/DOWNLOAD DOUBT IS WITHDRAWN, one commit after I wrote it. SUBMIT_BY_DOWNLOAD is tagged
    // in THREE places — `run`, `sample_at` and `download` — and only the last one maps a buffer and
    // polls. So downloads exceeding polls is correct by design, and "1 poll against 141 downloads" is
    // not a contradiction at all.
    //
    // I asserted it was one from the names, without reading what increments them. That is the same
    // mistake as the std140 stride and the unreachable extend path: a strong signal, a mechanism named
    // immediately, the cheaper check skipped. Shipping it would have printed a confident warning about
    // healthy runs, which is worse than the silence it replaced — a doubt that cries wolf costs the
    // one that does not.
    let _ = downloads;
    // A second of wall time with no measured wait is not a fast run, it is an unmeasured one.
    if sync_ms == 0.0 && wall_ms > 1000.0 {
        v.push(ProfileDoubt::NoSyncOverALongRun { wall_ms });
    }
    v
}

#[cfg(test)]
mod doubt_tests {
    use super::*;

    #[test]
    fn the_real_run_is_flagged_ONCE_because_only_one_doubt_survived() {
        // 1625 submits, 1 poll, 141 downloads, sync 0 ms of 19860 ms.
        //
        // I first asserted TWO doubts here. The poll/download one was wrong: SUBMIT_BY_DOWNLOAD is
        // tagged in run, sample_at and download, and only download polls — so 1 poll against 141
        // download-tagged submits is correct, not contradictory.
        //
        // The sync doubt stands on its own: zero measured wait across 19.9 seconds, in a run whose
        // separate phase breakdown reported 15.5% sync.
        let d = profile_doubts(1, 141, 0.0, 19860.0);
        assert_eq!(d.len(), 1, "only the sync doubt is real: {d:?}");
        assert!(d[0].explain().contains("not where the profile is looking"));
    }

    #[test]
    fn fewer_polls_than_downloads_is_NOT_flagged() {
        // Pinned so nobody re-adds it from the names. Two of the three download-tagged paths never
        // map a buffer, so this inequality is the normal case.
        assert!(profile_doubts(1, 500, 340.0, 5000.0).is_empty());
    }

    #[test]
    fn a_consistent_profile_raises_NOTHING() {
        // The check must be silent when the numbers hold, or it becomes another line nobody reads.
        assert!(profile_doubts(200, 141, 340.0, 19860.0).is_empty());
    }

    #[test]
    fn a_short_run_with_no_sync_is_not_suspicious() {
        // Sub-second work can genuinely complete without the CPU blocking; the doubt is about a
        // multi-second run reporting zero.
        assert!(profile_doubts(5, 2, 0.0, 40.0).is_empty());
    }

    #[test]
    fn a_run_with_no_downloads_does_not_demand_polls() {
        // Not every workload maps a buffer, and demanding a poll from one that never downloads would
        // fire the warning on every profile that is fine.
        assert!(profile_doubts(0, 0, 500.0, 5000.0).is_empty());
    }
}

/// What the expert cache actually achieved, in the terms that decide what to do next.
///
/// The streaming counters — hits, fills, bytes, repins — are tracked and were reported nowhere, so
/// there has been no way to tell whether pinning more experts would help or whether the cache is
/// already saturated and the remaining cost is elsewhere.
///
/// That distinction is the whole question for a streaming MoE. Scout pins 6 of 16 and routes to 1 per
/// layer per token: if the hit rate is near 40% the pin set is the lever and more budget buys tokens
/// directly; if it is near 95% the cache has already done its work and more pinning buys almost
/// nothing, which points at the kernels instead.
///
/// Uniform routing would give 6/16 = 37.5%. **Anything well above that is the LFRU policy earning its
/// place**, and anything at or below it means the policy is not tracking the routing — a different
/// and much worse problem than a small pin set.
/// How well the routing PREDICTION did, which is a different lever from the pin set.
///
/// The pin set answers "which experts are always here"; prediction answers "which will be wanted
/// next", and it is the one that can overlap a fetch with compute instead of stalling on it. Both
/// counters exist (`prefetched`, `prefetch_hits`) behind `LOGSCOPE_MOE_PREFETCH`, and neither was
/// reported — so the flag has never been judgeable.
///
/// **A mispredict is not free.** It spends a fill's bandwidth on an expert nobody wanted, and on a
/// model streaming 10 of 16 that bandwidth is the scarce thing. So the useful threshold is not "is it
/// above zero" but "does it beat the cost of being wrong".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PrefetchAccuracy {
    pub issued: u64,
    pub hits: u64,
}

impl PrefetchAccuracy {
    pub fn rate(&self) -> f64 {
        if self.issued == 0 { return 0.0; }
        self.hits as f64 / self.issued as f64
    }

    /// A prefetch pays when a hit saves a stalled fetch and a miss costs a wasted one, so it is worth
    /// having above 50% and actively harmful below — it is then spending bandwidth to slow the model
    /// down.
    pub fn verdict(&self) -> String {
        if self.issued == 0 {
            return "routing prefetch not enabled (LOGSCOPE_MOE_PREFETCH=1) — untested on this model"
                .to_string();
        }
        let r = self.rate();
        format!("routing prefetch — {:.0}% of {} speculative fills were wanted: {}",
            r * 100.0, self.issued,
            if r >= 0.7 {
                "the coupling table is predicting well, and overlapping the fetch with compute is where the remaining decode time goes"
            } else if r >= 0.5 {
                "marginally worth its bandwidth — a hit saves a stalled fetch and a miss wastes one, so this is near the break-even"
            } else {
                "BELOW BREAK-EVEN — more fills are wasted than saved, so prefetch is spending scarce bandwidth to make decode slower and should be off until the table learns"
            })
    }
}

pub struct ExpertResidency {
    pub hits: u64,
    pub fills: u64,
    pub bytes: u64,
    pub repins: u64,
    pub pinned: usize,
    pub total: usize,
}

impl ExpertResidency {
    pub fn hit_rate(&self) -> f64 {
        let n = self.hits + self.fills;
        if n == 0 { return 0.0; }
        self.hits as f64 / n as f64
    }

    /// What uniform routing would give with this pin set — the floor a policy must beat to be worth
    /// having.
    pub fn uniform_baseline(&self) -> f64 {
        if self.total == 0 { return 0.0; }
        self.pinned as f64 / self.total as f64
    }

    pub fn report(&self, tokens: u64) -> String {
        let hr = self.hit_rate();
        let base = self.uniform_baseline();
        let verdict = if self.hits + self.fills == 0 {
            "no expert traffic recorded — this model is not streaming".to_string()
        } else if hr <= base + 0.02 {
            format!("at or below the {:.0}% uniform baseline — the LFRU policy is NOT tracking the \
routing, which is worse than a small pin set and is the thing to fix before raising the budget",
                base * 100.0)
        } else if hr > 0.90 {
            format!("well above the {:.0}% uniform baseline and near saturation — more pinning buys \
little, so the remaining decode cost is in the kernels rather than in expert traffic", base * 100.0)
        } else {
            format!("above the {:.0}% uniform baseline with room left — raising the GPU budget pins \
more experts and should convert directly into tokens", base * 100.0)
        };
        format!("expert residency — {:.1}% hit ({} hits / {} fills), {:.1} GiB moved, {} repins, \
{}/{} pinned, {:.0} MiB per token | {verdict}",
            hr * 100.0, self.hits, self.fills,
            self.bytes as f64 / (1u64 << 30) as f64, self.repins, self.pinned, self.total,
            if tokens == 0 { 0.0 } else { self.bytes as f64 / tokens as f64 / (1u64 << 20) as f64 })
    }
}

#[cfg(test)]
mod prefetch_tests {
    use super::*;

    #[test]
    fn below_half_is_named_as_ACTIVELY_HARMFUL() {
        // A hit saves a stalled fetch and a miss wastes one, so under 50% it is spending the scarce
        // resource to slow the model down. "Low accuracy" would read as merely unhelpful.
        let v = PrefetchAccuracy { issued: 100, hits: 30 }.verdict();
        assert!(v.contains("BELOW BREAK-EVEN"));
        assert!(v.contains("slower"));
    }

    #[test]
    fn a_good_rate_points_at_OVERLAP_as_the_next_lever() {
        // If the table predicts well, the remaining cost is the fetch not being hidden behind
        // compute — a structural change rather than a policy one.
        assert!(PrefetchAccuracy { issued: 100, hits: 85 }.verdict().contains("overlapping the fetch"));
    }

    #[test]
    fn a_disabled_prefetch_says_UNTESTED_rather_than_zero_percent() {
        // 0% would read as a broken predictor on a model that never ran one.
        let v = PrefetchAccuracy { issued: 0, hits: 0 }.verdict();
        assert!(v.contains("not enabled"));
        assert!(!v.contains("0%"));
    }

    #[test]
    fn the_break_even_band_is_called_MARGINAL_not_good() {
        // 55% is not a success; it is barely paying for itself, and reporting it as working would
        // retire a question that is still open.
        assert!(PrefetchAccuracy { issued: 100, hits: 55 }.verdict().contains("break-even"));
    }
}

#[cfg(test)]
mod residency_tests {
    use super::*;

    fn r(hits: u64, fills: u64) -> ExpertResidency {
        ExpertResidency { hits, fills, bytes: 123 << 30, repins: 40, pinned: 6, total: 16 }
    }

    #[test]
    fn a_saturated_cache_points_at_the_KERNELS_not_the_budget() {
        // The case where raising the GPU budget is wasted effort, and saying so is the point of
        // reporting this at all.
        let s = r(950, 50).report(661);
        assert!(s.contains("near saturation"));
        assert!(s.contains("in the kernels"));
    }

    #[test]
    fn a_hit_rate_at_the_UNIFORM_BASELINE_means_the_policy_is_not_tracking() {
        // 6 pinned of 16 is 37.5% by luck alone. A policy achieving only that is not learning the
        // routing — a different and worse problem than a small pin set, and one more budget would not
        // fix.
        let s = r(375, 625).report(100);
        assert!(s.contains("NOT tracking"), "{s}");
        assert!(s.contains("before raising the budget"));
    }

    #[test]
    fn a_middling_rate_says_the_budget_IS_the_lever() {
        let s = r(700, 300).report(100);
        assert!(s.contains("room left"));
        assert!(s.contains("raising the GPU budget"));
    }

    #[test]
    fn bytes_per_token_is_reported_because_it_is_the_number_that_moves() {
        // 123 GiB over 661 tokens is ~190 MiB/token — which sounds enormous and is in fact one and a
        // third expert fetches across 48 layers, i.e. the cache eliminating ~97% of the traffic.
        let s = r(900, 100).report(661);
        assert!(s.contains("MiB per token"));
    }

    #[test]
    fn a_dense_model_reports_no_traffic_rather_than_a_zero_rate() {
        // 0% hit rate would read as a broken cache on a model that has no experts at all.
        assert!(r(0, 0).report(100).contains("not streaming"));
    }
}
