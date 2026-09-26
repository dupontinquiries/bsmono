//! signature — what makes Oldroar's nova look like frost and Stillsmoke's
//! look like a thunderhead.
//!
//! Every ability used to be the same ring of cubes in `AbilityDef::color`.
//! Thirteen ancestors, fifty-two abilities, one visual. The archetype effects
//! (`Event::NovaFx`, `BeamFx`, ...) stay — they are the GAMEPLAY read: where
//! the damage is. This adds the IDENTITY read on top, from the one event that
//! says who cast: `Event::Cast`.
//!
//! ## Shape
//!
//! [`Motif`] is per ancestor: an element with its own motion law, palette
//! ramp and residue. [`Gesture`] is per ability archetype: where the motif
//! is thrown — around the caster, along the aim, at the target point, or up
//! the body. `emit(motif, gesture, ..)` is the product, so one ancestor's
//! four abilities share an element and differ in gesture, and two ancestors
//! with the same archetype share a gesture and differ in element.
//!
//! Pure: particles out, no `Vfx`, no GPU. That is what lets
//! `tests::every_motif_is_measurably_its_own` hold the thirteen apart by
//! measurement instead of by taste.
//!
//! ## Budget
//!
//! A cast emits at most [`MAX_PER_CAST`] particles. Ten cats casting at once
//! on a PinePhone is the case to survive, and the particle buffer is one
//! instanced draw, so count is the whole cost.

use glam::Vec3;

use crate::gods::Archetype;
use crate::vfx::Particle;

pub const MAX_PER_CAST: usize = 96;

/// An ancestor's element. Order matches `gods::ROSTER` ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motif {
    /// Goldenmane — a roar made visible: straight golden spokes, embers.
    Sunroar,
    /// Duskstripe — three parallel claw rakes, orange over black.
    Rake,
    /// Rainspots — rain falling INTO the spot, rosette rings on the ground.
    Rainfall,
    /// Nightpelt — shadow that implodes toward the caster.
    Umbra,
    /// Windclaw — a rising helix.
    Gyre,
    /// Faryowl — arcs of sound travelling down the aim.
    Yowl,
    /// Oldroar — frost shards: fast out, then hanging.
    Rime,
    /// Twofang — two fangs stabbing down at the mark.
    Fangs,
    /// Suneye — a standing halo and sand at the paws.
    Halo,
    /// Snowhush — a dome of slow snow. Nearly still.
    Hush,
    /// Riptail — a whip of water sweeping across, and its splash.
    Lash,
    /// Stillsmoke — a jagged bolt and a column of smoke.
    Thunderhead,
    /// Stonewhisper — a fissure of stones thrown up along the aim.
    Scree,
    /// Cometail — shooting stars: streaks falling slantwise out of the sky
    /// onto the mark, straight and fast, and a white flash where they land.
    Starfall,
}

pub const MOTIFS: [Motif; 14] = [
    Motif::Sunroar, Motif::Rake, Motif::Rainfall, Motif::Umbra, Motif::Gyre, Motif::Yowl, Motif::Rime,
    Motif::Fangs, Motif::Halo, Motif::Hush, Motif::Lash, Motif::Thunderhead, Motif::Scree, Motif::Starfall,
];

impl Motif {
    pub fn of(def_id: u32) -> Motif {
        MOTIFS[def_id as usize % MOTIFS.len()]
    }

