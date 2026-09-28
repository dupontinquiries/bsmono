//! cover — correcting the plastic (or scratched glass) over the camera.
//!
//! Most phones' cameras sit behind a window: the phone's own cover glass, a
//! case's clear plastic, a screen protector folded over the lens. It does
//! two things to every photo:
//!
//! * **tint** — tinted or yellowed plastic multiplies each channel by a
//!   fixed amount: per-channel GAINS undo it;
//! * **haze** — scratches, scuffs and dust scatter light into every pixel
//!   (veiling glare): the blacks lift and contrast drops. A per-channel
//!   FLARE offset is subtracted and the rest stretched back.
//!
//! Both are properties of the cover, not the scene, so they are measured
//! ONCE from a calibration shot and applied to every photo, in the same
//! develop pass as everything else. The calibration shot: a grey or white
//! card filling the middle of the frame (tint is measured there), with
//! something truly dark somewhere in the frame, a shadow or a black object
//! (haze is measured from the darkest pixels — what "black" became). A
//! frame with no dark area gives no haze estimate, and the result says so.
//!
//! Honest limits: a real flare is not uniform (it is worse near bright
//! sources), and a gain per channel is a diagonal colour correction — it
//! fixes a tint, not a cover that shifts hues differently. It is the right
//! first-order model for "the plastic is yellow and scuffed", and it is
//! measurable from one shot.

use crate::develop::to_linear;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverProfile {
    pub gains: [f32; 3],
    pub flare: [f32; 3],
}

impl CoverProfile {
    pub fn none() -> CoverProfile { CoverProfile { gains: [1.0; 3], flare: [0.0; 3] } }

    pub fn render(&self) -> String {
        format!("# snapshot-cam cover correction (measured from a grey card)\ngains = {:.4}, {:.4}, {:.4}\nflare = {:.4}, {:.4}, {:.4}\n",
                self.gains[0], self.gains[1], self.gains[2], self.flare[0], self.flare[1], self.flare[2])
    }

    pub fn parse(text: &str) -> Option<CoverProfile> {
        let ini = config_kit::Ini::parse(text);
        let three = |k: &str| -> Option<[f32; 3]> {
            let v: Vec<f32> = ini.top(k)?.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
            (v.len() == 3 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2]])
        };
        let gains = three("gains")?;
        let flare = three("flare")?;
        (gains.iter().all(|g| (0.2..=5.0).contains(g)) && flare.iter().all(|f| (0.0..=0.5).contains(f)))
            .then_some(CoverProfile { gains, flare })
    }
}

/// What a calibration found.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    pub profile: CoverProfile,
    /// Plain-language findings, for the note line.
    pub notes: Vec<String>,
}

/// The fraction of the darkest pixels read as "what black became".
const DARK_PERCENTILE: f32 = 0.005;
/// A dark reference brighter than this is not dark: no haze estimate.
const NO_DARK_ABOVE: f32 = 0.12;

