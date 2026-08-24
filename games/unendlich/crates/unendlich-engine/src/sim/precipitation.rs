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

/// Drop a particle when its column has solid terrain ABOVE it — the fix for
/// rain (and snow) falling through cave ceilings.
///
/// `top_y_at(x, z)` is the y of the column's topmost terrain block (the
/// engine's `surface_y_at` semantics: terrain, not canopy — rain still falls
/// through leaves, which is a look, not a bug). A drop is kept only when it
/// sits ON or ABOVE that block's top face (`pos.y >= top + 1`): everything
/// below the surface is either inside rock (was invisible anyway, z-buffered
/// behind terrain — culling it here just saves the vertices) or in cave air,
/// where it was VISIBLE, which is the report this exists to close.
///
/// `None` means the column could not be sampled; those drops are KEPT — rain
/// popping off over an unsampleable column would be a new artifact, and the
/// engine's fallback prediction makes `None` rare anyway.
///
/// Order-preserving and allocation-light so the caller's vertex emission
/// loop is unchanged.
pub fn cull_covered(
    drops: Vec<Drop>,
    mut top_y_at: impl FnMut(f32, f32) -> Option<i32>,
) -> Vec<Drop> {
    let mut out = drops;
    out.retain(|d| match top_y_at(d.pos[0], d.pos[2]) {
        Some(top) => d.pos[1] >= (top + 1) as f32,
        None => true,
    });
    out
}

#[cfg(test)]
mod cull_tests {
    use super::*;

    fn column(center_y: f32, n: usize) -> Vec<Drop> {
        // a vertical spread straddling center_y, like field() produces
        (0..n)
            .map(|i| Drop {
                pos: [0.5, center_y - 8.0 + (i as f32) * (16.0 / n as f32), 0.5],
                fall01: i as f32 / n as f32,
            })
            .collect()
    }

    /// The report: player in a cave at y=20 under a surface at y=60. Every
    /// drop of the field around the player is below the surface — all culled.
    #[test]
    fn a_cave_under_solid_surface_gets_no_drops() {
        let drops = column(20.0, 64);
        assert_eq!(drops.len(), 64, "the scenario built something");
        let kept = cull_covered(drops, |_, _| Some(60));
        assert!(kept.is_empty(), "rain must not fall through the ceiling: {kept:?}");
    }

    /// Outdoors: surface at y=60, player standing on it. Drops above the
    /// surface stay; the below-ground half of the wrap column goes.
    #[test]
    fn outdoors_keeps_exactly_the_drops_above_the_surface() {
        let drops = column(62.0, 64);
        let total = drops.len();
        let kept = cull_covered(drops, |_, _| Some(60));
        assert!(!kept.is_empty() && kept.len() < total, "a real split, not a no-op");
        for d in &kept {
            assert!(d.pos[1] >= 61.0, "kept a drop below the surface: {:?}", d.pos);
        }
    }

    /// The boundary: a drop exactly on the surface block's top face is kept —
    /// rain reaches the ground it wets.
    #[test]
    fn a_drop_on_the_top_face_is_kept() {
        let drops = vec![Drop { pos: [0.0, 61.0, 0.0], fall01: 0.9 }];
        assert_eq!(cull_covered(drops, |_, _| Some(60)).len(), 1);
    }

    /// Unsampleable columns keep their drops.
    #[test]
    fn unsampleable_columns_keep_their_drops() {
        let drops = column(20.0, 16);
        assert_eq!(cull_covered(drops, |_, _| None).len(), 16);
    }

    /// A cave MOUTH: columns to one side covered, the other open — the
    /// dripline. Per-drop sampling means the split follows the drop's own
    /// column, not the player's.
    #[test]
    fn the_dripline_splits_by_the_drops_own_column() {
        let drops: Vec<Drop> = (0..40)
            .map(|i| Drop { pos: [i as f32 - 20.0, 30.0, 0.0], fall01: 0.5 })
            .collect();
        // x < 0: under rock (surface 60); x >= 0: open sky (surface 20)
        let kept = cull_covered(drops, |x, _| Some(if x < 0.0 { 60 } else { 20 }));
        assert_eq!(kept.len(), 20, "only the open half survives");
        assert!(kept.iter().all(|d| d.pos[0] >= 0.0));
    }

    /// Order is preserved: the caller's fade math indexes into nothing, but
    /// deterministic output is this module's contract (see the header).
    #[test]
    fn culling_preserves_order() {
        let drops = column(62.0, 32);
        let kept = cull_covered(drops.clone(), |_, _| Some(60));
        let expect: Vec<[f32; 3]> = drops.iter().filter(|d| d.pos[1] >= 61.0).map(|d| d.pos).collect();
        let got: Vec<[f32; 3]> = kept.iter().map(|d| d.pos).collect();
        assert_eq!(got, expect);
    }
}
