//! fusion -- what software alone can do across exposures, and a judge of which scene needs what.
//!
//! **The judge.** On a day's roll from this phone, 34 of 39 photos were HIGH CONTRAST (a burnt sky or
//! window taking 8-40% of the frame beside real shadows), 4 normal, 1 low light. The camera's own
//! exposure choice is the problem there, and software cannot undo clipping: the cure is capture-side
//! (expose for the highlights, then bracket), which needs exposure control the phone's libcamera path
//! does not hand us yet (HDR mode says so). `judge` names the scenario so the shell can say what a
//! photo needed, and so the capture-side work has a target.
//!
//! **What it can do now.** `fuse_brackets` makes virtual brackets from the one frame (gain steps in
//! linear light) and blends them with Mertens exposure fusion (`stack::fuse_exposures`): shadows and
//! subject detail come up, midtone highlights are compressed, nothing already clipped returns.
//! Tried on the real roll: the indoor shot gains clear detail, the burnt skies stay burnt.

use crate::develop::{render, Params};
use crate::stack::{fuse_exposures, Frames};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    Normal,
    /// A burnt sky/window beside real shadows: the exposure was wrong for the scene's range.
    HighContrast,
    /// Clipping with no real shadows: just too much light.
    Overexposed,
    LowLight,
    Bright,
}

impl Scenario {
    pub fn label(self) -> &'static str {
        match self {
            Scenario::Normal => "normal",
            Scenario::HighContrast => "high contrast (burnt sky or window)",
            Scenario::Overexposed => "overexposed",
            Scenario::LowLight => "low light",
            Scenario::Bright => "bright",
        }
    }
}

/// The numbers the judge reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stats {
    /// Share of pixels with a channel at 250+.
    pub clipped: f32,
    /// Median linear luminance after the black point is removed.
    pub median: f32,
    /// Share of pixels in real shadow (linear luminance < 0.02 after the black point).
    pub shadow: f32,
    /// Share of pixels that are neither shadow nor near-white.
    pub mid: f32,
}

pub fn stats(rgba: &[u8], w: u32, h: u32) -> Option<Stats> {
    let (w, h) = (w as usize, h as usize);
    if w < 8 || h < 8 || rgba.len() < w * h * 4 { return None; }
    let tone = crate::autotone::measure(rgba, w as u32, h as u32);
    let k = 1.0 / (1.0 - tone.black);
    let (mut ys, mut clipped) = (Vec::new(), 0usize);
    for y in (0..h).step_by(3) {
        for x in (0..w).step_by(3) {
            let p = &rgba[(y * w + x) * 4..];
            if p[0].max(p[1]).max(p[2]) >= 250 { clipped += 1; }
            let l = 0.2126 * crate::develop::to_linear(p[0]) + 0.7152 * crate::develop::to_linear(p[1]) + 0.0722 * crate::develop::to_linear(p[2]);
            ys.push(((l - tone.black) * k).clamp(0.0, 1.0));
        }
    }
    let n = ys.len() as f32;
    let shadow = ys.iter().filter(|y| **y < 0.02).count() as f32 / n;
    let mid = ys.iter().filter(|y| **y > 0.05 && **y < 0.8).count() as f32 / n;
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(Stats { clipped: clipped as f32 / n, median: ys[ys.len() / 2], shadow, mid })
}

/// Name the scene. The thresholds are what separated the day's roll sensibly.
pub fn judge(s: &Stats) -> Scenario {
    if s.clipped >= 0.08 {
        if s.shadow >= 0.02 || s.mid < 0.8 { Scenario::HighContrast } else { Scenario::Overexposed }
    } else if s.median < 0.06 {
        Scenario::LowLight
    } else if s.median > 0.45 {
        Scenario::Bright
    } else {
        Scenario::Normal
    }
}

/// Stops of the virtual brackets.
pub const EVS: [f32; 3] = [-1.0, 0.0, 1.0];

/// Blend virtual brackets of `rgba` (already developed: black point, white balance, exposure applied).
/// Same size back.
pub fn fuse_brackets(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    if w < 16 || h < 16 || rgba.len() < (w as usize) * (h as usize) * 4 { return rgba.to_vec(); }
    let frames: Vec<Vec<u8>> = EVS.iter().map(|ev| {
        let mut p = Params::identity(w, h);
        let e = 2f32.powf(*ev);
        p.gains = [e; 3];
        render(rgba, w, h, &p)
    }).collect();
    let fr = Frames { w, h, rgba: frames.iter().map(|v| &v[..]).collect() };
    let mut out = fuse_exposures(&fr);
    // What the camera already burnt stays white: the darker virtual bracket would otherwise turn a
    // clipped sky into a grey one. The mask is soft (from CLIP_KEEP_FROM up to white) so there is no seam.
    for (o, s) in out.chunks_mut(4).zip(rgba.chunks(4)) {
        let m = ((s[0].max(s[1]).max(s[2]) as f32 - CLIP_KEEP_FROM) / (255.0 - CLIP_KEEP_FROM)).clamp(0.0, 1.0);
        let m = m * m * (3.0 - 2.0 * m);
        for c in 0..3 { o[c] = (o[c] as f32 * (1.0 - m) + s[c] as f32 * m).round() as u8; }
    }
    out
}

