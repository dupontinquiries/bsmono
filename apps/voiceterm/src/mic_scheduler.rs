//! mic_scheduler: decide how many whisper instances to run and how to
//! assign mics to them, given known resource budgets — the pure core
//! of the multi-mic resource scheduler (F8 stage 2, see
//! docs/design/multi_mic_pipeline.md).
//!
//! Inputs are KNOWN QUANTITIES the system can report: total RAM, CPU
//! count, number of active mic streams, and a per-instance cost model
//! (RAM + CPUs a whisper instance needs). Output is a plan: instance
//! count, the mic→instance assignment, and the processing mode. PURE:
//! no spawning, no audio — whisper_pool acts on the plan (device-only).
//!
//! Three regimes fall out of the budget:
//!   - PerMic: budget >= mics → one instance per mic (lowest latency);
//!   - Shared: fewer instances than mics → round-robin mics across
//!     instances;
//!   - AsyncInsertion: ~one instance for many mics → VAD-gated frames
//!     inserted into a single queue (the many-mic→single-processor
//!     case the user named).

/// Per-whisper-instance resource cost (measured on the target SoC —
/// see the design doc's open question 3; these are caller-supplied so
/// the scheduler stays pure).
#[derive(Clone, Copy, Debug)]
pub struct InstanceCost {
    pub ram_mb: u32,
    pub cpus: u32,
}

impl Default for InstanceCost {
    fn default() -> Self {
        // Conservative placeholder for a small whisper model; the real
        // numbers come from measurement on M2 / Qualcomm.
        Self { ram_mb: 600, cpus: 2 }
    }
}

/// Available system budget + the active mic count.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub ram_mb: u32,
    pub cpus: u32,
    pub n_mics: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// One instance per mic — lowest latency, needs the most resources.
    PerMic,
    /// Fewer instances than mics — mics round-robin across instances.
    Shared,
    /// ~one instance for many mics — VAD-gated frames queued in.
    AsyncInsertion,
}

/// The scheduling decision.
#[derive(Clone, Debug, PartialEq)]
pub struct PipelinePlan {
    pub instances: u32,
    pub mode: Mode,
    /// assignment[mic_index] = instance_index. len == n_mics.
    pub assignment: Vec<u32>,
}

impl PipelinePlan {
    /// How many mics share the busiest instance (max fan-in). 0 if no
    /// mics. Useful for sizing the async queue / predicting backlog.
    pub fn max_fan_in(&self) -> u32 {
        if self.instances == 0 { return 0; }
        let mut counts = vec![0u32; self.instances as usize];
        for &inst in &self.assignment {
            if (inst as usize) < counts.len() { counts[inst as usize] += 1; }
        }
        counts.into_iter().max().unwrap_or(0)
    }
}

/// Compute the maximum number of instances the budget allows (bounded
/// by both RAM and CPU). At least 1 if there is any mic and any
/// resource; capped at n_mics (no point running more instances than
/// mics).
fn max_instances(budget: &Budget, cost: &InstanceCost) -> u32 {
    if budget.n_mics == 0 { return 0; }
    let by_ram = budget.ram_mb / cost.ram_mb.max(1);
    let by_cpu = budget.cpus / cost.cpus.max(1);
    by_ram.min(by_cpu).max(1).min(budget.n_mics)
}

