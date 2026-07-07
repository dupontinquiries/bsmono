//! `sim::fauna_mind` — the per-creature cognition tick that finally gives the
//! orphaned prey-cognition kernels a real caller.
//!
//! W1 wiring (r99): several pure cognition kernels existed with no caller. This
//! module is the small *feature* that composes them into one coherent
//! perceive→remember→decide→steer loop, and registers it as a
//! [`crate::sim::service_registry`] service so the smoke test ticks the whole
//! composition. The kernels it calls (previously orphaned):
//! - [`crate::sim::perception::perceive`] — awareness + "am I being watched".
//! - [`crate::sim::roar_fear::fear`] — fear spike from a nearby roar.
//! - [`crate::sim::threat_memory::update`] — EMA of remembered threat.
//! - [`crate::sim::mob_memory::MobMemory`] — remember/decay/recall threat places.
//! - [`crate::sim::flee_decision::should_flee`] — fight-or-flee.
//! - [`crate::sim::herd_cohesion::herd_steer`] — stay-with-the-group steering.
//!
//! Pure and deterministic (data in, data out); no `crate::render`, no GPU, no IO,
//! no clock, no RNG, no `.unwrap()`/`.expect()` in non-test code. The service
//! adapter ticks with neutral senses (player far, calm) — that proves the call
//! path links and runs; real per-frame senses are supplied by the caller.

use crate::sim::flee_decision;
use crate::sim::herd_cohesion;
use crate::sim::mob_memory::MobMemory;
use crate::sim::perception::{self, PerceptParams, Perception};
use crate::sim::roar_fear;
use crate::sim::threat_memory;

/// Per-tick sensory input a mind reacts to (the caller fills this from the world).
#[derive(Clone, Copy, Debug)]
pub struct WorldSense {
    pub player_pos: [f32; 3],
    pub player_yaw: f32,
    /// Heard noise `[0,1]`.
    pub noise: f32,
    /// Clear line of sight to the player (occlusion test done by the caller).
    pub has_los: bool,
    /// A roar event this tick: source power, distance, and audible range.
    pub roar_power: f32,
    pub roar_dist: f32,
    pub roar_range: f32,
    /// Herd context (XZ plane): the group's centroid and this creature's nearest
    /// neighbour, for cohesion/separation steering.
    pub herd_centroid: [f32; 2],
    pub nearest_neighbor: [f32; 2],
}

impl WorldSense {
    /// A calm, neutral sense: player far away, silent, no roar, alone. Ticking
    /// with this exercises every kernel without asserting any particular world.
    pub fn neutral() -> WorldSense {
        WorldSense {
            player_pos: [1.0e6, 0.0, 1.0e6],
            player_yaw: 0.0,
            noise: 0.0,
            has_los: false,
            roar_power: 0.0,
            roar_dist: 1.0e6,
            roar_range: 1.0,
            herd_centroid: [0.0, 0.0],
            nearest_neighbor: [1.0e6, 1.0e6],
        }
    }
}

/// What the mind decided this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MindOutput {
    pub awareness: f32,
    pub being_watched: bool,
    /// Remembered threat after this tick `[0,1]`.
    pub threat: f32,
    pub fleeing: bool,
    /// XZ steering vector (cohesion + separation).
    pub steer: [f32; 2],
}

/// How fast remembered threat tracks observation, per second.
const LEARN_PER_SEC: f32 = 3.0;
/// How fast remembered-place salience fades, per second.
const DECAY_PER_SEC: f32 = 0.02;
/// Observed-threat level above which a threat *place* is committed to memory.
const REMEMBER_THRESHOLD: f32 = 0.5;
/// Maps remembered threat `[0,1]` onto the `threat_power` scale `should_flee`
/// compares against `hp + courage`.
const THREAT_SCALE: f32 = 2.0;
/// Herd separation radius (XZ units).
const SEP_R: f32 = 3.0;
/// Landmark id used for "a threat was here" memories.
const THREAT_LANDMARK: u32 = 1;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// One creature's mind: pose + disposition + remembered threat + place memory.
#[derive(Clone, Debug)]
pub struct FaunaMind {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub size: f32,
    /// Health `[0,1]` and bravery `[0,1]` (drive fight-or-flee + roar resolve).
    pub hp: f32,
    pub courage: f32,
    /// Remembered threat `[0,1]` (EMA).
    pub threat: f32,
    pub memory: MobMemory,
    pub params: PerceptParams,
    tick: u64,
}

