//! perception — how aware a creature is of the player, and the linchpin signal
//! "am I being looked at." Awareness rises with the noise the player makes and
//! with a clear line of sight, and collapses when the player sits in the
//! creature's **blind spot** — a rear/under cone that widens with body size, so a
//! huge dragon genuinely can't see what's tucked behind or beneath it (the blind
//! cave dragon, crouch-to-hide). `being_watched` is true only when the player's
//! view frustum actually contains the mob — the Freeze beat reads it. Reactive
//! tier: cheap, pure, deterministic; the coordinator maps the outputs onto
//! `Signals`.

use std::f32::consts::{PI, TAU};

/// One perception readout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Perception {
    /// 0 (oblivious) .. 1 (fully alert).
    pub awareness: f32,
    /// The player's view frustum contains this mob.
    pub being_watched: bool,
}

/// Tuning.
#[derive(Clone, Copy, Debug)]
pub struct PerceptParams {
    /// How far the mob can see/sense the player (blocks).
    pub sight_range: f32,
    /// Half-angle of the player's view frustum (radians) for `being_watched`.
    pub frustum_half: f32,
    /// Base half-angle of the mob's rear blind cone (radians) at size 1.
    pub blind_half_base: f32,
    /// Extra blind half-angle per unit of size above 1.
    pub blind_half_per_size: f32,
    /// Weight of heard noise vs. seen presence in awareness.
    pub noise_weight: f32,
    pub sight_weight: f32,
}

impl Default for PerceptParams {
    fn default() -> PerceptParams {
        PerceptParams {
            sight_range: 32.0,
            frustum_half: 0.6,
            blind_half_base: 0.5,
            blind_half_per_size: 0.35,
            noise_weight: 0.5,
            sight_weight: 0.7,
        }
    }
}

fn wrap_tau(a: f32) -> f32 {
    let mut x = a % TAU;
    if x < 0.0 {
        x += TAU;
    }
    x
}
/// Shortest angular distance in `[0, PI]`.
fn ang_diff(a: f32, b: f32) -> f32 {
    let d = wrap_tau(a - b);
    if d > PI {
        TAU - d
    } else {
        d
    }
}
fn horiz_angle(from: [f32; 3], to: [f32; 3]) -> f32 {
    (to[2] - from[2]).atan2(to[0] - from[0])
}
fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The mob's rear blind-cone half-angle for a given size (bigger ⇒ wider blind spot).
pub fn blind_half(size: f32, p: &PerceptParams) -> f32 {
    (p.blind_half_base + p.blind_half_per_size * (size.max(1.0) - 1.0)).min(PI)
}

/// Is `point` in the mob's blind spot — the rear cone (mob faces `mob_yaw`) or
/// well beneath a large body?
pub fn in_blind_spot(mob_pos: [f32; 3], mob_yaw: f32, size: f32, point: [f32; 3], p: &PerceptParams) -> bool {
    let rear = mob_yaw + PI;
    let to = horiz_angle(mob_pos, point);
    if ang_diff(to, rear) <= blind_half(size, p) {
        return true;
    }
    // big bodies also can't see far beneath themselves
    (mob_pos[1] - point[1]) > size
}

/// Is the mob inside the player's view frustum (and in range)?
pub fn being_watched(player_pos: [f32; 3], player_yaw: f32, mob_pos: [f32; 3], p: &PerceptParams) -> bool {
    if dist(player_pos, mob_pos) > p.sight_range {
        return false;
    }
    let to_mob = horiz_angle(player_pos, mob_pos);
    ang_diff(to_mob, player_yaw) <= p.frustum_half
}

