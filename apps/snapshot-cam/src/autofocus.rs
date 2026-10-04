//! autofocus -- point-and-shoot focus: no tap needed, a tap overrides it.
//!
//! One sweep of the lens scores a 3x3 grid of regions at EVERY position at once (the same frame
//! feeds nine curves), then the lens is put where the weighted sum of those curves is highest:
//!
//! * each region's curve is normalised by its own best (a dim region and a bright one count alike)
//!   and a region with no real peak (a blank wall) counts for nothing;
//! * weights favour the centre of the picture, and regions that were MOVING while the camera was
//!   held up (a child, a pet) get a bonus: a basic motion cue standing in for subject detection.
//!   (There is no face detector in the repo; `Weights` is the seam one would plug into.)
//!
//! `Governor` decides WHEN to search: once the picture has been held steady, and again only when
//! the scene has really changed (and at least `MIN_GAP_MS` after the last search). A tap runs
//! `tapfocus` instead, which only scores the tapped region, and tells the governor to leave the
//! lens alone until the scene changes; a manual focus (a pro lens profile) switches all of this off.
//!
//! The timing rules are tapfocus's: a probe is scored on a frame at least 2 past the move and 150 ms
//! after it.

use capture_kit::focus;
use capture_kit::RawFrame;

use crate::tapfocus::{trace, Lens, AVG_FRAMES, FRESH_FRAMES, NUDGE, SETTLE_MS, TIMEOUT_MS, VERIFY_RATIO};

pub const GRID_W: usize = 3;
pub const GRID_H: usize = 3;
pub const N_REGIONS: usize = GRID_W * GRID_H;
/// Lens positions in the coarse sweep.
pub const COARSE: usize = 11;
/// A region's best must beat its own mean by this to count as having a subject.
pub const MIN_CONTRAST: f64 = 1.25;
/// A region counts only if its best sharpness is at least this share of the sharpest region's: the
/// edge pixels of a textured neighbour bleed a little detail across a border, and a region normalised
/// by its own best would otherwise count that sliver as a whole subject.
pub const MIN_SHARE: f64 = 0.03;
/// Centre weights: the middle, the four edge-middles, the four corners.
pub const W_CENTER: f32 = 1.0;
pub const W_EDGE: f32 = 0.5;
pub const W_CORNER: f32 = 0.25;
/// How much a fully moving region adds to its weight.
pub const MOTION_GAIN: f32 = 1.0;
/// The thumbnail used for motion and scene change: this many cells wide and tall.
pub const THUMB_W: usize = 32;
pub const THUMB_H: usize = 18;
/// Mean luma change per region (0..1) that counts as "still" overall, and as "the scene changed".
pub const STILL: f32 = 0.04;
pub const SCENE_CHANGED: f32 = 0.10;
/// Held still this long before a search starts; never wait longer than FIRST_WAIT_MS for the first.
pub const STEADY_MS: u64 = 600;
pub const FIRST_WAIT_MS: u64 = 3_000;
/// Between two searches.
pub const MIN_GAP_MS: u64 = 4_000;
/// How fast the per-region motion average follows (per frame).
pub const MOTION_ALPHA: f32 = 0.25;

/// The grid, row-major, as frame fractions.
pub fn regions() -> Vec<[f32; 4]> {
    let mut v = Vec::with_capacity(N_REGIONS);
    for gy in 0..GRID_H {
        for gx in 0..GRID_W {
            v.push([gx as f32 / GRID_W as f32, gy as f32 / GRID_H as f32, (gx + 1) as f32 / GRID_W as f32, (gy + 1) as f32 / GRID_H as f32]);
        }
    }
    v
}

/// Sharpness of every region of `f`; a region that cannot be scored counts 0.
pub fn region_scores(f: &RawFrame) -> Vec<f64> {
    regions().iter().map(|r| focus::sharpness(f, *r).unwrap_or(0.0)).collect()
}

/// The centre bias alone: the weights when nothing is moving.
pub fn center_weights() -> Vec<f32> {
    (0..N_REGIONS).map(|i| {
        let (gx, gy) = (i % GRID_W, i / GRID_W);
        match (gx == GRID_W / 2, gy == GRID_H / 2) {
            (true, true) => W_CENTER,
            (true, false) | (false, true) => W_EDGE,
            _ => W_CORNER,
        }
    }).collect()
}

/// A small luma picture: the cheap thing motion and scene change are measured on.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumb(pub Vec<f32>);

