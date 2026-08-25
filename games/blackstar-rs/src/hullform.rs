//! THE SHIP FROM THE OUTSIDE, shaped by what is inside it.
//!
//! The parked ship is a white box. That is not a placeholder that somebody
//! forgot to replace — it is what `Interior::sdf_local` literally is, an
//! axis-aligned block with the rooms carved out of it, and the mesh draws
//! that block because drawing anything else would put the silhouette and
//! the collider in different places again (the bug that had you bouncing
//! off a hull nobody had rendered).
//!
//! So the exterior cannot be modelled independently. It has to be DERIVED
//! from the interior, and then both the mesh and the collider have to read
//! that same derivation. That is what this is.
//!
//! ## Space fitting
//!
//! Walk the hull's length in stations. At each one, ask what the interior
//! actually needs there: the rooms crossing that station give a required
//! half-width and half-height, and the answer is the smallest cross
//! section that still contains them plus plating. A ship with a wide
//! hangar amidships bulges amidships. A ship whose bridge is a small room
//! at the bow tapers to it. Nothing is styled; the shape is a consequence.
//!
//! Then three things make it read as a vessel rather than as a lumpy
//! extrusion, and each is a rule rather than a decoration:
//!
//! * **Monotone taper at the ends.** The fore and aft stations may only
//!   shrink toward the tip. Without it, a room near the nose puts a bulge
//!   in front of a narrower waist and the ship looks swallowed.
//! * **A smoothing pass.** Neighbouring stations are averaged, twice, so
//!   the hull does not step wherever a room boundary falls. Rooms are
//!   boxes; hulls are not.
//! * **A superelliptic cross section.** `|x/a|^n + |y/b|^n = 1` with n
//!   around 3: boxy enough to use the interior volume, round enough to
//!   read as a hull. n = 2 is an ellipse and wastes the corners the rooms
//!   need; n = 8 is the box we started with.
//!
//! The containment guarantee is the whole point and is asserted in the
//! tests: every room corner must sit inside the profile. An exterior that
//! clips its own interior is worse than a box, because you would see rooms
//! sticking through the hull from outside.

use glam::Vec3;

use crate::interior::Cell;

/// How many stations along the hull. Enough to follow the rooms, few
/// enough that the loft stays cheap to build and to march.
pub const STATIONS: usize = 24;
/// Plating thickness between the room wall and the outer skin, in metres
/// of hull. A GUESS, chosen so a corridor does not read as a pipe.
const PLATE: f32 = 0.9;
/// Superellipse exponent — see the module note.
const N: f32 = 3.0;
/// The tip radius as a fraction of the widest station: a ship comes to a
/// point but not to a needle.
const TIP: f32 = 0.18;

/// A lofted hull: half-width and half-height at each station from stern
/// (index 0) to bow.
#[derive(Clone, Debug)]
pub struct Profile {
    pub half_z: f32,
    pub a: [f32; STATIONS],
    pub b: [f32; STATIONS],
}

