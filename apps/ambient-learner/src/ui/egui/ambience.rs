//! ambience — `HumiditySparkles` and `AnimatedBackground`, ported.
//!
//! The decorative layer, and it is decorative in the way a barometer's needle
//! is: everything moving on screen is driven by a reading. The season and the
//! hour set the gradient, humidity sets how many droplets drift across it, tilt
//! shifts the parallax, and the pressure trend nudges the drift. Nothing here
//! is random for its own sake.
//!
//! ## Why this is worth porting rather than dropping
//!
//! It is the only part of the app that communicates without being read. Someone
//! who glances at the screen learns roughly how humid and how bright it is
//! before focusing on a number, and that is most of what an ambient display is
//! for. A port that carried the pages and left this out would be a strictly
//! worse app that passed the same audit.
//!
//! ## The counts and ranges are the contract
//!
//! `desiredCount: Math.round(humidityPct / 4.0)` — zero droplets at 0%, about
//! twenty-five at 100%. The specific ceiling matters: the QML comment calls out
//! ~25 as the number, and past roughly thirty the screen reads as snow rather
//! than damp air, which is a different weather entirely.

/// Droplets at 100% humidity.
pub const MAX_SPARKLES: u32 = 25;

/// Percent of humidity per droplet — the QML's `/ 4.0`.
pub const PCT_PER_SPARKLE: f32 = 4.0;

/// How far tilt shifts the gradient origin, in pixels at full tilt.
///
/// Small on purpose: parallax that moves further than this stops reading as
/// depth and starts reading as the background being loose.
pub const PARALLAX_PX: f32 = 18.0;

/// Time of day, as the theme names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase { Dawn, Day, Evening, Night }

/// Season, as the theme names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Season { Spring, Summer, Autumn, Winter }

impl Phase {
    pub fn parse(s: &str) -> Phase {
        match s.trim().to_lowercase().as_str() {
            "dawn" => Phase::Dawn,
            "day" => Phase::Day,
            "night" => Phase::Night,
            // Evening is the QML's default property value, so an unknown
            // string lands where the app already sat rather than somewhere new.
            _ => Phase::Evening,
        }
    }
}

impl Season {
    pub fn parse(s: &str) -> Season {
        match s.trim().to_lowercase().as_str() {
            "spring" => Season::Spring,
            "summer" => Season::Summer,
            "autumn" | "fall" => Season::Autumn,
            _ => Season::Winter,
        }
    }
}

/// Droplets drifting over the background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sparkles {
    /// 0..=100.
    pub humidity_pct: f32,
}

impl Sparkles {
    /// How many droplets to draw.
    ///
    /// Zero at 0% is the important end. An ambient display that always shows a
    /// few droplets cannot say "dry", and the whole point is that the screen
    /// looks different in different air.
    pub fn count(&self) -> u32 {
        if self.humidity_pct <= 0.0 { return 0; }
        let n = (self.humidity_pct / PCT_PER_SPARKLE).round() as i64;
        n.clamp(0, MAX_SPARKLES as i64) as u32
    }

    /// Deterministic position for droplet `i`, in 0..=1 on each axis.
    ///
    /// Hashed from the index rather than drawn from a random source, so a
    /// droplet keeps its place across frames. With `Math.random()` per frame
    /// they would flicker across the screen; QML got away with it by creating
    /// the objects once, and this redraws every frame, so the determinism has
    /// to come from somewhere.
    pub fn position(&self, i: u32) -> (f32, f32) {
        let h = |k: u32| {
            let mut x = k.wrapping_mul(0x9E37_79B9) ^ 0x85EB_CA6B;
            x ^= x >> 15;
            x = x.wrapping_mul(0x2545_F491);
            x ^= x >> 13;
            (x % 10_000) as f32 / 10_000.0
        };
        (h(i * 2 + 1), h(i * 2 + 2))
    }

    /// Droplet radius in pixels — the QML's `3 + random * 3`, made stable.
    pub fn radius(&self, i: u32) -> f32 { 1.5 + self.position(i).0 * 1.5 }