pub fn thumb(f: &RawFrame) -> Option<Thumb> {
    if !f.pixels_plausible() { return None; }
    let (w, h) = (f.width as usize, f.height as usize);
    let mut out = vec![0f32; THUMB_W * THUMB_H];
    for ty in 0..THUMB_H {
        for tx in 0..THUMB_W {
            // the centre pixel of the cell is enough: this is for change, not for looks
            let (x, y) = (((tx * 2 + 1) * w) / (THUMB_W * 2), ((ty * 2 + 1) * h) / (THUMB_H * 2));
            let p = &f.rgba[(y.min(h - 1) * w + x.min(w - 1)) * 4..];
            out[ty * THUMB_W + tx] = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0;
        }
    }
    Some(Thumb(out))
}

/// Mean absolute luma change per grid region between two thumbnails, 0..1.
pub fn motion_by_region(a: &Thumb, b: &Thumb) -> Vec<f32> {
    let mut sum = vec![0f32; N_REGIONS];
    let mut n = vec![0f32; N_REGIONS];
    for ty in 0..THUMB_H {
        for tx in 0..THUMB_W {
            let r = (ty * GRID_H / THUMB_H) * GRID_W + tx * GRID_W / THUMB_W;
            sum[r] += (a.0[ty * THUMB_W + tx] - b.0[ty * THUMB_W + tx]).abs();
            n[r] += 1.0;
        }
    }
    sum.iter().zip(&n).map(|(s, n)| s / n.max(1.0)).collect()
}

/// The weights for a search: the centre bias plus the moving regions, the most moving one by `MOTION_GAIN`.
pub fn weights(motion: &[f32]) -> Vec<f32> {
    let top = motion.iter().cloned().fold(0.0f32, f32::max);
    let base = center_weights();
    // only a region clearly moving compared to the picture counts: noise does not get a bonus
    let mean = motion.iter().sum::<f32>() / motion.len().max(1) as f32;
    base.iter().zip(motion).map(|(w, m)| if top > STILL && *m > mean * 1.5 && *m > STILL { w + MOTION_GAIN * (m / top) } else { *w }).collect()
}

/// When to search.
#[derive(Debug, Clone, Default)]
pub struct Governor {
    prev: Option<Thumb>,
    /// The scene as it was when the lens was last set.
    reference: Option<Thumb>,
    motion_avg: Vec<f32>,
    still_since: Option<u64>,
    first_seen: Option<u64>,
    last_search: Option<u64>,
}

impl Governor {
    /// One frame. `busy` while a search (or a tap's) is running. Returns the weights when a search
    /// should start now.
    pub fn feed(&mut self, f: &RawFrame, now_ms: u64, busy: bool) -> Option<Vec<f32>> {
        let Some(cur) = thumb(f) else { return None };
        self.first_seen.get_or_insert(now_ms);
        if let Some(prev) = &self.prev {
            let m = motion_by_region(prev, &cur);
            if self.motion_avg.is_empty() { self.motion_avg = vec![0.0; N_REGIONS]; }
            for (a, m) in self.motion_avg.iter_mut().zip(&m) { *a += (m - *a) * MOTION_ALPHA; }
            let mean = m.iter().sum::<f32>() / m.len() as f32;
            if mean < STILL { self.still_since.get_or_insert(now_ms); } else { self.still_since = None; }
        }
        self.prev = Some(cur.clone());
        if busy { return None; }
        let steady = self.still_since.is_some_and(|t| now_ms.saturating_sub(t) >= STEADY_MS);
        let gap_ok = self.last_search.is_none_or(|t| now_ms.saturating_sub(t) >= MIN_GAP_MS);
        let want = match &self.reference {
            // never focused: once steady, or after FIRST_WAIT_MS of trying
            None => steady || now_ms.saturating_sub(self.first_seen.unwrap_or(now_ms)) >= FIRST_WAIT_MS,
            // focused: only when the scene has really changed and settled again
            Some(r) => {
                let diff = motion_by_region(r, &cur).iter().sum::<f32>() / N_REGIONS as f32;
                diff >= SCENE_CHANGED && steady
            }
        };
        if want && gap_ok {
            self.reference = Some(cur);
            self.last_search = Some(now_ms);
            return Some(weights(&self.motion_avg.clone().into_iter().collect::<Vec<_>>()));
        }
        None
    }

    /// A tap (or a manual position) set the lens: leave it until the scene changes.
    pub fn hold(&mut self, f: &RawFrame, now_ms: u64) {
        self.reference = thumb(f);
        self.last_search = Some(now_ms);
    }

    /// Forget everything (the camera changed).
    pub fn reset(&mut self) { *self = Governor::default(); }
}

