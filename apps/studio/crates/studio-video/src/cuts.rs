//! cuts — reverse-engineer an edit: where its cuts, fades and transitions are.
//!
//! The first step of learning from a finished edit (the analysis of "All My
//! Life" started here, in a script): find the shots. A CUT is a frame whose
//! change from the one before is a spike in BOTH colour-histogram distance and
//! mean pixel difference, judged against the frames around it (median and
//! spread of the neighbourhood), so a busy shot's constant motion -- a spinning
//! kaleidoscope, a dancer -- is its own normal and not a string of cuts.
//! A few-frame transition (a glitch, a flash) is one cut at its first frame. A
//! FADE is not a spike at all: brightness sliding steadily to or from black
//! over several frames, reported separately.

use crate::framepipe::Frame;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    /// A cut at this frame (the first frame of the new shot).
    Cut(usize),
    /// A fade to (or from) black over [start, end) frames.
    Fade { start: usize, end: usize, to_black: bool },
}

fn hist(f: &Frame) -> [f32; 64] {
    let mut h = [0f32; 64];
    for p in f.px.chunks(3) { h[(p[0] as usize >> 6) * 16 + (p[1] as usize >> 6) * 4 + (p[2] as usize >> 6)] += 1.0; }
    let n = (f.px.len() / 3).max(1) as f32;
    for v in h.iter_mut() { *v /= n; }
    h
}

fn robust_z(d: &[f32], i: usize, k: usize) -> f32 {
    let (lo, hi) = (i.saturating_sub(k), (i + k + 1).min(d.len()));
    let mut nb: Vec<f32> = (lo..hi).filter(|&j| j != i).map(|j| d[j]).collect();
    if nb.is_empty() { return 0.0; }
    nb.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let med = nb[nb.len() / 2];
    let mut dev: Vec<f32> = nb.iter().map(|v| (v - med).abs()).collect();
    dev.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mad = dev[dev.len() / 2].max(1e-3);
    (d[i] - med) / mad
}

/// Cuts and fades in a run of frames (any size; 160x90 is plenty).
pub fn detect(frames: &[Frame]) -> Vec<Event> {
    let n = frames.len();
    if n < 3 { return Vec::new(); }
    let hs: Vec<[f32; 64]> = frames.iter().map(hist).collect();
    let luma: Vec<f32> = frames.iter().map(|f| f.px.iter().map(|&v| v as f32).sum::<f32>() / f.px.len() as f32).collect();
    let mut dh = vec![0f32; n];
    let mut dp = vec![0f32; n];
    for i in 1..n {
        dh[i] = hs[i].iter().zip(&hs[i - 1]).map(|(a, b)| (a - b).abs()).sum();
        dp[i] = frames[i].px.iter().zip(&frames[i - 1].px).map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as f32).sum::<f32>()
            / frames[i].px.len() as f32;
    }
    let maxh = dh.iter().cloned().fold(0.0, f32::max);
    let mut out = Vec::new();
    let mut last_cut: Option<usize> = None;
    for i in 1..n {
        // both measures spike against their neighbourhood, and the histogram
        // change is a real one (not a spike out of near-zero noise)
        // A strong histogram spike alone is a cut: out of a busy shot (a
        // spinning pattern changing every pixel every frame) the PIXEL change
        // at the cut is nothing unusual, and requiring both missed it. A
        // weaker histogram spike still needs the pixel spike to agree.
        let (zh, zp) = (robust_z(&dh, i, 15), robust_z(&dp, i, 15));
        let real = dh[i] > 0.08 * maxh.max(1e-6) && dh[i] > 0.05;
        let is_cut = real && (zh > 8.0 || (zh > 4.0 && zp > 3.0));
        if is_cut {
            // a transition a few frames long is one cut at its start
            if last_cut.map(|c| i - c < 6).unwrap_or(false) { continue; }
            out.push(Event::Cut(i));
            last_cut = Some(i);
        }
    }
    // fades: brightness moving steadily toward or away from black
    let mut i = 1;
    while i < n {
        let down = luma[i] < luma[i - 1] - 0.5;
        let up = luma[i] > luma[i - 1] + 0.5;
        if down || up {
            let mut j = i;
            while j < n && ((down && luma[j] < luma[j - 1] - 0.3) || (up && luma[j] > luma[j - 1] + 0.3)) { j += 1; }
            let dark_end = if down { luma[j - 1] < 12.0 } else { luma[i - 1] < 12.0 };
            if j - i >= 6 && dark_end {
                out.push(Event::Fade { start: i, end: j, to_black: down });
                // a fade's frames are not cuts
                out.retain(|e| !matches!(e, Event::Cut(c) if *c >= i && *c < j));
                i = j;
                continue;
            }
        }
        i += 1;
    }
    out.sort_by_key(|e| match e { Event::Cut(c) => *c, Event::Fade { start, .. } => *start });
    out
}

