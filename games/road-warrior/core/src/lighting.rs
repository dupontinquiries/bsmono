//! lighting.rs — rays traced once per level, cached, reused every frame.
//!
//! ## Why baking is correct here and not a shortcut
//!
//! The camera is FIXED. The car sits at a constant screen position and the
//! world scrolls under it (see the shader's `car_screen`), the projection is
//! orthographic, and the sun does not move. So for any point on the track,
//! "how much sun reaches this" and "how much sky can this see" are
//! properties of the TERRAIN ALONE — they do not depend on where the player
//! is or which way anything is facing.
//!
//! That makes them functions of `x`, computable once when the level loads
//! and looked up for free thereafter. Tracing them per frame would compute
//! the same numbers sixty times a second.
//!
//! A moving or rotating camera would break this and the bake would have to
//! go. It is worth knowing which assumption is load-bearing.
//!
//! ## What is actually traced
//!
//! Two 1D fields along the track:
//!
//! * [`sun_visibility`] marches a ray from the surface toward the sun and
//!   reports how much of it gets through. This is what puts a ramp's shadow
//!   on the ground behind it and darkens the floor of a pit.
//! * [`sky_openness`] measures how much of the upper hemisphere the point
//!   can see, by finding the horizon angle either side. This is ambient
//!   occlusion: the inside of a pit is dim even at noon because most of the
//!   sky is behind rock.
//!
//! Both are pure functions of `Track`, so they test headlessly and the same
//! numbers can be asserted without a GPU.

use crate::track::Track;

/// Direction TOWARD the sun, in world space. Not normalised on
/// construction; [`normalized`](Sun::normalized) does that.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sun {
    pub dx: f32,
    pub dy: f32,
}

impl Default for Sun {
    /// Late afternoon, ahead and to the right of the driver.
    ///
    /// Behind-the-player light would put every shadow on the far side of
    /// its ramp where the camera cannot see it, which is a lot of tracing
    /// for no visible result.
    fn default() -> Self {
        Sun { dx: 0.55, dy: 0.84 }
    }
}

impl Sun {
    pub fn normalized(self) -> Sun {
        let len = (self.dx * self.dx + self.dy * self.dy).sqrt();
        if len < 1e-6 {
            return Sun { dx: 0.0, dy: 1.0 };
        }
        Sun { dx: self.dx / len, dy: self.dy / len }
    }
}

/// How far along the sun ray to keep looking, in world units. Beyond this a
/// ridge is too far away to matter, and the level is only ~88 units long.
const SHADOW_REACH: f32 = 40.0;
/// Ray marching step. Smaller catches thin ridges; larger is faster. At 0.5
/// on a track whose smallest feature is an 8-unit ramp this is comfortably
/// fine enough, and the whole thing runs once.
const STEP: f32 = 0.5;
/// Lift the ray origin off the surface so a point does not shadow itself on
/// the first step.
const BIAS: f32 = 0.05;

/// Fraction of the sun reaching the surface at `x`: 1.0 open, 0.0 fully
/// blocked.
///
/// Returns a SOFT value rather than a hard hit test. A ray that clears the
/// terrain by a hair is nearly blocked; one that clears by metres is open.
/// Hard shadows on a heightfield this coarse produce a jagged edge that
/// reads as an artifact, and the softening costs nothing because this runs
/// once.
///
/// Over a Gap there is no surface to light, and this returns 1.0 — the
/// caller should not be shading a column with no ground in it.
pub fn sun_visibility(track: &Track, x: f32, sun: Sun) -> f32 {
    let sun = sun.normalized();
    let Some(h0) = track.height_at(x) else {
        return 1.0;
    };
    // Sun below the horizon: nothing is lit.
    if sun.dy <= 0.0 {
        return 0.0;
    }
    let mut visibility: f32 = 1.0;
    let mut t = STEP;
    while t <= SHADOW_REACH {
        let sx = x + sun.dx * t;
        let sy = h0 + BIAS + sun.dy * t;
        if let Some(h) = track.height_at(sx) {
            // How far the terrain reaches above the ray at this step.
            let over = h - sy;
            if over > 0.0 {
                // Fully blocked once anything rises a clear metre above the
                // ray; partially before that.
                visibility = visibility.min((1.0 - over).clamp(0.0, 1.0));
                if visibility <= 0.0 {
                    return 0.0;
                }
            }
        }
        t += STEP;
    }
    visibility
}

