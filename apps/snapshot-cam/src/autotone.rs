//! autotone -- what a photo from this phone needs before anyone sees it.
//!
//! Measured on a day's roll from the FP5 (39 photos): the soft ISP has no black-level tuning for the
//! sensor, so the darkest 1% of pixels sits near 52/255 (a lifted black, a veil over everything),
//! the median is ~161 (bright), a fifth to a half of the frame is clipped, and most carry a green or
//! cyan cast. This measures each frame and answers with the three things `develop::render` already
//! takes: a **black point** (the `flare` subtraction), **white balance gains** from the near-neutral
//! surfaces only (a blue sky or a green wall must not be "corrected" to grey), and an **exposure
//! bias** toward a median that leaves the highlights room. It cannot bring back what the sensor
//! frame already clipped; that needs the exposure turned down at capture.
//!
//! Pure and cheap: every third pixel of every third row, a byte histogram per channel.

use crate::develop::to_linear;

/// The most black we will remove, linear (8%: the measured lift was 4-6%).
pub const MAX_BLACK: f32 = 0.08;
/// How much of the darkest tail is trusted as "black": the 0.5th percentile.
pub const BLACK_PERCENTILE: f32 = 0.005;
/// Pixels that count as neutral for white balance: not near-black or near-white, and this unsaturated.
pub const NEUTRAL_SAT: f32 = 0.30;
/// At least this share of the frame must be neutral, or we leave the white balance alone.
pub const MIN_NEUTRAL: f32 = 0.04;
/// Gains stay within this range of 1, and are applied this far (the camera's own AWB is partly right).
pub const GAIN_CLAMP: (f32, f32) = (0.70, 1.40);
pub const WB_STRENGTH: f32 = 0.6;
/// The median linear luminance to aim for (about sRGB 124), the bias limits in stops, and how far to go.
pub const TARGET_MEDIAN: f32 = 0.20;
pub const EV_LIMITS: (f32, f32) = (-1.0, 0.6);
pub const EV_STRENGTH: f32 = 0.7;
/// The clipped share of the frame at which the exposure bias stops darkening.
pub const CLIP_NO_DARKEN: f32 = 0.15;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    /// Linear black to subtract and stretch back (`Params::flare`), the same for all channels.
    pub black: f32,
    /// Linear per-channel gains, white balance only (no exposure).
    pub gains: [f32; 3],
    /// Exposure bias in stops.
    pub ev: f32,
}

impl Tone {
    pub fn none() -> Tone { Tone { black: 0.0, gains: [1.0; 3], ev: 0.0 } }

    /// The gains with the exposure bias folded in, ready to multiply into `Params::gains`.
    pub fn total_gains(&self) -> [f32; 3] {
        let e = 2f32.powf(self.ev);
        self.gains.map(|g| g * e)
    }
}

fn lum(r: f32, g: f32, b: f32) -> f32 { 0.2126 * r + 0.7152 * g + 0.0722 * b }

