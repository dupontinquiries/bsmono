//! The per-creature scheduler that ties perception together.
//!
//! A [`SentinelAgent`] bundles a [`Sentinel`] brain, its [`PerceptionQueue`], and
//! its [`Senses`], and runs one tick of the whole loop: pick this brain's
//! cognition LOD from its distance to the observer, enqueue this tick's labelled
//! perception, tick the queue, and — on a think-tick — drive the sentinel from
//! the bulk-reduced batch. The cascade has a state override: a brain that is not
//! `Calm` snaps to `Near` so a creature that has *noticed* something reacts every
//! tick, no matter how far out it is, then relaxes back down its distance band
//! once it settles.
//!
//! Pure: the caller (which holds the world) supplies this tick's perception
//! results — `sighted` from `vision::can_see` per living eye, `heard` from
//! `sound::heard_loudness_world` — and gets back the current [`Alarm`] (the
//! reveal) to surface to the player.

use crate::sim::cognition::perception_batch::{drive_sentinel, CogLod, PerceptionQueue, Stimulus, StimulusKind};
use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::physics::WorldQuery;
use crate::sim::sentinel::{Alarm, Posture, Senses, Sentinel};
use crate::sim::sound::{heard_loudness_world, SoundSource};
use crate::sim::sound::Loudness;
use crate::sim::vision;

/// Salience assigned to a sighting (any sighting alarms; the value only orders
/// competing sightings, nearest-strongest, within the Sight label).
const SIGHT_CONFIDENCE: i32 = 1000;

/// A brain-bearing creature's perception agent: brain + queue + senses + the
/// squared distance band edges that set its LOD.
#[derive(Debug, Clone, PartialEq)]
pub struct SentinelAgent {
    pub sentinel: Sentinel,
    pub queue: PerceptionQueue,
    pub senses: Senses,
    /// Squared distance edges `[near, mid, far]` for the cognition LOD cascade.
    pub bands2: [i64; 3],
}

impl SentinelAgent {
    pub fn new(senses: Senses, bands2: [i64; 3]) -> SentinelAgent {
        SentinelAgent { sentinel: Sentinel::new(), queue: PerceptionQueue::new(CogLod::Near), senses, bands2 }
    }

    /// Compose this tick's perception against the world: for an agent at `pos`
    /// with eyes facing `eye_facings` (from `Anatomy::eye_facings`), find the
    /// nearest threat any eye can see (cone + line-of-sight) and the loudest
    /// occluded sound. `threats` are `(position, emitted loudness)`. The result
    /// feeds straight into [`observe_tick`](Self::observe_tick).
    pub fn sense_threats<W: WorldQuery>(
        &self,
        pos: Vec3,
        eye_facings: &[(f32, f32)],
        threats: &[(Vec3, Loudness)],
        world: &W,
    ) -> (Option<Vec3>, Option<(Vec3, Loudness)>) {
        let mut sighted: Option<Vec3> = None;
        let mut best_d2 = Fixed::from_int(i32::MAX as i64);
        for &(tpos, _) in threats {
            let seen = eye_facings.iter().any(|&(fx, fz)| {
                vision::can_see(
                    pos,
                    Fixed::from_f64(fx as f64),
                    Fixed::from_f64(fz as f64),
                    tpos,
                    self.senses.sight_range,
                    self.senses.fov_cos2,
                    world,
                )
            });
            if seen {
                let d = tpos - pos;
                let d2 = d.x * d.x + d.y * d.y + d.z * d.z;
                if d2 < best_d2 {
                    best_d2 = d2;
                    sighted = Some(tpos);
                }
            }
        }

        let mut heard: Option<(Vec3, Loudness)> = None;
        let mut best_loud = 0;
        for &(tpos, base) in threats {
            let loud = heard_loudness_world(SoundSource { pos: tpos, loudness: base }, pos, world);
            if loud > best_loud {
                best_loud = loud;
                heard = Some((tpos, loud));
            }
        }
        (sighted, heard)
    }

    /// The LOD this agent will use this tick: its distance band, unless it's
    /// already reacting (not `Calm`), in which case it snaps to `Near`.
    pub fn lod_for(&self, observer_dist2: i64) -> CogLod {
        if self.sentinel.posture != Posture::Calm {
            CogLod::Near
        } else {
            CogLod::from_distance2(observer_dist2, self.bands2)
        }
    }