/// How much of the upper hemisphere is visible from the surface at `x`,
/// in `[0, 1]`. Flat open ground is 1.0; the floor of a deep pit approaches
/// 0.
///
/// Found by marching left and right to the steepest horizon angle each way
/// and taking the remaining sky wedge as a fraction of the full half-circle.
pub fn sky_openness(track: &Track, x: f32) -> f32 {
    let Some(h0) = track.height_at(x) else {
        return 1.0;
    };
    let horizon = |dir: f32| -> f32 {
        let mut max_angle: f32 = 0.0;
        let mut t = STEP;
        while t <= SHADOW_REACH {
            if let Some(h) = track.height_at(x + dir * t) {
                let rise = h - (h0 + BIAS);
                if rise > 0.0 {
                    max_angle = max_angle.max((rise / t).atan());
                }
            }
            t += STEP;
        }
        max_angle
    };
    let blocked = horizon(-1.0) + horizon(1.0);
    (1.0 - blocked / std::f32::consts::PI).clamp(0.0, 1.0)
}

/// A baked lighting field over the whole track, sampled at `samples` evenly
/// spaced points from 0 to `total_length`.
///
/// This is the cache. Build it once at level load; look up per frame.
#[derive(Debug, Clone)]
pub struct BakedLighting {
    pub sun: Vec<f32>,
    pub sky: Vec<f32>,
    length: f32,
}

impl BakedLighting {
    pub fn bake(track: &Track, sun: Sun, samples: usize) -> Self {
        let samples = samples.max(2);
        let mut sun_v = Vec::with_capacity(samples);
        let mut sky_v = Vec::with_capacity(samples);
        for i in 0..samples {
            let x = track.total_length * i as f32 / (samples - 1) as f32;
            sun_v.push(sun_visibility(track, x, sun));
            sky_v.push(sky_openness(track, x));
        }
        BakedLighting { sun: sun_v, sky: sky_v, length: track.total_length }
    }