/// What a finished search decided.
#[derive(Debug, Clone, PartialEq)]
pub enum AutoVerdict {
    /// Lens left at `pos`; `region` is the grid cell that decided it (to draw).
    Locked { pos: i64, region: usize },
    Refused,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum AStep {
    Probe(i64),
    Done(Option<(i64, usize)>),
}

/// The multi-region search: a coarse sweep, then a hill-climb refine on the weighted objective.
#[derive(Debug, Clone)]
struct AutoSearch {
    min: i64,
    step: i64,
    coarse: Vec<i64>,
    samples: Vec<Vec<f64>>,
    weights: Vec<f32>,
    // refine
    norm: Option<Vec<(f64, f64)>>, // per region: (best, contrast)
    best: Option<(i64, f64)>,
    d: i64,
    queue: Vec<i64>,
    refine: Vec<(i64, f64)>,
    pending: Option<i64>,
}

impl AutoSearch {
    fn new(min: i64, max: i64, step: i64, weights: Vec<f32>) -> AutoSearch {
        let step = step.max(1);
        let span = (max - min).max(0);
        let snap = |p: i64| (p - min) / step * step + min;
        let mut coarse: Vec<i64> = (0..COARSE as i64).map(|i| snap(min + span * i / (COARSE as i64 - 1))).collect();
        coarse.dedup();
        AutoSearch { min, step, coarse, samples: vec![], weights, norm: None, best: None, d: 0, queue: vec![], refine: vec![], pending: None }
    }

    fn max(&self) -> i64 { *self.coarse.last().unwrap_or(&self.min) }
    fn snap(&self, p: i64) -> i64 { ((p.clamp(self.min, self.max()) - self.min) / self.step) * self.step + self.min }

    /// The weighted objective for one set of region scores.
    fn objective(&self, scores: &[f64]) -> f64 {
        let Some(norm) = &self.norm else { return 0.0 };
        scores.iter().zip(norm).zip(&self.weights).map(|((s, (best, contrast)), w)| {
            if *contrast < MIN_CONTRAST || *best <= 0.0 { 0.0 } else { *w as f64 * (contrast.min(3.0) / 3.0) * (s / best) }
        }).sum()
    }

    fn step(&mut self) -> AStep {
        if let Some(p) = self.pending { return AStep::Probe(p); }
        if self.samples.len() < self.coarse.len() {
            let p = self.coarse[self.samples.len()];
            self.pending = Some(p);
            return AStep::Probe(p);
        }
        if self.norm.is_none() {
            let mut norm: Vec<(f64, f64)> = (0..N_REGIONS).map(|r| {
                let col: Vec<f64> = self.samples.iter().map(|s| s[r]).collect();
                let best = col.iter().cloned().fold(0.0, f64::max);
                let mean = col.iter().sum::<f64>() / col.len() as f64;
                (best, if mean > 0.0 { best / mean } else { 0.0 })
            }).collect();
            // a region far weaker than the sharpest one is a border bleed or noise, not a subject
            let top = norm.iter().map(|n| n.0).fold(0.0, f64::max);
            for n in norm.iter_mut() { if n.0 < top * MIN_SHARE { n.1 = 0.0; } }
            self.norm = Some(norm);
            let (bi, bj) = self.samples.iter().enumerate().map(|(i, s)| (i, self.objective(s))).fold((0usize, f64::MIN), |a, c| if c.1 > a.1 { c } else { a });
            if bj <= 0.0 { return AStep::Done(None); } // nothing in the picture has a peak
            self.best = Some((self.coarse[bi], bj));
            let gap = if self.coarse.len() > 1 { self.coarse[1] - self.coarse[0] } else { self.step };
            self.d = (gap / 2).max(self.step);
        }
        loop {
            if let Some(p) = self.queue.pop() { self.pending = Some(p); return AStep::Probe(p); }
            if !self.refine.is_empty() {
                let (bp, bj) = self.best.unwrap();
                let top = self.refine.iter().cloned().fold((bp, bj), |a, c| if c.1 > a.1 { c } else { a });
                let improved = top.0 != bp;
                self.best = Some(top);
                self.refine.clear();
                if !improved { self.d /= 2; }
            }
            if self.d < self.step {
                let (p, _) = self.best.unwrap();
                return AStep::Done(Some((p, self.deciding_region(p))));
            }
            let (bp, _) = self.best.unwrap();
            let (lo, hi) = (self.snap(bp - self.d), self.snap(bp + self.d));
            let mut q = vec![];
            for p in [hi, lo] { if p != bp && !q.contains(&p) { q.push(p); } }
            if q.is_empty() { self.d /= 2; continue; }
            self.queue = q;
        }
    }