/// Full readout. `noise` in `[0, 1]` is what the mob can hear; `has_los` is a
/// clear line of sight (the coordinator supplies the occlusion test). Awareness
/// blends heard noise (always counts) with seen presence (only when the player is
/// in range, not in the blind spot, and visible).
pub fn perceive(
    player_pos: [f32; 3],
    player_yaw: f32,
    mob_pos: [f32; 3],
    mob_yaw: f32,
    size: f32,
    noise: f32,
    has_los: bool,
    p: &PerceptParams,
) -> Perception {
    let noise = noise.clamp(0.0, 1.0);
    let d = dist(player_pos, mob_pos);

    let sight = if d > p.sight_range || !has_los || in_blind_spot(mob_pos, mob_yaw, size, player_pos, p) {
        0.0
    } else {
        (1.0 - d / p.sight_range).clamp(0.0, 1.0) // closer ⇒ more vivid
    };

    let awareness = (p.noise_weight * noise + p.sight_weight * sight).clamp(0.0, 1.0);
    Perception { awareness, being_watched: being_watched(player_pos, player_yaw, mob_pos, p) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn awareness_rises_with_noise() {
        let p = PerceptParams::default();
        let quiet = perceive([10.0, 0.0, 0.0], PI, [0.0; 3], 0.0, 1.0, 0.0, false, &p);
        let loud = perceive([10.0, 0.0, 0.0], PI, [0.0; 3], 0.0, 1.0, 0.9, false, &p);
        assert!(loud.awareness > quiet.awareness, "louder → more aware");
    }

    #[test]
    fn line_of_sight_in_front_raises_awareness() {
        let p = PerceptParams::default();
        // mob faces +X (yaw 0), player in front at +X, visible
        let seen = perceive([5.0, 0.0, 0.0], PI, [0.0; 3], 0.0, 1.0, 0.0, true, &p);
        let unseen = perceive([5.0, 0.0, 0.0], PI, [0.0; 3], 0.0, 1.0, 0.0, false, &p);
        assert!(seen.awareness > unseen.awareness, "seeing the player adds awareness");
    }

    #[test]
    fn blind_spot_drops_awareness() {
        let p = PerceptParams::default();
        // mob faces +X (yaw 0). Player in FRONT (+X) vs BEHIND (−X).
        let front = perceive([5.0, 0.0, 0.0], PI, [0.0; 3], 0.0, 2.0, 0.0, true, &p);
        let behind = perceive([-5.0, 0.0, 0.0], PI, [0.0; 3], 0.0, 2.0, 0.0, true, &p);
        assert!(behind.awareness < front.awareness, "player in the blind spot → less aware");
        assert!(in_blind_spot([0.0; 3], 0.0, 2.0, [-5.0, 0.0, 0.0], &p), "behind is blind");
        assert!(!in_blind_spot([0.0; 3], 0.0, 2.0, [5.0, 0.0, 0.0], &p), "front is not");
    }

    #[test]
    fn bigger_bodies_have_wider_blind_spots() {
        let p = PerceptParams::default();
        assert!(blind_half(4.0, &p) > blind_half(1.0, &p), "size widens the blind cone");
    }

    #[test]
    fn being_watched_only_when_in_frustum() {
        let p = PerceptParams::default();
        // player at origin; mob at +X. Player facing +X (yaw 0) → watched.
        assert!(being_watched([0.0; 3], 0.0, [5.0, 0.0, 0.0], &p), "looking at it");
        // facing away (yaw PI, toward −X) → not watched
        assert!(!being_watched([0.0; 3], PI, [5.0, 0.0, 0.0], &p), "looking away");
        // out of range → not watched
        assert!(!being_watched([0.0; 3], 0.0, [1000.0, 0.0, 0.0], &p), "too far");
    }

    #[test]
    fn prop_awareness_bounded_and_deterministic() {
        let p = PerceptParams::default();
        let mut seed = 0x9E1C_0FFEu32;
        for _ in 0..4000 {
            let pp = [lcg(&mut seed) * 60.0 - 30.0, lcg(&mut seed) * 20.0 - 10.0, lcg(&mut seed) * 60.0 - 30.0];
            let mp = [lcg(&mut seed) * 60.0 - 30.0, lcg(&mut seed) * 20.0 - 10.0, lcg(&mut seed) * 60.0 - 30.0];
            let pyaw = lcg(&mut seed) * TAU;
            let myaw = lcg(&mut seed) * TAU;
            let size = 1.0 + lcg(&mut seed) * 4.0;
            let noise = lcg(&mut seed);
            let los = lcg(&mut seed) < 0.5;
            let a = perceive(pp, pyaw, mp, myaw, size, noise, los, &p);
            let b = perceive(pp, pyaw, mp, myaw, size, noise, los, &p);
            assert_eq!(a, b, "deterministic");
            assert!(a.awareness.is_finite() && a.awareness >= 0.0 && a.awareness <= 1.0, "awareness bounded");
        }
    }
}