    /// Look up the baked values at world `x`, interpolating between samples
    /// and clamping outside the track.
    pub fn at(&self, x: f32) -> (f32, f32) {
        let n = self.sun.len();
        if n == 0 || self.length <= 0.0 {
            return (1.0, 1.0);
        }
        let t = (x / self.length).clamp(0.0, 1.0) * (n - 1) as f32;
        let i0 = t.floor() as usize;
        let i1 = (i0 + 1).min(n - 1);
        let f = t - i0 as f32;
        (
            self.sun[i0] + (self.sun[i1] - self.sun[i0]) * f,
            self.sky[i0] + (self.sky[i1] - self.sky[i0]) * f,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::Segment;

    fn flat() -> Track {
        Track::new("Flat", vec![Segment::Flat { length: 60.0, height: 0.0 }])
    }

    /// A wall rising at x=30, so points just before it are in its shadow
    /// when the sun comes from the right.
    fn wall() -> Track {
        Track::new(
            "Wall",
            vec![
                Segment::Flat { length: 30.0, height: 0.0 },
                Segment::Ramp { length: 2.0, from_height: 0.0, to_height: 12.0 },
                Segment::Flat { length: 28.0, height: 12.0 },
            ],
        )
    }

    #[test]
    fn open_flat_ground_is_fully_lit_and_fully_open() {
        let t = flat();
        assert!((sun_visibility(&t, 30.0, Sun::default()) - 1.0).abs() < 1e-3);
        assert!(sky_openness(&t, 30.0) > 0.99);
    }

    #[test]
    fn ground_in_front_of_a_wall_is_shadowed() {
        // This is the whole point of tracing: the wall at x=30 casts back
        // toward the driver with the sun ahead-right.
        let t = wall();
        let shadowed = sun_visibility(&t, 26.0, Sun::default());
        let open = sun_visibility(&t, 5.0, Sun::default());
        assert!(shadowed < 0.5, "just before the wall should be dark: {shadowed}");
        assert!(open > 0.9, "far from the wall should be lit: {open}");
    }

    #[test]
    fn the_top_of_the_wall_is_lit() {
        let t = wall();
        assert!(sun_visibility(&t, 45.0, Sun::default()) > 0.9);
    }

    #[test]
    fn shadow_softens_with_distance_instead_of_snapping() {
        // A hard edge on a heightfield this coarse reads as an artifact.
        // Visibility should climb as you walk away from the wall rather
        // than jumping 0 to 1 in one sample.
        let t = wall();
        let near = sun_visibility(&t, 26.0, Sun::default());
        let mid = sun_visibility(&t, 18.0, Sun::default());
        let far = sun_visibility(&t, 5.0, Sun::default());
        assert!(near <= mid && mid <= far, "not monotonic: {near} {mid} {far}");
        assert!(mid > near, "there should be a penumbra, not a step");
    }

    #[test]
    fn a_sun_below_the_horizon_lights_nothing() {
        let t = flat();
        assert_eq!(sun_visibility(&t, 10.0, Sun { dx: 1.0, dy: -0.2 }), 0.0);
    }

    #[test]
    fn a_trench_floor_sees_less_sky_than_open_ground() {
        // Ambient occlusion: walls either side eat the hemisphere.
        let trench = Track::new(
            "Trench",
            vec![
                Segment::Flat { length: 20.0, height: 10.0 },
                Segment::Ramp { length: 1.0, from_height: 10.0, to_height: 0.0 },
                Segment::Flat { length: 6.0, height: 0.0 },
                Segment::Ramp { length: 1.0, from_height: 0.0, to_height: 10.0 },
                Segment::Flat { length: 20.0, height: 10.0 },
            ],
        );
        let floor = sky_openness(&trench, 24.0);
        let rim = sky_openness(&trench, 5.0);
        assert!(floor < 0.6, "the trench floor should be occluded: {floor}");
        assert!(rim > 0.9, "the rim should be open: {rim}");
    }

    #[test]
    fn the_bake_matches_a_direct_trace() {
        // The cache is only useful if it says the same thing as the rays.
        let t = wall();
        let baked = BakedLighting::bake(&t, Sun::default(), 128);
        for x in [2.0f32, 12.0, 26.0, 45.0, 58.0] {
            let (s, k) = baked.at(x);
            let direct_s = sun_visibility(&t, x, Sun::default());
            let direct_k = sky_openness(&t, x);
            assert!((s - direct_s).abs() < 0.12, "sun at {x}: baked {s} vs {direct_s}");
            assert!((k - direct_k).abs() < 0.12, "sky at {x}: baked {k} vs {direct_k}");
        }
    }

    #[test]
    fn lookups_outside_the_track_clamp_instead_of_panicking() {
        let baked = BakedLighting::bake(&flat(), Sun::default(), 32);
        for x in [-100.0f32, 0.0, 1000.0] {
            let (s, k) = baked.at(x);
            assert!(s.is_finite() && k.is_finite());
            assert!((0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&k));
        }
    }

    #[test]
    fn baking_is_cheap_enough_to_do_at_load() {
        // 256 samples x 2 fields x 80 ray steps is ~40k height queries. This
        // is the number that justifies doing it once instead of per frame.
        let t = wall();
        let start = std::time::Instant::now();
        let baked = BakedLighting::bake(&t, Sun::default(), 256);
        let ms = start.elapsed().as_secs_f32() * 1000.0;
        assert_eq!(baked.sun.len(), 256);
        assert!(ms < 250.0, "bake took {ms}ms; too slow for level load");
    }
}
