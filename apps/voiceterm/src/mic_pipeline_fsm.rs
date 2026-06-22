//! mic_pipeline_fsm: the VAD-gated finite-state machine that ties the
//! multi-mic cores together (F8 stage 3, see
//! docs/design/multi_mic_pipeline.md). Every tick (~1500ms) it reads
//! fresh per-pair stats, measures DEVIATION from the current topology,
//! and reshapes the pipeline (re-classify → re-schedule) ONLY when the
//! change is real and sustained — never on a one-tick blip.
//!
//! The critical property is ANTI-THRASH: a transient (a cough, someone
//! shifting) must not trigger a rebuild. So a deviation must exceed a
//! threshold AND persist for a confirm window of consecutive ticks
//! before committing. Same hysteresis principle as the cinematic
//! camera's deadband and dead_air's min-silence-run.
//!
//! PURE: composes mic_topology + mic_scheduler over caller-supplied
//! stats. No audio threads, no whisper spawning — it emits a decision
//! (reshape to this plan / hold); the audio layer acts on it (device).

use crate::mic_topology::{self, PairStat, Topology, TopologyConfig};
use crate::mic_scheduler::{self, Budget, InstanceCost, PipelinePlan};

#[derive(Clone, Copy, Debug)]
pub struct FsmConfig {
    /// Deviation (0..1) at/above which a change is "significant".
    pub deviation_threshold: f32,
    /// Consecutive significant-deviation ticks required before a
    /// reshape commits (anti-thrash). 1 = reshape immediately on any
    /// over-threshold tick; higher = more inertia.
    pub confirm_ticks: u32,
    pub topo: TopologyConfig,
    pub cost: InstanceCost,
}

impl Default for FsmConfig {
    fn default() -> Self {
        Self {
            deviation_threshold: 0.5,
            confirm_ticks: 2, // ~3s of sustained change at a 1500ms tick
            topo: TopologyConfig::default(),
            cost: InstanceCost::default(),
        }
    }
}

/// Internal anti-thrash state.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Holding the current shape.
    Stable,
    /// Seen `n` consecutive over-threshold ticks, not yet committed.
    Pending(u32),
}

/// What the FSM decided this tick.
#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    /// Keep the current pipeline shape.
    Hold,
    /// Commit a reshape to this topology + plan.
    Reshape { topology: Topology, plan: PipelinePlan },
}

/// The running FSM. One per capture session.
pub struct MicPipelineFsm {
    cfg: FsmConfig,
    phase: Phase,
    /// The shape currently in effect (None until the first tick
    /// establishes one — the first tick always reshapes).
    current_topology: Option<Topology>,
    current_plan: Option<PipelinePlan>,
}

impl MicPipelineFsm {
    pub fn new(cfg: FsmConfig) -> Self {
        Self { cfg, phase: Phase::Stable, current_topology: None, current_plan: None }
    }

    pub fn current_plan(&self) -> Option<&PipelinePlan> { self.current_plan.as_ref() }
    pub fn current_topology(&self) -> Option<&Topology> { self.current_topology.as_ref() }