/// Measure a cover from a calibration frame (RGBA, `w × h`).
pub fn calibrate(rgba: &[u8], w: u32, h: u32) -> Result<Calibration, String> {
    let (w, h) = (w as usize, h as usize);
    if w < 16 || h < 16 || rgba.len() < w * h * 4 { return Err("the frame is too small to measure".into()); }
    let mut notes = Vec::new();
    // Haze: the darkest percentile per channel.
    let mut flare = [0.0f32; 3];
    let mut lum: Vec<f32> = rgba.chunks_exact(4).take(w * h)
        .map(|p| to_linear(((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8)).collect();
    lum.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let k = ((lum.len() as f32 * DARK_PERCENTILE) as usize).max(1);
    let dark_luma = lum[k];
    if dark_luma > NO_DARK_ABOVE {
        notes.push("no dark area in the frame: haze not measured (include a shadow or something black)".into());
    } else {
        for (c, f) in flare.iter_mut().enumerate() {
            let mut v: Vec<f32> = rgba.chunks_exact(4).take(w * h).map(|p| to_linear(p[c])).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            *f = v[k].clamp(0.0, 0.3);
        }
        notes.push(format!("haze {:.1}% of full scale", (flare[0] + flare[1] + flare[2]) / 3.0 * 100.0));
    }
    // Tint: the middle fifth, after the haze is taken out.
    let (x0, x1, y0, y1) = (w * 2 / 5, w * 3 / 5, h * 2 / 5, h * 3 / 5);
    let mut sum = [0.0f64; 3];
    let mut n = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = &rgba[(y * w + x) * 4..];
            for c in 0..3 { sum[c] += ((to_linear(p[c]) - flare[c]) / (1.0 - flare[c])).max(0.0) as f64; }
            n += 1.0;
        }
    }
    let mean = sum.map(|s| (s / n) as f32);
    if mean.iter().any(|m| *m < 0.03) {
        return Err("the middle of the frame is too dark or too saturated to be a grey card".into());
    }
    if mean.iter().any(|m| *m > 0.97) {
        return Err("the grey card is clipped: expose it darker".into());
    }
    let gains = [mean[1] / mean[0], 1.0, mean[1] / mean[2]];
    if gains.iter().any(|g| !(0.3..=3.0).contains(g)) {
        return Err(format!("the middle of the frame is strongly coloured ({:.2}, 1, {:.2}): is it a grey card?", gains[0], gains[2]));
    }
    notes.push(format!("tint gains R×{:.2} B×{:.2}", gains[0], gains[2]));
    Ok(Calibration { profile: CoverProfile { gains, flare }, notes })
}