    /// Run one tick. `observer_dist2` is squared distance to the player/camera;
    /// `sighted` / `heard` are this tick's perception (the caller ran
    /// `can_see` / `heard_loudness_world`). Returns the current reveal, if any.
    pub fn observe_tick(
        &mut self,
        observer_dist2: i64,
        sighted: Option<Vec3>,
        heard: Option<(Vec3, Loudness)>,
    ) -> Option<Alarm> {
        let lod = self.lod_for(observer_dist2);
        self.queue.set_lod(lod);

        if let Some(p) = sighted {
            self.queue.push(Stimulus { kind: StimulusKind::Sight, source: p, salience: SIGHT_CONFIDENCE });
        }
        if let Some((p, loud)) = heard {
            self.queue.push(Stimulus { kind: StimulusKind::Sound, source: p, salience: loud });
        }

        if let Some(batch) = self.queue.tick() {
            drive_sentinel(&mut self.sentinel, &batch, &self.senses);
        }
        self.sentinel.alarm()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fixed::Fixed;
    use crate::sim::sentinel::Sense;

    fn at(x: i64, z: i64) -> Vec3 {
        Vec3::new(Fixed::from_int(x), Fixed::ZERO, Fixed::from_int(z))
    }
    fn agent() -> SentinelAgent {
        // near < 100, mid < 400, far < 900 (squared).
        SentinelAgent::new(Senses::bird(), [100, 400, 900])
    }

    #[test]
    fn nearby_loud_sound_alarms_this_tick() {
        let mut a = agent();
        let loud = a.senses.hear_threshold * 3;
        let alarm = a.observe_tick(25, None, Some((at(3, 0), loud)));
        let alarm = alarm.expect("near agent thinks every tick → alarms now");
        assert_eq!(alarm.via, Sense::Hearing);
        assert_eq!(a.sentinel.posture, Posture::Alarmed);
    }

    #[test]
    fn far_quiet_agent_uses_a_coarse_lod_and_stays_calm() {
        let mut a = agent();
        assert_eq!(a.lod_for(5000), CogLod::VFar);
        for _ in 0..7 {
            assert!(a.observe_tick(5000, None, None).is_none());
        }
        assert_eq!(a.sentinel.posture, Posture::Calm);
    }

    #[test]
    fn arousal_cascades_the_lod_back_to_near() {
        let mut a = agent();
        // Far away, but a sighting drains (sight batches like anything else)…
        // force the alarm first by sighting at near range.
        a.observe_tick(10, Some(at(1, 0)), None);
        assert_eq!(a.sentinel.posture, Posture::Alarmed);
        // Now even at VFar distance it stays on Near while reacting.
        assert_eq!(a.lod_for(10_000), CogLod::Near, "an alarmed brain reacts every tick");
    }

    #[test]
    fn a_far_agent_eventually_alarms_from_a_batched_loud_sound() {
        let mut a = agent();
        let loud = a.senses.hear_threshold * 3;
        let mut fired = None;
        // VFar (stride 8): the loud sound this tick is queued and only acted on
        // when the window drains.
        for i in 0..8 {
            let heard = if i == 0 { Some((at(20, 0), loud)) } else { None };
            if let Some(al) = a.observe_tick(5000, None, heard) {
                fired = Some(al);
            }
        }
        let al = fired.expect("the batch drained on the 8th tick");
        assert_eq!(al.posture, Posture::Alarmed);
        assert_eq!(al.via, Sense::Hearing);
    }

    #[test]
    fn sense_threats_sees_ahead_and_hears_around() {
        use crate::sim::physics::SceneWorld;
        let a = agent();
        let world = SceneWorld::new(); // open air → clear line of sight
        let eyes = [(1.0f32, 0.0f32)]; // one eye facing +X
        let loud = a.senses.hear_threshold * 3;

        // A threat dead ahead is both seen and heard.
        let ahead = at(5, 0);
        let (sighted, heard) = a.sense_threats(at(0, 0), &eyes, &[(ahead, loud)], &world);
        assert_eq!(sighted, Some(ahead), "an eye facing +X sees a threat ahead");
        assert!(heard.is_some(), "a loud threat is heard");

        // A threat behind the eye is out of the cone — unseen, but still audible.
        let behind = at(-5, 0);
        let (s2, h2) = a.sense_threats(at(0, 0), &eyes, &[(behind, loud)], &world);
        assert_eq!(s2, None, "behind the facing → unseen");
        assert!(h2.is_some(), "hearing is omnidirectional");
    }
}
