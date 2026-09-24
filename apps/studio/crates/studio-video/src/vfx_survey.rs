//! vfx_survey — the setup pass of a touch-up: measure every shot of a locked
//! cut, and write down what each one needs, cheapest technique first.
//!
//! `docs/film/TOUCHUP_VFX.md` §1.4 asks for a ticket per shot that needs work
//! -- shot, range, problem, technique, status -- and §2 for the cheapest
//! technique that holds up at the release resolution. This module is that
//! pass, split the way everything here is split:
//!
//! * [`measure`] decodes a shot small and grey (one ffmpeg call) and reads
//!   two signals from it: camera JITTER (the measured path minus its own
//!   one-second average -- the part a person sees as shake, not the pan they
//!   meant) and the frame-to-frame LUMA wobble (flicker).
//! * [`triage`] is PURE: numbers in, [`Suggestion`]s out, every threshold a
//!   named field of [`Thresholds`]. It prices each fix in what it costs the
//!   picture, because the film's delivery forbids reframing: stabilising
//!   zooms in, so a bump near the clip's head or tail is "cut around" first,
//!   a short bump in a steady shot is `Dejerk` (the jump only), and only
//!   shake throughout is `Smooth`; any fix costing more than
//!   [`Thresholds::max_zoom`] of the frame says so.
//!
//! Flicker has no `touchup::Fix` yet (the doc lists deflicker as PLANNED):
//! it is reported with the technique named and `fix: None`, never dressed up
//! as something Studio will render.

use std::path::Path;

use crate::stabilize::{self, Budget, Flavor};
use crate::touchup::Fix;

/// What a shot measured.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotStats {
    pub fps: f64,
    /// Frame width the jitter is measured in, pixels.
    pub width: u32,
    /// Per frame: distance of the camera path from its one-second average.
    pub jitter: Vec<f32>,
    /// Per frame: mean luma, 0..255.
    pub luma: Vec<f32>,
    /// Source seconds available beyond the clip's trim, each side.
    pub handle_before: f64,
    pub handle_after: f64,
}

/// Every number triage decides by.
#[derive(Debug, Clone, PartialEq)]
pub struct Thresholds {
    /// Jitter above this share of the frame width is shake (0.6% ≈ 1 px at 160).
    pub shake: f32,
    /// Shake spans shorter than this are ignored (a single bad measurement).
    pub min_span: f64,
    /// A shake span inside this many seconds of the clip's head or tail can be
    /// cut around instead of fixed.
    pub cut_around: f64,
    /// Shake over more than this share of the shot is "throughout".
    pub throughout: f64,
    /// Luma wobble (std of the high-passed mean) above this is flicker.
    pub flicker: f32,
    /// Handles below this many seconds each side are too short (12 frames at 24).
    pub min_handle: f64,
    /// Largest zoom a fix may cost before it is called reframing.
    pub max_zoom: f32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds { shake: 0.006, min_span: 0.2, cut_around: 1.0, throughout: 0.5, flicker: 1.5,
                     min_handle: 0.5, max_zoom: 1.02 }
    }
}

/// A problem found, with where it is in the CLIP (seconds from its start).
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    Shake { from: f64, to: f64, peak: f32 },
    Flicker { wobble: f32 },
    ShortHandles { before: f64, after: f64 },
}

/// What to do about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub problem: Problem,
    /// The technique, in the doc's words, and why this one.
    pub technique: String,
    /// The fix a touch-up ticket would carry; `None` when Studio has none
    /// (flicker) or the answer is editorial (cut around).
    pub fix: Option<Fix>,
    /// The zoom the fix costs (1.0 = none).
    pub zoom: f32,
    /// Over the reframing budget: say so.
    pub over_budget: bool,
}

impl Suggestion {
    pub fn line(&self) -> String {
        let what = match &self.problem {
            Problem::Shake { from, to, peak } => format!("shake {from:.1}–{to:.1} s (peak {:.1}% of width)", peak * 100.0),
            Problem::Flicker { wobble } => format!("flicker (luma wobble {wobble:.1})"),
            Problem::ShortHandles { before, after } => format!("short handles ({before:.2} s / {after:.2} s)"),
        };
        let cost = if self.zoom > 1.0005 { format!(", costs {:.1}% zoom{}", (self.zoom - 1.0) * 100.0,
            if self.over_budget { " -- OVER the no-reframing budget" } else { "" }) } else { String::new() };
        format!("{what} → {}{cost}", self.technique)
    }
}

/// Spans (start frame, end frame exclusive) where `v[i] > t`, joined across
/// gaps of up to `join` frames.
fn spans(v: &[f32], t: f32, join: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for (i, x) in v.iter().enumerate() {
        if *x <= t { continue; }
        match out.last_mut() {
            Some(last) if i <= last.1 + join => last.1 = i + 1,
            _ => out.push((i, i + 1)),
        }
    }
    out
}