/// Just the cut frames.
pub fn cut_frames(ev: &[Event]) -> Vec<usize> {
    ev.iter().filter_map(|e| match e { Event::Cut(c) => Some(*c), _ => None }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(c: [u8; 3], jitter: u8, seed: usize) -> Frame {
        let (w, h) = (80u32, 45u32);
        let mut f = Frame::black(w, h);
        for y in 0..h { for x in 0..w {
            let n = ((x as usize * 7 + y as usize * 13 + seed * 31) % 17) as u8 % (jitter + 1);
            f.set(x, y, [c[0].saturating_add(n), c[1].saturating_add(n), c[2].saturating_add(n)]);
        }}
        f
    }

    #[test]
    fn hard_cuts_are_found_on_their_frame() {
        let mut v = Vec::new();
        for i in 0..40 { v.push(flat([200, 60, 60], 6, i)); }
        for i in 40..90 { v.push(flat([40, 120, 220], 6, i)); }
        for i in 90..120 { v.push(flat([60, 200, 90], 6, i)); }
        assert_eq!(cut_frames(&detect(&v)), vec![40, 90]);
    }

    #[test]
    fn a_busy_shot_is_its_own_normal() {
        // a shot whose every frame changes a lot (a spinning pattern) is not a string of cuts
        let mut v = Vec::new();
        for i in 0..60 {
            let mut f = Frame::black(80, 45);
            for y in 0..45u32 { for x in 0..80u32 {
                let a = ((x as f32 - 40.0).atan2(y as f32 - 22.0) * 3.0 + i as f32 * 0.9).sin();
                let c = if a > 0.0 { [220, 180, 240] } else { [60, 40, 90] };
                f.set(x, y, c);
            }}
            v.push(f);
        }
        for i in 60..90 { v.push(flat([30, 140, 60], 4, i)); }
        assert_eq!(cut_frames(&detect(&v)), vec![60], "only the real cut");
    }

    #[test]
    fn a_short_transition_is_one_cut_and_a_fade_is_a_fade() {
        let mut v = Vec::new();
        for i in 0..40 { v.push(flat([200, 200, 60], 5, i)); }
        for k in 0..4 { v.push(flat(if k % 2 == 0 { [250, 250, 250] } else { [10, 10, 10] }, 5, k)); }   // a flicker
        for i in 44..90 { v.push(flat([60, 60, 200], 5, i)); }
        for k in 0..12 { let s = 1.0 - k as f32 / 11.0; v.push(flat([(60.0 * s) as u8, (60.0 * s) as u8, (200.0 * s) as u8], 0, 0)); }
        for _ in 0..20 { v.push(flat([0, 0, 0], 0, 0)); }
        let ev = detect(&v);
        assert_eq!(cut_frames(&ev), vec![40], "the flicker is one cut at its first frame: {ev:?}");
        assert!(ev.iter().any(|e| matches!(e, Event::Fade { to_black: true, start, .. } if (89..92).contains(start))), "{ev:?}");
    }
}
