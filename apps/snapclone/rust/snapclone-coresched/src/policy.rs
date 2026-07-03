//! policy.rs — decide where each thread CLASS should run, and how to nudge it.
//!
//! Classification (topology.rs) tells us WHICH cpus are efficiency cores. This
//! module decides WHICH WORK belongs there and via WHICH mechanism, so the
//! caller just applies the returned `Placement` with the relevant syscall.
//!
//! # The mechanisms (layered, best-effort, graceful degradation)
//!
//!   * **Affinity** (`sched_setaffinity`): hard-pin a thread to a CPU set. The
//!     bluntest tool — guarantees the thread can ONLY run on E-cores. Good for
//!     truly-background work that must never steal a P-core, accepted that it
//!     can't burst even if P-cores are idle.
//!   * **uclamp** (`sched_setattr` with `SCHED_FLAG_UTIL_CLAMP`): set
//!     `util_clamp_max` low so the scheduler treats the thread as low-utilization
//!     and the EAS (Energy Aware Scheduler) naturally PREFERS E-cores and lower
//!     frequencies — but the thread can still migrate to a P-core under
//!     contention. The gentlest tool; ideal default for background work that
//!     shouldn't dominate but may occasionally need to catch up.
//!   * **nice** (`setpriority`): lower CPU priority so the thread yields to
//!     interactive work. Orthogonal to placement; always safe; the universal
//!     fallback when neither affinity nor uclamp is permitted (e.g. sandboxed).
//!
//! Policy: prefer uclamp (migration-friendly) for periodic/bursty background
//! work; use hard affinity only for steady low-priority loops; always also lower
//! nice. On a uniform CPU, placement reduces to "nice only" (nothing to pin).
//!
//! All pure decision logic — no syscalls here. The caller maps `Mechanism` +
//! `Placement.cpu_set` onto the OS calls and reports back what actually stuck.

use crate::topology::CpuTopology;

/// What a thread does, which determines how aggressively we can demote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkClass {
    /// The UI / main thread, or anything a human is actively waiting on. NEVER
    /// demoted.
    Interactive,
    /// Camera focus sweep, VCM control, live-preview transforms — latency-
    /// sensitive but not the UI thread. Stays on performance cores; we don't
    /// pin it to E-cores, only avoid demoting it.
    LatencySensitive,
    /// Periodic, bursty background: DHT re-announce, network-change polling,
    /// keepalives, peer-discovery beacons. Low urgency, runs forever in the
    /// background → uclamp toward E-cores (can still burst if needed).
    PeriodicBackground,
    /// Heavy but non-urgent compute the user isn't blocked on: thumbnail
    /// generation, ambient-palette derivation, face detection on captured
    /// frames, video post-processing. Hard-pin to E-cores when available —
    /// these can run a little slower to save battery, and pinning keeps them off
    /// the P-cores the UI wants.
    BackgroundCompute,
    /// Fire-and-forget I/O / shell-outs: firewall (ufw/nft) port management.
    /// Tiny and rare; lowest priority, E-core affinity if available.
    BackgroundIo,
}

/// Which placement mechanism the caller should apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mechanism {
    /// Leave the thread where the scheduler wants it (interactive/latency work).
    None,
    /// Lower priority only (setpriority/nice); no pinning.
    NiceOnly,
    /// uclamp util_clamp_max toward E-cores; thread can still migrate.
    UclampPreferEfficiency,
    /// Hard-pin to the efficiency CPU set (sched_setaffinity).
    AffinityEfficiency,
}

/// The concrete recommendation for a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub mechanism: Mechanism,
    /// CPUs to pin to (for AffinityEfficiency). Empty otherwise.
    pub cpu_set: Vec<u32>,
    /// Suggested nice increment (0..=19; higher = lower priority). Applied in
    /// addition to any placement, including for uclamp/affinity classes.
    pub nice_delta: i32,
    /// Suggested util_clamp_max as a 0..=1024 utilization value (for uclamp).
    /// 0 means "don't set uclamp." A low value (e.g. 256) tells EAS the thread
    /// needs little compute, so an E-core at low frequency suffices.
    pub uclamp_max: u32,
}

impl Placement {
    fn none() -> Self {
        Self { mechanism: Mechanism::None, cpu_set: vec![], nice_delta: 0, uclamp_max: 0 }
    }
}