/// PURE: the suggestions for one shot of `clip_len` seconds.
pub fn triage(s: &ShotStats, clip_len: f64, th: &Thresholds) -> Vec<Suggestion> {
    let mut out = Vec::new();
    let fps = s.fps.max(1.0);
    let w = s.width.max(1) as f32;
    let shake_px = th.shake * w;
    let found: Vec<(f64, f64, f32)> = spans(&s.jitter, shake_px, (fps * 0.15) as usize).into_iter()
        .map(|(a, b)| (a as f64 / fps, b as f64 / fps, s.jitter[a..b].iter().cloned().fold(0.0, f32::max) / w))
        .filter(|(a, b, _)| b - a >= th.min_span).collect();
    let shaky: f64 = found.iter().map(|(a, b, _)| b - a).sum();
    for &(from, to, peak) in &found {
        // what a stabiliser would have to zoom to hide this much motion: twice
        // the excursion (both edges), as a share of the frame
        let zoom = 1.0 + 2.0 * peak;
        let problem = Problem::Shake { from, to, peak };
        if from < th.cut_around || clip_len - to < th.cut_around {
            out.push(Suggestion { problem, technique: format!("cut around: the bump is within {:.0} s of the clip's {}; trim it off, no picture lost",
                th.cut_around, if from < th.cut_around { "head" } else { "tail" }), fix: None, zoom: 1.0, over_budget: false });
        } else if shaky / clip_len.max(1e-9) > th.throughout {
            out.push(Suggestion { problem, technique: "stabilize, smooth: shake through most of the shot".into(),
                fix: Some(Fix::Stabilize { flavor: Flavor::Smooth { sigma: (fps * 0.5) as f32 }, budget: Budget { max_zoom: zoom.max(th.max_zoom) } }),
                zoom, over_budget: zoom > th.max_zoom });
        } else {
            out.push(Suggestion { problem, technique: "stabilize, dejerk: a bump in a steady shot -- remove the jump only".into(),
                fix: Some(Fix::Stabilize { flavor: Flavor::Dejerk { threshold: shake_px }, budget: Budget { max_zoom: zoom.max(th.max_zoom) } }),
                zoom, over_budget: zoom > th.max_zoom });
        }
    }
    // flicker: the mean luma minus its 5-frame average, as a standard deviation
    if s.luma.len() >= 8 {
        let hp: Vec<f32> = (0..s.luma.len()).map(|i| {
            let (a, b) = (i.saturating_sub(2), (i + 3).min(s.luma.len()));
            s.luma[i] - s.luma[a..b].iter().sum::<f32>() / (b - a) as f32
        }).collect();
        let wobble = (hp.iter().map(|x| x * x).sum::<f32>() / hp.len() as f32).sqrt();
        if wobble > th.flicker {
            out.push(Suggestion { problem: Problem::Flicker { wobble },
                technique: "deflicker (ffmpeg deflicker as a v0) -- PLANNED in Studio, run by hand for now".into(),
                fix: None, zoom: 1.0, over_budget: false });
        }
    }
    if !out.is_empty() && (s.handle_before < th.min_handle || s.handle_after < th.min_handle) {
        out.push(Suggestion { problem: Problem::ShortHandles { before: s.handle_before, after: s.handle_after },
            technique: "render what exists; a later re-trim past it re-opens the ticket".into(), fix: None, zoom: 1.0, over_budget: false });
    }
    out
}

