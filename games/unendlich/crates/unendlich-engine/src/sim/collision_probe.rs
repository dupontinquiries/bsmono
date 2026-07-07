//! collision_probe — one terrain reading around a creature.
//!
//! Three locomotion systems were each re-sampling the ground on their own:
//! **posture** wants the four cardinal heights so it can lean, **leap** wants the
//! heights a few blocks straight ahead to decide whether to hop a ledge or clear
//! a gap, and **steering** wants to know whether the next step is walkable.
//!
//! This bundles a single sample into one [`GroundProbe`] so they all read the
//! same picture — and so the bin probes the world *once* per creature per tick
//! instead of three times. It is pure and deterministic: it asks a caller-
//! supplied `surface_at(x, z) -> Option<f32>` (the world Y a foot would rest on,
//! `None` where there is no standable ground) and reasons only about the values
//! it gets back.

use crate::sim::leap::Leap;

/// Standable-surface heights sampled around a creature, in world units (the Y a
/// foot would rest on). `here` is the ground under the creature; `front/back/
/// left/right` are one `span` away along/across the heading (for leaning); and
/// `ahead[i]` is the ground `i + 1` blocks straight ahead (for ledges, gaps, and
/// leap targets). Columns with no standable ground fall back to a caller-supplied
/// `default_y`, so a creature reads *flat* over a hole rather than tipping into a
/// void or seeing a phantom cliff.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundProbe {
    pub here: f32,
    pub front: f32,
    pub back: f32,
    pub left: f32,
    pub right: f32,
    /// Half-width of the cardinal sample cross (blocks). `pitch_roll` uses `2 *
    /// span` as the front-to-back baseline.
    pub span: f32,
    /// Ground height 1, 2, 3, 4 blocks straight ahead along the heading.
    pub ahead: [f32; 4],
}

impl GroundProbe {
    /// Sample the ground around `(x, z)` for a creature facing `yaw` (0 = +X),
    /// reaching `span` blocks to each side. `surface_at(x, z)` returns the
    /// standable surface Y at a world column or `None`; missing samples fall back
    /// to `default_y` (use the creature's current foot height).
    pub fn sample(
        x: f32,
        z: f32,
        yaw: f32,
        span: f32,
        default_y: f32,
        surface_at: impl Fn(f32, f32) -> Option<f32>,
    ) -> GroundProbe {
        let (s, c) = yaw.sin_cos(); // forward = (c, s); right = (s, -c); left = (-s, c)
        let get = |dx: f32, dz: f32| surface_at(x + dx, z + dz).unwrap_or(default_y);
        let mut ahead = [default_y; 4];
        for (i, a) in ahead.iter_mut().enumerate() {
            let d = (i + 1) as f32;
            *a = get(c * d, s * d);
        }
        GroundProbe {
            here: get(0.0, 0.0),
            front: get(c * span, s * span),
            back: get(-c * span, -s * span),
            left: get(-s * span, c * span),
            right: get(s * span, -c * span),
            span,
            ahead,
        }
    }

    /// Terrain lean for this reading: `(pitch, roll)` in radians, `+pitch` = nose
    /// up (front higher than back), `+roll` = left side higher than right. Uses the
    /// **stable** [`terrain_lean`](crate::sim::posture::terrain_lean) (clamped
    /// delta + deadzone + upright bias) with the given limits, so a cliff edge
    /// reads as a bounded slope rather than a somersault. Feed to
    /// [`crate::sim::posture::Posture::settle_to_terrain`].
    pub fn lean(&self, lim: &crate::sim::posture::LeanLimits) -> (f32, f32) {
        crate::sim::posture::terrain_lean(self.front, self.back, self.left, self.right, 2.0 * self.span, lim)
    }

    /// Lean with default [`LeanLimits`].
    pub fn lean_default(&self) -> (f32, f32) {
        self.lean(&crate::sim::posture::LeanLimits::default())
    }

    /// Size-aware lean: a larger `size` tilts less for the same terrain step (a
    /// dragon shrugs off a 1-block lip a deer would lean on). Delegates to
    /// [`crate::sim::posture::terrain_lean_sized`].
    pub fn lean_sized(&self, lim: &crate::sim::posture::LeanLimits, size: f32) -> (f32, f32) {
        crate::sim::posture::terrain_lean_sized(self.front, self.back, self.left, self.right, 2.0 * self.span, lim, size)
    }

    /// How broken the footing is here (spread of the foot samples) — feeds the
    /// stumble model. Delegates to
    /// [`crate::sim::posture::terrain_roughness`].
    pub fn roughness(&self) -> f32 {
        crate::sim::posture::terrain_roughness(self.front, self.back, self.left, self.right)
    }