    /// Advance one tick with fresh stats. `n_mics` + `pairs` describe
    /// the current acoustic situation; `budget` is the live resource
    /// budget (RAM/CPU may change as the system load shifts).
    ///
    /// Returns the Decision. On the FIRST tick (no current shape) it
    /// always Reshapes to establish one. Thereafter it Holds unless a
    /// sustained deviation commits a new shape.
    pub fn tick(&mut self, n_mics: usize, pairs: &[PairStat], budget: &Budget) -> Decision {
        // First tick: establish the initial shape unconditionally.
        if self.current_topology.is_none() {
            return self.commit(n_mics, pairs, budget);
        }

        // Measure deviation of fresh stats vs the current topology.
        let prev = self.current_topology.as_ref().unwrap();
        let dev = mic_topology::deviation(prev, pairs, &self.cfg.topo);
        let exceeds = dev >= self.cfg.deviation_threshold;

        // Also reshape if the BUDGET changed enough to alter the plan
        // even when topology is stable (e.g. system freed/consumed RAM
        // so we can run more/fewer whisper instances). Cheap to check:
        // recompute the plan for the CURRENT topology's mic count and
        // compare instance count.
        let budget_forces = self.budget_changed_plan(n_mics, budget);

        match self.phase {
            Phase::Stable => {
                if exceeds {
                    // start the confirm window; if it only needs 1 tick,
                    // commit now.
                    if self.cfg.confirm_ticks <= 1 {
                        return self.commit(n_mics, pairs, budget);
                    }
                    self.phase = Phase::Pending(1);
                    Decision::Hold
                } else if budget_forces {
                    // budget-only reshape: no topology change, re-plan
                    // immediately (no thrash risk — budget is stable-ish
                    // and re-planning is cheap/non-disruptive).
                    self.commit(n_mics, pairs, budget)
                } else {
                    Decision::Hold
                }
            }
            Phase::Pending(n) => {
                if exceeds {
                    let n2 = n + 1;
                    if n2 >= self.cfg.confirm_ticks {
                        self.commit(n_mics, pairs, budget)
                    } else {
                        self.phase = Phase::Pending(n2);
                        Decision::Hold
                    }
                } else {
                    // deviation cleared before confirming → it was a
                    // blip; abort the pending reshape.
                    self.phase = Phase::Stable;
                    Decision::Hold
                }
            }
        }
    }

    /// Would the current budget produce a different instance count than
    /// the plan in effect? (Topology unchanged case.)
    fn budget_changed_plan(&self, n_mics: usize, budget: &Budget) -> bool {
        let cur = match &self.current_plan { Some(p) => p, None => return true };
        let b = Budget { ram_mb: budget.ram_mb, cpus: budget.cpus, n_mics: n_mics as u32 };
        let fresh = mic_scheduler::schedule(&b, &self.cfg.cost);
        fresh.instances != cur.instances || fresh.mode != cur.mode
    }

