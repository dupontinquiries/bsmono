//! `precipitation` — a procedural falling rain/snow particle field around the
//! player (r492). Pure: given the player position, time, intensity, wind and a
//! rain/snow flag, it returns a set of world-space particle positions that the
//! renderer draws as small boxes (thin streaks for rain, drifting flakes for
//! snow). No GPU, no state — the field is a closed-form function of time, so it
//! needs no per-particle simulation and is trivially deterministic.
//!
//! The particle box is centred on the player and wraps in y, so flakes appear to
//! fall continuously. Wind leans the fall. Snow falls slower and sways; rain
//! falls fast and nearly straight.

/// One particle: world position + a per-particle phase (0..1 down the fall),
/// which the renderer can use to fade/scale near the ground.
#[derive(Clone, Copy, Debug)]
pub struct Drop {
    pub pos: [f32; 3],
    pub fall01: f32,
}

#[inline]
fn hash01(i: u32, salt: u32) -> f32 {
    // cheap integer hash → [0,1)
    let mut h = i.wrapping_mul(0x9E37_79B1) ^ salt.wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0x00FF_FFFF) as f32 / 16_777_216.0
}

/// Generate the precipitation field around `center`.
/// - `count`: number of particles (caller scales by intensity).
/// - `radius`: half-extent of the xz box around the player (blocks).
/// - `height`: vertical extent of the fall column (blocks).
/// - `fall_speed`: blocks/sec downward.
/// - `wind`: horizontal wind vector (blocks/sec) — leans the fall.
/// - `sway`: lateral sway amplitude (snow > rain) in blocks.
pub fn field(
    center: [f32; 3],
    t: f32,
    count: usize,
    radius: f32,
    height: f32,
    fall_speed: f32,
    wind: [f32; 2],
    sway: f32,
) -> Vec<Drop> {
    let mut out = Vec::with_capacity(count);
    let top = center[1] + height * 0.5;
    let cycle_per_sec = if height > 1e-3 { fall_speed / height } else { 0.0 };
    for i in 0..count as u32 {
        let hx = hash01(i, 0x11) * 2.0 - 1.0;
        let hz = hash01(i, 0x22) * 2.0 - 1.0;
        let hp = hash01(i, 0x33); // fall phase offset
        let hs = hash01(i, 0x44); // sway phase
        // fall progress 0 (top) .. 1 (bottom), wrapping
        let fall01 = (t * cycle_per_sec + hp).rem_euclid(1.0);
        let y = top - fall01 * height;
        // base xz around the player, leaned downwind as the drop falls, plus a
        // gentle sway for snow.
        let swx = (t * 1.7 + hs * 6.28).sin() * sway;
        let swz = (t * 1.3 + hs * 6.28).cos() * sway;
        let x = center[0] + hx * radius + wind[0] * fall01 * 0.6 + swx;
        let z = center[2] + hz * radius + wind[1] * fall01 * 0.6 + swz;
        out.push(Drop { pos: [x, y, z], fall01 });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_stay_within_the_column() {
        let c = [100.0, 64.0, -50.0];
        let f = field(c, 3.7, 200, 10.0, 16.0, 9.0, [1.0, -0.5], 0.0);
        assert_eq!(f.len(), 200);
        for d in &f {
            assert!(d.pos[1] <= c[1] + 8.0 + 1e-3 && d.pos[1] >= c[1] - 8.0 - 1e-3, "y in column");
            assert!(d.fall01 >= 0.0 && d.fall01 < 1.0);
            // xz within radius + lean + nothing absurd
            assert!((d.pos[0] - c[0]).abs() < 10.0 + 1.0);
        }
    }

    #[test]
    fn field_advances_with_time() {
        let c = [0.0, 64.0, 0.0];
        let a = field(c, 0.0, 50, 8.0, 16.0, 9.0, [0.0, 0.0], 0.0);
        let b = field(c, 0.5, 50, 8.0, 16.0, 9.0, [0.0, 0.0], 0.0);
        // at least some drops moved down
        let moved = a.iter().zip(&b).filter(|(p, q)| (p.pos[1] - q.pos[1]).abs() > 1e-3).count();
        assert!(moved > 0, "drops fall over time");
    }
}
