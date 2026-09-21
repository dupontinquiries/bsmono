//! signals — many inputs, weighted by the operator, and every decision says why.
//!
//! The lesson from analysing "All My Life": the kick drum is one metric among
//! many, and the edit's real habits (it ends shots during the performer's
//! movement) are soft tendencies, not rules. So an automated cut should never
//! rest on one signal, and the operator should be able to see and steer all
//! of them:
//!
//! * a [`Signal`] is one lane, 0..1 over time: kick onsets, a control
//!   channel's level, where transcript lines start, performer motion;
//! * each has a weight and an on/off switch the operator sets;
//! * [`Mark`]s are hard inputs: "cut here" forces a candidate, "no cut"
//!   forbids any within its window, whatever the lanes say;
//! * [`candidates`] picks cut times from the weighted sum and returns, for
//!   each, which lanes fired and how much each counted -- so when the
//!   automation does something odd the operator sees why and turns the knob
//!   that matters, instead of accepting a black box.

/// One lane: `values[i]` at `i * hop` seconds, each 0..1.
#[derive(Debug, Clone, PartialEq)]
pub struct Signal {
    pub name: String,
    pub hop: f64,
    pub values: Vec<f32>,
    pub weight: f32,
    pub on: bool,
}

impl Signal {
    pub fn new(name: &str, hop: f64, values: Vec<f32>) -> Signal {
        Signal { name: name.into(), hop, values, weight: 1.0, on: true }
    }
    /// The lane's value at `t` seconds (linear between samples, 0 outside).
    pub fn at(&self, t: f64) -> f32 {
        if self.values.is_empty() || t < 0.0 { return 0.0; }
        let x = t / self.hop;
        let i = x.floor() as usize;
        if i + 1 >= self.values.len() { return self.values.last().copied().filter(|_| i < self.values.len()).unwrap_or(0.0); }
        let f = (x - i as f64) as f32;
        self.values[i] * (1.0 - f) + self.values[i + 1] * f
    }
}

/// A lane of short pulses at given times -- line starts, markers -- each
/// `width` seconds wide, so a pulse is found even between samples.
pub fn pulses(name: &str, times: &[f64], length: f64, hop: f64, width: f64) -> Signal {
    let n = (length / hop).ceil() as usize + 1;
    let mut v = vec![0.0f32; n];
    for &t in times {
        let (a, b) = (((t - width / 2.0) / hop).floor().max(0.0) as usize, ((t + width / 2.0) / hop).ceil() as usize);
        for (i, val) in v.iter_mut().enumerate().take(b.min(n)).skip(a) {
            let d = ((i as f64 * hop - t).abs() / (width / 2.0)).min(1.0);
            *val = val.max((1.0 - d) as f32);
        }
    }
    Signal::new(name, hop, v)
}

/// Onset strength in one frequency band -- the kick is the low band -- as a
/// 0..1 lane: spectral flux, local mean removed, normalised to its own 98th
/// percentile so one huge hit does not flatten everything else.
pub fn onset_lane(name: &str, mono: &[f32], sample_rate: u32, hop: f64, band: (f32, f32)) -> Signal {
    let n = 1024usize;
    let step = ((hop * sample_rate as f64).round() as usize).max(1);
    let bins: Vec<usize> = (0..n / 2).filter(|k| {
        let f = *k as f32 * sample_rate as f32 / n as f32; f >= band.0 && f < band.1
    }).collect();
    let mut prev: Vec<f32> = vec![0.0; bins.len()];
    let mut flux = Vec::new();
    let mut i = 0;
    while i + n <= mono.len() {
        let frame = &mono[i..i + n];
        // a small DFT over just the band's bins: the band is narrow for a kick
        let mags: Vec<f32> = bins.iter().map(|&k| {
            let (mut re, mut im) = (0.0f32, 0.0f32);
            for (j, x) in frame.iter().enumerate() {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * j as f32 / n as f32).cos();
                let ph = std::f32::consts::TAU * k as f32 * j as f32 / n as f32;
                re += x * w * ph.cos(); im -= x * w * ph.sin();
            }
            (1.0 + 10.0 * re.hypot(im)).ln()
        }).collect();
        flux.push(mags.iter().zip(&prev).map(|(m, p)| (m - p).max(0.0)).sum::<f32>());
        prev = mags;
        i += step;
    }
    let k = 16usize;
    let local: Vec<f32> = (0..flux.len()).map(|i| {
        let (a, b) = (i.saturating_sub(k), (i + k + 1).min(flux.len()));
        flux[a..b].iter().sum::<f32>() / (b - a) as f32
    }).collect();
    let mut v: Vec<f32> = flux.iter().zip(&local).map(|(f, l)| (f - l).max(0.0)).collect();
    let mut sorted = v.clone(); sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let top = sorted.get((sorted.len() as f32 * 0.98) as usize).copied().unwrap_or(1.0).max(1e-6);
    for x in v.iter_mut() { *x = (*x / top).min(1.0); }
    Signal::new(name, hop, v)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarkKind { CutHere, NoCut }