pub fn path() -> std::path::PathBuf { config_kit::config_dir(crate::store::APP).join("cover.conf") }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::{render, to_srgb, Params};

    /// A scene: grey card in the middle, a black patch in a corner, a ramp
    /// elsewhere.
    fn scene(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h { for x in 0..w {
            let px = if x > w * 3 / 10 && x < w * 7 / 10 && y > h * 3 / 10 && y < h * 7 / 10 { [128, 128, 128] }
                     else if x < w / 8 && y < h / 8 { [0, 0, 0] }
                     else { [(x * 255 / w) as u8, (y * 255 / h) as u8, 90] };
            v.extend_from_slice(&[px[0], px[1], px[2], 255]);
        } }
        v
    }

    /// Through a yellowed, scuffed cover: tint then veiling glare, in linear.
    fn through_cover(img: &[u8], tint: [f32; 3], haze: [f32; 3]) -> Vec<u8> {
        img.chunks_exact(4).flat_map(|p| {
            let c = |i: usize| to_srgb(to_linear(p[i]) * tint[i] * (1.0 - haze[i]) + haze[i]);
            [c(0), c(1), c(2), 255]
        }).collect()
    }

    #[test]
    fn a_calibration_shot_measures_the_cover_and_undoing_it_restores_the_scene() {
        let (w, h) = (80, 60);
        let clean = scene(w, h);
        let covered = through_cover(&clean, [1.0, 0.92, 0.7], [0.04, 0.035, 0.03]);
        let cal = calibrate(&covered, w, h).expect("a usable calibration");
        let f = cal.profile.flare;
        assert!((f[0] - 0.04).abs() < 0.01 && (f[2] - 0.03).abs() < 0.01, "haze {f:?}");
        // Corrected, the grey card is grey again and black is black again.
        let fixed = render(&covered, w, h, &Params { gains: cal.profile.gains, flare: cal.profile.flare, ..Params::identity(w, h) });
        let at = |img: &[u8], x: u32, y: u32| { let i = ((y * w + x) * 4) as usize; [img[i] as i32, img[i + 1] as i32, img[i + 2] as i32] };
        let card = at(&fixed, w / 2, h / 2);
        assert!((card[0] - card[1]).abs() <= 3 && (card[2] - card[1]).abs() <= 3, "card {card:?}");
        let black = at(&fixed, 2, 2);
        assert!(black.iter().all(|v| *v <= 8), "black {black:?} (covered: {:?})", at(&covered, 2, 2));
        assert!(at(&covered, 2, 2)[0] > 40, "the test cover really did lift the blacks");
    }

    #[test]
    fn no_dark_area_means_no_haze_estimate_and_it_says_so() {
        let (w, h) = (40, 30);
        let flat: Vec<u8> = (0..w * h).flat_map(|_| [150u8, 140, 110, 255]).collect();
        let cal = calibrate(&flat, w, h).unwrap();
        assert_eq!(cal.profile.flare, [0.0; 3]);
        assert!(cal.notes[0].contains("haze not measured"));
        assert!(cal.profile.gains[2] > 1.2, "the tint is still measured");
    }

    #[test]
    fn a_frame_that_is_not_a_grey_card_is_refused() {
        let (w, h) = (40, 30);
        let red: Vec<u8> = (0..w * h).flat_map(|_| [220u8, 20, 20, 255]).collect();
        assert!(calibrate(&red, w, h).is_err());
        let white: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 255, 255, 255]).collect();
        assert!(calibrate(&white, w, h).unwrap_err().contains("clipped"));
    }

    #[test]
    fn the_profile_round_trips_and_nonsense_is_refused() {
        let p = CoverProfile { gains: [1.05, 1.0, 1.4], flare: [0.02, 0.018, 0.015] };
        let back = CoverProfile::parse(&p.render()).unwrap();
        assert!((back.gains[2] - 1.4).abs() < 1e-3 && (back.flare[0] - 0.02).abs() < 1e-3);
        assert!(CoverProfile::parse("gains = 1, 1\nflare = 0, 0, 0").is_none());
        assert!(CoverProfile::parse("gains = 1, 1, 90\nflare = 0, 0, 0").is_none());
    }
}
//! cover — correcting the plastic (or scratched glass) over the camera.
//!
//! Most phones' cameras sit behind a window: the phone's own cover glass, a
//! case's clear plastic, a screen protector folded over the lens. It does
//! two things to every photo:
//!
//! * **tint** — tinted or yellowed plastic multiplies each channel by a
//!   fixed amount: per-channel GAINS undo it;
//! * **haze** — scratches, scuffs and dust scatter light into every pixel
//!   (veiling glare): the blacks lift and contrast drops. A per-channel
//!   FLARE offset is subtracted and the rest stretched back.
//!
//! Both are properties of the cover, not the scene, so they are measured
//! ONCE from a calibration shot and applied to every photo, in the same
//! develop pass as everything else. The calibration shot: a grey or white
//! card filling the middle of the frame (tint is measured there), with
//! something truly dark somewhere in the frame, a shadow or a black object
//! (haze is measured from the darkest pixels — what "black" became). A
//! frame with no dark area gives no haze estimate, and the result says so.
//!
//! Honest limits: a real flare is not uniform (it is worse near bright
//! sources), and a gain per channel is a diagonal colour correction — it
//! fixes a tint, not a cover that shifts hues differently. It is the right
//! first-order model for "the plastic is yellow and scuffed", and it is
//! measurable from one shot.

use crate::develop::to_linear;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverProfile {
    pub gains: [f32; 3],
    pub flare: [f32; 3],
}

impl CoverProfile {
    pub fn none() -> CoverProfile { CoverProfile { gains: [1.0; 3], flare: [0.0; 3] } }

    pub fn render(&self) -> String {
        format!("# snapshot-cam cover correction (measured from a grey card)\ngains = {:.4}, {:.4}, {:.4}\nflare = {:.4}, {:.4}, {:.4}\n",
                self.gains[0], self.gains[1], self.gains[2], self.flare[0], self.flare[1], self.flare[2])
    }

