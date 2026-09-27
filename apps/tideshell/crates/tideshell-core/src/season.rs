//! The living design: what season and time of day it is, blended into
//! colour, warmth, motion and breathing. Nothing switches: the year is a
//! continuous mix of four seasonal characters, the day a continuous mix of
//! night, dawn, day and dusk, with dawn and dusk moving through the year.
//! Pure; the view applies `Mood` to the palette and the springs.
//!
//! Seasons (northern hemisphere; the southern one is shifted half a year):
//!   winter: ice (a muted cyan), slow and quiet motion
//!   spring: leaf green, lively motion, more bounce
//!   summer: straw gold, bright, brisk
//!   autumn: ember orange, mellow, softer motion
//! Day: night is dim with slow, calm springs and an EMBER accent (never a
//! blue: low blue light at night, and blue-everywhere is what this
//! replaces); dawn is apricot, dusk is rose; midday is the season's own.
//! The accents come from the bliss family (muted, warm pastels), not a
//! saturated default blue.

use std::f32::consts::PI;

/// The bliss-family accents the scenes are mixed from (RGB).
pub const ICE: [u8; 3] = [0x96, 0xb5, 0xb4];
pub const LEAF: [u8; 3] = [0xa3, 0xbe, 0x8c];
pub const STRAW: [u8; 3] = [0xeb, 0xcb, 0x8b];
pub const EMBER: [u8; 3] = [0xd0, 0x87, 0x70];
pub const NIGHT_EMBER: [u8; 3] = [0xab, 0x79, 0x67];
pub const APRICOT: [u8; 3] = [0xe0, 0xa0, 0x78];
pub const ROSE: [u8; 3] = [0xbf, 0x61, 0x6a];

pub fn mix_rgb(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2])]
}

