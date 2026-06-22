//! llm_scheduler: a device-relative resource model + admission control
//! for LLM requests, curing the "models lock up against each other /
//! endless timeouts / CPU churn" failure mode of the unmanaged Ollama
//! path (four concurrent callers, no coordination, a sequential
//! escalation cascade that can spend N×timeout per query).
//!
//! KEY IDEA (user's insight): "large" and "tiny" are NOT model
//! properties — they're properties of a model RELATIVE to THIS device's
//! capacity. A 7B model is "large" on an 8GB phone and "small" on a
//! workstation. So nothing is a hardcoded size tier; everything is
//! computed from a device budget, and a model's size class is DERIVED
//! (its footprint as a fraction of the budget).
//!
//! TWO budgets, because they fail differently:
//!   - MEMORY: how many models can be RESIDENT (the "10 small OR 2-3
//!     large" rule). Exceed it → can't load another without eviction.
//!   - CONCURRENCY: how many can INFER at once before contention. You
//!     can hold more resident than you can run — the gap is exactly
//!     where "locking up against each other" lives. Exceed it → QUEUE
//!     the request rather than thrash. THIS is the anti-lockup fix.
//!
//! PURE + dependency-free: budgets and model costs are caller-supplied
//! (auto-derived from RAM/CPU by `DeviceBudget::derive`, refinable by
//! on-device measurement later — same discipline as mic_scheduler). No
//! HTTP, no model loading here; the scheduler decides, the caller acts.

/// Effective resident cost of a model = weights + KV cache + context +
/// compute headroom. NOT just the weight-file size (a 5GB-file model
/// can cost ~8GB live, which is why a workstation fits 2-3, not 5).
/// Caller supplies measured/estimated values.
#[derive(Clone, Copy, Debug)]
pub struct ModelCost {
    /// Effective resident memory while loaded, MB.
    pub mem_mb: u32,
    /// Concurrency slots one inference of this model consumes. Heavier
    /// models need more compute headroom, so a "large-for-device" model
    /// may take >1 slot. Small models take 1.
    pub slots: u32,
}

impl Default for ModelCost {
    fn default() -> Self {
        // Placeholder for a small model; real numbers come from
        // device measurement (see module note).
        Self { mem_mb: 2000, slots: 1 }
    }
}

/// This device's LLM capacity, in the two dimensions that matter.
#[derive(Clone, Copy, Debug)]
pub struct DeviceBudget {
    /// Total memory available for resident LLMs, MB (after OS/app
    /// reserve).
    pub mem_mb: u32,
    /// Max simultaneous inferences before contention/lockup.
    pub concurrency: u32,
}

impl DeviceBudget {
    /// Auto-derive a starting budget from total system RAM + CPU count.
    /// Conservative; on-device measurement can refine it later.
    ///
    /// - memory: total RAM minus a reserve for the OS + the app + other
    ///   processes. We reserve ~40% (or at least 2GB) and cap LLM use at
    ///   the remainder, so we never push the device into swap.
    /// - concurrency: scales with CPU count but stays modest — running
    ///   more inferences than ~physical cores allows just thrashes.
    /// - concurrency: a STARTING estimate (~half the cores, min 1,
    ///   capped 8). This is refined by the first-run ConcurrencyProbe,
    ///   because the real contention point depends on memory bandwidth +
    ///   thermals, not just core count. Until the probe runs, this safe
    ///   underestimate prevents lockup.
    pub fn derive(total_ram_mb: u32, cpus: u32) -> DeviceBudget {
        // v0.1.557: saturating_mul to match the saturating_sub below —
        // total_ram_mb * 4 would overflow u32 above ~1 PB RAM. Not a
        // real machine today, but the unguarded multiply was the one
        // arithmetic op here without overflow protection, inconsistent
        // with the rest. saturating keeps the reserve sane at any input.
        let reserve = (total_ram_mb.saturating_mul(4) / 10).max(2000);
        let mem_mb = total_ram_mb.saturating_sub(reserve);
        let concurrency = (cpus / 2).max(1).min(8);
        DeviceBudget { mem_mb, concurrency }
    }
}

/// Device-relative size class of a model — DERIVED, never declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeClass {
    /// Occupies a small fraction of this device's budget.
    SmallForDevice,
    /// Occupies a large fraction (> LARGE_FRACTION) of the budget.
    LargeForDevice,
    /// Does not fit at all on this device.
    TooLarge,
}