/// A hard operator input: a forced cut, or a window where no cut may land.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark { pub at: f64, pub kind: MarkKind, pub width: f64 }

/// Read hard marks from marker names: "cut here…" and "no cut…" (any case),
/// the convention the operator types into a marker.
pub fn marks_from_names(markers: &[(f64, String)]) -> Vec<Mark> {
    markers.iter().filter_map(|(t, n)| {
        let l = n.trim().to_lowercase();
        if l.starts_with("cut here") { Some(Mark { at: *t, kind: MarkKind::CutHere, width: 0.0 }) }
        else if l.starts_with("no cut") { Some(Mark { at: *t, kind: MarkKind::NoCut, width: 1.0 }) }
        else { None }
    }).collect()
}

/// A suggested cut and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub at: f64,
    pub score: f32,
    /// Each lane's weighted contribution at this moment, largest first.
    pub why: Vec<(String, f32)>,
    /// Forced by a "cut here" mark rather than chosen from the lanes.
    pub forced: bool,
}

impl Candidate {
    /// "kick 0.42 · level 0.18 · line start 0.10", or "forced (cut here)".
    pub fn explain(&self) -> String {
        if self.forced { return "forced (cut here)".into(); }
        self.why.iter().filter(|(_, c)| *c > 0.005).map(|(n, c)| format!("{n} {c:.2}")).collect::<Vec<_>>().join(" · ")
    }
}

/// The weighted score at `t` and each lane's part of it. Weights are
/// normalised over the lanes that are on, so turning one off re-balances the
/// rest instead of lowering every score.
pub fn score_at(signals: &[Signal], t: f64) -> (f32, Vec<(String, f32)>) {
    let total: f32 = signals.iter().filter(|s| s.on).map(|s| s.weight.max(0.0)).sum::<f32>().max(1e-6);
    let mut parts: Vec<(String, f32)> = signals.iter().filter(|s| s.on)
        .map(|s| (s.name.clone(), s.weight.max(0.0) / total * s.at(t))).collect();
    parts.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    (parts.iter().map(|p| p.1).sum(), parts)
}