fn blend4(w: [f32; 4], c: [[u8; 3]; 4]) -> [u8; 3] {
    let mut out = [0.0f32; 3];
    for i in 0..4 {
        for k in 0..3 {
            out[k] += w[i] * c[i][k] as f32;
        }
    }
    [out[0].round() as u8, out[1].round() as u8, out[2].round() as u8]
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mood {
    /// The scene's accent colour (season by day, ember by night, apricot
    /// at dawn, rose at dusk), RGB.
    pub accent: [u8; 3],
    /// -1 cool .. +1 warm (dawn/dusk warm, night cool).
    pub warmth: f32,
    /// Multiplier on the palette's saturation.
    pub saturation: f32,
    /// Multiplier on the shell's surface brightness (night dims a little).
    pub brightness: f32,
    /// Spring speed and bounce for the jiggle: night slow and damped,
    /// spring morning quick and bouncy.
    pub omega: f32,
    pub zeta: f32,
    /// Breathing: period (s) and amplitude (0..1) of the idle pulse.
    pub breath_period: f32,
    pub breath_amp: f32,
    /// For the trace / settings: "winter night", "summer dawn".
    pub season: &'static str,
    pub phase: &'static str,
}

/// Weights of (winter, spring, summer, autumn) for a day of the year,
/// north or south; a smooth cosine mix peaking at the solstices/equinoxes
/// (winter peak day 20, spring 110, summer 200, autumn 290 in the north).
pub fn season_weights(day_of_year: u32, southern: bool) -> [f32; 4] {
    let d = (day_of_year.clamp(1, 366) as f32 + if southern { 182.5 } else { 0.0 }) % 365.0;
    let peaks = [20.0f32, 110.0, 200.0, 290.0];
    let mut w = [0.0f32; 4];
    for (i, p) in peaks.iter().enumerate() {
        let mut dist = (d - p).abs();
        dist = dist.min(365.0 - dist);
        w[i] = (0.5 + 0.5 * (PI * dist / 91.25).cos()).max(0.0);
        if dist > 91.25 {
            w[i] = 0.0;
        }
    }
    let sum: f32 = w.iter().sum();
    w.map(|x| x / sum.max(1e-6))
}

/// Dawn and dusk (minutes after midnight) for a day, mid-latitude
/// approximation: winter 07:45 / 16:30, summer 05:00 / 21:30.
pub fn dawn_dusk(day_of_year: u32, southern: bool) -> (f32, f32) {
    let w = season_weights(day_of_year, southern);
    let summer = w[2] + 0.5 * (w[1] + w[3]);
    let dawn = 465.0 - (465.0 - 300.0) * summer;
    let dusk = 990.0 + (1290.0 - 990.0) * summer;
    (dawn, dusk)
}

/// Weights of (night, dawn, day, dusk) for a minute of the day, with 50-minute
/// fades around dawn and dusk.
pub fn phase_weights(minute: u32, dawn: f32, dusk: f32) -> [f32; 4] {
    let m = (minute % 1440) as f32;
    let fade = 50.0;
    let bump = |c: f32| (1.0 - ((m - c).abs() / fade)).max(0.0);
    let dawn_w = bump(dawn);
    let dusk_w = bump(dusk);
    let day_w = if m > dawn + fade && m < dusk - fade { 1.0 } else if m > dawn && m < dusk { 1.0 - dawn_w.max(dusk_w) } else { 0.0 };
    let night_w = if m < dawn - fade || m > dusk + fade { 1.0 } else if m <= dawn || m >= dusk { 1.0 - dawn_w.max(dusk_w) } else { 0.0 };
    let mut w = [night_w, dawn_w, day_w, dusk_w];
    let sum: f32 = w.iter().sum();
    w = w.map(|x| x / sum.max(1e-6));
    w
}

fn lerp4(w: [f32; 4], v: [f32; 4]) -> f32 {
    w[0] * v[0] + w[1] * v[1] + w[2] * v[2] + w[3] * v[3]
}

/// The mood for a day of the year and a minute of the day.
pub fn mood(day_of_year: u32, minute: u32, southern: bool) -> Mood {
    let s = season_weights(day_of_year, southern);
    let (dawn, dusk) = dawn_dusk(day_of_year, southern);
    let p = phase_weights(minute, dawn, dusk);
    // Season: the base accent, saturation, motion.
    let season_accent = blend4(s, [ICE, LEAF, STRAW, EMBER]);
    // Day: night pulls it to ember, dawn to apricot, dusk to rose.
    let phase_accent = blend4(p, [NIGHT_EMBER, APRICOT, season_accent, ROSE]);
    let accent = mix_rgb(season_accent, phase_accent, lerp4(p, [0.85, 0.55, 0.0, 0.55]));
    let sat_s = lerp4(s, [0.85, 1.05, 1.1, 1.0]);
    let omega_s = lerp4(s, [20.0, 32.0, 30.0, 24.0]);
    let zeta_s = lerp4(s, [0.6, 0.35, 0.4, 0.5]);
    // Day: warmth, brightness, breathing and pace.
    let warmth = lerp4(p, [-0.5, 0.8, 0.0, 0.9]) + lerp4(s, [-0.2, 0.0, 0.2, 0.15]);
    let brightness = lerp4(p, [0.85, 0.95, 1.0, 0.95]);
    let sat_p = lerp4(p, [0.8, 1.05, 1.0, 1.1]);
    let omega_p = lerp4(p, [0.65, 0.95, 1.0, 0.85]);
    let zeta_p = lerp4(p, [1.4, 1.0, 1.0, 1.15]);
    let breath_period = lerp4(p, [7.0, 4.5, 4.0, 5.5]);
    let breath_amp = lerp4(p, [0.25, 0.5, 0.35, 0.45]);
    let names = ["winter", "spring", "summer", "autumn"];
    let phases = ["night", "dawn", "day", "dusk"];
    let argmax = |w: [f32; 4]| w.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).map(|(i, _)| i).unwrap_or(0);
    Mood {
        accent,
        warmth: warmth.clamp(-1.0, 1.0),
        saturation: sat_s * sat_p,
        brightness,
        omega: omega_s * omega_p,
        zeta: (zeta_s * zeta_p).clamp(0.3, 0.95),
        breath_period,
        breath_amp,
        season: names[argmax(s)],
        phase: phases[argmax(p)],
    }
}

/// The breathing value at time `t` seconds: a raised sine in 0..1 times the amplitude.
/// Seconds of full breathing after the last interaction (a touch, a wake).
pub const BREATH_ACTIVE_S: f32 = 20.0;
/// Then it fades out over this long and holds still.
pub const BREATH_SETTLE_S: f32 = 3.0;

/// How much of the breath to show `idle_s` seconds after the last
/// interaction: 1 while someone is around, easing to 0, then exactly 0.
/// Exactly 0 matters: the frames stop changing, so the shell stops painting.
/// A lock screen left lit on a desk used to repaint the whole panel four
/// times a second to animate a gradient nobody was watching.
pub fn breath_envelope(idle_s: f32) -> f32 {
    (1.0 - (idle_s - BREATH_ACTIVE_S) / BREATH_SETTLE_S).clamp(0.0, 1.0)
}

pub fn breath(m: &Mood, t_secs: f32) -> f32 {
    m.breath_amp * 0.5 * (1.0 - (2.0 * PI * t_secs / m.breath_period).cos())
}

