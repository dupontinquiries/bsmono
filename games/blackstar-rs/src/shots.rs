//! LIVE SHOTS — the projectile verbs stop being hitscan.
//!
//! `weapons::fire` resolved every verb instantly: the volley found its
//! targets in the same call that paid the boost. That is right for a beam
//! and wrong for everything with a flight time — and the two modules that
//! know better already exist with nobody driving them. `ballistics` prices
//! drag, range and falloff per MEDIUM (vacuum carries forever, seawater
//! stops a slug in metres); `guidance` steers a missile under proportional
//! navigation instead of tail-chasing. This module is the pool that flies
//! them: the shell spawns from `fire_weapon`, steps the pool in the fixed
//! loop, and applies impacts through the SAME delta-v -> section-damage
//! conversion the instant path used, so a railshot is worth the same
//! whether it arrived this frame or three later.
//!
//! Who is guided: a Blast-delivery shot fired with a live lock (a target
//! inside the muzzle cone at trigger time) flies as a missile —
//! `guidance::steer(Law::Proportional)`, lateral acceleration clamped by
//! its own turn budget. Everything else flies ballistic under gravity and
//! the medium's drag.
//!
//! GUESSES, labelled: the slug and rocket mass/area/cd below stand in for
//! numbers the weapon catalog does not carry yet; when `gear` grows real
//! projectile specs these constants retire.

use glam::Vec3;

use crate::ballistics::{self, Ambient, Delivery};
use crate::guidance::{self, Law, Missile, Target};
use crate::weapons::Element;

/// A thrown slug, absent real catalog numbers: rifle-ball mass, small
/// frontal area, clean shape. GUESS.
const SLUG: (f32, f32, f32) = (0.02, 3.0e-4, 0.30);
/// A launched charge: heavier, blunter, draggier. GUESS.
const ROCKET: (f32, f32, f32) = (1.2, 8.0e-3, 0.50);
/// How long a rocket motor burns, in seconds. Its flight distance is this
/// times its speed — a separate question from its blast radius, which is
/// what `ballistics::range_m` answers for a Blast.
const ROCKET_BURN_S: f32 = 8.0;
/// A missile's turn budget, m/s² lateral. GUESS sized so a crossing car is
/// catchable and a jinking one is work.
const TURN: f32 = 60.0;
/// Close enough counts, metres.
const LETHAL_R: f32 = 1.6;

pub struct Live {
    pub pos: Vec3,
    pub vel: Vec3,
    pub spec: ballistics::Shot,
    /// delta-v this shot lands on arrival, before falloff
    pub impulse: f32,
    pub element: Element,
    /// travelled so far, for `ballistics::falloff`
    pub travelled: f32,
    /// despawn beyond this — the medium's own effective range
    pub range: f32,
    /// bot index being chased, when this flies as a missile
    pub seek: Option<usize>,
}

pub struct Impact {
    /// bot index, or None for a ground/timeout end
    pub target: Option<usize>,
    pub at: Vec3,
    /// delta-v to land, falloff already applied
    pub dv: Vec3,
    pub element: Element,
    /// what arrived, so the shell maps it to the right damage Kind
    pub delivery: Delivery,
}

/// One target's kinematics, as the pool needs them each step.
#[derive(Clone, Copy)]
pub struct Mark {
    pub pos: Vec3,
    pub vel: Vec3,
    pub r: f32,
}

#[derive(Default)]
pub struct Shots {
    pub live: Vec<Live>,
}

