//! Sky / day-night cycle.
//!
//! Tracks world time, computes sun and moon directions from it, and exposes
//! ambient colors. Other systems consume this:
//!   - The mesher uses sun direction to compute per-face directional shading.
//!   - The renderer (eventually) uses ambient color for the clear color and
//!     fog. A real sky shader would consume the same sun direction for
//!     scattering / glow.
//!   - Mob AI can check "is it night?" to decide spawning and behavior.
//!   - Plant growth could check sun exposure (raycast toward sun) to decide
//!     growth rate.
//!
//! This is the *minimal* version. No atmospheric scattering, no real moon
//! lighting (moon is just a dimmer sun on the opposite side), no eclipse,
//! no varying day length per latitude.
//!
//! ## Rollback note
//!
//! Sky time today advances on wall-clock `real_dt` from the bin. That
//! makes it NOT rollback-safe: same TickId on different machines
//! produces different sun positions if frame timings differ.
//!
//! For VISUAL consumers (mesh shading, fog) that's fine — visuals
//! don't need to roll back. But once a SIMULATION consumer appears
//! (mob spawner that fires at night, plant growth that reads sun
//! exposure), the sky should switch to ticking on `TickId × period`
//! instead. Until then this is acceptable technical debt; the
//! Snapshottable impl captures the wall-clock-derived time anyway,
//! so rollback at least produces a *consistent* sky on replay.
//!
//! Time model:
//!   - One full day = `DAY_LENGTH_SECONDS` real seconds (default 600 = 10 min).
//!   - Day phase 0.0 = sunrise (sun on horizon, +X direction).
//!   - Day phase 0.25 = noon (sun overhead, +Y).
//!   - Day phase 0.5 = sunset (sun on horizon, -X direction).
//!   - Day phase 0.75 = midnight (sun "below" -Y; we render with the moon
//!     direction = -sun direction, dimmer).
//!
//! All time arithmetic is in real seconds for now. Game time = real time ×
//! `time_scale` (default 1.0).

/// Default real seconds per in-game day. 600 = 10 minutes per day, which is
/// fast enough that you see day/night cycles during a play session and slow
/// enough that the lighting changes feel atmospheric, not flickery.
pub const DAY_LENGTH_SECONDS: f32 = 600.0;

/// Sun direction at noon in world space. We point the sun "down" toward the
/// world from above. (-Y means light comes from above and shines toward -Y,
/// so face normals pointing +Y get full lighting at noon.)
///
/// Currently unused — the sun direction is computed dynamically in
/// `Sky::sun_dir` from `time_of_day`. Kept here as a reference value
/// for future regression tests and as documentation of the convention.
#[allow(dead_code)]
const NOON_SUN_DIR: [f32; 3] = [0.0, -1.0, 0.0];

/// Range for directional lighting contribution. Min is what a fully back-lit
/// face gets (so it isn't pitch black); max is what a fully sun-lit face gets.
/// Together with ambient (in mesher) these clamp the on-screen brightness.
pub const SUN_LIGHT_MIN: f32 = 0.45;
pub const SUN_LIGHT_MAX: f32 = 1.0;

/// Phase below which we consider it night (sun is below horizon).
const NIGHT_PHASE_START: f32 = 0.5;
const NIGHT_PHASE_END: f32 = 1.0;

/// Sky / world-time resource. Cheap to clone; pass by value to consumers
/// that just need a snapshot.
#[derive(Debug, Clone, Copy)]
pub struct Sky {
    /// In-game time in seconds since world creation. Wraps within the day
    /// for phase calculations but the absolute value can be very large for
    /// long-running worlds.
    pub time_seconds: f64,
    /// Day length override (defaults to `DAY_LENGTH_SECONDS`).
    pub day_length: f32,
    /// Real-time-to-game-time scale. 1.0 = real time. 60.0 = 1 real second
    /// = 1 game minute. Useful for testing day/night without waiting.
    pub time_scale: f32,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            // Start at phase 0.2 — mid-morning. Avoids spawning at exact
            // noon (boring) or pitch black (player can't see).
            time_seconds: (DAY_LENGTH_SECONDS as f64) * 0.2,
            day_length: DAY_LENGTH_SECONDS,
            time_scale: 1.0,
        }
    }
}

impl Sky {
    pub fn new() -> Self { Self::default() }

    /// Advance world time. `real_dt` is real-world seconds elapsed since the
    /// last call.
    pub fn tick(&mut self, real_dt: f32) {
        self.time_seconds += (real_dt * self.time_scale) as f64;
    }