    /// Height change one block ahead (`+` uphill/ledge, `-` drop). The immediate
    /// step the creature would take walking forward.
    pub fn rise_ahead(&self) -> f32 {
        self.ahead[0] - self.here
    }

    /// Can the creature simply *walk* the next block forward — i.e. step up no
    /// more than `step_up` blocks and drop no more than `safe_fall`? When this is
    /// false and the way is still wanted, that's the cue to try a leap.
    pub fn walkable_ahead(&self, step_up: i32, safe_fall: i32) -> bool {
        let r = self.rise_ahead();
        r <= step_up as f32 + 1e-3 && r >= -(safe_fall as f32)
    }

    /// The nearest forward distance (1..=4 blocks) whose ground a `leap` arc would
    /// reach under `gravity`, or `None` if no probed landing is reachable. This
    /// only reports *reachability* — the decision to leap belongs to the caller
    /// (typically: only when [`walkable_ahead`](Self::walkable_ahead) is false and
    /// the creature is agile).
    pub fn leap_target(&self, leap: &Leap, gravity: f32) -> Option<f32> {
        for (i, &h) in self.ahead.iter().enumerate() {
            let d = (i + 1) as f32;
            let rise = h - self.here;
            if leap.clears_gap(d, rise, gravity) {
                return Some(d);
            }
        }
        None
    }
}