impl Shots {
    /// Spawn from a trigger pull. `seek` is the lock resolved at fire time
    /// — the shell owns aiming, the pool owns flying.
    pub fn spawn(
        &mut self,
        origin: Vec3,
        aim: Vec3,
        delivery: Delivery,
        speed: f32,
        impulse: f32,
        element: Element,
        amb: &Ambient,
        seek: Option<usize>,
    ) {
        let (mass_kg, area_m2, cd) = match delivery {
            Delivery::Blast => ROCKET,
            _ => SLUG,
        };
        let spec = ballistics::Shot { delivery, speed_ms: speed.max(1.0), mass_kg, area_m2, cd };
        // HOW FAR IT FLIES, not how far it hurts. `ballistics::range_m`
        // answers "where has the shot lost most of what it had" — and for
        // a BLAST that is the medium's ability to carry a pressure wave,
        // i.e. the damage radius, about 12 m in air. Using it as the
        // distance the projectile may travel killed every missile at 12 m:
        // a rocket fired at a target 120 m away spent itself a tenth of
        // the way there, which is "the missile spent itself without
        // arriving".
        //
        // A guided rocket's flight distance is what its motor and its life
        // give it, so that is what it gets: speed times burn time. The
        // blast radius stays exactly what it was and is still what
        // `falloff` reads — the two are different questions and this was
        // one function answering both.
        let range = match delivery {
            Delivery::Blast => spec.speed_ms * ROCKET_BURN_S,
            _ => ballistics::range_m(&spec, amb).max(30.0),
        };
        self.live.push(Live {
            pos: origin,
            vel: aim.normalize_or(Vec3::NEG_Z) * spec.speed_ms,
            spec,
            impulse,
            element,
            travelled: 0.0,
            range,
            seek,
        });
    }