    /// Phase within the current day, in [0, 1). 0 = sunrise, 0.25 = noon,
    /// 0.5 = sunset, 0.75 = midnight.
    pub fn day_phase(&self) -> f32 {
        let phase = (self.time_seconds % self.day_length as f64) / self.day_length as f64;
        phase as f32
    }

    /// Is it currently nighttime?
    pub fn is_night(&self) -> bool {
        let p = self.day_phase();
        p >= NIGHT_PHASE_START && p < NIGHT_PHASE_END
    }

    /// Unit vector pointing FROM the sun TOWARD the world (i.e. the direction
    /// sunlight is traveling). Computed from the day phase: sun rises at +X,
    /// peaks overhead, sets at -X.
    ///
    /// During night this returns the moon direction (= -sun direction). The
    /// `is_night()` flag tells you whether to dim the lighting.
    pub fn sun_direction(&self) -> [f32; 3] {
        let phase = self.day_phase();
        // Map phase [0,1) → angle [0, 2π).
        let angle = phase * std::f32::consts::TAU;
        // Sun moves in the X-Y plane (skips Z for simplicity — no seasons yet).
        // Phase 0 = sunrise: sun is at +X direction (from sun's perspective,
        //   light travels in -X direction).
        // Phase 0.25 = noon: sun is at +Y (light travels in -Y direction = down).
        // Phase 0.5 = sunset: sun is at -X (light travels +X direction).
        // Phase 0.75 = midnight: sun is at -Y (light travels +Y, comes from below).
        let sun_pos_x = angle.cos();
        let sun_pos_y = angle.sin();
        // Light travels FROM sun TO world, so direction is -sun_pos.
        [-sun_pos_x, -sun_pos_y, 0.0]
    }

    /// Directional light intensity multiplier. 1.0 at noon, fades to 0 at
    /// horizon, 0 at night (we render night as moonlit ambient only — the
    /// directional contribution from the moon is folded into the ambient).
    pub fn directional_intensity(&self) -> f32 {
        // intensity = max(0, sin(angle)) * smoothing.
        // At noon (angle=π/2) intensity = 1; at sunset/sunrise (angle = 0 or
        // π) intensity = 0; at night intensity stays at 0 (we hide the moon
        // contribution to keep this simple).
        let phase = self.day_phase();
        let angle = phase * std::f32::consts::TAU;
        angle.sin().max(0.0)
    }

    /// RGB ambient color. Daytime is warm sunlight white; night is cool blue
    /// moonlight. Smooth transition over a small phase window.
    pub fn ambient_color(&self) -> [f32; 3] {
        let phase = self.day_phase();
        // Linear interpolate between night and day colors at the boundaries.
        let day_color = [0.92_f32, 0.92, 0.88];
        let night_color = [0.30_f32, 0.32, 0.45];
        // Transition windows: 0.0..0.05 (sunrise), 0.45..0.55 (sunset).
        let blend_to_day = if phase < 0.05 {
            phase / 0.05
        } else if phase >= 0.45 && phase < 0.55 {
            1.0 - (phase - 0.45) / 0.10
        } else if phase >= 0.55 {
            0.0
        } else {
            1.0
        };
        let day_w = blend_to_day.clamp(0.0, 1.0);
        let night_w = 1.0 - day_w;
        [
            day_color[0] * day_w + night_color[0] * night_w,
            day_color[1] * day_w + night_color[1] * night_w,
            day_color[2] * day_w + night_color[2] * night_w,
        ]
    }