impl FaunaMind {
    pub fn new(pos: [f32; 3], hp: f32, courage: f32) -> FaunaMind {
        FaunaMind {
            pos,
            yaw: 0.0,
            size: 1.0,
            hp: clamp01(hp),
            courage: clamp01(courage),
            threat: 0.0,
            memory: MobMemory::new(8),
            params: PerceptParams::default(),
            tick: 0,
        }
    }

    /// Advance the mind one tick against `sense`. Composes perception → threat
    /// memory (+ roar) → place memory → flee decision → herd steering.
    pub fn step(&mut self, sense: &WorldSense, dt: f32) -> MindOutput {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };

        // 1) Perceive the player.
        let p: Perception = perception::perceive(
            sense.player_pos,
            sense.player_yaw,
            self.pos,
            self.yaw,
            self.size,
            sense.noise,
            sense.has_los,
            &self.params,
        );

        // 2) Blend in any roar fear (resolve grows with courage + health).
        let resolve = clamp01(self.courage * 0.5 + self.hp * 0.5);
        let roar = roar_fear::fear(sense.roar_dist, sense.roar_range, sense.roar_power, resolve);
        let observed = clamp01(p.awareness.max(roar));

        // 3) Update remembered threat (EMA) and commit a threat place if strong.
        let learn = clamp01(dt * LEARN_PER_SEC);
        self.threat = threat_memory::update(self.threat, observed, learn);
        if observed > REMEMBER_THRESHOLD {
            self.memory.remember(THREAT_LANDMARK, sense.player_pos, observed, self.tick);
        }
        self.memory.decay(dt * DECAY_PER_SEC);

        // 4) Fight or flee on the remembered threat.
        let fleeing = flee_decision::should_flee(self.hp, self.threat * THREAT_SCALE, self.courage);

        // 5) Herd steering in the XZ plane.
        let steer = herd_cohesion::herd_steer(
            [self.pos[0], self.pos[2]],
            sense.herd_centroid,
            sense.nearest_neighbor,
            SEP_R,
        );

        self.tick = self.tick.wrapping_add(1);
        MindOutput { awareness: p.awareness, being_watched: p.being_watched, threat: self.threat, fleeing, steer }
    }

    /// Recall the nearest remembered threat place within `radius`, if any.
    pub fn recalled_threat(&self, radius: f32) -> Option<[f32; 3]> {
        self.memory.recall(self.pos, radius).map(|m| m.pos)
    }
}

// ── service registration (W1 r99): tick the whole composition each frame ─────
struct FaunaMindService(FaunaMind);
impl crate::sim::service_registry::Service for FaunaMindService {
    fn name(&self) -> &'static str {
        "fauna_mind"
    }
    fn tick(&mut self, ctx: &crate::sim::service_registry::ServiceCtx) {
        // Neutral senses prove the call path links + runs panic-free; the real
        // game supplies live senses via FaunaMind::step.
        let s = WorldSense::neutral();
        let _ = self.0.step(&s, ctx.dt);
    }
}
#[linkme::distributed_slice(crate::sim::service_registry::SERVICES)]
static FAUNA_MIND_SERVICE: crate::sim::service_registry::ServiceEntry =
    crate::sim::service_registry::ServiceEntry {
        name: "fauna_mind",
        make: || Box::new(FaunaMindService(FaunaMind::new([0.0, 64.0, 0.0], 1.0, 0.5))),
    };

#[cfg(test)]
mod tests {
    use super::*;

    fn threat_sense(player_xz: [f32; 2]) -> WorldSense {
        WorldSense {
            player_pos: [player_xz[0], 64.0, player_xz[1]],
            player_yaw: 0.0,
            noise: 0.9,
            has_los: true,
            roar_power: 0.0,
            roar_dist: 1.0e6,
            roar_range: 1.0,
            herd_centroid: [10.0, 0.0],
            nearest_neighbor: [1.0e6, 1.0e6],
        }
    }

