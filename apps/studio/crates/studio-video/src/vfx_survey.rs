//! vfx_survey — the setup pass of a touch-up: measure every shot of the cut,
//! say what is wrong with it, and propose the cheapest fix that holds.
//!
//! `docs/film/TOUCHUP_VFX.md` §1.4 asks for a ticket per shot that needs
//! work -- shot, source range, problem, technique, status -- and §2 orders
//! the techniques cheapest first. Finding those shots is watching the whole
//! cut with a notepad; this is the notepad filled in from measurements, for
//! a person to confirm. It PROPOSES fixes (`touchup::Fix` where Studio has
//! one), it never renders them.
//!
//! [`measure`] decodes a shot small and grey (160 px wide, 12 fps: enough to
//! see a camera move, cheap enough to run over a cut) and records the
//! camera path and the frame brightness. [`triage`] is pure: stats in,
//! [`Finding`]s out.

use std::path::Path;

use crate::framepipe::{Pipe, Size};

/// Measurement size and rate: a survey, not a render.
pub const W: u32 = 160;
pub const H: u32 = 90;
pub const FPS: f64 = 12.0;

/// What [`measure`] saw in one shot.
#[derive(Debug, Clone, PartialEq)]
pub struct ShotStats {
    /// Seconds into the source the measured range starts.
    pub from: f64,
    /// Camera jitter per frame: distance (px at [`W`]) of the measured
    /// position from its own one-second running mean -- a pan is not shake,
    /// the wobble on top of it is -- taken as the SMALLER of the two frame
    /// halves' (see [`measure`]).
    pub jitter: Vec<f32>,
    /// Mean brightness per frame, 0..255.
    pub luma: Vec<f32>,
    /// Source seconds available beyond the trim, each side, for handles.
    pub room_before: f64,
    pub room_after: f64,
}

/// The per-frame jitter of a camera path: each position's distance from the
/// mean of the positions within `half` frames of it.
pub fn jitter(path: &[(f32, f32)], half: usize) -> Vec<f32> {
    (0..path.len()).map(|i| {
        let (a, b) = (i.saturating_sub(half), (i + half + 1).min(path.len()));
        let n = (b - a) as f32;
        let (mx, my) = path[a..b].iter().fold((0.0, 0.0), |s, p| (s.0 + p.0, s.1 + p.1));
        ((path[i].0 - mx / n).powi(2) + (path[i].1 - my / n).powi(2)).sqrt()
    }).collect()
}

/// One half of a frame (left or right), as its own image.
fn half(g: &crate::slate_ocr::Gray, right: bool) -> crate::slate_ocr::Gray {
    let hw = g.w / 2;
    let x0 = if right { g.w - hw } else { 0 };
    let px = (0..g.h).flat_map(|y| g.px[y * g.w + x0..y * g.w + x0 + hw].iter().copied()).collect();
    crate::slate_ocr::Gray { w: hw, h: g.h, px }
}

/// Jitter of each half of the frame, measured on its own.
fn half_jitter(frames: &[crate::slate_ocr::Gray], right: bool) -> Vec<f32> {
    let halves: Vec<_> = frames.iter().map(|f| half(f, right)).collect();
    let track: Vec<(f32, f32)> = crate::stabilize::measure(&halves, 16).iter().map(|p| (p.x, p.y)).collect();
    jitter(&track, FPS as usize / 2)
}

/// Measure `len` seconds of `path` from `src_in`, in a source `source_len`
/// seconds long.
pub fn measure(path: &Path, src_in: f64, len: f64, source_len: f64) -> Result<ShotStats, String> {
    let frames = Pipe { input: path.to_path_buf(), range: Some((src_in, len)), size: Size { w: W, h: H }, fps: FPS }.gray()?;
    if frames.len() < 3 { return Err(format!("{}: too few frames to measure", path.display())); }
    // CAMERA shake moves the whole frame; a subject moves one part of it. A
    // single whole-frame estimate cannot tell them apart -- on the fixture a
    // close-up with a moving figure (1.0 px) out-wobbled the wide's real
    // shake (0.8 px). So each half is measured on its own and a frame counts
    // only as much as the LESS wobbly half: both halves must move.
    let (l, r) = (half_jitter(&frames, false), half_jitter(&frames, true));
    let coherent: Vec<f32> = l.iter().zip(&r).map(|(a, b)| a.min(*b)).collect();
    let luma = frames.iter().map(|f| f.px.iter().map(|v| *v as f32).sum::<f32>() / f.px.len().max(1) as f32).collect();
    Ok(ShotStats { from: src_in, jitter: coherent, luma,
                   room_before: src_in.max(0.0), room_after: (source_len - src_in - len).max(0.0) })
}