/// Build the plan. Pure: same inputs → same plan.
pub fn schedule(budget: &Budget, cost: &InstanceCost) -> PipelinePlan {
    let n = budget.n_mics;
    if n == 0 {
        return PipelinePlan { instances: 0, mode: Mode::PerMic, assignment: Vec::new() };
    }
    let max_inst = max_instances(budget, cost);

    let (instances, mode) = if max_inst >= n {
        (n, Mode::PerMic)
    } else if max_inst <= 1 {
        (1, Mode::AsyncInsertion)
    } else {
        (max_inst, Mode::Shared)
    };

    // Round-robin assign mics to instances (balanced fan-in).
    let assignment: Vec<u32> = (0..n).map(|m| m % instances).collect();

    PipelinePlan { instances, mode, assignment }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cost() -> InstanceCost { InstanceCost { ram_mb: 600, cpus: 2 } }

    #[test]
    fn phone_single_stream_per_mic() {
        // phone: top+bottom already phase-combined upstream → 1 stream
        let p = schedule(&Budget { ram_mb: 8000, cpus: 8, n_mics: 1 }, &cost());
        assert_eq!(p.instances, 1);
        assert_eq!(p.mode, Mode::PerMic);
        assert_eq!(p.assignment, vec![0]);
    }

    #[test]
    fn studio_ample_resources_per_mic() {
        let p = schedule(&Budget { ram_mb: 32000, cpus: 16, n_mics: 4 }, &cost());
        assert_eq!(p.instances, 4);
        assert_eq!(p.mode, Mode::PerMic);
        assert_eq!(p.assignment, vec![0, 1, 2, 3]);
        assert_eq!(p.max_fan_in(), 1);
    }

    #[test]
    fn constrained_shares_round_robin() {
        // 4 mics but RAM only fits 2 instances → Shared, balanced
        let p = schedule(&Budget { ram_mb: 1500, cpus: 16, n_mics: 4 }, &cost());
        assert_eq!(p.instances, 2);
        assert_eq!(p.mode, Mode::Shared);
        assert_eq!(p.assignment, vec![0, 1, 0, 1]);
        assert_eq!(p.max_fan_in(), 2, "balanced 2-and-2");
    }

    #[test]
    fn many_mics_one_processor_async() {
        let p = schedule(&Budget { ram_mb: 700, cpus: 1, n_mics: 8 }, &cost());
        assert_eq!(p.instances, 1);
        assert_eq!(p.mode, Mode::AsyncInsertion);
        assert!(p.assignment.iter().all(|&i| i == 0));
        assert_eq!(p.max_fan_in(), 8);
    }

    #[test]
    fn zero_mics_empty_plan() {
        let p = schedule(&Budget { ram_mb: 8000, cpus: 8, n_mics: 0 }, &cost());
        assert_eq!(p.instances, 0);
        assert!(p.assignment.is_empty());
        assert_eq!(p.max_fan_in(), 0);
    }

    #[test]
    fn cpu_is_the_binding_constraint() {
        // tons of RAM, few CPUs → CPU caps instances
        let p = schedule(&Budget { ram_mb: 64000, cpus: 4, n_mics: 6 }, &cost());
        // by_cpu = 4/2 = 2 instances
        assert_eq!(p.instances, 2);
        assert_eq!(p.mode, Mode::Shared);
    }

    #[test]
    fn ram_is_the_binding_constraint() {
        // tons of CPUs, little RAM → RAM caps instances
        let p = schedule(&Budget { ram_mb: 600, cpus: 32, n_mics: 6 }, &cost());
        // by_ram = 600/600 = 1 → async (one instance, many mics)
        assert_eq!(p.instances, 1);
        assert_eq!(p.mode, Mode::AsyncInsertion);
    }

    #[test]
    fn assignment_always_covers_all_mics() {
        for n in 1..=12u32 {
            let p = schedule(&Budget { ram_mb: 2000, cpus: 8, n_mics: n }, &cost());
            assert_eq!(p.assignment.len(), n as usize, "one assignment per mic");
            assert!(p.assignment.iter().all(|&i| i < p.instances),
                "every mic assigned to a valid instance");
        }
    }

    #[test]
    fn fan_in_is_balanced() {
        // 7 mics across 3 instances → max fan-in ceil(7/3)=3
        let p = schedule(&Budget { ram_mb: 2000, cpus: 6, n_mics: 7 }, &cost());
        assert_eq!(p.instances, 3);
        assert_eq!(p.max_fan_in(), 3, "round-robin keeps fan-in balanced");
    }
}
