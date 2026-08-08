//! dispatchpick.rs — choosing among machines that can all do the job.
//!
//! `fleet::candidates` returns everyone capable. With three capable machines the choice decides the
//! throughput, and the obvious rule — pick the fastest — is wrong in two ways that matter.
//!
//! ## The fastest machine is not the fastest answer
//!
//! A machine already running three jobs finishes a fourth later than an idle slower one. **Queue
//! depth beats raw capability** past the first job, and a picker that ignores it produces the
//! classic pathology: every task goes to the best machine, which becomes the bottleneck while the
//! rest idle.
//!
//! ## The best machine for a task may be the only machine for another
//!
//! One device has the target triple and can execute; three can only cross-compile. Spending the
//! native one on a task any of them could do means the verification task waits — so a picker needs
//! to know what a machine is UNIQUELY good for, not only what it is best at.

use crate::deviceplan::{Device, Task};

/// A machine that has already passed `fleet::assign`.
#[derive(Clone, PartialEq, Debug)]
pub struct Candidate {
    pub device: Device,
    /// Jobs already accepted and not finished.
    pub queued: u32,
    /// Is this the only member with the target triple that can execute?
    pub uniquely_capable: bool,
}

/// Link speed at which transfer costs about as much as the job itself.
///
/// The reference point for the ratio, not a threshold: a link twice this fast halves the transfer
/// term and a link half this fast doubles it, continuously.
pub const BASELINE_LINK_MIB_S: f32 = 100.0;

impl Candidate {
    /// Rough seconds before this machine could finish one more job of this task.
    ///
    /// **Queue depth multiplies, capability divides.** Both matter and only one of them is what a
    /// spec sheet shows.
    pub fn eta_s(&self, t: Task, job_s: f32) -> f32 {
        let ahead = self.queued as f32 * job_s;
        // A slower link stretches bandwidth-bound work and leaves a build check alone, which is the
        // same asymmetry deviceplan encodes for admission.
        let transfer = if t.bandwidth_bound() {
            // NO FLOOR. The first version clamped this ratio at 1.0 to avoid a zero, and that made
            // every link at or above the baseline score identically — a 1000 MiB/s link and a 100
            // MiB/s one came out equal, so the picker could not tell them apart at exactly the job
            // it exists to route. A defensive max() erased the discrimination the function is for.
            //
            // Zero link is handled by returning a large cost rather than dividing by it.
            if self.device.link_mib_s <= 0.0 { f32::MAX / 4.0 }
            else { job_s * (BASELINE_LINK_MIB_S / self.device.link_mib_s) }
        } else { self.device.rtt_s };
        ahead + job_s + transfer
    }
}

/// Why a pick was made, so a scheduler's choice can be argued with.
#[derive(Clone, PartialEq, Debug)]
pub struct Pick {
    pub index: usize,
    pub reason: String,
}