/// Measure an RGBA8 frame (sRGB bytes).
pub fn measure(rgba: &[u8], w: u32, h: u32) -> Tone {
    let (w, h) = (w as usize, h as usize);
    if w < 8 || h < 8 || rgba.len() < w * h * 4 { return Tone::none(); }
    // luminance histogram (by sRGB byte of the luma) for the black point and the median
    let mut ybins = [0u32; 256];
    let mut samples: Vec<[f32; 3]> = Vec::with_capacity((w / 3 + 1) * (h / 3 + 1));
    for y in (0..h).step_by(3) {
        for x in (0..w).step_by(3) {
            let i = (y * w + x) * 4;
            let l = [to_linear(rgba[i]), to_linear(rgba[i + 1]), to_linear(rgba[i + 2])];
            let yl = lum(l[0], l[1], l[2]);
            ybins[(yl.clamp(0.0, 1.0) * 255.0) as usize] += 1;
            samples.push(l);
        }
    }
    let n = samples.len() as f32;
    let percentile = |p: f32| -> f32 {
        let want = (n * p) as u32;
        let mut acc = 0;
        for (b, c) in ybins.iter().enumerate() {
            acc += c;
            if acc > want { return b as f32 / 255.0; }
        }
        1.0
    };
    // black point
    let black = (percentile(BLACK_PERCENTILE) * 0.9).clamp(0.0, MAX_BLACK);
    let k = 1.0 / (1.0 - black);
    let un = |v: f32| ((v - black) * k).max(0.0);

    // white balance from near-neutral surfaces
    let (mut sum, mut cnt) = ([0.0f32; 3], 0u32);
    for s in &samples {
        let c = [un(s[0]), un(s[1]), un(s[2])];
        let mx = c[0].max(c[1]).max(c[2]);
        let mn = c[0].min(c[1]).min(c[2]);
        let y = lum(c[0], c[1], c[2]);
        if mx < 0.97 && y > 0.04 && y < 0.85 && (mx - mn) / mx.max(1e-6) < NEUTRAL_SAT {
            for i in 0..3 { sum[i] += c[i]; }
            cnt += 1;
        }
    }
    let mut gains = [1.0f32; 3];
    if cnt as f32 >= n * MIN_NEUTRAL {
        let m = sum.map(|v| (v / cnt as f32).max(1e-4));
        let raw = [m[1] / m[0], 1.0, m[1] / m[2]];
        gains = raw.map(|g| 1.0 + (g.clamp(GAIN_CLAMP.0, GAIN_CLAMP.1) - 1.0) * WB_STRENGTH);
    }

    // exposure bias toward the target median, with the black and the gains applied
    let mut ys: Vec<f32> = samples.iter().map(|s| lum(un(s[0]) * gains[0], un(s[1]) * gains[1], un(s[2]) * gains[2]).min(1.0)).collect();
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = ys[ys.len() / 2].max(0.005);
    let mut ev = (TARGET_MEDIAN / median).log2().clamp(EV_LIMITS.0, EV_LIMITS.1) * EV_STRENGTH;
    let clipped = samples.iter().filter(|s| s[0].max(s[1]).max(s[2]) >= 0.98).count() as f32 / n;
    // never brighten a frame that is already clipping
    if clipped > 0.10 && ev > 0.0 { ev = 0.0; }
    // and darken less the more of it is clipped: what the sensor frame already burnt cannot come back,
    // so a bias just turns the white sky grey. By CLIP_NO_DARKEN of the frame clipped, no darkening at all.
    if ev < 0.0 {
        ev *= 1.0 - (clipped / CLIP_NO_DARKEN).min(1.0);
    }
    Tone { black, gains, ev }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::{render, Params};

    /// A scene: a ramp of grey from `lo` to `hi` (linear), tinted, with the black lifted by `lift` (linear).
    fn scene(w: u32, h: u32, lo: f32, hi: f32, tint: [f32; 3], lift: f32) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let t = (x as f32 / w as f32) * 0.8 + (y as f32 / h as f32) * 0.2;
                let l = lo + (hi - lo) * t;
                for c in 0..3 { v.push(crate::develop::to_srgb((l * tint[c] + lift).min(1.0))); }
                v.push(255);
            }
        }
        v
    }

    fn stats(rgba: &[u8]) -> (f32, f32, f32) {
        let mut y: Vec<f32> = rgba.chunks(4).map(|p| lum(to_linear(p[0]), to_linear(p[1]), to_linear(p[2]))).collect();
        y.sort_by(|a, b| a.partial_cmp(b).unwrap());
        (y[y.len() / 100], y[y.len() / 2], y[y.len() * 99 / 100])
    }

    fn develop(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
        let t = measure(rgba, w, h);
        let mut p = Params::identity(w, h);
        p.flare = [t.black; 3];
        p.gains = t.total_gains();
        render(rgba, w, h, &p)
    }

    /// A veiled, bright, cyan photo comes out with real blacks, a lower median and a neutral cast; a clean one is left nearly alone.
    #[test]
    fn a_lifted_bright_cyan_frame_is_brought_back() {
        let (w, h) = (96, 64);
        let bad = scene(w, h, 0.0, 0.9, [0.85, 1.0, 1.0], 0.05);
        let (p1_bad, med_bad, _) = stats(&bad);
        let t = measure(&bad, w, h);
        assert!(t.black > 0.03 && t.black <= MAX_BLACK, "found the lift: {t:?}");
        assert!(t.gains[0] > 1.05 && t.gains[2] < 1.05, "red up for a cyan cast: {t:?}");
        assert!(t.ev < 0.0, "a bright frame is brought down: {t:?}");
        let out = develop(&bad, w, h);
        let (p1, med, _) = stats(&out);
        assert!(p1 < p1_bad * 0.5, "blacks are black again: {p1_bad} -> {p1}");
        assert!(med < med_bad, "median down: {med_bad} -> {med}");
        let mean = |c: usize| out.chunks(4).map(|p| to_linear(p[c])).sum::<f32>() / (w * h) as f32;
        assert!((mean(0) / mean(1) - 1.0).abs() < (0.85f32 - 1.0).abs() * 0.8, "the red/green balance moved toward neutral: {}", mean(0) / mean(1));

        let clean = scene(w, h, 0.0, 0.5, [1.0; 3], 0.0);
        let t = measure(&clean, w, h);
        assert!(t.black < 0.02 && t.ev.abs() < 0.3 && t.gains.iter().all(|g| (g - 1.0).abs() < 0.03), "a clean frame is left nearly alone: {t:?}");
    }

    /// A blue sky filling the frame is not neutral: no white balance is applied to it; a clipping frame is never brightened.
    #[test]
    fn a_blue_sky_is_not_greyed_and_a_clipping_frame_is_not_brightened() {
        let (w, h) = (64, 48);
        let sky = scene(w, h, 0.25, 0.4, [0.3, 0.6, 1.0], 0.0);
        let t = measure(&sky, w, h);
        assert_eq!(t.gains, [1.0; 3], "no neutral surface to judge by: {t:?}");
        let dark_clip: Vec<u8> = (0..w * h).flat_map(|i| if i % 5 == 0 { [255u8, 255, 255, 255] } else { [10, 10, 10, 255] }).collect();
        let t = measure(&dark_clip, w, h);
        assert!(t.ev <= 0.0, "a frame with a clipped fifth is not brightened: {t:?}");
    }

    /// Degenerate input is a no-op, not a panic.
    #[test]
    fn tiny_or_torn_frames_are_left_alone() {
        assert_eq!(measure(&[], 0, 0), Tone::none());
        assert_eq!(measure(&[0; 16], 100, 100), Tone::none(), "a torn frame");
        assert_eq!(measure(&[0; 4 * 4 * 4], 4, 4), Tone::none(), "too small to judge");
    }

    /// The bias never leaves its limits, however dark or bright.
    #[test]
    fn the_exposure_bias_stays_within_its_limits() {
        let (w, h) = (64, 48);
        let dark = scene(w, h, 0.0, 0.01, [1.0; 3], 0.0);
        let t = measure(&dark, w, h);
        assert!(t.ev > 0.0 && t.ev <= EV_LIMITS.1 * EV_STRENGTH + 1e-4, "{t:?}");
        let bright = scene(w, h, 0.7, 0.99, [1.0; 3], 0.0);
        let t = measure(&bright, w, h);
        assert!(t.ev < 0.0 && t.ev >= EV_LIMITS.0 * EV_STRENGTH - 1e-4, "{t:?}");
    }

    /// A bright frame is darkened, but not once a good part of it is clipped: that only greys the burnt-out sky.
    #[test]
    fn darkening_fades_out_as_the_frame_clips() {
        let (w, h) = (64, 48);
        let bright = scene(w, h, 0.5, 0.9, [1.0; 3], 0.0);
        let free = measure(&bright, w, h);
        assert!(free.ev < -0.3, "unclipped and bright: darkened: {free:?}");
        // the same frame with a third of it blown to white
        let mut burnt = bright.clone();
        for (i, p) in burnt.chunks_mut(4).enumerate() {
            if i % 3 == 0 { p[0] = 255; p[1] = 255; p[2] = 255; }
        }
        let t = measure(&burnt, w, h);
        assert!(t.ev > -0.05, "a third clipped: left alone: {t:?}");
        let mut some = bright.clone();
        for (i, p) in some.chunks_mut(4).enumerate() {
            if i % 14 == 0 { p[0] = 255; p[1] = 255; p[2] = 255; }
        }
        let t = measure(&some, w, h);
        assert!(t.ev < -0.01 && t.ev > free.ev, "a little clipped: darkened less than clean: {} vs {}", t.ev, free.ev);
    }
}