    pub fn parse(text: &str) -> Option<CoverProfile> {
        let ini = config_kit::Ini::parse(text);
        let three = |k: &str| -> Option<[f32; 3]> {
            let v: Vec<f32> = ini.top(k)?.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
            (v.len() == 3 && v.iter().all(|x| x.is_finite())).then(|| [v[0], v[1], v[2]])
        };
        let gains = three("gains")?;
        let flare = three("flare")?;
        (gains.iter().all(|g| (0.2..=5.0).contains(g)) && flare.iter().all(|f| (0.0..=0.5).contains(f)))
            .then_some(CoverProfile { gains, flare })
    }
}

/// What a calibration found.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    pub profile: CoverProfile,
    /// Plain-language findings, for the note line.
    pub notes: Vec<String>,
}

/// The fraction of the darkest pixels read as "what black became".
const DARK_PERCENTILE: f32 = 0.005;
/// A dark reference brighter than this is not dark: no haze estimate.
const NO_DARK_ABOVE: f32 = 0.12;

/// Measure a cover from a calibration frame (RGBA, `w × h`).
pub fn calibrate(rgba: &[u8], w: u32, h: u32) -> Result<Calibration, String> {
    let (w, h) = (w as usize, h as usize);
    if w < 16 || h < 16 || rgba.len() < w * h * 4 { return Err("the frame is too small to measure".into()); }
    let mut notes = Vec::new();
    // Haze: the darkest percentile per channel.
    let mut flare = [0.0f32; 3];
    let mut lum: Vec<f32> = rgba.chunks_exact(4).take(w * h)
        .map(|p| to_linear(((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8)).collect();
    lum.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let k = ((lum.len() as f32 * DARK_PERCENTILE) as usize).max(1);
    let dark_luma = lum[k];
    if dark_luma > NO_DARK_ABOVE {
        notes.push("no dark area in the frame: haze not measured (include a shadow or something black)".into());
    } else {
        for (c, f) in flare.iter_mut().enumerate() {
            let mut v: Vec<f32> = rgba.chunks_exact(4).take(w * h).map(|p| to_linear(p[c])).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            *f = v[k].clamp(0.0, 0.3);
        }
        notes.push(format!("haze {:.1}% of full scale", (flare[0] + flare[1] + flare[2]) / 3.0 * 100.0));
    }
    // Tint: the middle fifth, after the haze is taken out.
    let (x0, x1, y0, y1) = (w * 2 / 5, w * 3 / 5, h * 2 / 5, h * 3 / 5);
    let mut sum = [0.0f64; 3];
    let mut n = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = &rgba[(y * w + x) * 4..];
            for c in 0..3 { sum[c] += ((to_linear(p[c]) - flare[c]) / (1.0 - flare[c])).max(0.0) as f64; }
            n += 1.0;
        }
    }
    let mean = sum.map(|s| (s / n) as f32);
    if mean.iter().any(|m| *m < 0.03) {
        return Err("the middle of the frame is too dark or too saturated to be a grey card".into());
    }
    if mean.iter().any(|m| *m > 0.97) {
        return Err("the grey card is clipped: expose it darker".into());
    }
    let gains = [mean[1] / mean[0], 1.0, mean[1] / mean[2]];
    if gains.iter().any(|g| !(0.3..=3.0).contains(g)) {
        return Err(format!("the middle of the frame is strongly coloured ({:.2}, 1, {:.2}): is it a grey card?", gains[0], gains[2]));
    }
    notes.push(format!("tint gains R×{:.2} B×{:.2}", gains[0], gains[2]));
    Ok(Calibration { profile: CoverProfile { gains, flare }, notes })
}