    /// The world's sky color, for use as the renderer's clear color
    /// (i.e. what the player sees BEYOND loaded geometry — the
    /// "atmospheric horizon"). Tracks the diurnal cycle:
    ///
    ///   - Phase 0.0 (sunrise): warm orange-pink, simulating sun just
    ///     hitting the horizon.
    ///   - Phase 0.25 (noon): bright cyan-blue, full daylight sky.
    ///   - Phase 0.5 (sunset): warm orange-pink (mirror of sunrise).
    ///   - Phase 0.75 (midnight): deep navy-blue, moonlit night sky.
    ///
    /// Returns RGBA with alpha=1.0. Color values are linear-sRGB-ish
    /// (the swapchain handles gamma).
    ///
    /// The sky color picked here also ELEGANTLY HIDES the partial-
    /// load state: chunks generating at the edges of render distance
    /// fade into the same color as the void beyond them, so the
    /// "floating green island" perception during world load is
    /// reduced. The world feels like it's emerging from atmosphere
    /// rather than appearing in a hard void.
    pub fn sky_color_rgba(&self) -> [f32; 4] {
        let phase = self.day_phase();
        // Three keyframe colors: noon, sunset/sunrise edge, midnight.
        let noon = [0.55_f32, 0.75, 0.95];
        let twilight = [0.95_f32, 0.55, 0.40]; // warm orange-pink
        let midnight = [0.04_f32, 0.05, 0.15]; // deep navy

        // Compute weights for each keyframe based on phase.
        // - noon weight peaks at 0.25.
        // - twilight peaks at 0.0 and 0.5.
        // - midnight peaks at 0.75.
        //
        // We piecewise-blend to keep the math simple and predictable:
        //   [0.0, 0.1):   twilight → noon
        //   [0.1, 0.4):   pure noon
        //   [0.4, 0.55):  noon → twilight
        //   [0.55, 0.65): twilight → midnight
        //   [0.65, 0.95): pure midnight
        //   [0.95, 1.0):  midnight → twilight (toward next sunrise)
        let (a, b, t) = if phase < 0.10 {
            (twilight, noon, phase / 0.10)
        } else if phase < 0.40 {
            (noon, noon, 0.0)
        } else if phase < 0.55 {
            (noon, twilight, (phase - 0.40) / 0.15)
        } else if phase < 0.65 {
            (twilight, midnight, (phase - 0.55) / 0.10)
        } else if phase < 0.95 {
            (midnight, midnight, 0.0)
        } else {
            (midnight, twilight, (phase - 0.95) / 0.05)
        };

        let inv_t = 1.0 - t;
        [
            a[0] * inv_t + b[0] * t,
            a[1] * inv_t + b[1] * t,
            a[2] * inv_t + b[2] * t,
            1.0,
        ]
    }