impl Profile {
    /// Fit a hull around these rooms.
    pub fn fit(cells: &[Cell], hull_half: Vec3) -> Profile {
        let half_z = hull_half.z.max(0.5);
        let (mut a, mut b) = ([0.0f32; STATIONS], [0.0f32; STATIONS]);
        for i in 0..STATIONS {
            let z = station_z(i, half_z);
            // what the interior needs at this station
            for c in cells {
                if (z - c.centre.z).abs() <= c.half.z {
                    a[i] = a[i].max(c.centre.x.abs() + c.half.x + PLATE);
                    b[i] = b[i].max(c.centre.y.abs() + c.half.y + PLATE);
                }
            }
        }
        let widest = a.iter().copied().fold(0.0f32, f32::max).max(1.0);
        let tallest = b.iter().copied().fold(0.0f32, f32::max).max(0.6);
        // stations with nothing in them still need a skin: interpolate
        // from their neighbours rather than collapsing to zero
        for i in 0..STATIONS {
            if a[i] <= 0.0 {
                let near = (0..STATIONS)
                    .filter(|j| a[*j] > 0.0)
                    .min_by_key(|j| (*j as i32 - i as i32).abs());
                a[i] = near.map(|j| a[j] * 0.7).unwrap_or(widest * TIP);
                b[i] = near.map(|j| b[j] * 0.7).unwrap_or(tallest * TIP);
            }
        }
        // MONOTONE TAPER at both ends, walking in from the tips
        let (tip_a, tip_b) = (widest * TIP, tallest * TIP);
        a[0] = a[0].min(tip_a);
        b[0] = b[0].min(tip_b);
        a[STATIONS - 1] = a[STATIONS - 1].min(tip_a);
        b[STATIONS - 1] = b[STATIONS - 1].min(tip_b);
        for i in 1..STATIONS / 2 {
            a[i] = a[i].max(a[i - 1]);
            b[i] = b[i].max(b[i - 1]);
        }
        for i in (STATIONS / 2..STATIONS - 1).rev() {
            a[i] = a[i].max(a[i + 1]);
            b[i] = b[i].max(b[i + 1]);
        }
        let mut p = Profile { half_z, a, b };
        p.smooth();
        p.smooth();
        p
    }

    /// Average each station with its neighbours — but never below what the
    /// rooms needed, which is why `fit` smooths AFTER the taper and why
    /// this takes the max with the original.
    ///
    /// BUG (found chasing the ship-interior test failures, c5blackstar-008):
    /// this used to clamp to `a0[i] * 0.92`, not `a0[i]` — an 8% shrink
    /// "for roundness" that the comment above never actually allowed for.
    /// `fit` already bakes in `PLATE` (0.9m) between a room's true extent
    /// and the pre-smooth station requirement, so an 8% cut only breaks
    /// containment once a station is wide enough that 8% of it exceeds
    /// 0.9m — which is exactly the mid-size-and-larger ships the failing
    /// tests use (`len` 40/90/200) and exactly why a small hull could
    /// still pass while `the_hull_contains_every_room_it_was_fitted_to`
    /// failed at every length tried. `interior.rs`/`aboard.rs` build their
    /// own containment on this same `Profile`, which is why their failures
    /// (room bridge outside the hull, walkable hull corners, "mostly
    /// corridor") were one root cause wearing four names, not four bugs.
    fn smooth(&mut self) {
        let (a0, b0) = (self.a, self.b);
        for i in 1..STATIONS - 1 {
            self.a[i] = ((a0[i - 1] + a0[i] * 2.0 + a0[i + 1]) * 0.25).max(a0[i]);
            self.b[i] = ((b0[i - 1] + b0[i] * 2.0 + b0[i + 1]) * 0.25).max(b0[i]);
        }
    }

    /// Cross-section half-extents at a z, interpolated between stations.
    pub fn at(&self, z: f32) -> (f32, f32) {
        let t = ((z + self.half_z) / (2.0 * self.half_z) * (STATIONS - 1) as f32)
            .clamp(0.0, (STATIONS - 1) as f32);
        let i = t.floor() as usize;
        let j = (i + 1).min(STATIONS - 1);
        let f = t - i as f32;
        (
            self.a[i] * (1.0 - f) + self.a[j] * f,
            self.b[i] * (1.0 - f) + self.b[j] * f,
        )
    }

    /// Signed distance to the hull skin — negative inside. Approximate
    /// (the superellipse is evaluated as a scaled radial test rather than
    /// exactly), which is the usual trade for a marched SDF and is why the
    /// mesh below is built from the SAME `at()` rather than independently.
    pub fn sdf(&self, p: Vec3) -> f32 {
        let dz = p.z.abs() - self.half_z;
        let (a, b) = self.at(p.z);
        let (a, b) = (a.max(1e-3), b.max(1e-3));
        // superelliptic radius: <1 inside, >1 outside
        let r = ((p.x / a).abs().powf(N) + (p.y / b).abs().powf(N)).powf(1.0 / N);
        let scale = a.min(b);
        let radial = (r - 1.0) * scale;
        if dz > 0.0 {
            (radial.max(0.0).powi(2) + dz * dz).sqrt()
        } else {
            radial.max(dz)
        }
    }