/// Choose one, or explain why none.
///
/// Reserving the uniquely-capable machine is checked FIRST and is a refusal to pick it, not a
/// preference against it: if it is the only candidate, it takes the job anyway. A reservation that
/// starves the queue is worse than no reservation.
pub fn pick(cands: &[Candidate], t: Task, job_s: f32) -> Option<Pick> {
    if cands.is_empty() { return None }

    // Everyone who is not the last machine able to VERIFY. Spending it on work any machine could do
    // means the verification task waits behind something that did not need it.
    let reservable: Vec<usize> = cands.iter().enumerate()
        .filter(|(_, c)| !(c.uniquely_capable && t != Task::NativeRun))
        .map(|(i, _)| i).collect();

    let pool: Vec<usize> = if reservable.is_empty() {
        // Everything left is reserved, so the reservation would starve the queue. Take it.
        (0..cands.len()).collect()
    } else { reservable };

    let best = pool.iter().copied()
        .min_by(|&a, &b| cands[a].eta_s(t, job_s)
            .partial_cmp(&cands[b].eta_s(t, job_s))
            .unwrap_or(std::cmp::Ordering::Equal))?;

    let c = &cands[best];
    let reason = if c.queued > 0 {
        format!("{} — {:.0}s eta with {} queued; a faster machine with a longer queue finishes later",
            c.device.name, c.eta_s(t, job_s), c.queued)
    } else if pool.len() < cands.len() {
        format!("{} — idle, and the machine that can RUN the target was held back for work only it \
can verify", c.device.name)
    } else {
        format!("{} — idle, shortest eta at {:.0}s", c.device.name, c.eta_s(t, job_s))
    };
    Some(Pick { index: best, reason })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(name: &str, link: f32, queued: u32, unique: bool) -> Candidate {
        Candidate {
            device: Device {
                name: name.into(), target_triple: "aarch64-unknown-linux-gnu".into(),
                gpu_gib: 24.0, rtt_s: 0.02, link_mib_s: link, can_execute: true,
            },
            queued, uniquely_capable: unique,
        }
    }

    #[test]
    fn a_QUEUED_fast_machine_loses_to_an_idle_slower_one() {
        // The classic pathology a naive picker produces: every task goes to the best machine, which
        // becomes the bottleneck while the rest idle.
        let cs = vec![cand("fast", 1000.0, 3, false), cand("idle", 200.0, 0, false)];
        assert_eq!(pick(&cs, Task::BuildCheck, 10.0).unwrap().index, 1);
    }

    #[test]
    fn with_EQUAL_queues_the_faster_link_wins_for_bandwidth_bound_work() {
        let cs = vec![cand("slow", 100.0, 0, false), cand("fast", 1000.0, 0, false)];
        assert_eq!(pick(&cs, Task::Inference, 10.0).unwrap().index, 1);
    }

    #[test]
    fn a_ZERO_link_is_costed_as_unusable_rather_than_dividing_by_zero() {
        // The case the discarded max() was defending against, handled where it belongs.
        let mut c = cand("dead", 0.0, 0, false);
        c.device.link_mib_s = 0.0;
        assert!(c.eta_s(Task::Inference, 10.0) > 1e6);
    }

    #[test]
    fn link_speed_discriminates_CONTINUOUSLY_and_not_only_below_a_threshold() {
        // The bug the first version had: a floor at the baseline made every fast link identical.
        let fast = cand("fast", 1000.0, 0, false);
        let mid = cand("mid", 400.0, 0, false);
        assert!(fast.eta_s(Task::Inference, 10.0) < mid.eta_s(Task::Inference, 10.0));
    }

    #[test]
    fn the_link_speed_does_NOT_decide_a_build_check() {
        // A build ships source once; deviceplan encodes that for admission and the picker must not
        // contradict it by preferring bandwidth anyway.
        let cs = vec![cand("slowlink", 10.0, 0, false), cand("fastlink", 1000.0, 1, false)];
        assert_eq!(pick(&cs, Task::BuildCheck, 10.0).unwrap().index, 0,
            "the idle machine wins on queue, not on link");
    }

    #[test]
    fn the_UNIQUELY_CAPABLE_machine_is_held_back_from_work_others_could_do() {
        // One device has the triple and can execute; spending it on a cross-compile means the
        // verification task waits behind something that never needed it.
        let cs = vec![cand("only-runner", 1000.0, 0, true), cand("builder", 1000.0, 0, false)];
        let p = pick(&cs, Task::BuildCheck, 10.0).unwrap();
        assert_eq!(p.index, 1);
        assert!(p.reason.contains("held back for work only it can verify"));
    }

    #[test]
    fn it_IS_picked_when_the_task_actually_needs_it() {
        let cs = vec![cand("only-runner", 1000.0, 0, true), cand("builder", 1000.0, 0, false)];
        assert_eq!(pick(&cs, Task::NativeRun, 10.0).unwrap().index, 0);
    }

    #[test]
    fn a_reservation_that_would_STARVE_the_queue_is_dropped() {
        // If the reserved machine is the only candidate, holding it back schedules nothing — and a
        // reservation that starves the queue is worse than no reservation.
        let cs = vec![cand("only-runner", 1000.0, 0, true)];
        assert_eq!(pick(&cs, Task::BuildCheck, 10.0).unwrap().index, 0);
    }

    #[test]
    fn an_EMPTY_candidate_set_returns_None_rather_than_panicking() {
        assert!(pick(&[], Task::BuildCheck, 10.0).is_none());
    }

    #[test]
    fn the_REASON_names_the_queue_when_that_is_what_decided_it() {
        // A scheduler's choice has to be arguable, or an operator watching the wrong machine get
        // every job cannot tell whether it is a bug.
        let cs = vec![cand("a", 1000.0, 2, false), cand("b", 1000.0, 5, false)];
        assert!(pick(&cs, Task::BuildCheck, 10.0).unwrap().reason.contains("2 queued"));
    }
}