    /// Compute the lighting multiplier for a face with the given outward
    /// normal. This is what the mesher calls per-face. Returns a value in
    /// [SUN_LIGHT_MIN, SUN_LIGHT_MAX].
    pub fn face_brightness(&self, normal: [f32; 3]) -> f32 {
        let sun_dir = self.sun_direction();
        // Direction FROM face TO sun = -sun_dir (since sun_dir is light flow
        // direction from sun to ground). Lighting = max(0, dot(normal, -sun_dir)).
        let to_sun = [-sun_dir[0], -sun_dir[1], -sun_dir[2]];
        let dot = normal[0] * to_sun[0] + normal[1] * to_sun[1] + normal[2] * to_sun[2];
        let directional = dot.max(0.0) * self.directional_intensity();
        // Lerp between min and max based on directional contribution.
        SUN_LIGHT_MIN + (SUN_LIGHT_MAX - SUN_LIGHT_MIN) * directional.clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sky_starts_during_day() {
        let s = Sky::default();
        assert!(!s.is_night());
        assert!(s.day_phase() > 0.0 && s.day_phase() < 0.5);
    }

    #[test]
    fn tick_advances_time() {
        let mut s = Sky::new();
        let p0 = s.day_phase();
        s.tick(60.0); // 1 minute
        let p1 = s.day_phase();
        assert!(p1 > p0);
    }

    #[test]
    fn full_day_cycle() {
        let mut s = Sky::new();
        s.time_seconds = 0.0;
        // Sunrise.
        assert!((s.day_phase() - 0.0).abs() < 1e-3);
        // Quarter day = noon.
        s.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.25;
        assert!((s.day_phase() - 0.25).abs() < 1e-3);
        assert!(!s.is_night());
        // Half day = sunset (still day per our convention).
        s.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.5;
        assert!(s.is_night(), "phase 0.5 should be classified as night");
        // Three-quarter = midnight.
        s.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.75;
        assert!(s.is_night());
    }

    #[test]
    fn noon_top_face_is_brightest() {
        let mut s = Sky::new();
        s.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.25; // noon
        let top_face = [0.0, 1.0, 0.0];
        let bot_face = [0.0, -1.0, 0.0];
        let bright_top = s.face_brightness(top_face);
        let bright_bot = s.face_brightness(bot_face);
        assert!(bright_top > bright_bot);
        assert!((bright_top - SUN_LIGHT_MAX).abs() < 1e-3);
    }

    #[test]
    fn night_face_brightness_clamped_to_min() {
        let mut s = Sky::new();
        s.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.75; // midnight
        // All faces should get the minimum brightness (no directional sun).
        for n in &[[1.0,0.0,0.0_f32], [0.0,1.0,0.0], [0.0,0.0,1.0],
                   [-1.0,0.0,0.0], [0.0,-1.0,0.0], [0.0,0.0,-1.0]] {
            let b = s.face_brightness(*n);
            assert!((b - SUN_LIGHT_MIN).abs() < 1e-3,
                "midnight brightness for {:?} = {}, expected {}", n, b, SUN_LIGHT_MIN);
        }
    }

    #[test]
    fn sun_direction_unit_length() {
        let mut s = Sky::new();
        for phase_step in 0..10 {
            s.time_seconds = (DAY_LENGTH_SECONDS as f64) * (phase_step as f64 / 10.0);
            let d = s.sun_direction();
            let len = (d[0]*d[0] + d[1]*d[1] + d[2]*d[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "sun direction not unit length: {} at phase {}",
                len, s.day_phase());
        }
    }

    /// Sky color tracks the diurnal cycle. Noon should be blue-cyan;
    /// midnight should be deep navy. This is the visual feedback that
    /// time of day actually matters.
    #[test]
    fn sky_color_at_noon_is_blue() {
        let mut s = Sky::new();
        s.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.25;
        let c = s.sky_color_rgba();
        // Noon keyframe is [0.55, 0.75, 0.95]: blue dominant.
        assert!(c[2] > c[0], "noon blue should exceed red: {:?}", c);
        assert!(c[2] > 0.7, "noon blue should be bright: {:?}", c);
        assert_eq!(c[3], 1.0, "alpha should be 1.0");
    }

    #[test]
    fn sky_color_at_midnight_is_dark() {
        let mut s = Sky::new();
        s.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.75;
        let c = s.sky_color_rgba();
        // Midnight keyframe is [0.04, 0.05, 0.15]: very dark, blue-tinted.
        assert!(c[0] < 0.15, "midnight red should be dark: {:?}", c);
        assert!(c[1] < 0.15, "midnight green should be dark: {:?}", c);
        assert!(c[2] < 0.25, "midnight blue should be dark: {:?}", c);
        // Blue should still be slightly more than red (cool tint).
        assert!(c[2] > c[0], "midnight should keep blue tint: {:?}", c);
    }

    #[test]
    fn sky_color_at_sunset_is_warm() {
        let mut s = Sky::new();
        // Sunset is around phase 0.475 (deep into the noon→twilight blend).
        s.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.475;
        let c = s.sky_color_rgba();
        // Twilight keyframe is [0.95, 0.55, 0.40]: warm orange.
        // At phase 0.475 we're 0.075/0.15 = 50% blended toward twilight.
        // Expect red-shifted compared to noon.
        let mut noon_sky = Sky::new();
        noon_sky.time_seconds = (DAY_LENGTH_SECONDS as f64) * 0.25;
        let noon_c = noon_sky.sky_color_rgba();
        assert!(c[0] > noon_c[0],
            "sunset should be redder than noon: sunset={:?} noon={:?}", c, noon_c);
    }

    #[test]
    fn sky_color_smoothly_interpolates() {
        // Check no sudden jumps between adjacent phases.
        let mut s = Sky::new();
        let mut prev = [0.0_f32; 4];
        let mut max_jump = 0.0_f32;
        for phase_step in 0..200 {
            let phase = phase_step as f64 / 200.0;
            s.time_seconds = (DAY_LENGTH_SECONDS as f64) * phase;
            let c = s.sky_color_rgba();
            if phase_step > 0 {
                let jump = ((c[0] - prev[0]).abs()
                          + (c[1] - prev[1]).abs()
                          + (c[2] - prev[2]).abs())
                    / 3.0;
                max_jump = max_jump.max(jump);
            }
            prev = c;
        }
        // Adjacent (0.5% phase apart) samples should differ by < 0.06 per
        // channel-average. The threshold accommodates the steepest
        // transition (twilight → midnight, 0.10-phase window with
        // channel deltas of up to ~0.55).
        assert!(max_jump < 0.06,
            "sky color should be smooth, max jump = {}", max_jump);
    }

    #[test]
    fn sky_color_alpha_always_one() {
        let mut s = Sky::new();
        for phase_step in 0..100 {
            let phase = phase_step as f64 / 100.0;
            s.time_seconds = (DAY_LENGTH_SECONDS as f64) * phase;
            let c = s.sky_color_rgba();
            assert_eq!(c[3], 1.0,
                "alpha should always be 1.0, got {} at phase {}", c[3], phase);
        }
    }

    #[test]
    fn sky_color_rgb_in_unit_range() {
        let mut s = Sky::new();
        for phase_step in 0..100 {
            let phase = phase_step as f64 / 100.0;
            s.time_seconds = (DAY_LENGTH_SECONDS as f64) * phase;
            let c = s.sky_color_rgba();
            for ch in 0..3 {
                assert!(c[ch] >= 0.0 && c[ch] <= 1.0,
                    "sky channel {} out of range: {} at phase {}", ch, c[ch], phase);
            }
        }
    }
}