/// Source pixels whose brightest channel is above this start to keep their own value (full at 255).
pub const CLIP_KEEP_FROM: f32 = 230.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// Left half: a dim textured scene. Right half: a bright textured scene. `clip` burns a share of the right.
    fn scene(w: u32, h: u32, dim: f32, bright: f32, clip_every: u32) -> Vec<u8> {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let tex = if (x / 2 + y / 2) % 2 == 0 { 1.0 } else { 0.7 };
                let l = if x < w / 2 { dim * tex } else { (bright * tex).min(1.0) };
                let b = if x >= w / 2 && clip_every > 0 && (x + y * 7) % clip_every == 0 { 255 } else { crate::develop::to_srgb(l) };
                v.extend_from_slice(&[b, b, b, 255]);
            }
        }
        v
    }

    fn detail(rgba: &[u8], w: u32, h: u32, x0: u32, x1: u32) -> f64 {
        let (w, h) = (w as usize, h as usize);
        let l = |x: usize, y: usize| rgba[(y * w + x) * 4] as f64;
        let mut s = 0.0;
        let mut n = 0.0;
        for y in 1..h - 1 { for x in (x0 as usize).max(1)..(x1 as usize).min(w - 1) {
            let lap = 4.0 * l(x, y) - l(x - 1, y) - l(x + 1, y) - l(x, y - 1) - l(x, y + 1);
            s += lap * lap; n += 1.0;
        } }
        s / n
    }

    #[test]
    fn the_judge_names_the_scenes_the_roll_showed() {
        let (w, h) = (96, 64);
        // a burnt window beside a real shadow
        let burnt = scene(w, h, 0.01, 0.9, 3);
        assert_eq!(judge(&stats(&burnt, w, h).unwrap()), Scenario::HighContrast);
        // a plain mid-tone room
        let normal = scene(w, h, 0.12, 0.25, 0);
        assert_eq!(judge(&stats(&normal, w, h).unwrap()), Scenario::Normal);
        // dark
        let dark = scene(w, h, 0.004, 0.01, 0);
        assert_eq!(judge(&stats(&dark, w, h).unwrap()), Scenario::LowLight);
        // all white-ish with burnt patches and no shadow
        let washed = scene(w, h, 0.5, 0.9, 2);
        let sc = judge(&stats(&washed, w, h).unwrap());
        assert!(matches!(sc, Scenario::Overexposed | Scenario::HighContrast), "{sc:?}");
        assert_eq!(stats(&[0; 16], 100, 100), None, "a torn frame is not judged");
        assert!(!Scenario::HighContrast.label().is_empty());
    }

    /// Virtual-bracket fusion lifts the detail in a dim region and keeps the bright region from running away.
    #[test]
    fn fusing_virtual_brackets_brings_up_the_shadows_without_burning_the_highlights() {
        let (w, h) = (96u32, 64u32);
        let src = scene(w, h, 0.02, 0.5, 0);
        let out = fuse_brackets(&src, w, h);
        assert_eq!(out.len(), src.len());
        let (before, after) = (detail(&src, w, h, 4, w / 2 - 4), detail(&out, w, h, 4, w / 2 - 4));
        assert!(after > before * 1.5, "shadow detail rose: {before} -> {after}");
        let mean = |v: &[u8], x0: u32, x1: u32| -> f64 {
            let mut s = 0.0; let mut n = 0.0;
            for y in 0..h { for x in x0..x1 { s += v[((y * w + x) * 4) as usize] as f64; n += 1.0; } }
            s / n
        };
        assert!(mean(&out, 4, w / 2 - 4) > mean(&src, 4, w / 2 - 4) + 6.0, "the dim half came up: {} -> {}", mean(&src, 4, w / 2 - 4), mean(&out, 4, w / 2 - 4));
        assert!(mean(&out, w / 2 + 4, w - 4) < mean(&src, w / 2 + 4, w - 4) + 25.0, "the bright half did not run away");
    }

    #[test]
    fn it_cannot_bring_back_what_was_burnt_and_tiny_frames_pass_through() {
        let (w, h) = (96u32, 64u32);
        let burnt: Vec<u8> = (0..w * h).flat_map(|_| [255u8, 255, 255, 255]).collect();
        let out = fuse_brackets(&burnt, w, h);
        let min = out.chunks(4).map(|p| p[0]).min().unwrap();
        assert!(min >= 250, "a fully burnt frame stays white (no grey sky, no detail invented): {min}");
        let tiny = vec![9u8; 4 * 4 * 4];
        assert_eq!(fuse_brackets(&tiny, 4, 4), tiny);
    }
}