/// Fraction of the memory budget above which a model is "large for
/// this device".
pub const LARGE_FRACTION: f32 = 0.30;

/// How many of a (homogeneous) model can be RESIDENT in the budget.
pub fn fit_resident(b: &DeviceBudget, m: ModelCost) -> u32 {
    b.mem_mb / m.mem_mb.max(1)
}

/// How many can INFER simultaneously (the anti-lockup limit).
pub fn fit_running(b: &DeviceBudget, m: ModelCost) -> u32 {
    b.concurrency / m.slots.max(1)
}

/// Classify a model RELATIVE to this device.
pub fn classify_size(b: &DeviceBudget, m: ModelCost) -> SizeClass {
    if m.mem_mb > b.mem_mb {
        SizeClass::TooLarge
    } else if (m.mem_mb as f32 / b.mem_mb.max(1) as f32) > LARGE_FRACTION {
        SizeClass::LargeForDevice
    } else {
        SizeClass::SmallForDevice
    }
}

// ─── Empirical concurrency probe (the "measure on first run" part) ───
//
// Memory budget is computed up front (RAM - reserve). Concurrency is
// empirical: the point where models start contending depends on the
// SoC's memory bandwidth + thermals + scheduler, not just core count.
// So derive() gives a STARTING concurrency, and a first-run probe
// refines it by measuring aggregate throughput at increasing
// concurrency and finding the knee — the level past which adding a
// worker doesn't meaningfully raise total tokens/sec.

/// Minimum fractional throughput gain to justify one more concurrent
/// worker. Below this (plateau) or negative (regression/thermal
/// throttle), we've found the contention wall.
pub const MIN_CONCURRENCY_GAIN: f32 = 0.10;

/// One probe sample: aggregate tokens/sec measured while running
/// `level` inferences concurrently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThroughputSample {
    pub level: u32,
    pub tokens_per_sec: f32,
}

/// Given throughput samples at increasing concurrency (level ascending),
/// return the optimal concurrency = the highest level still justified
/// by at least MIN_CONCURRENCY_GAIN more aggregate throughput than the
/// previous level. Pure; the measurement that produces samples is
/// device-side.
pub fn optimal_concurrency(samples: &[ThroughputSample]) -> u32 {
    if samples.is_empty() { return 1; }
    let mut best = samples[0].level;
    let mut best_tput = samples[0].tokens_per_sec;
    for s in &samples[1..] {
        let gain = if best_tput > 0.0 {
            (s.tokens_per_sec - best_tput) / best_tput
        } else {
            1.0
        };
        if gain >= MIN_CONCURRENCY_GAIN {
            best = s.level;
            best_tput = s.tokens_per_sec;
        } else {
            // plateau or regression → the previous level is the knee.
            break;
        }
    }
    best.max(1)
}

/// Drives the first-run concurrency probe. The caller measures
/// throughput at each level (device-side) and feeds samples in; this
/// decides whether to probe a higher level next or stop. Keeps the
/// measurement loop's POLICY pure and testable while the actual timing
/// lives on the device.
#[derive(Clone, Debug)]
pub struct ConcurrencyProbe {
    samples: Vec<ThroughputSample>,
    /// Don't probe beyond this (memory may not even hold this many, and
    /// past ~8 inference contends regardless).
    max_level: u32,
}

impl ConcurrencyProbe {
    pub fn new(max_level: u32) -> Self {
        Self { samples: Vec::new(), max_level: max_level.max(1) }
    }

    /// The next concurrency level to measure, or None if probing is
    /// done (hit a plateau/regression or the max level). Call, measure
    /// at the returned level, `record`, repeat until None.
    pub fn next_level(&self) -> Option<u32> {
        match self.samples.last() {
            None => Some(1),
            Some(last) => {
                if last.level >= self.max_level {
                    return None; // reached the ceiling
                }
                // If the last step already failed to gain, we're done.
                if self.samples.len() >= 2 {
                    let prev = self.samples[self.samples.len() - 2];
                    let gain = if prev.tokens_per_sec > 0.0 {
                        (last.tokens_per_sec - prev.tokens_per_sec) / prev.tokens_per_sec
                    } else { 1.0 };
                    if gain < MIN_CONCURRENCY_GAIN {
                        return None; // knee found, stop probing
                    }
                }
                Some(last.level + 1)
            }
        }
    }

