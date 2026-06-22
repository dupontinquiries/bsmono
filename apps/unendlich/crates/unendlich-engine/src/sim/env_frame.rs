//! `sim::env_frame` — composes the ambient-environment cores into the two calls
//! the bin actually makes each frame / chunk tick, so the wiring is tested here
//! and the bin stays thin.
//!
//! - [`frame_env`]: time-of-day + weather → sun direction, shader [`Shading`],
//!   configured fog, and the ambient sound-masking floor (light → shading/fog;
//!   wind/rain → ambient noise).
//! - [`tile_step`]: one combined water-flow → erosion → moisture step over a
//!   chunk tile (water → erosion; water → moisture → plants).
//!
//! Pure, deterministic, panic-free.

use crate::sim::erosion::{erode_step, ErosionParams};
use crate::sim::fog_volume::{self, FogParams};
use crate::sim::light_physics;
use crate::sim::shading_uniforms::{shading, Shading, SkyParams};
use crate::sim::water_physics::{flow_step, moisture_step, WaterParams};

#[derive(Clone, Copy, Debug)]
pub struct TimeOfDay {
    pub t01: f32,
    pub latitude_rad: f32,
    pub season01: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Weather {
    pub overcast01: f32,
    pub humidity01: f32,
    pub turbidity01: f32,
    pub wind_speed: f32,
    pub rain01: f32,
}

impl Default for Weather {
    fn default() -> Self {
        Weather { overcast01: 0.0, humidity01: 0.3, turbidity01: 0.3, wind_speed: 2.0, rain01: 0.0 }
    }
}

/// Everything the renderer + AI need from the environment this frame.
#[derive(Clone, Copy, Debug)]
pub struct FrameEnv {
    pub sun_dir: [f32; 3],
    pub sun_elevation: f32,
    pub shading: Shading,
    pub fog: FogParams,
    pub fog_color: [f32; 3],
    /// Masking floor (dB-ish) for `sound_masking::can_hear`.
    pub ambient_sound_floor: f32,
}

/// Ambient noise floor from wind + rain (dB-ish): quiet baseline, louder with
/// stronger wind and heavier rain. Monotone, finite.
pub fn ambient_floor(wind_speed: f32, rain01: f32) -> f32 {
    let w = wind_speed.max(0.0);
    let r = rain01.clamp(0.0, 1.0);
    (20.0 + 8.0 * (1.0 + w).ln() + 18.0 * r).clamp(15.0, 90.0)
}

/// Compose the frame-level environment from time of day + weather.
pub fn frame_env(tod: TimeOfDay, w: Weather) -> FrameEnv {
    let elev = light_physics::sun_elevation(tod.t01, tod.latitude_rad, tod.season01);
    let sun_dir = light_physics::sun_direction(tod.t01, tod.latitude_rad, tod.season01);
    let irr = light_physics::sun_irradiance(tod.t01, tod.latitude_rad, tod.season01);
    let shading = shading(elev, SkyParams { turbidity01: w.turbidity01, overcast01: w.overcast01 });
    let mut fog = FogParams { humidity01: w.humidity01, day_night01: irr.clamp(0.0, 1.0), ..FogParams::default() };
    // heavier rain/overcast thickens fog a little
    fog.base_density *= 1.0 + 0.5 * w.rain01.clamp(0.0, 1.0) + 0.3 * w.overcast01.clamp(0.0, 1.0);
    let fog_color = fog_volume::fog_color(&fog);
    FrameEnv {
        sun_dir,
        sun_elevation: elev,
        shading,
        fog,
        fog_color,
        ambient_sound_floor: ambient_floor(w.wind_speed, w.rain01),
    }
}

/// One combined environment step over a chunk's heightfield tile (row-major,
/// `w*h`): add rain, flow water downhill, erode terrain, diffuse soil moisture.
/// Mutates the buffers in place; safe no-op on mismatched lengths.
pub fn tile_step(
    heights: &mut [f32],
    water: &mut [f32],
    moisture: &mut [f32],
    w: usize,
    h: usize,
    rain: f32,
    talus: f32,
) {
    if w == 0 || h == 0 || heights.len() != w * h || water.len() != w * h || moisture.len() != w * h {
        return;
    }
    let r = rain.max(0.0);
    for wv in water.iter_mut() {
        *wv = (*wv + r).max(0.0);
    }
    let wp = WaterParams::default();
    flow_step(heights, water, moisture, w, h, &wp);
    // rain already added above, so the erosion pass adds none of its own
    let ep = ErosionParams { rain: 0.0, talus: talus.max(0.0), ..ErosionParams::default() };
    erode_step(heights, water, w, h, &ep);
    moisture_step(moisture, w, h, &wp);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tod(t: f32) -> TimeOfDay { TimeOfDay { t01: t, latitude_rad: 0.6, season01: 0.5 } }

    #[test]
    fn day_is_brighter_than_night() {
        let day = frame_env(tod(0.5), Weather::default());
        let night = frame_env(tod(0.0), Weather::default());
        assert!(day.shading.sun_intensity > night.shading.sun_intensity);
        assert!(day.fog.day_night01 > night.fog.day_night01, "fog tracks daylight");
        // sun direction is a unit vector
        let d = day.sun_dir;
        assert!(((d[0]*d[0]+d[1]*d[1]+d[2]*d[2]).sqrt() - 1.0).abs() < 1e-3);
    }

    #[test]
    fn weather_thickens_fog_and_raises_noise() {
        let clear = frame_env(tod(0.5), Weather::default());
        let storm = frame_env(tod(0.5), Weather { overcast01: 1.0, humidity01: 1.0, wind_speed: 12.0, rain01: 1.0, ..Weather::default() });
        assert!(storm.fog.base_density > clear.fog.base_density);
        assert!(storm.ambient_sound_floor > clear.ambient_sound_floor);
        assert!(storm.shading.sun_intensity < clear.shading.sun_intensity, "overcast dims the sun");
    }

    #[test]
    fn ambient_floor_monotone_and_bounded() {
        assert!(ambient_floor(10.0, 0.0) > ambient_floor(1.0, 0.0), "wind raises noise");
        assert!(ambient_floor(2.0, 1.0) > ambient_floor(2.0, 0.0), "rain raises noise");
        for (w, r) in [(0.0, 0.0), (50.0, 1.0), (1e6, 1.0)] {
            let f = ambient_floor(w, r);
            assert!((15.0..=90.0).contains(&f) && f.is_finite());
        }
    }

    #[test]
    fn tile_step_flows_and_wets_no_nan() {
        let wsz = 5;
        // a slope so water moves
        let mut hs = vec![0.0f32; wsz * wsz];
        for y in 0..wsz { for x in 0..wsz { hs[y*wsz+x] = (wsz - x) as f32; } }
        let mut wa = vec![0.0f32; wsz * wsz];
        let mut mo = vec![0.0f32; wsz * wsz];
        for _ in 0..20 {
            tile_step(&mut hs, &mut wa, &mut mo, wsz, wsz, 0.05, 0.8);
        }
        assert!(wa.iter().all(|w| w.is_finite() && *w >= 0.0));
        assert!(mo.iter().all(|m| m.is_finite() && (0.0..=1.0).contains(m)));
        assert!(mo.iter().any(|&m| m > 0.0), "soil picked up moisture from rain/water");
        assert!(hs.iter().all(|h| h.is_finite()));
    }

    #[test]
    fn tile_step_size_mismatch_is_noop() {
        let mut hs = vec![1.0f32; 9];
        let mut wa = vec![1.0f32; 8];
        let mut mo = vec![0.0f32; 9];
        let before = hs.clone();
        tile_step(&mut hs, &mut wa, &mut mo, 3, 3, 0.1, 0.8);
        assert_eq!(hs, before);
    }

    #[test]
    fn deterministic() {
        let a = frame_env(tod(0.3), Weather::default());
        let b = frame_env(tod(0.3), Weather::default());
        assert_eq!(a.sun_elevation, b.sun_elevation);
        assert_eq!(a.ambient_sound_floor, b.ambient_sound_floor);
    }
}