    fn feed(&mut self, scores: Vec<f64>) {
        let Some(p) = self.pending.take() else { return };
        if self.samples.len() < self.coarse.len() { self.samples.push(scores); }
        else { let j = self.objective(&scores); self.refine.push((p, j)); }
    }

    /// The region contributing most at `pos` (from the nearest coarse sample).
    fn deciding_region(&self, pos: i64) -> usize {
        let Some(norm) = &self.norm else { return N_REGIONS / 2 };
        let i = self.coarse.iter().enumerate().min_by_key(|(_, c)| (**c - pos).abs()).map(|(i, _)| i).unwrap_or(0);
        (0..N_REGIONS).map(|r| {
            let (best, contrast) = norm[r];
            let v = if contrast < MIN_CONTRAST || best <= 0.0 { 0.0 } else { self.weights[r] as f64 * self.samples[i][r] / best };
            (r, v)
        }).fold((N_REGIONS / 2, -1.0), |a, c| if c.1 > a.1 { c } else { a }).0
    }
}

#[derive(Debug, Clone, Copy)]
struct Probe { pos: i64, issued_ms: u64, issued_seq: u64 }

/// The driver: like `tapfocus::TapFocus` but for the whole grid.
pub struct AutoRun {
    search: Option<AutoSearch>,
    probe: Option<Probe>,
    started_ms: u64,
    previous: Option<i64>,
    /// Settled frames collected for the current probe (a probe is their per-region median, see `tapfocus::AVG_FRAMES`).
    acc: Vec<Vec<f64>>,
    /// Every probe's averaged region scores, to know what the search measured at the position it chose.
    seen: Vec<(i64, Vec<f64>)>,
    /// After the search: the landing is re-measured before the lens is called locked (see `tapfocus::VERIFY_RATIO`).
    verify: Option<Verify>,
    /// The last result and when, for drawing.
    pub result: Option<(AutoVerdict, u64)>,
}

/// The re-measure of the position the search chose, in the region that decided it.
#[derive(Debug, Clone)]
struct Verify { region: usize, want: f64, probe: Option<Probe>, acc: Vec<f64>, best: (i64, f64), todo: Vec<i64> }

fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

impl Default for AutoRun {
    fn default() -> Self { AutoRun { search: None, probe: None, started_ms: 0, previous: None, acc: Vec::new(), seen: Vec::new(), verify: None, result: None } }
}

pub const RESULT_SHOW_MS: u64 = 1_200;

impl AutoRun {
    pub fn is_searching(&self) -> bool { self.search.is_some() || self.verify.is_some() }

    pub fn begin(&mut self, w: Vec<f32>, range: (i64, i64, i64), lens: &mut dyn Lens, now_ms: u64) {
        self.search = Some(AutoSearch::new(range.0, range.1, range.2, w));
        self.probe = None;
        self.acc.clear();
        self.seen.clear();
        self.verify = None;
        self.started_ms = now_ms;
        self.previous = lens.current();
        self.result = None;
    }

    pub fn cancel(&mut self) { self.search = None; self.probe = None; self.verify = None; self.acc.clear(); }

    pub fn on_frame(&mut self, f: &RawFrame, now_ms: u64, lens: &mut dyn Lens) -> Option<AutoVerdict> {
        if self.search.is_none() && self.verify.is_none() { return None; }
        if now_ms.saturating_sub(self.started_ms) > TIMEOUT_MS + 3_000 {
            return self.finish(AutoVerdict::Failed("autofocus timed out".into()), now_ms);
        }
        if self.verify.is_some() { return self.verify_frame(f, now_ms, lens); }
        let search = self.search.as_mut()?;
        loop {
            match self.probe {
                None => match search.step() {
                    AStep::Probe(p) => {
                        if let Err(e) = lens.move_to(p) { return self.finish(AutoVerdict::Failed(e), now_ms); }
                        self.acc.clear();
                        self.probe = Some(Probe { pos: p, issued_ms: now_ms, issued_seq: f.seq });
                        return None;
                    }
                    AStep::Done(Some((p, r))) => {
                        // what the search measured at the chosen position, in the deciding region
                        if trace() { eprintln!("af-auto: DONE position={p} region={r}"); }
                        let want = self.seen.iter().filter(|(pos, _)| *pos == p).filter_map(|(_, sc)| sc.get(r).copied()).fold(0.0f64, f64::max);
                        if want <= 0.0 {
                            let v = match lens.move_to(p) { Ok(()) => AutoVerdict::Locked { pos: p, region: r }, Err(e) => AutoVerdict::Failed(e) };
                            return self.finish(v, now_ms);
                        }
                        // re-measure where the lens is about to be left: the answer first, then nudges either side
                        if let Err(e) = lens.move_to(p) { return self.finish(AutoVerdict::Failed(e), now_ms); }
                        self.search = None;
                        self.probe = None;
                        self.verify = Some(Verify {
                            region: r, want, acc: Vec::new(), best: (p, 0.0),
                            probe: Some(Probe { pos: p, issued_ms: now_ms, issued_seq: f.seq }),
                            todo: vec![p - 2 * NUDGE, p + 2 * NUDGE, p - NUDGE, p + NUDGE],
                        });
                        return None;
                    }
                    AStep::Done(None) => {
                        if let Some(prev) = self.previous { let _ = lens.move_to(prev); }
                        return self.finish(AutoVerdict::Refused, now_ms);
                    }
                },
                Some(pr) => {
                    let settled = now_ms.saturating_sub(pr.issued_ms) >= SETTLE_MS && f.seq >= pr.issued_seq + FRESH_FRAMES;
                    if !settled { return None; }
                    if !f.pixels_plausible() { return None; }
                    self.acc.push(region_scores(f));
                    if self.acc.len() < AVG_FRAMES { return None; } // a median of several frames, not one noisy one
                    let n = self.acc[0].len();
                    let m: Vec<f64> = (0..n).map(|k| { let mut col: Vec<f64> = self.acc.iter().map(|a| a[k]).collect(); median(&mut col) }).collect();
                    self.acc.clear();
                    if trace() { eprintln!("af-auto: PROBE pos={} regions={}", pr.pos, m.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(" ")); }
                    self.seen.push((pr.pos, m.clone()));
                    search.feed(m);
                    self.probe = None;
                }
            }
        }
    }