/// Cut candidates over [0, length): local peaks of the weighted score above
/// `threshold`, no closer than `min_gap` seconds, plus every "cut here"; none
/// inside a "no cut" window. Sorted by time.
pub fn candidates(signals: &[Signal], marks: &[Mark], length: f64, hop: f64, threshold: f32, min_gap: f64) -> Vec<Candidate> {
    let n = (length / hop).ceil() as usize;
    let sc: Vec<f32> = (0..n).map(|i| score_at(signals, i as f64 * hop).0).collect();
    let blocked = |t: f64| marks.iter().any(|m| m.kind == MarkKind::NoCut && (t - m.at).abs() <= m.width / 2.0);
    let mut peaks: Vec<(usize, f32)> = (1..n.saturating_sub(1))
        .filter(|&i| sc[i] >= sc[i - 1] && sc[i] > sc[i + 1] && sc[i] >= threshold && !blocked(i as f64 * hop))
        .map(|i| (i, sc[i])).collect();
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<Candidate> = marks.iter().filter(|m| m.kind == MarkKind::CutHere && m.at >= 0.0 && m.at < length)
        .map(|m| Candidate { at: m.at, score: 1.0, why: vec![], forced: true }).collect();
    for (i, s) in peaks {
        let t = i as f64 * hop;
        if out.iter().all(|c| (c.at - t).abs() >= min_gap) {
            out.push(Candidate { at: t, score: s, why: score_at(signals, t).1, forced: false });
        }
    }
    out.sort_by(|a, b| a.at.partial_cmp(&b.at).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOP: f64 = 0.02;

    #[test]
    fn candidates_say_which_lanes_fired() {
        // kicks every second; a line starts at 2.5 s; both lanes on
        let kicks = pulses("kick", &[1.0, 2.0, 3.0, 4.0], 5.0, HOP, 0.1);
        let lines = pulses("line start", &[2.5], 5.0, HOP, 0.1);
        let c = candidates(&[kicks.clone(), lines.clone()], &[], 5.0, HOP, 0.2, 0.4);
        assert_eq!(c.iter().map(|c| (c.at * 10.0).round() as i32).collect::<Vec<_>>(), vec![10, 20, 25, 30, 40]);
        let at25 = c.iter().find(|c| (c.at - 2.5).abs() < 0.03).unwrap();
        assert_eq!(at25.why[0].0, "line start", "{at25:?}");
        assert!(at25.explain().starts_with("line start 0.50"), "{}", at25.explain());
        // weight the kick up and the line down: the line's candidate drops below the bar
        let (mut k2, mut l2) = (kicks, lines);
        k2.weight = 3.0; l2.weight = 0.2;
        let c2 = candidates(&[k2.clone(), l2.clone()], &[], 5.0, HOP, 0.2, 0.4);
        assert!(c2.iter().all(|c| (c.at - 2.5).abs() > 0.03), "the operator's weights decide: {c2:?}");
        // switch the kick off: only the line remains, and the weights re-balance
        k2.on = false;
        let c3 = candidates(&[k2, l2], &[], 5.0, HOP, 0.2, 0.4);
        assert_eq!(c3.len(), 1, "{c3:?}");
        assert!((c3[0].score - 1.0).abs() < 0.05, "one lane on counts in full: {}", c3[0].score);
    }

    #[test]
    fn hard_marks_override_the_lanes() {
        // a kick at 3.4 s sits 0.2 s from the forced cut at 3.6 s -- inside the gap
        let kicks = pulses("kick", &[1.0, 2.0, 3.4], 4.0, HOP, 0.1);
        let marks = marks_from_names(&[(2.0, "no cut — she is mid-line".into()), (3.6, "Cut here".into()), (1.0, "S1: nun scene".into())]);
        assert_eq!(marks.len(), 2, "only the two operator marks count: {marks:?}");
        let c = candidates(&[kicks], &marks, 4.0, HOP, 0.2, 0.4);
        assert!(c.iter().all(|c| (c.at - 2.0).abs() > 0.4), "nothing inside the no-cut window: {c:?}");
        let forced = c.iter().find(|c| (c.at - 3.6).abs() < 1e-9).expect("the forced cut is there");
        assert!(forced.forced && forced.explain() == "forced (cut here)");
        assert!(c.iter().all(|c| c.forced || (c.at - 3.6).abs() >= 0.4), "a lane peak too close to a forced cut yields to it: {c:?}");
        assert!(c.iter().any(|c| (c.at - 1.0).abs() < 0.03), "and the rest stand: {c:?}");
    }

    #[test]
    fn a_kick_lane_finds_kicks_in_audio() {
        // 60 Hz thumps every 0.5 s over a steady 2 kHz tone: the low band sees the thumps only
        let sr = 16_000u32;
        let mut a = vec![0.0f32; sr as usize * 3];
        for (i, s) in a.iter_mut().enumerate() {
            let t = i as f32 / sr as f32;
            *s = 0.2 * (std::f32::consts::TAU * 2000.0 * t).sin();
            let ph = t % 0.5;
            if ph < 0.12 { *s += 0.8 * (-ph * 30.0).exp() * (std::f32::consts::TAU * 60.0 * t).sin(); }
        }
        let lane = onset_lane("kick", &a, sr, HOP, (20.0, 150.0));
        let c = candidates(&[lane], &[], 3.0, HOP, 0.3, 0.25);
        assert!(c.len() >= 4, "{:?}", c.iter().map(|c| c.at).collect::<Vec<_>>());
        for x in &c { let off = (x.at % 0.5).min(0.5 - x.at % 0.5); assert!(off < 0.09, "a candidate off the kicks: {}", x.at); }
    }
}