    /// Three-stop ramp: core, body, fringe. One flat colour is what made
    /// everything read as the same effect.
    pub fn ramp(self) -> [[f32; 3]; 3] {
        match self {
            Motif::Sunroar => [[1.0, 0.95, 0.70], [1.0, 0.72, 0.18], [0.85, 0.30, 0.05]],
            Motif::Rake => [[1.0, 0.62, 0.15], [0.90, 0.35, 0.05], [0.10, 0.06, 0.05]],
            Motif::Rainfall => [[0.80, 0.92, 1.0], [0.35, 0.62, 0.85], [0.78, 0.66, 0.35]],
            Motif::Umbra => [[0.55, 0.35, 0.85], [0.18, 0.08, 0.30], [0.03, 0.02, 0.06]],
            Motif::Gyre => [[0.95, 1.0, 0.92], [0.70, 0.88, 0.72], [0.80, 0.74, 0.50]],
            Motif::Yowl => [[0.98, 0.62, 0.86], [0.80, 0.45, 0.75], [0.55, 0.78, 0.40]],
            Motif::Rime => [[0.92, 0.98, 1.0], [0.55, 0.82, 1.0], [0.30, 0.45, 0.80]],
            Motif::Fangs => [[1.0, 0.98, 0.90], [0.85, 0.80, 0.68], [0.60, 0.10, 0.08]],
            Motif::Halo => [[1.0, 1.0, 0.85], [1.0, 0.80, 0.25], [0.80, 0.62, 0.35]],
            Motif::Hush => [[1.0, 1.0, 1.0], [0.85, 0.90, 0.98], [0.65, 0.72, 0.85]],
            Motif::Lash => [[0.75, 1.0, 0.95], [0.10, 0.62, 0.60], [0.05, 0.28, 0.25]],
            Motif::Thunderhead => [[0.85, 0.90, 1.0], [0.55, 0.50, 0.95], [0.30, 0.30, 0.34]],
            Motif::Scree => [[0.78, 0.76, 0.70], [0.48, 0.46, 0.43], [0.30, 0.27, 0.22]],
            Motif::Starfall => [[1.0, 1.0, 0.95], [1.0, 0.86, 0.45], [0.45, 0.55, 1.0]],
        }
    }
}

/// Where an archetype throws the motif.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Around the caster, outward (novas, storms, pulls).
    Burst,
    /// From the caster along the aim (bolts, beams, volleys, dashes).
    Line,
    /// At the target point (lobs, rings, blinks).
    Mark,
    /// Up and around the caster's own body (shields, buffs, heals).
    Mantle,
}

impl Gesture {
    pub fn of(arch: &Archetype) -> Gesture {
        use Archetype as A;
        match arch {
            A::Nova { .. } | A::Storm { .. } | A::Pull { .. } => Gesture::Burst,
            A::Bolt { .. } | A::Volley { .. } | A::Beam { .. } | A::Dash { .. } => Gesture::Line,
            A::Lob { .. } | A::Ring { .. } | A::Blink { .. } | A::Meteor { .. } => Gesture::Mark,
            A::Shield { .. } | A::Buff { .. } | A::Heal { .. } => Gesture::Mantle,
        }
    }
}

/// Deterministic, so a replay draws the same sparks.
pub struct Rng(pub u32);
impl Rng {
    pub fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1664525).wrapping_add(1013904223);
        ((self.0 >> 8) & 0xFFFF) as f32 / 65535.0
    }
    /// -1..1
    pub fn s(&mut self) -> f32 {
        self.f() * 2.0 - 1.0
    }
}