/// Day of the year from a Gregorian date.
pub fn day_of_year(year: i32, month: u32, day: u32) -> u32 {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    days.iter().take((month.clamp(1, 12) - 1) as usize).sum::<u32>() + day.clamp(1, 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_breath_settles_to_exactly_zero_when_nobody_is_around() {
        assert_eq!(breath_envelope(0.0), 1.0);
        assert_eq!(breath_envelope(BREATH_ACTIVE_S), 1.0);
        let mid = breath_envelope(BREATH_ACTIVE_S + BREATH_SETTLE_S / 2.0);
        assert!(mid > 0.0 && mid < 1.0);
        assert_eq!(breath_envelope(BREATH_ACTIVE_S + BREATH_SETTLE_S), 0.0);
        assert_eq!(breath_envelope(3600.0), 0.0);
    }

    #[test]
    fn the_year_is_a_smooth_mix_that_peaks_at_the_right_times() {
        let w = season_weights(20, false);
        assert!(w[0] > 0.95, "mid-January is winter: {w:?}");
        let w = season_weights(200, false);
        assert!(w[2] > 0.95, "mid-July is summer");
        let w = season_weights(65, false);
        assert!(w[0] > 0.3 && w[1] > 0.3, "early March is both: {w:?}");
        assert!(season_weights(20, true)[2] > 0.95, "January in the south is summer");
        // Continuity: consecutive days differ by little.
        for d in 1..365 {
            let a = season_weights(d, false);
            let b = season_weights(d + 1, false);
            assert!((0..4).all(|i| (a[i] - b[i]).abs() < 0.03), "jump at day {d}");
        }
    }

    #[test]
    fn dawn_and_dusk_move_with_the_season() {
        let (wd, wk) = dawn_dusk(20, false);
        let (sd, sk) = dawn_dusk(200, false);
        assert!(wd > sd + 120.0, "winter dawn is later: {wd} vs {sd}");
        assert!(sk > wk + 240.0, "summer dusk is much later: {sk} vs {wk}");
    }

    #[test]
    fn night_is_cool_slow_and_dim_a_summer_noon_is_neutral_and_quick() {
        let night = mood(200, 2 * 60, false);
        let noon = mood(200, 12 * 60, false);
        let dusk = mood(200, (dawn_dusk(200, false).1) as u32, false);
        assert_eq!((night.phase, noon.phase, dusk.phase), ("night", "day", "dusk"));
        assert!(night.warmth < 0.0 && dusk.warmth > 0.5);
        assert!(night.brightness < noon.brightness);
        assert!(night.omega < noon.omega * 0.75, "night springs are slower: {} vs {}", night.omega, noon.omega);
        assert!(night.zeta > noon.zeta, "and more damped");
        assert!(night.breath_period > noon.breath_period, "and the breathing slower");
        assert_eq!(mood(20, 12 * 60, false).season, "winter");
        let spring = mood(110, 12 * 60, false).accent;
        assert!(spring[1] > spring[0] && spring[1] > spring[2], "spring day is green: {spring:?}");
        let winter = mood(20, 12 * 60, false).accent;
        assert!(winter[2] >= winter[0] && winter[1] >= winter[0], "winter day is ice: {winter:?}");
    }

    #[test]
    fn nothing_is_blue_at_night_in_any_season() {
        for day in [20, 110, 200, 290] {
            let a = mood(day, 2 * 60, false).accent;
            assert!(a[0] > a[2] + 30, "night accent day {day} is warm, not blue: {a:?}");
            let d = mood(day, (dawn_dusk(day, false).0) as u32, false).accent;
            assert!(d[0] > d[2], "dawn is warm: {d:?}");
        }
        let a = mix_rgb([0, 0, 0], [200, 100, 50], 0.5);
        assert_eq!(a, [100, 50, 25]);
    }

    #[test]
    fn breathing_is_a_raised_sine_within_the_amplitude() {
        let m = mood(200, 12 * 60, false);
        let vals: Vec<f32> = (0..80).map(|i| breath(&m, i as f32 * 0.05)).collect();
        assert!(vals.iter().all(|v| *v >= 0.0 && *v <= m.breath_amp + 1e-6));
        assert!(vals[0].abs() < 1e-6 && (vals[40] - m.breath_amp).abs() < 1e-3, "one period is 4 s here");
    }

    #[test]
    fn day_of_year_handles_leap_years() {
        assert_eq!(day_of_year(2026, 1, 1), 1);
        assert_eq!(day_of_year(2026, 3, 1), 60);
        assert_eq!(day_of_year(2028, 3, 1), 61, "leap year");
        assert_eq!(day_of_year(2026, 12, 31), 365);
    }
}
