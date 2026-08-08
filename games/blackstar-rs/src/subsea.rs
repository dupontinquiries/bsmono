//! SUBSEA — lighting a raid that happens under the water.
//!
//! The interesting thing about underwater light is that it is not a filter
//! over the same scene. Water absorbs different wavelengths at wildly
//! different rates, so DEPTH IS A PALETTE: red is gone by about five metres,
//! orange by fifteen, yellow by thirty, green by fifty. Blue keeps going,
//! which is why the sea is blue and why a wound bleeds green at depth.
//!
//! That gives depth a colour before anyone paints one, and it means the
//! player reads their own depth off the hue rather than off a number.
//!
//! **The idea worth building the module for: caustics read the tide.**
//! Surface waves focus sunlight into moving bands on the seabed, and the
//! pattern is a direct image of what the surface is doing. Slack water gives
//! slow wide bands; the flood gives fast narrow ones. So the light on the
//! floor tells you how long you have before the current takes you — the same
//! design as the flare warning, where the physics supplies the tell and
//! nobody has to draw a meter.
//!
//! Below the photic zone none of it applies and the only light is what
//! something living decided to make, which is its own kind of tell.

use crate::hydro::{Tide, Zone};
use crate::sun::SunState;

/// Absorption coefficient per metre for clear seawater, roughly Jerlov type
/// I. These are the real reason the sea is blue.
pub const K_RED: f32 = 0.45;
pub const K_GREEN: f32 = 0.055;
pub const K_BLUE: f32 = 0.021;

/// Where sunlight stops mattering, metres.
pub const PHOTIC_M: f32 = 200.0;

/// Light at depth, as a colour and a strength.
#[derive(Clone, Copy, Debug)]
pub struct Subsea {
    /// linear rgb multiplier on the surface light
    pub tint: [f32; 3],
    /// 0 dark .. 1 as bright as the surface
    pub level: f32,
    /// caustic band intensity, 0 none .. 1 hard bands
    pub caustic: f32,
    /// how fast the bands travel, Hz — this is the tide tell
    pub caustic_hz: f32,
    /// half-angle of Snell's window looking up, degrees. Outside it the
    /// surface is a mirror showing the seabed back at you.
    pub snell_deg: f32,
    /// light the world makes for itself, 0..1
    pub biolum: f32,
}

/// Depth in metres for a zone — the raid's own geography.
pub fn depth_of(z: Zone) -> f32 {
    match z {
        Zone::Surf => 1.5,
        Zone::Shallows => 12.0,
        Zone::OpenWater => 40.0,
        Zone::Trench => 260.0,
    }
}

/// Beer-Lambert: what fraction of a wavelength survives `d` metres.
pub fn survives(k: f32, d: f32) -> f32 {
    (-k * d.max(0.0)).exp()
}

/// SNELL'S WINDOW. Looking up from under, the whole sky compresses into a
/// cone of about 48.6 degrees half-angle — arcsin(1/1.34). Outside it the
/// surface mirrors. The window narrows as the surface roughens, because a
/// tilted facet refracts elsewhere.
pub fn snell_half_angle_deg(surface_roughness: f32) -> f32 {
    let base = (1.0f32 / 1.34).asin().to_degrees();
    base * (1.0 - 0.35 * surface_roughness.clamp(0.0, 1.0))
}

/// The lighting at a place and a moment.
pub fn light(z: Zone, sun: &SunState, tide: &Tide, t: f32) -> Subsea {
    let d = depth_of(z);
    // sunlight entering the water at all: a low sun reflects off the surface
    // rather than penetrating, so dusk goes dark underwater long before it
    // does above it
    let entry = (sun.elev.max(0.0)).powf(0.5);
    let surface = sun.strength * entry;

    let tint = [survives(K_RED, d), survives(K_GREEN, d), survives(K_BLUE, d)];
    let level = surface * (tint[0] + tint[1] + tint[2]) / 3.0;

    // CAUSTICS READ THE TIDE. The current is what stirs the surface, so band
    // speed tracks |current| directly and intensity falls with depth as the
    // focus spreads out.
    let cur = tide.current(t, z).abs();
    let stir = (cur / tide.peak_ms.max(1e-3)).clamp(0.0, 1.0);
    let caustic = surface * survives(K_BLUE, d * 1.6) * (0.35 + 0.65 * (1.0 - stir));
    // slow wide bands at slack, fast narrow ones on the flood
    let caustic_hz = 0.25 + 2.4 * stir;

    // below the photic zone the sun is irrelevant and something else lights
    // the place
    let biolum = ((d - PHOTIC_M * 0.35) / (PHOTIC_M * 0.65)).clamp(0.0, 1.0);

    Subsea {
        tint,
        level: level.clamp(0.0, 1.0),
        caustic: caustic.clamp(0.0, 1.0),
        caustic_hz,
        snell_deg: snell_half_angle_deg(stir),
        biolum,
    }
}