/// Emit one cast's worth of signature.
///
/// `origin` is the caster's feet, `aim` the world point aimed at. `ult`
/// scales count and reach; it does not change the element.
pub fn emit(motif: Motif, gesture: Gesture, origin: Vec3, aim: Vec3, ult: bool, rng: &mut Rng) -> Vec<Particle> {
    let mut out: Vec<Particle> = Vec::with_capacity(MAX_PER_CAST);
    if !origin.is_finite() || !aim.is_finite() {
        return out;
    }
    let to = Vec3::new(aim.x - origin.x, 0.0, aim.z - origin.z);
    let reach = to.length().clamp(2.0, 14.0);
    let dir = if to.length_squared() > 1e-4 { to.normalize() } else { Vec3::X };
    let side = Vec3::new(-dir.z, 0.0, dir.x);
    let k = if ult { 1.6 } else { 1.0 };
    let n = ((match gesture { Gesture::Burst => 54.0, Gesture::Line => 44.0, Gesture::Mark => 48.0, Gesture::Mantle => 36.0 }) * k) as usize;
    let n = n.min(MAX_PER_CAST);
    let ramp = motif.ramp();
    // Where this gesture is centred, and the frame the motif draws in.
    let centre = match gesture {
        Gesture::Mark => origin + dir * reach,
        _ => origin,
    };
    let span = match gesture {
        Gesture::Line => reach,
        Gesture::Burst => 4.5 * k,
        Gesture::Mark => 3.2 * k,
        Gesture::Mantle => 1.4,
    };
    let tau = std::f32::consts::TAU;
    let mut push = |pos: Vec3, vel: Vec3, life: f32, size: f32, stop: usize, alpha: f32, gravity: f32, drag: f32| {
        out.push(Particle { pos, vel, life, max_life: life, size, color: ramp[stop.min(2)], alpha, gravity, drag });
    };

    for i in 0..n {
        let u = i as f32 / n.max(1) as f32;
        let a = u * tau;
        let ring = Vec3::new(a.cos(), 0.0, a.sin());
        // Along the gesture: 0..1 down the line, or a radius fraction.
        let (along, lateral) = match gesture {
            Gesture::Line => (dir * (u * span), side),
            _ => (ring * (0.3 + 0.7 * rng.f()) * span * 0.5, Vec3::new(-ring.z, 0.0, ring.x)),
        };
        let p = centre + along;
        match motif {
            Motif::Sunroar => {
                // Straight spokes: twelve directions only, fast, no gravity.
                let spoke = ((u * 12.0).floor() / 12.0) * tau;
                let d = if gesture == Gesture::Line { (dir + side * rng.s() * 0.12).normalize() } else { Vec3::new(spoke.cos(), 0.0, spoke.sin()) };
                push(centre + Vec3::Y * 1.0 + d * rng.f() * 0.8, d * (9.0 + 5.0 * rng.f()) * k + Vec3::Y * 1.5, 0.45, 0.20, (i % 3 == 0) as usize, 1.0, -5.0, 1.2);
            }
            Motif::Rake => {
                // Three parallel lines, swept sideways across them.
                let lane = (i % 3) as f32 - 1.0;
                let q = centre + dir * (u * span.max(3.0)) + side * lane * 0.55 + Vec3::Y * (0.9 + lane * 0.15);
                push(q, side * 7.0 + dir * 2.0, 0.28, 0.17, if i % 5 == 0 { 2 } else { (i % 2) as usize }, 1.0, 0.0, 5.0);
            }
            Motif::Rainfall => {
                // Falls INTO place from above; every sixth is a ground rosette.
                if i % 6 == 0 {
                    push(Vec3::new(p.x, 0.12, p.z), lateral * 1.2, 0.9, 0.14, 2, 0.9, 0.0, 3.0);
                } else {
                    push(p + Vec3::Y * (4.0 + 3.0 * rng.f()), Vec3::new(0.6, -11.0 - 4.0 * rng.f(), 0.0), 0.55, 0.09, (i % 2) as usize, 0.85, 6.0, 0.0);
                }
            }
            Motif::Umbra => {
                // Starts OUT and is drawn IN. Negative radial velocity is
                // the whole identity of this one.
                let start = centre + ring * (span * 0.5 + 1.5) + Vec3::Y * (0.3 + 1.6 * rng.f());
                let inward = (centre + Vec3::Y * 0.9 - start).normalize_or_zero();
                push(start, inward * (5.0 + 3.0 * rng.f()) + lateral * 2.0, 0.7, 0.24, 1 + (i % 2) as usize, 0.8, -0.5, 0.6);
            }
            Motif::Gyre => {
                // Tangential and rising: a helix.
                let r = 0.6 + u * span * 0.35;
                let q = centre + ring * r + Vec3::Y * (u * 0.5);
                push(q, Vec3::new(-ring.z, 0.0, ring.x) * (6.5 + 2.0 * rng.f()) + Vec3::Y * (4.5 + 2.0 * u), 0.8, 0.13, (i % 3).min(2), 0.85, -2.0, 0.9);
            }
            Motif::Yowl => {
                // Arcs perpendicular to the aim, marching down it in ranks.
                let rank = (i % 4) as f32;
                let across = (u * 4.0).fract() * 2.0 - 1.0;
                let bow = 1.0 - across * across;
                let q = origin + dir * (1.0 + rank * 0.9 + bow * 0.7) + side * across * (1.0 + rank * 0.5) + Vec3::Y * 1.1;
                // Slow, long-lived, and WIDENING: a sound spreads as it goes.
                push(q, dir * (4.5 + rank * 0.5) + side * across * 4.0, 0.9, 0.15, rank as usize % 3, 0.9, 0.0, 0.4);
            }
            Motif::Rime => {
                // Fast out, then HANGS: very high drag, long life.
                let d = (ring + Vec3::Y * (0.2 + 0.8 * rng.f())).normalize();
                let base = if gesture == Gesture::Line { p + Vec3::Y * 0.8 } else { centre + Vec3::Y * 0.6 };
                push(base, d * (12.0 + 6.0 * rng.f()), 1.5, 0.11 + 0.12 * rng.f(), (i % 3).min(2), 0.95, 0.6, 7.5);
            }
            Motif::Fangs => {
                // Two columns, stabbing DOWN.
                let fang = if i % 2 == 0 { -0.45 } else { 0.45 };
                let mark = match gesture { Gesture::Mark | Gesture::Line => origin + dir * reach.min(span.max(3.0)), _ => centre + dir * 1.4 };
                let h = 3.4 * (1.0 - u * 0.85);
                push(mark + side * fang + Vec3::Y * h, Vec3::new(0.0, -14.0, 0.0), 0.32, 0.22 * (0.4 + 0.6 * (1.0 - u)), if u > 0.8 { 2 } else { (i % 2) as usize }, 1.0, 20.0, 0.0);
            }
            Motif::Halo => {
                // A ring standing on edge behind the head; sand at the paws.
                if i % 3 == 2 {
                    push(Vec3::new(p.x, 0.15, p.z), lateral * 3.0 + Vec3::Y * 0.6, 0.7, 0.10, 2, 0.8, 2.0, 2.5);
                } else {
                    let q = centre + Vec3::Y * (2.3 + a.sin() * 1.1 * k) + side * (a.cos() * 1.1 * k);
                    push(q, Vec3::Y * 0.4, 0.9, 0.16, (i % 2) as usize, 1.0, 0.0, 0.5);
                }
            }
            Motif::Hush => {
                // A dome of snow that barely moves.
                let el = rng.f() * 1.4;
                let d = Vec3::new(ring.x * el.cos(), el.sin(), ring.z * el.cos());
                push(centre + d * span * (0.35 + 0.35 * rng.f()) + Vec3::Y * 0.4, Vec3::new(rng.s() * 0.4, -0.5, rng.s() * 0.4), 2.2, 0.10, (i % 3).min(2), 0.9, 0.25, 1.5);
            }
            Motif::Lash => {
                // A whip: particles along an arc, each thrown sideways later
                // than the last, so it SWEEPS.
                let sweep = u * 2.4 - 1.2;
                let d = (dir * sweep.cos() + side * sweep.sin()).normalize();
                let len = if gesture == Gesture::Line { span.min(7.0) } else { span * 0.5 + 1.0 };
                push(centre + d * len * (0.4 + 0.6 * rng.f()) + Vec3::Y * 0.8, (side * sweep.cos() - dir * sweep.sin()) * 9.0 + Vec3::Y * 2.5, 0.5, 0.13, (i % 3).min(2), 0.9, 12.0, 0.8);
            }
            Motif::Thunderhead => {
                // Two thirds bolt (jagged, instant), one third smoke (slow, tall).
                if i % 3 == 2 {
                    push(centre + Vec3::new(rng.s() * 0.6, 0.4 + 2.5 * rng.f(), rng.s() * 0.6), Vec3::new(rng.s() * 0.3, 1.1, rng.s() * 0.3), 1.8, 0.30, 2, 0.55, -0.3, 0.8);
                } else {
                    let jag = side * ((i as f32 * 2.399).sin() * 0.7) + Vec3::Y * (1.1 + (i as f32 * 1.7).cos() * 0.6);
                    let base = if gesture == Gesture::Line || gesture == Gesture::Mark { origin + dir * (u * reach) } else { centre + ring * u * span * 0.5 };
                    push(base + jag, Vec3::ZERO, 0.16, 0.19, (i % 2) as usize, 1.0, 0.0, 0.0);
                }
            }
            Motif::Starfall => {
                // Three in four are shooting stars: born high and off to one
                // side, falling slantwise INTO the mark in straight lines (no
                // gravity -- they are already falling as fast as they will).
                // The fourth is the flash where one lands: low, flung out.
                if i % 4 == 3 {
                    push(Vec3::new(p.x, 0.2, p.z), lateral * 1.5 + ring * 3.5 + Vec3::Y * 2.0, 0.5, 0.12, 0, 1.0, 4.0, 2.0);
                } else {
                    let high = p + ring * 3.0 + Vec3::Y * (6.0 + 3.0 * rng.f());
                    let fall = (Vec3::new(p.x, 0.0, p.z) - high).normalize_or_zero();
                    push(high, fall * (17.0 + 5.0 * rng.f()), 0.35, 0.15, (i % 2) as usize + (i % 7 == 0) as usize, 1.0, 0.0, 0.0);
                }
            }
            Motif::Scree => {
                // Thrown UP out of the ground and straight back down.
                let base = if gesture == Gesture::Line { origin + dir * (u * span) + side * rng.s() * 0.4 } else { p };
                push(Vec3::new(base.x, 0.05, base.z), Vec3::new(rng.s() * 1.5, 6.5 + 4.0 * rng.f(), rng.s() * 1.5), 0.9, 0.16 + 0.16 * rng.f(), (i % 3).min(2), 1.0, 22.0, 0.3);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a motif measurably IS, from its particles alone.
    #[derive(Debug)]
    struct Print {
        radial: f32,   // mean velocity away from the centre (negative = implodes)
        rise: f32,     // mean vertical velocity
        gravity: f32,
        life: f32,
        speed: f32,
        height: f32,   // mean spawn height
        spread: f32,   // mean distance off the aim axis: a beam vs a fan
        hue: [f32; 3],
    }

    fn print_of(m: Motif, g: Gesture) -> Print {
        let origin = Vec3::new(3.0, 0.0, -2.0);
        let aim = origin + Vec3::new(8.0, 0.0, 0.0);
        let ps = emit(m, g, origin, aim, false, &mut Rng(7));
        assert!(!ps.is_empty());
        let centre = if g == Gesture::Mark { aim } else { origin };
        let n = ps.len() as f32;
        let mut p = Print { radial: 0.0, rise: 0.0, gravity: 0.0, life: 0.0, speed: 0.0, height: 0.0, spread: 0.0, hue: [0.0; 3] };
        for q in &ps {
            let out = Vec3::new(q.pos.x - centre.x, 0.0, q.pos.z - centre.z).normalize_or_zero();
            p.radial += q.vel.dot(out) / n;
            p.rise += q.vel.y / n;
            p.gravity += q.gravity / n;
            p.life += q.max_life / n;
            p.speed += q.vel.length() / n;
            p.height += q.pos.y / n;
            // aim is +X in this harness, so off-axis is |z|; include where it is HEADED.
            p.spread += ((q.pos.z - origin.z).abs() + (q.vel.z * q.max_life).abs()) / n;
            for c in 0..3 {
                p.hue[c] += q.color[c] / n;
            }
        }
        p
    }

    /// Normalised distance between two prints. Each axis is scaled by what a
    /// viewer would call "clearly different" on it.
    fn apart(a: &Print, b: &Print) -> f32 {
        let d = |x: f32, y: f32, scale: f32| ((x - y) / scale).abs().min(3.0);
        let hue = (0..3).map(|c| (a.hue[c] - b.hue[c]).abs()).sum::<f32>() / 0.45;
        d(a.radial, b.radial, 4.0) + d(a.rise, b.rise, 4.0) + d(a.gravity, b.gravity, 6.0) + d(a.life, b.life, 0.5)
            + d(a.speed, b.speed, 4.0) + d(a.height, b.height, 1.0) + d(a.spread, b.spread, 1.0) + hue.min(3.0)
    }

    /// The claim this module exists to make. If two ancestors' bursts are
    /// within one "clearly different" of each other across every axis, they
    /// are the same effect in a different colour, which is where this began.
    #[test]
    fn every_motif_is_measurably_its_own() {
        let mut pairs = 0;
        let mut closest = (f32::MAX, Motif::Sunroar, Motif::Sunroar);
        for g in [Gesture::Burst, Gesture::Line] {
            let prints: Vec<Print> = MOTIFS.iter().map(|m| print_of(*m, g)).collect();
            for i in 0..MOTIFS.len() {
                for j in i + 1..MOTIFS.len() {
                    let d = apart(&prints[i], &prints[j]);
                    if d < closest.0 {
                        closest = (d, MOTIFS[i], MOTIFS[j]);
                    }
                    assert!(d > 2.0, "{:?} and {:?} are only {d:.2} apart as a {g:?}:\n{:?}\n{:?}", MOTIFS[i], MOTIFS[j], prints[i], prints[j]);
                    pairs += 1;
                }
            }
        }
        assert_eq!(pairs, 2 * 14 * 13 / 2);
        eprintln!("closest pair: {closest:?}");
    }

    /// The identities named in the enum docs, as measurements. If Umbra
    /// stops imploding it is just purple Sunroar.
    #[test]
    fn the_named_motion_laws_hold() {
        let b = |m| print_of(m, Gesture::Burst);
        assert!(b(Motif::Umbra).radial < -2.0, "shadow is drawn IN: {:?}", b(Motif::Umbra));
        assert!(b(Motif::Sunroar).radial > 6.0, "a roar goes OUT: {:?}", b(Motif::Sunroar));
        assert!(b(Motif::Gyre).rise > 3.5 && b(Motif::Gyre).radial.abs() < 2.0, "a helix rises and circles: {:?}", b(Motif::Gyre));
        assert!(b(Motif::Fangs).rise < -10.0, "fangs stab down");
        assert!(b(Motif::Rainfall).rise < -6.0 && b(Motif::Rainfall).height > 3.0, "rain starts high and falls");
        assert!(b(Motif::Hush).speed < 1.2 && b(Motif::Hush).life > 1.8, "snow barely moves and lingers");
        assert!(b(Motif::Scree).gravity > 18.0 && b(Motif::Scree).rise > 5.0, "stones go up hard and come down harder");
        assert!(b(Motif::Rime).life > 1.2, "frost hangs");
    }

    #[test]
    fn every_cast_is_bounded_finite_and_deterministic() {
        let o = Vec3::new(1.0, 0.0, 1.0);
        let mut total = 0;
        for m in MOTIFS {
            for g in [Gesture::Burst, Gesture::Line, Gesture::Mark, Gesture::Mantle] {
                for ult in [false, true] {
                    let a = emit(m, g, o, o + Vec3::Z * 9.0, ult, &mut Rng(3));
                    let b = emit(m, g, o, o + Vec3::Z * 9.0, ult, &mut Rng(3));
                    assert!((12..=MAX_PER_CAST).contains(&a.len()), "{m:?}/{g:?}: {} particles", a.len());
                    assert_eq!(a.len(), b.len());
                    for (x, y) in a.iter().zip(&b) {
                        assert!(x.pos.is_finite() && x.vel.is_finite() && x.life > 0.0 && x.size > 0.0, "{m:?}/{g:?}");
                        assert_eq!((x.pos, x.vel), (y.pos, y.vel), "same seed, same sparks");
                        assert!(x.pos.distance(o) < 40.0, "{m:?}/{g:?} spawned a particle {} away", x.pos.distance(o));
                    }
                    total += 1;
                }
            }
        }
        assert_eq!(total, 14 * 4 * 2);
        // Aim on top of the caster, and garbage: no panic, no NaN.
        assert!(emit(Motif::Yowl, Gesture::Line, o, o, false, &mut Rng(1)).iter().all(|p| p.pos.is_finite()));
        assert!(emit(Motif::Rake, Gesture::Line, Vec3::splat(f32::NAN), o, false, &mut Rng(1)).is_empty());
    }

    /// One ancestor's kit should use more than one gesture, or its four
    /// abilities are one flourish four times.
    #[test]
    fn rosters_kits_spread_across_gestures() {
        let mut multi = 0;
        for def in crate::gods::ROSTER.iter() {
            let mut seen: Vec<Gesture> = Vec::new();
            for a in &def.abilities {
                let g = Gesture::of(&a.arch);
                if !seen.contains(&g) {
                    seen.push(g);
                }
            }
            multi += (seen.len() >= 2) as usize;
        }
        assert_eq!(crate::gods::ROSTER.len(), MOTIFS.len(), "one motif per ancestor, exactly");
        assert!(multi >= 12, "only {multi}/14 kits use two or more gestures");
    }
}