    /// Does this box sit entirely inside the skin? The containment
    /// guarantee, used by the tests and worth having at runtime.
    pub fn contains_box(&self, centre: Vec3, half: Vec3) -> bool {
        for sx in [-1.0f32, 1.0] {
            for sy in [-1.0f32, 1.0] {
                for sz in [-1.0f32, 1.0] {
                    let c = centre + half * Vec3::new(sx, sy, sz);
                    if self.sdf(c) > 0.0 {
                        return false;
                    }
                }
            }
        }
        true
    }
}

fn station_z(i: usize, half_z: f32) -> f32 {
    -half_z + (2.0 * half_z) * (i as f32 / (STATIONS - 1) as f32)
}

/// Points on the superellipse at one station, for lofting.
pub fn ring(a: f32, b: f32, seg: usize) -> Vec<(f32, f32)> {
    (0..seg)
        .map(|k| {
            let t = k as f32 / seg as f32 * std::f32::consts::TAU;
            let (c, s) = (t.cos(), t.sin());
            // superellipse in the sign-preserving power form
            let x = c.signum() * c.abs().powf(2.0 / N) * a;
            let y = s.signum() * s.abs().powf(2.0 / N) * b;
            (x, y)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interior::Interior;

    #[test]
    fn the_hull_contains_every_room_it_was_fitted_to() {
        // the guarantee: an exterior that clips its own interior is worse
        // than a box, because rooms poke through the skin from outside
        for len in [40.0f32, 90.0, 200.0] {
            let ship = Interior::generate(len);
            let p = Profile::fit(&ship.cells, ship.hull_half);
            for c in &ship.cells {
                assert!(
                    p.contains_box(c.centre, c.half),
                    "len {len}: room {} is outside the hull it was fitted to",
                    c.id
                );
            }
        }
    }

    #[test]
    fn the_shape_follows_the_rooms_rather_than_being_a_tube() {
        let ship = Interior::generate(120.0);
        let p = Profile::fit(&ship.cells, ship.hull_half);
        let widest = p.a.iter().copied().fold(0.0f32, f32::max);
        let narrowest = p.a.iter().copied().fold(f32::MAX, f32::min);
        assert!(
            widest > narrowest * 1.25,
            "the hull is a constant tube: {narrowest:.1}..{widest:.1}"
        );
        // and it comes to a point at both ends
        assert!(p.a[0] < widest * 0.5, "the stern is not tapered");
        assert!(p.a[STATIONS - 1] < widest * 0.5, "the bow is not tapered");
    }

    #[test]
    fn inside_is_negative_and_outside_is_positive() {
        let ship = Interior::generate(90.0);
        let p = Profile::fit(&ship.cells, ship.hull_half);
        assert!(p.sdf(Vec3::ZERO) < 0.0, "the middle of the ship reads as outside");
        let far = Vec3::new(0.0, 0.0, p.half_z * 3.0);
        assert!(p.sdf(far) > 0.0, "a point well past the bow reads as inside");
        let wide = Vec3::new(p.a.iter().copied().fold(0.0f32, f32::max) * 3.0, 0.0, 0.0);
        assert!(p.sdf(wide) > 0.0, "a point well outside the beam reads as inside");
    }

    #[test]
    fn the_skin_is_smooth_enough_not_to_step_at_every_bulkhead() {
        let ship = Interior::generate(140.0);
        let p = Profile::fit(&ship.cells, ship.hull_half);
        let widest = p.a.iter().copied().fold(0.0f32, f32::max);
        for i in 1..STATIONS {
            let step = (p.a[i] - p.a[i - 1]).abs();
            assert!(
                step < widest * 0.55,
                "station {i} jumps {step:.1} — the hull steps where a room ends"
            );
        }
    }
}