/// Thresholds, at the survey's measuring size.
#[derive(Debug, Clone)]
pub struct Options {
    /// Coherent jitter (px at [`W`]) that counts as shake. On the fixture a
    /// real shake measured 0.5-0.8 and still shots stayed at 0.0-0.1, with
    /// one isolated 0.4 frame from a moving figure -- which `shake_min`
    /// excludes.
    pub shake_px: f32,
    /// Shake must last this long (seconds) to be a problem.
    pub shake_min: f64,
    /// Under this, a problem is "cut around" first (the doc's rule: on
    /// screen for less than a second).
    pub cut_around: f64,
    /// Share of frame steps that reverse brightness by more than
    /// `flicker_step` levels: above this, the shot flickers.
    pub flicker_share: f32,
    pub flicker_step: f32,
    /// Handle each side a fix needs (0.5 s = 12 frames at 24).
    pub handle: f64,
}

impl Default for Options {
    fn default() -> Options {
        Options { shake_px: 0.3, shake_min: 0.25, cut_around: 1.0, flicker_share: 0.3, flicker_step: 1.5, handle: 0.5 }
    }
}

/// A problem in a shot, with where it is (source seconds).
#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    Shake { from: f64, to: f64, peak: f32 },
    Flicker { share: f32 },
    ShortHandles { before: f64, after: f64 },
}

/// A problem and the proposed technique -- the cheapest that holds, per
/// `TOUCHUP_VFX.md` §2 -- with the `Fix` Studio would render, when it has one.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub problem: Problem,
    pub technique: String,
    pub fix: Option<crate::touchup::Fix>,
    pub why: String,
}

/// Spans where `jitter` stays at or over `px` for at least `min_frames`.
pub fn shake_spans(jitter: &[f32], px: f32, min_frames: usize) -> Vec<(usize, usize, f32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < jitter.len() {
        if jitter[i] < px { i += 1; continue; }
        let start = i;
        let mut peak = 0.0f32;
        while i < jitter.len() && jitter[i] >= px { peak = peak.max(jitter[i]); i += 1; }
        if i - start >= min_frames { out.push((start, i, peak)); }
    }
    out
}

/// Share of consecutive brightness steps that reverse direction by more
/// than `step` levels: a flicker goes up, down, up; a light change or a cut
/// goes one way once.
pub fn flicker_share(luma: &[f32], step: f32) -> f32 {
    let d: Vec<f32> = luma.windows(2).map(|w| w[1] - w[0]).collect();
    if d.len() < 2 { return 0.0; }
    let reversals = d.windows(2).filter(|w| w[0].abs() > step && w[1].abs() > step && w[0].signum() != w[1].signum()).count();
    reversals as f32 / (d.len() - 1) as f32
}

/// Pure: what is wrong with a shot, and what to do about it.
pub fn triage(st: &ShotStats, o: &Options) -> Vec<Finding> {
    let mut out = Vec::new();
    let min_frames = (o.shake_min * FPS).ceil().max(1.0) as usize;
    for (a, b, peak) in shake_spans(&st.jitter, o.shake_px, min_frames) {
        let (from, to) = (st.from + a as f64 / FPS, st.from + b as f64 / FPS);
        let long = to - from >= o.cut_around;
        let stabilize = crate::touchup::Fix::Stabilize {
            flavor: crate::stabilize::Flavor::Smooth { sigma: 12.0 },
            budget: crate::stabilize::Budget { max_zoom: 1.05 } };
        out.push(Finding {
            problem: Problem::Shake { from, to, peak },
            technique: if long { "stabilize (smooth)".into() } else { "cut around it; else stabilize (smooth)".into() },
            fix: Some(stabilize),
            why: format!("the whole frame wobbles {:.1} px at survey size for {:.2} s{}", peak, to - from,
                if long { "; stabilizing zooms up to 5% -- a reframe the delivery rules want avoided, so check it at 1:1" }
                else { "; under a second, so cutting around it costs nothing" }),
        });
    }
    let share = flicker_share(&st.luma, o.flicker_step);
    if share >= o.flicker_share {
        out.push(Finding { problem: Problem::Flicker { share }, fix: None,
            technique: "deflicker (ffmpeg deflicker as a v0 -- PLANNED in Studio, not a Fix yet)".into(),
            why: format!("brightness reverses on {:.0}% of frames -- an LED practical at 24 fps", share * 100.0) });
    }
    // Handles matter only to a shot that will be RENDERED: on a clean shot
    // the warning is noise (the first run in the app flagged every shot of
    // the fixture, each placed as a whole camera file).
    if !out.is_empty() && (st.room_before < o.handle || st.room_after < o.handle) {
        out.push(Finding { problem: Problem::ShortHandles { before: st.room_before, after: st.room_after }, fix: None,
            technique: "render what exists; re-trim inside it".into(),
            why: format!("only {:.2} s before / {:.2} s after the trim -- under {:.1} s, a later extension would show unfixed frames",
                         st.room_before, st.room_after, o.handle) });
    }
    out
}

