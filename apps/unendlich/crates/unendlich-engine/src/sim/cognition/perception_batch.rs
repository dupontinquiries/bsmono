//! Perception batching — a cognition-LOD queue so a populated world can afford
//! to think.
//!
//! Running every brain every tick (line-of-sight per eye, sound occlusion per
//! source) is the expensive part of perception. This is its level-of-detail, the
//! direct analogue of chunk LOD (`UNENDLICH_LOD_FACTORS = 1,2,4,8`): a brain near
//! the observer thinks every tick; one farther out thinks every 2nd / 4th / 8th.
//! Between its think-ticks the stimuli it would have reacted to are **queued**
//! and **labelled** (sound vs sight), and on a think-tick the whole window is
//! drained and **bulk-reduced per label** — the loudest sound, the strongest
//! sighting — instead of replaying it tick by tick. The think-cadence can
//! **cascade** live: move a brain to a coarser band (or, for the caller, snap an
//! aroused one back to `Near`) and the stride changes on the next tick.
//!
//! This lives in the non-rolled-back cognition layer: it holds per-brain state
//! and emits a compact [`Batch`] that the deterministic core consumes as input.

use crate::sim::fixed::Vec3;
use crate::sim::sentinel::{Senses, Sentinel};
use crate::sim::sound::Loudness;

/// The label on a queued datum — what lets a batch be bulk-processed (all
/// sounds folded to the loudest, all sights to the strongest) rather than
/// handled event by event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StimulusKind {
    Sound,
    Sight,
}

/// One labelled perception event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stimulus {
    pub kind: StimulusKind,
    pub source: Vec3,
    /// Loudness for `Sound`; a positive confidence for `Sight`. Higher wins the
    /// per-label reduction.
    pub salience: i32,
}

/// Cognition level of detail → how many ticks a brain batches between thinks.
/// Mirrors the renderer's 1/2/4/8 LOD factors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CogLod {
    #[default]
    Near,
    Mid,
    Far,
    VFar,
}

impl CogLod {
    /// Ticks between think-ticks at this level.
    pub fn stride(self) -> u32 {
        match self {
            CogLod::Near => 1,
            CogLod::Mid => 2,
            CogLod::Far => 4,
            CogLod::VFar => 8,
        }
    }

    /// Cascade by squared distance against three squared band edges
    /// (`[near, mid, far]`). Beyond the last edge → `VFar`.
    pub fn from_distance2(dist2: i64, edges2: [i64; 3]) -> CogLod {
        if dist2 < edges2[0] {
            CogLod::Near
        } else if dist2 < edges2[1] {
            CogLod::Mid
        } else if dist2 < edges2[2] {
            CogLod::Far
        } else {
            CogLod::VFar
        }
    }
}

/// What a think-tick produces: the window length (for decay catch-up) plus the
/// bulk-reduced strongest stimulus of each label.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Batch {
    /// Ticks this batch covers (= the elapsed window). The consumer advances its
    /// decay/cooldowns by this so a coarse-LOD brain stays in step with time.
    pub ticks: u32,
    pub sound: Option<Stimulus>,
    pub sight: Option<Stimulus>,
}

/// One brain's accumulator + LOD scheduler. Server-side cognition state (not
/// part of the rolled-back core), so a plain bounded `Vec` queue is fine.
#[derive(Debug, Clone, PartialEq)]
pub struct PerceptionQueue {
    lod: CogLod,
    phase: u32,
    elapsed: u32,
    queue: Vec<Stimulus>,
    cap: usize,
}

impl PerceptionQueue {
    pub fn new(lod: CogLod) -> PerceptionQueue {
        Self::with_capacity(lod, 32)
    }

    pub fn with_capacity(lod: CogLod, cap: usize) -> PerceptionQueue {
        PerceptionQueue { lod, phase: 0, elapsed: 0, queue: Vec::new(), cap: cap.max(1) }
    }

    pub fn lod(&self) -> CogLod {
        self.lod
    }

    /// Change the think-cadence live. The new stride takes effect on the next
    /// [`tick`](Self::tick) — coarser → longer windows, finer → drains sooner.
    pub fn set_lod(&mut self, lod: CogLod) {
        self.lod = lod;
    }

    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Queue a labelled stimulus. Bounded: at capacity it evicts the
    /// weakest-salience entry only if the newcomer is stronger, so the loudest
    /// events are never lost to a flood of quiet ones.
    pub fn push(&mut self, s: Stimulus) {
        if self.queue.len() < self.cap {
            self.queue.push(s);
            return;
        }
        if let Some((i, weakest)) = self.queue.iter().enumerate().min_by_key(|(_, q)| q.salience) {
            if s.salience > weakest.salience {
                self.queue[i] = s;
            }
        }
    }

    /// Advance one global tick. On a think-tick (every `stride`) drains the
    /// queue and returns the bulk-reduced [`Batch`]; otherwise keeps
    /// accumulating and returns `None`.
    pub fn tick(&mut self) -> Option<Batch> {
        self.phase += 1;
        self.elapsed += 1;
        if self.phase < self.lod.stride() {
            return None;
        }
        let mut sound: Option<Stimulus> = None;
        let mut sight: Option<Stimulus> = None;
        for s in &self.queue {
            let slot = match s.kind {
                StimulusKind::Sound => &mut sound,
                StimulusKind::Sight => &mut sight,
            };
            if slot.map_or(true, |cur| s.salience > cur.salience) {
                *slot = Some(*s);
            }
        }
        let batch = Batch { ticks: self.elapsed, sound, sight };
        self.phase = 0;
        self.elapsed = 0;
        self.queue.clear();
        Some(batch)
    }
}