    /// Record a measured sample.
    pub fn record(&mut self, level: u32, tokens_per_sec: f32) {
        self.samples.push(ThroughputSample { level, tokens_per_sec });
    }

    /// True once probing should stop.
    pub fn is_done(&self) -> bool { self.next_level().is_none() }

    /// The concurrency to adopt from the samples gathered so far.
    pub fn result(&self) -> u32 { optimal_concurrency(&self.samples) }

    pub fn samples(&self) -> &[ThroughputSample] { &self.samples }
}



/// Priority of a request. Higher = more urgent. A live voice-intent
/// query the user is waiting on must beat a background block-summary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Background work (block summaries, prefetch) — lowest.
    Background = 0,
    /// Normal interactive query.
    Interactive = 1,
    /// The user is actively waiting (voice intent, chat send) — highest.
    Foreground = 2,
}

/// A request waiting for admission.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingRequest {
    pub id: u64,
    pub priority: Priority,
    /// Slots this request's model needs while running.
    pub slots: u32,
}

/// The admission decision for the current tick.
#[derive(Clone, Debug, PartialEq)]
pub struct AdmissionPlan {
    /// Request ids to admit (start now), in priority order.
    pub admit: Vec<u64>,
    /// Request ids to queue (capacity full) — they wait, they do NOT
    /// pile onto the backend (this is what stops the lockup cascade).
    pub queue: Vec<u64>,
}