/// HOW LONG THE LIGHT SAYS YOU HAVE. Caustic speed is a direct readout of
/// current, so a player who reads the floor knows the crossing window
/// without a UI element. Returns seconds to the next slack.
pub fn tell_s(tide: &Tide, t: f32) -> f32 {
    tide.next_slack_s(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noon() -> SunState {
        SunState { dir: [0.0, 1.0, 0.0], strength: 0.7, color: [1.0; 3], day: 1.0, elev: 1.0, flaring: false }
    }
    fn dusk() -> SunState {
        SunState { elev: 0.08, strength: 0.25, ..noon() }
    }
    fn tide() -> Tide {
        Tide { period_s: 150.0, peak_ms: 5.2 }
    }

    #[test]
    fn depth_is_a_palette_and_red_goes_first() {
        // the reason the sea is blue, and the reason a player reads their
        // depth off the hue instead of off a number
        let shallow = light(Zone::Surf, &noon(), &tide(), 0.0);
        let deep = light(Zone::OpenWater, &noon(), &tide(), 0.0);
        assert!(shallow.tint[0] > 0.4, "red should survive the surf: {}", shallow.tint[0]);
        assert!(deep.tint[0] < 0.01, "red should be gone at 40 m: {}", deep.tint[0]);
        // and blue is still most of what is left
        assert!(deep.tint[2] > deep.tint[1] && deep.tint[1] > deep.tint[0]);
        assert!(deep.tint[2] > 0.3, "blue should carry: {}", deep.tint[2]);
    }

    #[test]
    fn dusk_goes_dark_under_the_water_long_before_it_does_above_it() {
        // a low sun reflects off the surface rather than entering it
        let day = light(Zone::Shallows, &noon(), &tide(), 0.0);
        let eve = light(Zone::Shallows, &dusk(), &tide(), 0.0);
        assert!(eve.level < day.level * 0.35, "day {} dusk {}", day.level, eve.level);
    }

    #[test]
    fn the_caustics_read_the_tide_which_is_the_whole_point() {
        let t = tide();
        // slack water: slow wide bands
        let slack = light(Zone::Shallows, &noon(), &t, 0.0);
        // peak flood: fast narrow ones
        let flood = light(Zone::Shallows, &noon(), &t, t.period_s * 0.25);
        assert!(flood.caustic_hz > slack.caustic_hz * 3.0,
            "slack {:.2} Hz, flood {:.2} Hz", slack.caustic_hz, flood.caustic_hz);
        assert!(slack.caustic > flood.caustic, "slack water should give the harder bands");
        // and the number a player could act on is available
        assert!(tell_s(&t, 0.1) > 70.0 && tell_s(&t, 74.0) < 2.0);
    }

    #[test]
    fn snells_window_is_about_forty_nine_degrees_and_narrows_when_it_is_rough() {
        let calm = snell_half_angle_deg(0.0);
        assert!((calm - 48.6).abs() < 0.5, "{calm}");
        assert!(snell_half_angle_deg(1.0) < calm * 0.7);
        // and the raid's own moments differ
        let t = tide();
        let slack = light(Zone::OpenWater, &noon(), &t, 0.0);
        let flood = light(Zone::OpenWater, &noon(), &t, t.period_s * 0.25);
        assert!(slack.snell_deg > flood.snell_deg);
    }

    #[test]
    fn the_trench_lights_itself_because_nothing_else_will() {
        let deep = light(Zone::Trench, &noon(), &tide(), 0.0);
        let shallow = light(Zone::Shallows, &noon(), &tide(), 0.0);
        assert!(deep.biolum > 0.9, "the trench should be self-lit: {}", deep.biolum);
        assert_eq!(shallow.biolum, 0.0, "the shallows should not be");
        // and the sun contributes essentially nothing down there
        assert!(deep.level < shallow.level * 0.02, "deep {} shallow {}", deep.level, shallow.level);
        assert!(deep.caustic < 0.001, "no caustics at 260 m");
    }

    #[test]
    fn absorption_is_beer_lambert_rather_than_a_curve_i_liked() {
        // halving twice over the same distance, which is what an exponential
        // does and a hand-drawn falloff does not
        let a = survives(K_GREEN, 10.0);
        let b = survives(K_GREEN, 20.0);
        assert!((b - a * a).abs() < 1e-6, "{a} {b}");
        assert_eq!(survives(K_RED, 0.0), 1.0);
    }

    #[test]
    fn every_zone_produces_something_a_renderer_can_use() {
        for z in [Zone::Surf, Zone::Shallows, Zone::OpenWater, Zone::Trench] {
            let l = light(z, &noon(), &tide(), 3.0);
            assert!((0.0..=1.0).contains(&l.level), "{}", z.label());
            assert!((0.0..=1.0).contains(&l.caustic));
            assert!(l.caustic_hz > 0.0 && l.snell_deg > 0.0);
            assert!(l.tint.iter().all(|c| (0.0..=1.0).contains(c)));
        }
    }
}