/// Measure a shot: `src_in`/`len` seconds of `path`, whose whole length is
/// `source_len` seconds (for the handles). Decoded at 160 px wide, 12 fps.
pub fn measure(path: &Path, src_in: f64, len: f64, source_len: f64) -> Result<ShotStats, String> {
    let (w, h, fps) = (160u32, 90u32, 12.0f64);
    let frames = crate::framepipe::Pipe { input: path.to_path_buf(), range: Some((src_in, len)),
        size: crate::framepipe::Size { w, h }, fps }.gray()?;
    if frames.len() < 2 { return Err(format!("{}: fewer than two frames in range", path.display())); }
    let poses = stabilize::measure(&frames, 16);
    let win = fps as usize / 2; // one second, centred
    let jitter = (0..poses.len()).map(|i| {
        let (a, b) = (i.saturating_sub(win), (i + win + 1).min(poses.len()));
        let n = (b - a) as f32;
        let (mx, my) = (poses[a..b].iter().map(|p| p.x).sum::<f32>() / n, poses[a..b].iter().map(|p| p.y).sum::<f32>() / n);
        ((poses[i].x - mx).powi(2) + (poses[i].y - my).powi(2)).sqrt()
    }).collect();
    let luma = frames.iter().map(|f| f.px.iter().map(|&v| v as f32).sum::<f32>() / f.px.len().max(1) as f32).collect();
    Ok(ShotStats { fps, width: w, jitter, luma, handle_before: src_in.max(0.0),
                   handle_after: (source_len - src_in - len).max(0.0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(jitter: Vec<f32>, luma: Vec<f32>) -> ShotStats {
        ShotStats { fps: 10.0, width: 100, jitter, luma, handle_before: 1.0, handle_after: 1.0 }
    }

    #[test]
    fn a_bump_mid_shot_is_dejerked_near_an_edge_is_cut_around_and_throughout_is_smoothed() {
        let th = Thresholds::default(); // shake: 0.6 px at width 100
        let steady = vec![0.1f32; 100]; // 10 s
        let mut mid = steady.clone();
        for j in &mut mid[50..58] { *j = 1.5; }
        let s = triage(&stats(mid, vec![]), 10.0, &th);
        assert_eq!(s.len(), 1, "{s:?}");
        assert!(matches!(s[0].fix, Some(Fix::Stabilize { flavor: Flavor::Dejerk { .. }, .. })), "{:?}", s[0]);
        assert!(matches!(s[0].problem, Problem::Shake { from, to, .. } if (from - 5.0).abs() < 1e-9 && (to - 5.8).abs() < 1e-9));
        let mut head = steady.clone();
        for j in &mut head[3..8] { *j = 1.5; }
        let s = triage(&stats(head, vec![]), 10.0, &th);
        assert!(s[0].technique.starts_with("cut around") && s[0].fix.is_none(), "{:?}", s[0]);
        let s = triage(&stats(vec![1.2; 100], vec![]), 10.0, &th);
        assert!(matches!(s[0].fix, Some(Fix::Stabilize { flavor: Flavor::Smooth { .. }, .. })), "{:?}", s[0]);
        assert!(triage(&stats(steady, vec![]), 10.0, &th).is_empty(), "a steady shot needs nothing");
    }

    #[test]
    fn a_fix_that_costs_more_than_the_reframing_budget_says_so() {
        let th = Thresholds::default();
        let mut j = vec![0.1f32; 100];
        for x in &mut j[40..50] { *x = 3.0; } // 3% of the width
        let s = triage(&stats(j, vec![]), 10.0, &th);
        assert!(s[0].over_budget && (s[0].zoom - 1.06).abs() < 1e-4, "{:?}", s[0]);
        assert!(s[0].line().contains("OVER the no-reframing budget"), "{}", s[0].line());
    }

    #[test]
    fn flicker_is_reported_but_never_offered_as_a_fix_studio_does_not_have() {
        let th = Thresholds::default();
        let lamp: Vec<f32> = (0..60).map(|i| 120.0 + if i % 2 == 0 { 4.0 } else { -4.0 }).collect();
        let s = triage(&stats(vec![0.0; 60], lamp), 6.0, &th);
        assert_eq!(s.len(), 1);
        assert!(matches!(s[0].problem, Problem::Flicker { .. }) && s[0].fix.is_none() && s[0].technique.contains("PLANNED"));
        // a slow fade is not flicker
        let fade: Vec<f32> = (0..60).map(|i| 200.0 - i as f32 * 3.0).collect();
        assert!(triage(&stats(vec![0.0; 60], fade), 6.0, &th).is_empty());
    }

    #[test]
    fn short_handles_are_noted_only_on_a_shot_that_needs_work() {
        let th = Thresholds::default();
        let mut s = stats(vec![0.1; 100], vec![]);
        s.handle_before = 0.1;
        assert!(triage(&s, 10.0, &th).is_empty(), "nothing to fix, nothing to render: handles do not matter");
        for j in &mut s.jitter[50..58] { *j = 1.5; }
        let t = triage(&s, 10.0, &th);
        assert!(t.iter().any(|x| matches!(x.problem, Problem::ShortHandles { .. })), "{t:?}");
    }

    /// The fixture's ground truth: "the wide shakes at 7.5–8.5 s"; the close
    /// cameras do not shake.
    #[test]
    fn the_fixtures_wide_shakes_where_its_readme_says_and_the_closes_do_not() {
        if !crate::testkit::ffmpeg_or_skip("the_fixtures_wide_shakes_where_its_readme_says_and_the_closes_do_not") { return; }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
        let th = Thresholds::default();
        let wide = measure(&dir.join("A001_wide.mp4"), 0.0, 14.0, 14.0).expect("measure the wide");
        let s = triage(&wide, 14.0, &th);
        let shake: Vec<_> = s.iter().filter_map(|x| match x.problem { Problem::Shake { from, to, .. } => Some((from, to)), _ => None }).collect();
        assert_eq!(shake.len(), 1, "{s:?}");
        let (from, to) = shake[0];
        assert!(from > 7.0 && from < 8.0 && to > 8.0 && to < 9.2, "{from:.2}–{to:.2}");
        for close in ["A002_close_left.mp4", "A003_close_right.mp4"] {
            let st = measure(&dir.join(close), 0.0, 13.0, 13.1).expect("measure");
            assert!(!triage(&st, 13.0, &th).iter().any(|x| matches!(x.problem, Problem::Shake { .. })), "{close} does not shake");
        }
    }
}