pub fn path() -> std::path::PathBuf { config_kit::config_dir(crate::store::APP).join("cover.conf") }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::{render, to_srgb, Params};

    /// A scene: grey card in the middle, a black patch in a corner, a ramp
    /// elsewhere.
    fn scene(w: u32, h: u32) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h { for x in 0..w {
            let px = if x > w * 3 / 10 && x < w * 7 / 10 && y > h * 3 / 10 && y < h * 7 / 10 { [128, 128, 128] }
                     else if x < w / 8 && y < h / 8 { [0, 0, 0] }
                     else { [(x * 255 / w) as u8, (y * 255 / h) as u8, 90] };
            v.extend_from_slice(&[px[0], px[1], px[2], 255]);
        } }
        v
    }

    /// Through a yellowed, scuffed cover: tint then veiling glare, in linear.
    fn through_cover(img: &[u8], tint: [f32; 3], haze: [f32; 3]) -> Vec<u8> {
        img.chunks_exact(4).flat_map(|p| {
            let c = |i: usize| to_srgb(to_linear(p[i]) * tint[i] * (1.0 - haze[i]) + haze[i]);
            [c(0), c(1), c(2), 255]
        }).collect()
    }

    #[test]
    fn a_calibration_shot_measures_the_cover_and_undoing_it_restores_the_scene() {
        let (w, h) = (80, 60);
        let clean = scene(w, h);
        let covered = through_cover(&clean, [1.0, 0.92, 0.7], [0.04, 0.035, 0.03]);
        let cal = calibrate(&covered, w, h).expect("a usable calibration");
        let f = cal.profile.flare;
        assert!((f[0] - 0.04).abs() < 0.01 && (f[2] - 0.03).abs() < 0.01, "haze {f:?}");
        // Corrected, the grey card is grey again and black is black again.
        let fixed = render(&covered, w, h, &Params { gains: cal.profile.gains, flare: cal.profile.flare, ..Params::identity(w, h) });
        let at = |img: &[u8], x: u32, y: u32| { let i = ((y * w + x) * 4) as usize; [img[i] as i32, img[i + 1] as i32, img[i + 2] as i32] };
        let card = at(&fixed, w / 2, h / 2);
        assert!((card[0] - card[1]).abs() <= 3 && (card[2] - card[1]).abs() <= 3, "card {card:?}");
        let black = at(&fixed, 2, 2);
        assert!(black.iter().all(|v| *v <= 8), "black {black:?} (covered: {:?})", at(&covered, 2, 2));
        assert!(at(&covered, 2, 2)[0] > 40, "the test cover really did lift the blacks");
    }

    #[test]
    fn no_dark_area_means_no_haze_estimate_and_it_says_so() {
        let (w, h) = (40, 30);
        let flat: Vec<u8> = (0..w * h).flat_map(|_| [150u8, 140, 110, 255]).collect();
        let cal = calibrate(&flat, w, h).unwrap();
        assert_eq!(cal.profile.flare, [0.0; 3]);
        assert!(cal.notes[0].contains("haze not measured"));
        assert!(cal.profile.gains[2] > 1.2, "the tint is still measured");
    }

    #[test]
    fn a_frame_that_is_not_a_grey_card_is_refused() {
        let (w, h) = (40, 30);
        let red: Vec<u8> = (0..w * h).flat_map(|_| [220u8, 20, 20, 255]).collect();
        assert!(calibrate(&red, w, h).is_err());
        let white: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 255, 255, 255]).collect();
        assert!(calibrate(&white, w, h).unwrap_err().contains("clipped"));
    }

    #[test]
    fn the_profile_round_trips_and_nonsense_is_refused() {
        let p = CoverProfile { gains: [1.05, 1.0, 1.4], flare: [0.02, 0.018, 0.015] };
        let back = CoverProfile::parse(&p.render()).unwrap();
        assert!((back.gains[2] - 1.4).abs() < 1e-3 && (back.flare[0] - 0.02).abs() < 1e-3);
        assert!(CoverProfile::parse("gains = 1, 1\nflare = 0, 0, 0").is_none());
        assert!(CoverProfile::parse("gains = 1, 1, 90\nflare = 0, 0, 0").is_none());
    }
}