    /// Opacity for droplet `i` at a given time, 0..=1.
    ///
    /// Each droplet breathes on its own phase. In unison they would pulse like
    /// a single alarm rather than drift like moisture.
    pub fn alpha(&self, i: u32, t_ms: f32) -> f32 {
        if i >= self.count() { return 0.0; }
        let period = 12_000.0 + self.position(i).1 * 8_000.0;
        let phase = (t_ms / period + self.position(i).0) * std::f32::consts::TAU;
        // Peaks well below opaque: these sit in FRONT of the gradient and must
        // not compete with the text over them.
        0.35 * ((phase.sin() + 1.0) * 0.5)
    }
}

/// The mood background.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ambience {
    pub season: Season,
    pub phase: Phase,
    /// Device tilt, -1..=1 on each axis.
    pub tilt: (f32, f32),
    /// Ambient light, 0..=1.
    pub light_norm: f32,
    /// hPa per 30 minutes, as the pressure gauge reports it.
    pub pressure_trend: f32,
}

impl Default for Ambience {
    fn default() -> Self {
        Ambience { season: Season::Winter, phase: Phase::Evening,
                   tilt: (0.0, 0.0), light_norm: 0.0, pressure_trend: 0.0 }
    }
}

impl Ambience {
    /// Where the gradient origin sits, in pixels from centre.
    ///
    /// Clamped, because a tilt reading that overshoots would slide the gradient
    /// off its own panel and expose the background behind it.
    pub fn parallax(&self) -> (f32, f32) {
        (self.tilt.0.clamp(-1.0, 1.0) * PARALLAX_PX,
         self.tilt.1.clamp(-1.0, 1.0) * PARALLAX_PX)
    }

    /// How fast the droplets drift sideways, in fractions of width per second.
    ///
    /// Falling pressure drifts them one way and rising the other — the sign is
    /// carried through rather than taken as a magnitude, because "weather
    /// coming in" and "weather clearing" should not look identical.
    pub fn drift_per_sec(&self) -> f32 {
        (self.pressure_trend / 6.0).clamp(-0.05, 0.05)
    }

    /// A brightness multiplier for the whole scene, 0.55..=1.0.
    ///
    /// Never reaches zero. A dark room should dim the display, not blank it —
    /// the user is still standing there, and a screen that goes black looks
    /// broken rather than considerate.
    pub fn brightness(&self) -> f32 {
        0.55 + 0.45 * self.light_norm.clamp(0.0, 1.0)
    }
}