    /// Re-classify + re-schedule and adopt the result as the new shape.
    fn commit(&mut self, n_mics: usize, pairs: &[PairStat], budget: &Budget) -> Decision {
        let topology = mic_topology::classify(n_mics, pairs, &self.cfg.topo);
        let b = Budget { ram_mb: budget.ram_mb, cpus: budget.cpus, n_mics: n_mics as u32 };
        let plan = mic_scheduler::schedule(&b, &self.cfg.cost);
        self.current_topology = Some(topology.clone());
        self.current_plan = Some(plan.clone());
        self.phase = Phase::Stable;
        Decision::Reshape { topology, plan }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mic_scheduler::Mode;

    fn pair(a: usize, b: usize, c: f32, d: f32) -> PairStat {
        PairStat { a, b, correlation: c, delay_ms: d }
    }
    fn budget(ram: u32, cpus: u32, mics: u32) -> Budget {
        Budget { ram_mb: ram, cpus, n_mics: mics }
    }
    fn cfg() -> FsmConfig { FsmConfig::default() }

    #[test]
    fn first_tick_always_reshapes() {
        let mut f = MicPipelineFsm::new(cfg());
        let d = f.tick(2, &[pair(0, 1, 0.95, 0.3)], &budget(8000, 8, 2));
        match d {
            Decision::Reshape { topology, plan } => {
                assert_eq!(topology, Topology::PhaseArray { group: vec![0, 1] });
                assert_eq!(plan.instances, 2); // 2 mics, ample budget
            }
            Decision::Hold => panic!("first tick must establish a shape"),
        }
    }

    #[test]
    fn stable_setup_holds() {
        let mut f = MicPipelineFsm::new(cfg());
        let b = budget(8000, 8, 2);
        let p = [pair(0, 1, 0.95, 0.3)];
        f.tick(2, &p, &b); // establish
        // same stats for many ticks → always Hold
        for _ in 0..10 {
            assert_eq!(f.tick(2, &p, &b), Decision::Hold);
        }
    }

    #[test]
    fn one_tick_blip_does_not_reshape() {
        let mut f = MicPipelineFsm::new(FsmConfig { confirm_ticks: 2, ..cfg() });
        let b = budget(8000, 8, 2);
        f.tick(2, &[pair(0, 1, 0.95, 0.3)], &b); // phase array established
        // one tick where the pair reads decoupled (blip) ...
        assert_eq!(f.tick(2, &[pair(0, 1, 0.1, 9.0)], &b), Decision::Hold);
        // ... then back to coupled → no reshape happened
        assert_eq!(f.tick(2, &[pair(0, 1, 0.95, 0.3)], &b), Decision::Hold);
    }

    #[test]
    fn sustained_change_reshapes_after_confirm_window() {
        let mut f = MicPipelineFsm::new(FsmConfig { confirm_ticks: 2, ..cfg() });
        let b = budget(8000, 8, 2);
        f.tick(2, &[pair(0, 1, 0.95, 0.3)], &b); // phase array
        // mic moves away: decoupled, sustained
        assert_eq!(f.tick(2, &[pair(0, 1, 0.1, 9.0)], &b), Decision::Hold); // pending(1)
        let d = f.tick(2, &[pair(0, 1, 0.1, 9.0)], &b); // pending(2) → commit
        match d {
            Decision::Reshape { topology, .. } => {
                assert_eq!(topology, Topology::RoomMics { mics: vec![0, 1] });
            }
            Decision::Hold => panic!("sustained change should reshape"),
        }
    }

    #[test]
    fn confirm_ticks_one_reshapes_immediately() {
        let mut f = MicPipelineFsm::new(FsmConfig { confirm_ticks: 1, ..cfg() });
        let b = budget(8000, 8, 2);
        f.tick(2, &[pair(0, 1, 0.95, 0.3)], &b);
        // immediate reshape on first over-threshold tick
        let d = f.tick(2, &[pair(0, 1, 0.1, 9.0)], &b);
        assert!(matches!(d, Decision::Reshape { .. }));
    }

    #[test]
    fn partial_burst_then_clear_aborts() {
        let mut f = MicPipelineFsm::new(FsmConfig { confirm_ticks: 3, ..cfg() });
        let b = budget(8000, 8, 2);
        f.tick(2, &[pair(0, 1, 0.95, 0.3)], &b);
        assert_eq!(f.tick(2, &[pair(0, 1, 0.1, 9.0)], &b), Decision::Hold); // pending(1)
        assert_eq!(f.tick(2, &[pair(0, 1, 0.1, 9.0)], &b), Decision::Hold); // pending(2)
        assert_eq!(f.tick(2, &[pair(0, 1, 0.95, 0.3)], &b), Decision::Hold); // cleared → abort
        // and a subsequent stable run keeps holding
        assert_eq!(f.tick(2, &[pair(0, 1, 0.95, 0.3)], &b), Decision::Hold);
    }

    #[test]
    fn budget_change_replans_even_when_topology_stable() {
        let mut f = MicPipelineFsm::new(cfg());
        // 4 mics, ample budget → 4 instances PerMic
        let p = [
            pair(0,1,0.1,9.0), pair(0,2,0.1,9.0), pair(0,3,0.1,9.0),
            pair(1,2,0.1,9.0), pair(1,3,0.1,9.0), pair(2,3,0.1,9.0),
        ];
        f.tick(4, &p, &budget(32000, 16, 4));
        assert_eq!(f.current_plan().unwrap().instances, 4);
        // RAM collapses (other apps) → same topology, but the plan must
        // shrink. Same stats (no deviation) but budget forces a replan.
        let d = f.tick(4, &p, &budget(1200, 16, 4));
        match d {
            Decision::Reshape { plan, .. } => {
                assert!(plan.instances < 4, "budget squeeze shrinks instances");
                assert_eq!(plan.mode, Mode::Shared);
            }
            Decision::Hold => panic!("budget change should force a replan"),
        }
    }

    #[test]
    fn current_plan_tracks_committed_shape() {
        let mut f = MicPipelineFsm::new(cfg());
        assert!(f.current_plan().is_none());
        f.tick(1, &[], &budget(8000, 8, 1));
        assert_eq!(f.current_topology(), Some(&Topology::Single));
        assert_eq!(f.current_plan().unwrap().instances, 1);
    }
}