/// The shot list `TOUCHUP_VFX.md` §1.4 asks for, as Markdown: one row per
/// finding, status `open`. `shots` is `(shot name, source in, length, findings)`.
pub fn shot_list(shots: &[(String, f64, f64, Vec<Finding>)]) -> String {
    let mut t = String::from("# VFX shot list (drafted by the survey -- confirm each row)\n\n\
        | shot | source range | problem | technique | status |\n|---|---|---|---|---|\n");
    let mut clean = Vec::new();
    for (name, from, len, fs) in shots {
        if fs.is_empty() { clean.push(name.as_str()); continue; }
        for f in fs {
            let (range, problem) = match &f.problem {
                Problem::Shake { from, to, .. } => (format!("{from:.2}–{to:.2} s"), format!("shake: {}", f.why)),
                Problem::Flicker { .. } => (format!("{:.2}–{:.2} s", from, from + len), format!("flicker: {}", f.why)),
                Problem::ShortHandles { .. } => (format!("{:.2}–{:.2} s", from, from + len), format!("handles: {}", f.why)),
            };
            t.push_str(&format!("| {name} | {range} | {problem} | {} | open |\n", f.technique));
        }
    }
    if !clean.is_empty() { t.push_str(&format!("\nNothing found in: {}.\n", clean.join(", "))); }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(jitter: Vec<f32>, luma: Vec<f32>) -> ShotStats {
        ShotStats { from: 10.0, jitter, luma, room_before: 2.0, room_after: 2.0 }
    }

    #[test]
    fn a_short_shake_is_cut_around_first_and_a_long_one_stabilized_and_a_lone_frame_is_nothing() {
        let o = Options::default();
        let mut j = vec![0.0f32; 60];
        for x in &mut j[12..21] { *x = 0.7; } // 9 frames = 0.75 s
        j[40] = 0.4;                           // one frame: a moving figure
        let f = triage(&stats(j, vec![60.0; 60]), &o);
        assert_eq!(f.len(), 1, "{f:?}");
        let Problem::Shake { from, to, .. } = f[0].problem else { panic!() };
        assert!((from - 11.0).abs() < 1e-9 && (to - 11.75).abs() < 1e-9, "source time: {from}..{to}");
        assert!(f[0].technique.starts_with("cut around"), "{}", f[0].technique);
        assert!(matches!(f[0].fix, Some(crate::touchup::Fix::Stabilize { .. })), "the fallback is ready to render");
        let mut long = vec![0.0f32; 60];
        for x in &mut long[5..30] { *x = 0.9; }
        let f = triage(&stats(long, vec![60.0; 60]), &o);
        assert!(f[0].technique.starts_with("stabilize") && f[0].why.contains("1:1"), "{f:?}");
    }

    #[test]
    fn flicker_is_reversals_not_a_light_change_and_short_handles_are_said() {
        let o = Options::default();
        let flick: Vec<f32> = (0..48).map(|i| if i % 2 == 0 { 60.0 } else { 64.0 }).collect();
        let ramp: Vec<f32> = (0..48).map(|i| 40.0 + i as f32 * 2.0).collect(); // a light coming up
        let step: Vec<f32> = (0..48).map(|i| if i < 10 { 95.0 } else { 62.0 }).collect(); // the fixture's slate flash
        assert!(triage(&stats(vec![0.0; 48], flick), &o).iter().any(|f| matches!(f.problem, Problem::Flicker { .. })));
        assert!(triage(&stats(vec![0.0; 48], ramp), &o).is_empty());
        assert!(triage(&stats(vec![0.0; 48], step), &o).is_empty());
        let mut st = stats(vec![0.0; 24], vec![60.0; 24]);
        st.room_after = 0.1;
        assert!(triage(&st, &o).is_empty(), "a clean shot needs no handles");
        for x in &mut st.jitter[4..16] { *x = 0.8; }
        let f = triage(&st, &o);
        assert!(f.iter().any(|f| matches!(f.problem, Problem::ShortHandles { .. }) && f.why.contains("0.10 s after")), "{f:?}");
    }

    /// The fixture's own ground truth: "the wide shakes at 7.5–8.5 s", and the
    /// close-ups have figures moving in them but no camera shake.
    #[test]
    fn the_survey_finds_the_wides_shake_and_not_the_close_ups_moving_figures() {
        if !crate::testkit::ffmpeg_or_skip("the_survey_finds_the_wides_shake_and_not_the_close_ups_moving_figures") { return; }
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cartoon-fragile");
        let o = Options::default();
        let mut shots = Vec::new();
        for n in ["A001_wide", "A002_close_left", "A003_close_right"] {
            let st = measure(&dir.join(format!("{n}.mp4")), 2.0, 10.0, 14.0).unwrap();
            shots.push((n.to_string(), 2.0, 10.0, triage(&st, &o)));
        }
        let wide = &shots[0].3;
        assert_eq!(wide.len(), 1, "{wide:?}");
        let Problem::Shake { from, to, .. } = wide[0].problem else { panic!("{wide:?}") };
        assert!(from >= 7.2 && from <= 7.7 && to >= 8.1 && to <= 8.7, "near 7.5–8.5 s: {from:.2}–{to:.2}");
        assert!(shots[1].3.is_empty() && shots[2].3.is_empty(), "{:?} / {:?}", shots[1].3, shots[2].3);
        let md = shot_list(&shots);
        assert!(md.contains("| A001_wide |") && md.contains("| open |") && md.contains("Nothing found in: A002_close_left, A003_close_right."), "{md}");
    }
}