    /// One fixed step for every live shot. `g` is the world's gravity
    /// (m/s², down); `ground` its height field; `marks` this frame's
    /// targets, indexed as the shell indexes its bots.
    pub fn step(
        &mut self,
        dt: f32,
        g: f32,
        amb: &Ambient,
        marks: &[Mark],
        ground: &dyn Fn(f32, f32) -> f32,
    ) -> Vec<Impact> {
        let mut out = Vec::new();
        let rho = amb.density();
        self.live.retain_mut(|s| {
            // steering first: a locked blast flies pro-nav at its target,
            // budgeted by TURN — the tail-chase fix guidance.rs exists for
            if let Some(i) = s.seek {
                match marks.get(i) {
                    Some(m) => {
                        let missile = Missile { pos: s.pos, vel: s.vel, turn: TURN, lethal_r: LETHAL_R };
                        // agility is the JINK model's budget inside
                        // `guidance::intercept`; a live mark evades for
                        // real (the pool re-reads its pos/vel every step),
                        // so the modelled budget stays zero here
                        let tgt = Target { pos: m.pos, vel: m.vel, agility: 0.0 };
                        let a = guidance::steer(Law::Proportional, &missile, &tgt);
                        let a = a.clamp_length_max(TURN);
                        s.vel += a * dt;
                        // a rocket holds its pace: renormalize so PN turns
                        // rather than brakes
                        s.vel = s.vel.normalize_or(Vec3::NEG_Z) * s.spec.speed_ms;
                    }
                    None => s.seek = None,
                }
            } else {
                s.vel.y -= g * dt;
            }
            // the medium takes its cut: a = -½ρ·cd·A·v|v| / m
            if rho > 0.0 {
                let v = s.vel.length();
                if v > 0.01 {
                    let drag = 0.5 * rho * s.spec.cd * s.spec.area_m2 * v * v / s.spec.mass_kg;
                    s.vel -= s.vel.normalize() * (drag * dt).min(v);
                }
            }
            let stepv = s.vel * dt;
            s.pos += stepv;
            s.travelled += stepv.length();

            // arrivals: a mark first, then the ground, then the range
            for (i, m) in marks.iter().enumerate() {
                if (s.pos - m.pos).length() < m.r + LETHAL_R {
                    let fall = ballistics::falloff(&s.spec, amb, s.travelled);
                    out.push(Impact {
                        target: Some(i),
                        at: s.pos,
                        dv: s.vel.normalize_or(Vec3::NEG_Z) * (s.impulse * fall),
                        element: s.element,
                        delivery: s.spec.delivery,
                    });
                    return false;
                }
            }
            if s.pos.y <= ground(s.pos.x, s.pos.z) {
                // GROUND IMPACT — same falloff a locked hit gets, not the
                // zero vector this used to report. A Blast landing in the
                // dirt beside a bot should still be able to hurt it once
                // the shell splashes it; a dead `dv` made that impossible
                // however close the miss was.
                let fall = ballistics::falloff(&s.spec, amb, s.travelled);
                out.push(Impact {
                    target: None,
                    at: s.pos,
                    dv: s.vel.normalize_or(Vec3::NEG_Z) * (s.impulse * fall),
                    element: s.element,
                    delivery: s.spec.delivery,
                });
                return false;
            }
            if s.travelled > s.range {
                return false; // spent: past effective range, nothing to report
            }
            true
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_: f32, _: f32) -> f32 {
        -1.0e6 // no floor: flight tests want the air
    }

    #[test]
    fn a_slug_slows_in_air_and_does_not_in_vacuum() {
        let air = Ambient::earthlike();
        let vac = Ambient::vacuum();
        let mut a = Shots::default();
        let mut b = Shots::default();
        a.spawn(Vec3::ZERO, Vec3::X, Delivery::Kinetic, 300.0, 5.0, Element::None, &air, None);
        b.spawn(Vec3::ZERO, Vec3::X, Delivery::Kinetic, 300.0, 5.0, Element::None, &vac, None);
        for _ in 0..120 {
            a.step(1.0 / 120.0, 0.0, &air, &[], &flat);
            b.step(1.0 / 120.0, 0.0, &vac, &[], &flat);
        }
        let (va, vb) = (a.live[0].vel.length(), b.live[0].vel.length());
        assert!(va < vb * 0.98, "air took nothing: {va} vs {vb}");
        assert!((vb - 300.0).abs() < 1.0, "vacuum took something: {vb}");
    }

    #[test]
    fn a_locked_blast_catches_a_crossing_mark() {
        let amb = Ambient::earthlike();
        let mut p = Shots::default();
        // crossing target, classic PN test geometry
        let mut mark = Mark { pos: Vec3::new(120.0, 0.0, 0.0), vel: Vec3::new(0.0, 0.0, 22.0), r: 1.5 };
        p.spawn(Vec3::ZERO, Vec3::X, Delivery::Blast, 90.0, 8.0, Element::None, &amb, Some(0));
        for _ in 0..(6 * 120) {
            mark.pos += mark.vel * (1.0 / 120.0);
            let hits = p.step(1.0 / 120.0, 0.0, &amb, &[mark], &flat);
            if hits.iter().any(|h| h.target == Some(0)) {
                return;
            }
            if p.live.is_empty() {
                panic!("the missile spent itself without arriving");
            }
        }
        panic!("proportional navigation tail-chased after all");
    }

    #[test]
    fn an_unlocked_shot_drops_under_gravity_and_finds_the_floor() {
        let amb = Ambient::earthlike();
        let mut p = Shots::default();
        p.spawn(Vec3::new(0.0, 5.0, 0.0), Vec3::X, Delivery::Kinetic, 60.0, 5.0, Element::None, &amb, None);
        let floor = |_: f32, _: f32| 0.0;
        for _ in 0..(10 * 120) {
            let ends = p.step(1.0 / 120.0, 9.81, &amb, &[], &floor);
            if let Some(e) = ends.first() {
                assert!(e.target.is_none());
                assert!(e.at.y <= 0.5, "ended above the floor: {}", e.at.y);
                return;
            }
        }
        panic!("gravity never brought it down");
    }

    #[test]
    fn falloff_reaches_the_impact() {
        // the same shot lands harder up close than at range
        let amb = Ambient::earthlike();
        let hit_dv = |dist: f32| -> f32 {
            let mut p = Shots::default();
            let m = Mark { pos: Vec3::new(dist, 0.0, 0.0), vel: Vec3::ZERO, r: 1.0 };
            p.spawn(Vec3::ZERO, Vec3::X, Delivery::Kinetic, 300.0, 10.0, Element::None, &amb, None);
            for _ in 0..(5 * 120) {
                if let Some(h) = p.step(1.0 / 120.0, 0.0, &amb, &[m], &flat).first() {
                    return h.dv.length();
                }
            }
            0.0
        };
        let (near, far) = (hit_dv(10.0), hit_dv(200.0));
        assert!(near > 0.0 && far > 0.0, "one of them never landed: {near} {far}");
        assert!(near > far, "range cost nothing: {near} vs {far}");
    }
}