/// Draw the background and its droplets.
pub fn ambience_ui(ui: &mut egui::Ui, a: &Ambience, s: &Sparkles, t_ms: f32,
                   rect: egui::Rect, c: &uikit_egui::PaletteColors) {
    let painter = ui.painter();
    let (px, py) = a.parallax();
    let mut g = rect;
    g = g.translate(egui::vec2(px, py));
    // The panel is drawn larger than the rect so the parallax shift cannot
    // expose an edge.
    g = g.expand(PARALLAX_PX);
    painter.rect_filled(g, 0.0, c.bg_base);

    let b = a.brightness();
    let drift = a.drift_per_sec() * (t_ms / 1000.0);
    for i in 0..s.count() {
        let (fx, fy) = s.position(i);
        // Wrapped rather than clamped: a droplet that reached the edge and
        // stopped would pile up there.
        let x = ((fx + drift).rem_euclid(1.0)) * rect.width();
        let y = fy * rect.height();
        let alpha = s.alpha(i, t_ms) * b;
        if alpha <= 0.001 { continue; }
        let col = egui::Color32::from_rgba_unmultiplied(
            0xa8, 0xe6, 0xf0, (alpha * 255.0) as u8);
        painter.circle_filled(egui::pos2(rect.left() + x, rect.top() + y),
                              s.radius(i), col);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(pct: f32) -> Sparkles { Sparkles { humidity_pct: pct } }

    #[test]
    fn dry_air_shows_nothing_at_all() {
        // The important end. A display that always shows a few droplets cannot
        // say "dry", and the whole point is that the screen looks different in
        // different air.
        assert_eq!(sp(0.0).count(), 0);
        assert_eq!(sp(0.0).alpha(0, 500.0), 0.0);
    }

    #[test]
    fn full_humidity_shows_about_twenty_five() {
        // Past roughly thirty the screen reads as snow rather than damp air,
        // which is a different weather entirely.
        assert_eq!(sp(100.0).count(), MAX_SPARKLES);
        assert_eq!(sp(50.0).count(), 13);
    }

    #[test]
    fn a_reading_above_a_hundred_is_clamped_rather_than_trusted() {
        // A sensor reporting 140% is a fault; a blizzard of droplets on screen
        // is a worse way to find out about it.
        assert_eq!(sp(140.0).count(), MAX_SPARKLES);
        assert_eq!(sp(-5.0).count(), 0);
    }

    #[test]
    fn a_droplet_keeps_its_place_across_frames() {
        // QML used Math.random() at construction and kept the objects. This
        // redraws every frame, so without deterministic placement the droplets
        // would flicker across the screen.
        let s = sp(100.0);
        assert_eq!(s.position(3), s.position(3));
        assert_ne!(s.position(3), s.position(4));
    }

    #[test]
    fn droplets_are_spread_rather_than_stacked() {
        // A hash that clustered would put every droplet in one corner and look
        // like a rendering fault.
        let s = sp(100.0);
        let xs: Vec<f32> = (0..s.count()).map(|i| s.position(i).0).collect();
        let lo = xs.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(hi - lo > 0.6, "spread was only {}", hi - lo);
        assert!(xs.iter().all(|x| (0.0..=1.0).contains(x)));
    }

    #[test]
    fn droplets_breathe_on_their_own_phases() {
        // In unison they would pulse like a single alarm rather than drift like
        // moisture.
        let s = sp(100.0);
        let a = s.alpha(1, 3_000.0);
        let b = s.alpha(7, 3_000.0);
        assert!((a - b).abs() > 0.01, "{a} vs {b}");
    }

    #[test]
    fn droplet_opacity_stays_well_below_opaque() {
        // They sit in FRONT of the gradient and must not compete with the text
        // over them.
        let s = sp(100.0);
        for i in 0..s.count() {
            for t in [0, 1_000, 5_000, 20_000] {
                assert!(s.alpha(i, t as f32) <= 0.36);
            }
        }
    }

    #[test]
    fn an_unknown_phase_lands_where_the_app_already_sat() {
        // Evening is the QML's default property value, so an unrecognised
        // string must not move the display somewhere it has never been.
        assert_eq!(Phase::parse("nonsense"), Phase::Evening);
        assert_eq!(Season::parse("nonsense"), Season::Winter);
        assert_eq!(Phase::parse("DAWN"), Phase::Dawn);
        assert_eq!(Season::parse("fall"), Season::Autumn);
    }

    #[test]
    fn parallax_is_small_and_clamped() {
        // Further than this stops reading as depth and starts reading as the
        // background being loose. A tilt overshoot would slide the gradient off
        // its own panel and expose what is behind it.
        let a = Ambience { tilt: (1.0, -1.0), ..Default::default() };
        assert_eq!(a.parallax(), (PARALLAX_PX, -PARALLAX_PX));
        let over = Ambience { tilt: (5.0, -5.0), ..Default::default() };
        assert_eq!(over.parallax(), (PARALLAX_PX, -PARALLAX_PX));
    }

    #[test]
    fn the_pressure_trend_keeps_its_sign() {
        // "Weather coming in" and "weather clearing" must not look identical.
        let falling = Ambience { pressure_trend: -3.0, ..Default::default() };
        let rising = Ambience { pressure_trend: 3.0, ..Default::default() };
        assert!(falling.drift_per_sec() < 0.0);
        assert!(rising.drift_per_sec() > 0.0);
    }

    #[test]
    fn a_storm_does_not_send_the_droplets_flying() {
        // A 20 hPa collapse is a real reading and must not turn the display
        // into a stampede.
        let storm = Ambience { pressure_trend: -20.0, ..Default::default() };
        assert!(storm.drift_per_sec().abs() <= 0.05);
    }

    #[test]
    fn a_dark_room_dims_the_display_rather_than_blanking_it() {
        // The user is still standing there, and a screen that goes black looks
        // broken rather than considerate.
        let dark = Ambience { light_norm: 0.0, ..Default::default() };
        let bright = Ambience { light_norm: 1.0, ..Default::default() };
        assert!(dark.brightness() >= 0.55);
        assert_eq!(bright.brightness(), 1.0);
        assert!(dark.brightness() < bright.brightness());
    }

    #[test]
    fn an_out_of_range_light_reading_is_clamped() {
        let a = Ambience { light_norm: 4.0, ..Default::default() };
        assert_eq!(a.brightness(), 1.0);
        let b = Ambience { light_norm: -1.0, ..Default::default() };
        assert!(b.brightness() >= 0.55);
    }
}