/// Decide placement for a work class given the detected topology.
///
/// On a uniform CPU (`!has_efficiency_cores`), placement degrades to nice-only
/// for background classes and None for interactive — there's no E-core to aim
/// at, but lowering priority still helps the scheduler favor interactive work.
pub fn placement_for(class: WorkClass, topo: &CpuTopology) -> Placement {
    let has_e = topo.has_efficiency_cores();
    match class {
        WorkClass::Interactive => Placement::none(),

        WorkClass::LatencySensitive => {
            // Don't demote; but it's fine to mark it slightly nice on a busy
            // system so it yields to the actual UI thread.
            Placement { mechanism: Mechanism::None, cpu_set: vec![], nice_delta: 0, uclamp_max: 0 }
        }

        WorkClass::PeriodicBackground => {
            if has_e {
                // uclamp toward efficiency: low util ceiling so EAS prefers
                // E-cores + low freq, but it can still migrate under load.
                Placement {
                    mechanism: Mechanism::UclampPreferEfficiency,
                    cpu_set: topo.efficiency.clone(),
                    nice_delta: 5,
                    uclamp_max: 256,
                }
            } else {
                Placement { mechanism: Mechanism::NiceOnly, cpu_set: vec![], nice_delta: 5, uclamp_max: 0 }
            }
        }

        WorkClass::BackgroundCompute => {
            if has_e {
                // Hard-pin: keep heavy compute off the P-cores the UI wants.
                Placement {
                    mechanism: Mechanism::AffinityEfficiency,
                    cpu_set: topo.efficiency.clone(),
                    nice_delta: 10,
                    uclamp_max: 0, // affinity already bounds it to E-cores
                }
            } else {
                Placement { mechanism: Mechanism::NiceOnly, cpu_set: vec![], nice_delta: 10, uclamp_max: 0 }
            }
        }

        WorkClass::BackgroundIo => {
            if has_e {
                Placement {
                    mechanism: Mechanism::AffinityEfficiency,
                    cpu_set: topo.efficiency.clone(),
                    nice_delta: 15,
                    uclamp_max: 0,
                }
            } else {
                Placement { mechanism: Mechanism::NiceOnly, cpu_set: vec![], nice_delta: 15, uclamp_max: 0 }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::classify_by_capacity;

    fn hetero() -> CpuTopology {
        classify_by_capacity(&[
            (0, 410), (1, 410), (2, 410), (3, 410),
            (4, 1024), (5, 1024), (6, 1024), (7, 1024),
        ])
    }
    fn uniform() -> CpuTopology {
        classify_by_capacity(&[(0, 1024), (1, 1024), (2, 1024), (3, 1024)])
    }

    #[test]
    fn interactive_is_never_demoted() {
        assert_eq!(placement_for(WorkClass::Interactive, &hetero()).mechanism, Mechanism::None);
        assert_eq!(placement_for(WorkClass::Interactive, &uniform()).mechanism, Mechanism::None);
    }

    #[test]
    fn latency_sensitive_stays_on_perf() {
        let p = placement_for(WorkClass::LatencySensitive, &hetero());
        assert_eq!(p.mechanism, Mechanism::None);
        assert!(p.cpu_set.is_empty(), "not pinned to E-cores");
    }

    #[test]
    fn periodic_background_uses_uclamp_toward_efficiency() {
        let p = placement_for(WorkClass::PeriodicBackground, &hetero());
        assert_eq!(p.mechanism, Mechanism::UclampPreferEfficiency);
        assert_eq!(p.cpu_set, vec![0, 1, 2, 3]);
        assert!(p.uclamp_max > 0 && p.uclamp_max < 1024, "low util ceiling");
        assert!(p.nice_delta > 0);
    }

    #[test]
    fn background_compute_hard_pins_to_efficiency() {
        let p = placement_for(WorkClass::BackgroundCompute, &hetero());
        assert_eq!(p.mechanism, Mechanism::AffinityEfficiency);
        assert_eq!(p.cpu_set, vec![0, 1, 2, 3]);
        assert!(p.nice_delta >= 10);
    }

    #[test]
    fn background_io_lowest_priority() {
        let io = placement_for(WorkClass::BackgroundIo, &hetero());
        let compute = placement_for(WorkClass::BackgroundCompute, &hetero());
        assert!(io.nice_delta > compute.nice_delta, "IO yields even more");
        assert_eq!(io.mechanism, Mechanism::AffinityEfficiency);
    }

    #[test]
    fn uniform_cpu_degrades_to_nice_only() {
        // No E-cores: background work can't be pinned, but lowering priority
        // still helps interactive work win. No mechanism tries to pin to an
        // empty set.
        for class in [WorkClass::PeriodicBackground, WorkClass::BackgroundCompute, WorkClass::BackgroundIo] {
            let p = placement_for(class, &uniform());
            assert_eq!(p.mechanism, Mechanism::NiceOnly, "{class:?} → nice only on uniform CPU");
            assert!(p.cpu_set.is_empty(), "never pin to a nonexistent E-core set");
            assert!(p.nice_delta > 0);
        }
    }

    #[test]
    fn nice_increases_with_lower_urgency() {
        let t = hetero();
        let periodic = placement_for(WorkClass::PeriodicBackground, &t).nice_delta;
        let compute = placement_for(WorkClass::BackgroundCompute, &t).nice_delta;
        let io = placement_for(WorkClass::BackgroundIo, &t).nice_delta;
        assert!(periodic < compute && compute < io, "monotonic by urgency");
    }
}