/// Drive a [`Sentinel`] from a think-tick batch: feed the reduced sight/sound,
/// then advance its alarm decay across the whole window so a coarse-LOD sentinel
/// calms on the same wall-clock as a near one.
pub fn drive_sentinel(sentinel: &mut Sentinel, batch: &Batch, senses: &Senses) {
    let sighted = batch.sight.map(|s| s.source);
    let heard = batch.sound.map(|s| (s.source, s.salience as Loudness));
    sentinel.perceive(sighted, heard, senses);
    for _ in 0..batch.ticks {
        sentinel.tick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;

    fn snd(salience: i32) -> Stimulus {
        Stimulus { kind: StimulusKind::Sound, source: Vec3::new(Fixed::from_int(1), Fixed::ZERO, Fixed::ZERO), salience }
    }
    fn sgt(salience: i32) -> Stimulus {
        Stimulus { kind: StimulusKind::Sight, source: Vec3::new(Fixed::from_int(2), Fixed::ZERO, Fixed::ZERO), salience }
    }

    #[test]
    fn near_lod_drains_every_tick() {
        let mut q = PerceptionQueue::new(CogLod::Near);
        q.push(snd(10));
        let b = q.tick().expect("near thinks every tick");
        assert_eq!(b.ticks, 1);
        assert_eq!(b.sound.unwrap().salience, 10);
    }

    #[test]
    fn far_lod_batches_four_ticks_and_keeps_the_loudest() {
        let mut q = PerceptionQueue::new(CogLod::Far); // stride 4
        q.push(snd(10));
        assert!(q.tick().is_none());
        q.push(snd(30));
        assert!(q.tick().is_none());
        q.push(snd(20));
        assert!(q.tick().is_none());
        let b = q.tick().expect("drains on the 4th tick");
        assert_eq!(b.ticks, 4, "window covers all four ticks");
        assert_eq!(b.sound.unwrap().salience, 30, "bulk-reduced to the loudest");
    }

    #[test]
    fn labels_are_reduced_independently() {
        let mut q = PerceptionQueue::new(CogLod::Near);
        q.push(snd(40));
        q.push(sgt(5));
        q.push(snd(10));
        let b = q.tick().unwrap();
        assert_eq!(b.sound.unwrap().salience, 40);
        assert_eq!(b.sight.unwrap().salience, 5, "sight kept separately from sound");
    }

    #[test]
    fn cascading_to_a_finer_lod_drains_sooner() {
        let mut q = PerceptionQueue::new(CogLod::Far); // stride 4
        q.push(snd(7));
        assert!(q.tick().is_none()); // phase 1
        assert!(q.tick().is_none()); // phase 2
        q.set_lod(CogLod::Near); // cascade up — react now
        let b = q.tick().expect("finer LOD drains immediately");
        assert_eq!(b.ticks, 3, "still accounts for the whole elapsed window");
        assert_eq!(b.sound.unwrap().salience, 7);
    }

    #[test]
    fn capacity_keeps_the_strongest_under_a_flood() {
        let mut q = PerceptionQueue::with_capacity(CogLod::Far, 2);
        for s in [1, 2, 3, 4] {
            q.push(snd(s));
        }
        // 4 ticks to drain stride-4; nothing else pushed.
        for _ in 0..3 {
            assert!(q.tick().is_none());
        }
        let b = q.tick().unwrap();
        assert_eq!(b.sound.unwrap().salience, 4, "the loudest survived the cap");
    }

    #[test]
    fn distance_cascades_through_the_bands() {
        let edges = [100, 400, 900];
        assert_eq!(CogLod::from_distance2(50, edges), CogLod::Near);
        assert_eq!(CogLod::from_distance2(200, edges), CogLod::Mid);
        assert_eq!(CogLod::from_distance2(500, edges), CogLod::Far);
        assert_eq!(CogLod::from_distance2(5000, edges), CogLod::VFar);
        assert_eq!(CogLod::Near.stride(), 1);
        assert_eq!(CogLod::VFar.stride(), 8);
    }

    #[test]
    fn a_far_sentinel_still_alarms_from_its_batch() {
        use crate::sim::sentinel::{Posture, Sentinel};
        let senses = Senses::bird();
        let mut q = PerceptionQueue::new(CogLod::VFar); // stride 8
        let mut sen = Sentinel::new();
        // A loud sound arrives during the window.
        q.push(snd(senses.hear_threshold * 3));
        let mut fired = None;
        for _ in 0..8 {
            if let Some(b) = q.tick() {
                fired = Some(b);
            }
        }
        let b = fired.expect("VFar drains on the 8th tick");
        assert_eq!(b.ticks, 8);
        drive_sentinel(&mut sen, &b, &senses);
        assert_eq!(sen.posture, Posture::Alarmed, "loud batch alarms the sentinel");
    }
}