    fn verify_frame(&mut self, f: &RawFrame, now_ms: u64, lens: &mut dyn Lens) -> Option<AutoVerdict> {
        let v = self.verify.as_mut()?;
        let pr = v.probe?;
        let settled = now_ms.saturating_sub(pr.issued_ms) >= SETTLE_MS && f.seq >= pr.issued_seq + FRESH_FRAMES;
        if !settled || !f.pixels_plausible() { return None; }
        v.acc.push(region_scores(f).get(v.region).copied().unwrap_or(0.0));
        if v.acc.len() < AVG_FRAMES { return None; }
        let m = median(&mut v.acc);
        v.acc.clear();
        if trace() { eprintln!("af-auto: VERIFY pos={} region={} score={:.3} want>={:.3}", pr.pos, v.region, m, v.want * VERIFY_RATIO); }
        if m > v.best.1 { v.best = (pr.pos, m); }
        if m >= v.want * VERIFY_RATIO {
            // reproduced: this is where the lens is, and it is sharp
            let (pos, region) = (pr.pos, v.region);
            return self.finish(AutoVerdict::Locked { pos, region }, now_ms);
        }
        if let Some(next) = v.todo.pop() {
            if let Err(e) = lens.move_to(next) { return self.finish(AutoVerdict::Failed(e), now_ms); }
            v.probe = Some(Probe { pos: next, issued_ms: now_ms, issued_seq: f.seq });
            return None;
        }
        // Nothing near the answer reproduced the peak: noise, or the scene moved. Do not leave the lens somewhere blurry.
        if trace() { eprintln!("af-auto: REFUSED after verify"); }
        if let Some(prev) = self.previous { let _ = lens.move_to(prev); }
        self.finish(AutoVerdict::Refused, now_ms)
    }

    fn finish(&mut self, v: AutoVerdict, now_ms: u64) -> Option<AutoVerdict> {
        self.search = None;
        self.probe = None;
        self.verify = None;
        self.acc.clear();
        self.result = Some((v.clone(), now_ms));
        Some(v)
    }