/// Wrap a fine block surface into a **coarser** one for a large creature: it
/// reads terrain in `factor`×`factor` tiles, so sub-tile bumps and 1-block cracks
/// vanish — a dragon doesn't trip on a pebble or fall in a gap its body spans.
/// Each tile's height is the **max** of its present blocks (so the creature never
/// under-reads a real step and clip into it), and the tile is void only if every
/// block in it is void. The returned closure takes world XZ and yields the
/// standable foot height (top of the tile) for feeding [`GroundProbe::sample`].
pub fn coarsened(factor: i32, fine: impl Fn(i32, i32) -> Option<i32>) -> impl Fn(f32, f32) -> Option<f32> {
    let f = factor.max(1);
    move |x: f32, z: f32| {
        let bx = (x.floor() as i32).div_euclid(f) * f;
        let bz = (z.floor() as i32).div_euclid(f) * f;
        let mut hi: Option<i32> = None;
        for dz in 0..f {
            for dx in 0..f {
                if let Some(h) = fine(bx + dx, bz + dz) {
                    hi = Some(hi.map_or(h, |m| m.max(h)));
                }
            }
        }
        hi.map(|h| (h + 1) as f32) // foot rests on top of the tile
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::fall::{SAFE_FALL_BLOCKS, STEP_UP_BLOCKS};
    use crate::sim::leap::{Leap, GRAVITY};

    fn flat(_x: f32, _z: f32) -> Option<f32> {
        Some(64.0)
    }

    #[test]
    fn flat_ground_reads_level() {
        let p = GroundProbe::sample(0.0, 0.0, 0.0, 1.0, 64.0, flat);
        let (pitch, roll) = p.lean_default();
        assert!(pitch.abs() < 1e-4, "flat ⇒ no pitch, got {pitch}");
        assert!(roll.abs() < 1e-4, "flat ⇒ no roll, got {roll}");
        assert!(p.rise_ahead().abs() < 1e-4);
        assert!(p.walkable_ahead(STEP_UP_BLOCKS, SAFE_FALL_BLOCKS));
    }

    #[test]
    fn front_high_pitches_nose_up() {
        // Ground rises toward +X (the heading at yaw 0): front higher than back.
        let terr = |x: f32, _z: f32| Some(64.0 + x); // +1 per block forward
        let p = GroundProbe::sample(0.0, 0.0, 0.0, 1.0, 64.0, terr);
        let (pitch, roll) = p.lean_default();
        assert!(pitch > 0.0, "front-high ⇒ nose up, got {pitch}");
        assert!(roll.abs() < 1e-3, "no cross-slope ⇒ no roll, got {roll}");
    }

    #[test]
    fn left_high_rolls() {
        // Ground rises toward +Z; at yaw 0 the creature's left is +Z.
        let terr = |_x: f32, z: f32| Some(64.0 + z);
        let p = GroundProbe::sample(0.0, 0.0, 0.0, 1.0, 64.0, terr);
        let (_pitch, roll) = p.lean_default();
        assert!(roll > 0.0, "left-high ⇒ positive roll, got {roll}");
    }

    #[test]
    fn ledge_not_walkable_but_strong_leaper_clears_it() {
        // A 1.5-block step up everywhere ahead: too tall to walk (step-up 1), but a
        // strong leaper's arc reaches it two blocks out.
        let terr = |x: f32, _z: f32| Some(if x > 0.5 { 65.5 } else { 64.0 });
        let p = GroundProbe::sample(0.0, 0.0, 0.0, 1.0, 64.0, terr);
        assert!(!p.walkable_ahead(STEP_UP_BLOCKS, SAFE_FALL_BLOCKS), "1.5 up isn't a walk");
        let strong = Leap::from_genes(1.0, 1.0); // up 9, forward 8
        assert_eq!(p.leap_target(&strong, GRAVITY), Some(2.0));
    }

    #[test]
    fn weak_leaper_cannot_clear_a_high_ledge() {
        let terr = |x: f32, _z: f32| Some(if x > 0.5 { 65.5 } else { 64.0 });
        let p = GroundProbe::sample(0.0, 0.0, 0.0, 1.0, 64.0, terr);
        let weak = Leap::from_genes(0.0, 0.0); // up 3, forward 2 — barely hops
        assert_eq!(p.leap_target(&weak, GRAVITY), None);
    }

    #[test]
    fn missing_ground_ahead_falls_back_to_default() {
        // A hole everywhere forward (+X): no phantom cliff — reads as default_y.
        let terr = |x: f32, _z: f32| if x > 0.5 { None } else { Some(64.0) };
        let p = GroundProbe::sample(0.0, 0.0, 0.0, 1.0, 70.0, terr);
        assert_eq!(p.front, 70.0);
        assert_eq!(p.ahead[0], 70.0);
        assert_eq!(p.here, 64.0); // the column under it still reads real ground
    }

    #[test]
    fn coarsen_ignores_a_one_block_crack_a_big_body_spans() {
        // Flat 64 everywhere except a single 1-block void at (1,1).
        let fine = |x: i32, z: i32| if (x, z) == (1, 1) { None } else { Some(64) };
        let coarse = coarsened(2, fine); // 2×2 tiles
        // The tile covering (0..2, 0..2) still reads solid (the crack is bridged).
        assert_eq!(coarse(0.5, 0.5), Some(65.0));
        assert_eq!(coarse(1.5, 1.5), Some(65.0), "the 1-block crack vanished");
    }

    #[test]
    fn coarsen_takes_the_max_so_a_step_is_not_under_read() {
        // A 67 spike sits in the tile at (3,1); the tile reads the spike, not 64.
        let fine = |x: i32, z: i32| Some(if (x, z) == (3, 1) { 67 } else { 64 });
        let coarse = coarsened(2, fine); // tile (2..4, 0..2)
        assert_eq!(coarse(2.5, 0.5), Some(68.0), "max height wins so it won't clip");
    }

    #[test]
    fn coarsen_keeps_a_real_gap_a_whole_tile_wide() {
        // A 2-wide void column the whole tile spans stays void (a dragon can't
        // cross what it fully overhangs either).
        let fine = |x: i32, _z: i32| if (2..4).contains(&x) { None } else { Some(64) };
        let coarse = coarsened(2, fine);
        assert_eq!(coarse(2.5, 0.5), None, "a full-tile gap is still a gap");
        assert_eq!(coarse(0.5, 0.5), Some(65.0));
    }

    #[test]
    fn a_large_body_barely_leans_on_a_one_block_step() {
        use crate::sim::posture::LeanLimits;
        // Ground steps up one block right in front (front higher than back).
        let terr = |x: f32, _z: f32| Some(if x > 0.0 { 65.0 } else { 64.0 });
        let p = GroundProbe::sample(0.0, 0.0, 0.0, 1.0, 64.0, terr);
        let lim = LeanLimits::default();
        let (deer_pitch, _) = p.lean_sized(&lim, 1.0);
        let (dragon_pitch, _) = p.lean_sized(&lim, 3.0);
        assert!(deer_pitch.abs() > 0.0, "a small creature leans on a 1-block step");
        assert!(
            dragon_pitch.abs() < deer_pitch.abs() * 0.5,
            "a size-3 body should tilt far less: deer {deer_pitch} vs dragon {dragon_pitch}"
        );
    }

    #[test]
    fn a_large_body_still_leans_on_a_real_slope() {
        use crate::sim::posture::LeanLimits;
        // A steep multi-block rise ahead — a dragon should still lean here.
        let terr = |x: f32, _z: f32| Some(64.0 + x * 4.0);
        let p = GroundProbe::sample(0.0, 0.0, 0.0, 1.0, 64.0, terr);
        let (dragon_pitch, _) = p.lean_sized(&LeanLimits::default(), 3.0);
        assert!(dragon_pitch.abs() > 0.05, "a genuine slope still tilts a dragon, got {dragon_pitch}");
    }
}