    #[test]
    fn calm_world_keeps_low_threat_and_no_flee() {
        let mut m = FaunaMind::new([0.0, 64.0, 0.0], 1.0, 0.5);
        let calm = WorldSense::neutral();
        let mut out = m.step(&calm, 1.0 / 60.0);
        for _ in 0..300 {
            out = m.step(&calm, 1.0 / 60.0);
        }
        assert!(out.threat < 0.1, "calm world ⇒ low remembered threat, got {}", out.threat);
        assert!(!out.fleeing, "healthy + calm ⇒ no flee");
        assert!(!out.being_watched, "distant player ⇒ not watched");
    }

    #[test]
    fn close_watching_threat_raises_awareness_and_memory() {
        let mut m = FaunaMind::new([0.0, 64.0, 0.0], 1.0, 0.5);
        let near = threat_sense([2.0, 0.0]); // player 2 units away, loud, LOS
        let first = m.step(&near, 1.0 / 60.0);
        let mut last = first;
        for _ in 0..120 {
            last = m.step(&near, 1.0 / 60.0);
        }
        assert!(last.awareness > 0.3, "close, loud, visible ⇒ aware, got {}", last.awareness);
        assert!(last.threat > first.threat, "remembered threat should climb");
        // a threat place was committed and is recallable nearby
        assert!(m.recalled_threat(50.0).is_some(), "should remember the threat place");
    }

    #[test]
    fn low_health_under_threat_decides_to_flee() {
        let mut m = FaunaMind::new([0.0, 64.0, 0.0], 0.1, 0.1); // frail, timid
        let near = threat_sense([1.5, 0.0]);
        let mut out = m.step(&near, 1.0 / 60.0);
        for _ in 0..120 {
            out = m.step(&near, 1.0 / 60.0);
        }
        assert!(out.fleeing, "frail creature under sustained threat should flee");
    }

    #[test]
    fn roar_alone_spikes_threat() {
        let mut m = FaunaMind::new([0.0, 64.0, 0.0], 1.0, 0.2);
        let mut s = WorldSense::neutral();
        s.roar_power = 1.0;
        s.roar_dist = 2.0;
        s.roar_range = 20.0;
        let mut out = m.step(&s, 1.0 / 60.0);
        for _ in 0..120 {
            out = m.step(&s, 1.0 / 60.0);
        }
        assert!(out.threat > 0.2, "a close roar should raise remembered threat, got {}", out.threat);
    }

    #[test]
    fn herd_steer_pulls_toward_centroid() {
        let mut m = FaunaMind::new([0.0, 64.0, 0.0], 1.0, 0.5);
        let mut s = WorldSense::neutral();
        s.herd_centroid = [10.0, 0.0]; // group is to +X
        let out = m.step(&s, 1.0 / 60.0);
        assert!(out.steer[0] > 0.0, "should steer toward the herd centroid (+X), got {:?}", out.steer);
    }

    #[test]
    fn deterministic_and_finite() {
        let run = || {
            let mut m = FaunaMind::new([0.0, 64.0, 0.0], 0.7, 0.4);
            let near = threat_sense([3.0, 1.0]);
            let mut outs = Vec::new();
            for i in 0..200 {
                let s = if i % 2 == 0 { near } else { WorldSense::neutral() };
                outs.push(m.step(&s, 1.0 / 60.0));
            }
            outs
        };
        let a = run();
        let b = run();
        assert_eq!(a, b, "same inputs ⇒ same outputs");
        for o in a {
            assert!(o.threat.is_finite() && o.threat >= 0.0 && o.threat <= 1.0);
            assert!(o.awareness.is_finite());
            assert!(o.steer[0].is_finite() && o.steer[1].is_finite());
        }
    }

    #[test]
    fn service_adapter_ticks_panic_free() {
        use crate::sim::service_registry::{Service, ServiceCtx};
        let mut svc = FaunaMindService(FaunaMind::new([0.0, 64.0, 0.0], 1.0, 0.5));
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..240 {
            svc.tick(&ctx);
        }
        assert_eq!(svc.name(), "fauna_mind");
    }
}