    /// The decided region while its result is still showing: `(rect, fade)`.
    pub fn shown_region(&self, now_ms: u64) -> Option<([f32; 4], f32)> {
        let (AutoVerdict::Locked { region, .. }, at) = self.result.as_ref()? else { return None };
        let age = now_ms.saturating_sub(*at);
        if age >= RESULT_SHOW_MS { return None; }
        Some((regions()[*region], 1.0 - (age as f32 / RESULT_SHOW_MS as f32).powi(2)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A scene as a 3x3 grid of subjects: each region has its own best lens position, or none (flat).
    #[derive(Clone)]
    struct Scene { peaks: [Option<i64>; N_REGIONS] }

    fn frame(scene: &Scene, lens_pos: i64, seq: u64, shift: u32) -> RawFrame {
        frame_ex(scene, lens_pos, seq, shift, 160.0, u64::MAX, 0, 0)
    }

    /// `width`: how quickly a region's sharpness falls off from its best lens position. After frame `drift_at` every
    /// peak moves by `drift` (a subject that walked away). `noise` != 0: no peaks at all, a different amount of texture
    /// in every region every frame, whatever the lens does.
    fn frame_ex(scene: &Scene, lens_pos: i64, seq: u64, shift: u32, width: f32, drift_at: u64, drift: i64, noise: u32) -> RawFrame {
        let (w, h) = (180u32, 90u32);
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let r = (y as usize * GRID_H / h as usize) * GRID_W + x as usize * GRID_W / w as usize;
                let contrast = if noise != 0 {
                    let k = seq.wrapping_mul(2654435761).wrapping_add(noise as u64 * 40503).wrapping_add(r as u64 * 7919) >> 7;
                    0.25 + 0.12 * ((k % 1000) as f32 / 1000.0)
                } else {
                    match scene.peaks[r] {
                        Some(p) => { let d = (lens_pos - (p + if seq >= drift_at { drift } else { 0 })) as f32 / width; (-d * d).exp() }
                        None => 0.0,
                    }
                };
                let chk = if (x + y + shift) % 2 == 0 { 1.0 } else { -1.0 };
                let l = (128.0 + 100.0 * contrast * chk) as u8;
                v.extend_from_slice(&[l, l, l, 255]);
            }
        }
        RawFrame { width: w, height: h, captured_ms: seq * 33, seq, rgba: Arc::from(v) }
    }

    struct FakeLens { pos: i64, history: Vec<i64>, moves: Vec<i64>, lag: usize }
    impl FakeLens {
        fn new(pos: i64, lag: usize) -> FakeLens { FakeLens { pos, history: vec![], moves: vec![], lag } }
        fn tick(&mut self) { self.history.push(self.pos); }
        fn seen(&self) -> i64 { let n = self.history.len(); self.history[n.saturating_sub(1 + self.lag)] }
    }
    impl Lens for FakeLens {
        fn move_to(&mut self, pos: i64) -> Result<(), String> { self.pos = pos; self.moves.push(pos); Ok(()) }
        fn current(&mut self) -> Option<i64> { Some(self.pos) }
    }

    fn run(scene: &Scene, w: Vec<f32>, start: i64) -> (Option<AutoVerdict>, FakeLens) {
        let mut lens = FakeLens::new(start, 2);
        let mut run = AutoRun::default();
        run.begin(w, (0, 1023, 1), &mut lens, 0);
        let mut verdict = None;
        for seq in 0..600u64 {
            lens.tick();
            if let Some(v) = run.on_frame(&frame(scene, lens.seen(), seq, 0), seq * 33, &mut lens) { verdict = Some(v); break; }
        }
        (verdict, lens)
    }

    fn only(r: usize, p: i64) -> Scene { let mut peaks = [None; N_REGIONS]; peaks[r] = Some(p); Scene { peaks } }

    /// One subject anywhere in the grid is found, and the deciding region is the one that held it.
    #[test]
    fn it_focuses_on_the_one_subject_wherever_it_is() {
        for r in [0usize, 4, 8] {
            let (v, lens) = run(&only(r, 420), center_weights(), 100);
            let Some(AutoVerdict::Locked { pos, region }) = v else { panic!("{v:?}") };
            assert!((pos - 420).abs() <= 40, "region {r}: focused near 420, got {pos}");
            assert_eq!(region, r);
            assert_eq!(*lens.moves.last().unwrap(), pos);
        }
    }

    /// Two subjects at different distances: the centre one wins by default, a moving off-centre one wins when it moves.
    #[test]
    fn the_centre_wins_by_default_and_a_moving_subject_can_outweigh_it() {
        let mut peaks = [None; N_REGIONS];
        peaks[4] = Some(300);   // centre
        peaks[2] = Some(800);   // top right corner
        let scene = Scene { peaks };
        let (v, _) = run(&scene, center_weights(), 100);
        let Some(AutoVerdict::Locked { pos, region }) = v else { panic!("{v:?}") };
        assert!((pos - 300).abs() <= 60 && region == 4, "centre by default: {pos} region {region}");
        // the corner was moving
        let mut motion = vec![0.0f32; N_REGIONS];
        motion[2] = 0.3;
        let w = weights(&motion);
        assert!(w[2] > w[4], "a moving corner outweighs a still centre: {w:?}");
        let (v, _) = run(&scene, w, 100);
        let Some(AutoVerdict::Locked { pos, region }) = v else { panic!("{v:?}") };
        assert!((pos - 800).abs() <= 60 && region == 2, "the mover wins: {pos} region {region}");
    }

    /// A scene with nothing to focus on is refused and the lens put back.
    #[test]
    fn a_scene_with_no_subject_is_refused() {
        let (v, lens) = run(&Scene { peaks: [None; N_REGIONS] }, center_weights(), 640);
        assert_eq!(v, Some(AutoVerdict::Refused));
        assert_eq!(lens.pos, 640);
    }

    /// A compromise: with two equal-weight subjects 200 apart the lens lands between or on one, never past both.
    #[test]
    fn two_equal_subjects_get_a_position_between_or_on_them() {
        let mut peaks = [None; N_REGIONS];
        peaks[3] = Some(400);
        peaks[5] = Some(560);
        let (v, _) = run(&Scene { peaks }, vec![1.0; N_REGIONS], 50);
        let Some(AutoVerdict::Locked { pos, .. }) = v else { panic!("{v:?}") };
        assert!((380..=580).contains(&pos), "between them: {pos}");
    }

    #[test]
    fn motion_is_measured_per_region_and_noise_gets_no_bonus() {
        let scene = only(4, 500);
        let a = thumb(&frame(&scene, 500, 0, 0)).unwrap();
        // the same scene, but the left column's texture shifted
        let b_frame = {
            let mut f = frame(&scene, 500, 1, 0);
            let w = f.width as usize;
            let mut px = f.rgba.to_vec();
            for y in 0..f.height as usize { for x in 0..w / 3 { for c in 0..3 { px[(y * w + x) * 4 + c] = 20; } } }
            f.rgba = Arc::from(px);
            f
        };
        let b = thumb(&b_frame).unwrap();
        let m = motion_by_region(&a, &b);
        assert!(m[0] > 0.02 && m[3] > 0.02 && m[6] > 0.02, "the left column moved: {m:?}");
        assert!(m[1] < 0.01 && m[4] < 0.01 && m[8] < 0.01, "the rest did not: {m:?}");
        let w = weights(&m);
        assert!(w[3] > center_weights()[3] && (w[4] - W_CENTER).abs() < 1e-6, "{w:?}");
        let quiet = weights(&vec![0.01; N_REGIONS]);
        assert_eq!(quiet, center_weights(), "tiny uniform change is noise");
    }

    /// The governor waits for the camera to be held steady, then searches once, then only after the scene really changes.
    #[test]
    fn the_governor_searches_when_steady_and_again_only_on_a_new_scene() {
        let scene_a = only(4, 300);
        let scene_b = Scene { peaks: [Some(300); N_REGIONS] };
        let mut g = Governor::default();
        let mut starts = vec![];
        let mut t = 0u64;
        // 3 s of the same scene (a still camera), then a different scene for 12 s
        for seq in 0..500u64 {
            t = seq * 33;
            let f = if t < 3_000 { frame(&scene_a, 300, seq, 0) } else { frame(&scene_b, 300, seq, 0) };
            if g.feed(&f, t, false).is_some() { starts.push(t); }
        }
        assert!(starts.len() == 2, "one search for each scene, no more: {starts:?}");
        assert!((STEADY_MS..=STEADY_MS + 400).contains(&starts[0]), "the first once steady: {}", starts[0]);
        assert!(starts[1] >= 3_000 && starts[1] - starts[0] >= MIN_GAP_MS, "the second after the change and the gap: {starts:?}");
        // while busy it never starts
        let mut busy = Governor::default();
        for seq in 0..200u64 { assert!(busy.feed(&frame(&scene_a, 300, seq, 0), seq * 33, true).is_none()); }
        let _ = t;
    }

    /// A tap holds the lens until the scene changes.
    #[test]
    fn a_tap_holds_the_lens_until_the_scene_changes() {
        let scene_a = only(4, 300);
        let scene_b = Scene { peaks: [Some(300); N_REGIONS] };
        let mut g = Governor::default();
        g.hold(&frame(&scene_a, 300, 0, 0), 0);
        for seq in 1..200u64 { assert!(g.feed(&frame(&scene_a, 300, seq, 0), seq * 33 + 5_000, false).is_none(), "same scene: left alone"); }
        let mut again = None;
        for seq in 200..400u64 { if let Some(w) = g.feed(&frame(&scene_b, 300, seq, 0), seq * 33 + 5_000, false) { again = Some(w); break; } }
        assert!(again.is_some(), "a new scene is searched again");
    }

    #[test]
    fn the_result_region_shows_then_fades() {
        let mut r = AutoRun::default();
        r.result = Some((AutoVerdict::Locked { pos: 10, region: 4 }, 1000));
        let (rect, a) = r.shown_region(1100).unwrap();
        assert_eq!(rect, regions()[4]);
        assert!(a > 0.9);
        assert!(r.shown_region(1000 + RESULT_SHOW_MS + 1).is_none());
        r.result = Some((AutoVerdict::Refused, 1000));
        assert!(r.shown_region(1010).is_none(), "a refusal draws no region");
    }

    /// A lens that lands short or long depending on the direction of travel (a voice coil's hysteresis), with lag.
    struct HystLens { cmd: i64, bias: i64, history: Vec<i64>, lag: usize, up: i64, down: i64 }
    impl HystLens {
        fn new(pos: i64, lag: usize, up: i64, down: i64) -> HystLens { HystLens { cmd: pos, bias: 0, history: vec![], lag, up, down } }
        fn effective(&self) -> i64 { self.cmd + self.bias }
        fn tick(&mut self) { let e = self.effective(); self.history.push(e); }
        fn seen(&self) -> i64 { let n = self.history.len(); self.history[n.saturating_sub(1 + self.lag)] }
    }
    impl Lens for HystLens {
        fn move_to(&mut self, pos: i64) -> Result<(), String> {
            if pos > self.cmd { self.bias = self.up; } else if pos < self.cmd { self.bias = self.down; }
            self.cmd = pos; Ok(())
        }
        fn current(&mut self) -> Option<i64> { Some(self.cmd) }
    }

    #[test]
    fn a_lens_that_lands_short_or_long_still_ends_up_sharp_because_the_landing_is_re_measured() {
        // width 40 and a motor that lands 12 steps off by direction: arriving from the other side than the probe did
        // leaves the glass 24 steps from where it was measured (score x0.70, under the 0.85 bar; a nudge of 10-20 fixes it). Many starts and peaks so some arrive that way.
        for peak in [300, 500, 800] {
            for start in [0, 100, 250, 400, 650, 800, 950, 1023] {
                let scene = only(4, peak);
                let mut lens = HystLens::new(start, 2, -12, 12);
                let mut run = AutoRun::default();
                run.begin(center_weights(), (0, 1023, 1), &mut lens, 0);
                let mut verdict = None;
                for seq in 0..800u64 {
                    lens.tick();
                    if let Some(v) = run.on_frame(&frame_ex(&scene, lens.seen(), seq, 0, 40.0, u64::MAX, 0, 0), seq * 33, &mut lens) { verdict = Some(v); break; }
                }
                let Some(AutoVerdict::Locked { .. }) = verdict else { panic!("peak {peak} start {start}: {verdict:?}") };
                let off = (lens.effective() - peak).abs();
                assert!(off <= 14, "peak {peak} start {start}: the glass ends {off} steps from the peak (effective {}, commanded {})", lens.effective(), lens.cmd);
            }
        }
    }

    #[test]
    fn a_subject_that_is_gone_when_the_lens_arrives_is_refused_not_locked() {
        let scene = only(4, 400);
        let go = |drift_at: u64| {
            let mut lens = FakeLens::new(100, 2);
            let mut run = AutoRun::default();
            run.begin(center_weights(), (0, 1023, 1), &mut lens, 0);
            let mut verdict = None;
            let mut seq_end = 0;
            for seq in 0..800u64 {
                lens.tick();
                if let Some(v) = run.on_frame(&frame_ex(&scene, lens.seen(), seq, 0, 60.0, drift_at, 500, 0), seq * 33, &mut lens) { verdict = Some(v); seq_end = seq; break; }
            }
            (verdict, lens, seq_end)
        };
        let (_, _, end) = go(u64::MAX);
        let (v, lens, _) = go(end.saturating_sub(12));
        assert_eq!(v, Some(AutoVerdict::Refused), "the subject moved away during the search");
        assert_eq!(lens.pos, 100, "and the lens went back instead of staying on a spot that is now blurry");
    }

    #[test]
    fn a_scene_that_is_only_noise_is_never_locked_even_when_noise_makes_a_peak_look_good() {
        for seed in 1..=12u32 {
            let scene = Scene { peaks: [None; N_REGIONS] };
            let mut lens = FakeLens::new(321, 2);
            let mut run = AutoRun::default();
            run.begin(center_weights(), (0, 1023, 1), &mut lens, 0);
            let mut verdict = None;
            for seq in 0..800u64 {
                lens.tick();
                if let Some(v) = run.on_frame(&frame_ex(&scene, lens.seen(), seq, 0, 160.0, u64::MAX, 0, seed), seq * 33, &mut lens) { verdict = Some(v); break; }
            }
            assert!(!matches!(verdict, Some(AutoVerdict::Locked { .. })), "seed {seed}: noise was locked as focus ({verdict:?})");
            assert_eq!(lens.pos, 321, "seed {seed}: the lens did not go back");
        }
    }
}