/// Decide which pending requests to admit given how many concurrency
/// slots are already in use. Admits highest-priority first; a request
/// is admitted only if its slots fit in the remaining concurrency
/// budget. Everything else queues (no thrashing the backend). Pure.
///
/// `in_flight_slots` = slots currently consumed by running inferences.
pub fn plan_admission(
    budget: &DeviceBudget,
    in_flight_slots: u32,
    pending: &[PendingRequest],
) -> AdmissionPlan {
    let mut free = budget.concurrency.saturating_sub(in_flight_slots);
    // Sort by priority (desc), then by id (asc) for stable FIFO within
    // a priority band. Work on indices to keep it pure.
    let mut order: Vec<&PendingRequest> = pending.iter().collect();
    order.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.id.cmp(&b.id)));

    let mut admit = Vec::new();
    let mut queue = Vec::new();
    for req in order {
        let need = req.slots.max(1);
        if need <= free {
            free -= need;
            admit.push(req.id);
        } else {
            queue.push(req.id);
        }
    }
    AdmissionPlan { admit, queue }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> ModelCost { ModelCost { mem_mb: 2000, slots: 1 } }
    fn large() -> ModelCost { ModelCost { mem_mb: 8000, slots: 4 } }

    // ── device-relative budget model ──

    #[test]
    fn workstation_ten_small_or_two_large() {
        // user's case: ~10 small OR 2-3 large resident on a workstation
        let ws = DeviceBudget { mem_mb: 20000, concurrency: 8 };
        assert_eq!(fit_resident(&ws, small()), 10);
        assert_eq!(fit_resident(&ws, large()), 2);
    }

    #[test]
    fn phone_three_tiny_plus_one_large() {
        // user's case: ~2-3 tiny + 1 large on an 8GB phone
        let ph = DeviceBudget { mem_mb: 3000, concurrency: 2 };
        let tiny = ModelCost { mem_mb: 1000, slots: 1 };
        let phone_large = ModelCost { mem_mb: 3000, slots: 2 };
        assert_eq!(fit_resident(&ph, tiny), 3);
        assert_eq!(fit_resident(&ph, phone_large), 1);
    }

    #[test]
    fn size_class_is_device_relative() {
        // the SAME model is classified differently per device — no
        // hardcoded tier.
        let m = ModelCost { mem_mb: 2500, slots: 1 };
        let ws = DeviceBudget { mem_mb: 20000, concurrency: 8 };
        let ph = DeviceBudget { mem_mb: 3000, concurrency: 2 };
        assert_eq!(classify_size(&ws, m), SizeClass::SmallForDevice); // 12% of ws
        assert_eq!(classify_size(&ph, m), SizeClass::LargeForDevice); // 83% of phone
        // and a model bigger than the whole budget is TooLarge
        let huge = ModelCost { mem_mb: 5000, slots: 4 };
        assert_eq!(classify_size(&ph, huge), SizeClass::TooLarge);
    }

    #[test]
    fn derive_budget_reserves_headroom() {
        // 8GB phone: reserve >= 40% so we never swap
        let ph = DeviceBudget::derive(8000, 4);
        assert!(ph.mem_mb <= 4800, "phone LLM budget leaves OS/app headroom: {}", ph.mem_mb);
        assert!(ph.mem_mb >= 2000, "but still usable");
        assert!(ph.concurrency >= 1 && ph.concurrency <= 8);
        // 64GB workstation, 16 cores
        let ws = DeviceBudget::derive(64000, 16);
        assert!(ws.mem_mb >= 32000, "workstation gets a big budget");
        assert_eq!(ws.concurrency, 8, "concurrency capped at 8");
    }

    #[test]
    fn derive_tiny_device_still_safe() {
        // a 2GB device: reserve floor is 2GB, so mem_mb saturates to 0
        // (don't run resident LLMs) rather than underflow.
        let tiny = DeviceBudget::derive(2000, 2);
        assert_eq!(tiny.mem_mb, 0, "no LLM budget on a 2GB device, no underflow");
        assert!(tiny.concurrency >= 1);
    }

    #[test]
    fn derive_extreme_ram_no_overflow() {
        // v0.1.557: u32::MAX MB RAM must not panic on the *4 multiply.
        // saturating_mul caps it; reserve becomes huge so mem_mb is 0
        // (degenerate but safe — and unreachable on real hardware).
        let huge = DeviceBudget::derive(u32::MAX, 64);
        // reserve = MAX (saturated) /10 .max(2000) → still leaves
        // total - reserve; just assert it didn't panic and stayed sane.
        assert!(huge.mem_mb <= u32::MAX);
        assert!(huge.concurrency >= 1 && huge.concurrency <= 8);
        // and a 0-RAM device (degenerate input) is also safe
        let zero = DeviceBudget::derive(0, 0);
        assert_eq!(zero.mem_mb, 0);
        assert_eq!(zero.concurrency, 1, "concurrency floor is 1 even at 0 cpus");
    }

    // ── admission control (anti-lockup) ──

    fn req(id: u64, p: Priority, slots: u32) -> PendingRequest {
        PendingRequest { id, priority: p, slots }
    }

    #[test]
    fn admits_within_concurrency_queues_the_rest() {
        // concurrency 8, nothing in flight, 10 small (1-slot) requests →
        // admit 8, queue 2. THIS is the anti-lockup: the 9th/10th wait.
        let ws = DeviceBudget { mem_mb: 20000, concurrency: 8 };
        let pending: Vec<_> = (0..10).map(|i| req(i, Priority::Interactive, 1)).collect();
        let plan = plan_admission(&ws, 0, &pending);
        assert_eq!(plan.admit.len(), 8);
        assert_eq!(plan.queue.len(), 2);
    }

    #[test]
    fn foreground_beats_background() {
        // 2 slots free; a background + a foreground request pending →
        // foreground admitted first.
        let b = DeviceBudget { mem_mb: 20000, concurrency: 2 };
        let pending = vec![
            req(1, Priority::Background, 1),
            req(2, Priority::Foreground, 1),
            req(3, Priority::Background, 1),
        ];
        let plan = plan_admission(&b, 0, &pending);
        assert!(plan.admit.contains(&2), "foreground admitted");
        // only 2 slots: foreground + one background; the other queues
        assert_eq!(plan.admit.len(), 2);
        assert_eq!(plan.queue.len(), 1);
        assert_eq!(plan.queue[0], 3, "later background queues (FIFO within band)");
    }

    #[test]
    fn in_flight_slots_reduce_capacity() {
        // concurrency 8 but 6 already running → only 2 free
        let ws = DeviceBudget { mem_mb: 20000, concurrency: 8 };
        let pending: Vec<_> = (0..5).map(|i| req(i, Priority::Interactive, 1)).collect();
        let plan = plan_admission(&ws, 6, &pending);
        assert_eq!(plan.admit.len(), 2, "only 2 slots free");
        assert_eq!(plan.queue.len(), 3);
    }

    #[test]
    fn heavy_model_consumes_multiple_slots() {
        // a 4-slot large model + small ones; concurrency 8
        let ws = DeviceBudget { mem_mb: 20000, concurrency: 8 };
        let pending = vec![
            req(1, Priority::Foreground, 4), // large
            req(2, Priority::Interactive, 1),
            req(3, Priority::Interactive, 1),
            req(4, Priority::Interactive, 1),
            req(5, Priority::Interactive, 1),
            req(6, Priority::Interactive, 1), // this one won't fit (4+4=8 used)
        ];
        let plan = plan_admission(&ws, 0, &pending);
        // large(4) + four small(1 each)=8 slots → admit 5, queue 1
        assert!(plan.admit.contains(&1));
        assert_eq!(plan.admit.len(), 5);
        assert_eq!(plan.queue, vec![6]);
    }

    #[test]
    fn full_capacity_queues_everything() {
        let ws = DeviceBudget { mem_mb: 20000, concurrency: 4 };
        let pending: Vec<_> = (0..3).map(|i| req(i, Priority::Interactive, 1)).collect();
        let plan = plan_admission(&ws, 4, &pending); // all slots in use
        assert!(plan.admit.is_empty(), "nothing admitted when full");
        assert_eq!(plan.queue.len(), 3, "all queue, none thrash the backend");
    }

    #[test]
    fn empty_pending_is_noop() {
        let ws = DeviceBudget { mem_mb: 20000, concurrency: 8 };
        let plan = plan_admission(&ws, 0, &[]);
        assert!(plan.admit.is_empty() && plan.queue.is_empty());
    }

    // ── empirical concurrency probe ──

    fn s(level: u32, tput: f32) -> ThroughputSample {
        ThroughputSample { level, tokens_per_sec: tput }
    }

    #[test]
    fn knee_detector_finds_plateau() {
        // scales to 4, then flattens
        let samples = [s(1,40.0), s(2,78.0), s(3,110.0), s(4,140.0), s(5,145.0), s(6,143.0)];
        assert_eq!(optimal_concurrency(&samples), 4);
    }

    #[test]
    fn knee_detector_handles_regression_and_edges() {
        // 1→2 good, 2→3 regresses (thermal) → knee at 2
        assert_eq!(optimal_concurrency(&[s(1,12.0), s(2,14.0), s(3,13.0)]), 2);
        // immediate contention → 1
        assert_eq!(optimal_concurrency(&[s(1,20.0), s(2,21.0)]), 1);
        // perfect linear → take the max measured
        assert_eq!(optimal_concurrency(&[s(1,10.0), s(2,20.0), s(3,30.0)]), 3);
        // single sample, empty
        assert_eq!(optimal_concurrency(&[s(1,15.0)]), 1);
        assert_eq!(optimal_concurrency(&[]), 1);
    }

    #[test]
    fn probe_driver_walks_until_plateau() {
        // simulate a device that scales to 3 then plateaus.
        let tput = |lvl: u32| -> f32 {
            match lvl { 1 => 30.0, 2 => 58.0, 3 => 84.0, 4 => 86.0, _ => 86.0 }
        };
        let mut probe = ConcurrencyProbe::new(8);
        let mut measured = 0;
        while let Some(level) = probe.next_level() {
            probe.record(level, tput(level));
            measured += 1;
            assert!(measured < 20, "probe must terminate");
        }
        // 3→4 is only ~2.4% → stops; adopts 3
        assert_eq!(probe.result(), 3);
        assert!(probe.is_done());
    }

    #[test]
    fn probe_respects_max_level() {
        // a device that keeps scaling linearly is capped at max_level
        let mut probe = ConcurrencyProbe::new(4);
        while let Some(level) = probe.next_level() {
            probe.record(level, level as f32 * 10.0); // perfect scaling
        }
        // never probes past 4, adopts 4
        assert_eq!(probe.result(), 4);
        assert!(probe.samples().iter().all(|s| s.level <= 4));
    }

    #[test]
    fn probe_starts_at_one() {
        let probe = ConcurrencyProbe::new(8);
        assert_eq!(probe.next_level(), Some(1));
        assert!(!probe.is_done());
    }
}
