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

/// How the angle for each shot is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefer {
    /// Take turns: the signals decide only when.
    RoundRobin,
    /// The angle whose performer moves most over the coming shot.
    MostMotion,
    /// The calmest angle over the coming shot.
    LeastMotion,
}

/// One kind of per-angle input -- performer motion, who is heard -- with a
/// lane per angle (in the group's angle order) and the operator's weight.
#[derive(Debug, Clone, PartialEq)]
pub struct AngleInput { pub name: String, pub per_angle: Vec<Signal>, pub weight: f32 }

/// Each angle's speech loudness relative to the loudest angle at every
/// moment, 0..1: 1 for the camera that hears the voice best, falling off by
/// how many dB quieter the others are (`db_range` to reach 0). Whoever is
/// speaking is loudest on the mic nearest them, so this says which angle is
/// close to the voice without any speaker labels.
pub fn relative_voice(envelopes_db: &[Vec<f32>], hop: f64, db_range: f32) -> Vec<Signal> {
    let n = envelopes_db.iter().map(|e| e.len()).min().unwrap_or(0);
    (0..envelopes_db.len()).map(|k| {
        let v: Vec<f32> = (0..n).map(|i| {
            let loudest = envelopes_db.iter().map(|e| e[i]).fold(f32::MIN, f32::max);
            // silence everywhere says nothing about who is speaking
            if loudest < -50.0 { return 0.0; }
            (1.0 - (loudest - envelopes_db[k][i]) / db_range.max(0.1)).clamp(0.0, 1.0)
        }).collect();
        Signal::new(&format!("voice: angle {}", k + 1), hop, v)
    }).collect()
}

/// Choose an angle for each shot between consecutive cut times over
/// [0, length). Each angle's score over the coming shot is the weighted mean
/// of its inputs (motion, voice, ...); the rule takes the highest, the lowest,
/// or turns. With `no_repeat`, a cut never lands on the angle already on
/// screen -- a cut to the same angle is a jump cut. Each choice comes with its
/// reason, naming every input's part and saying when no-repeat decided it.
pub fn choose_angles(cuts: &[f64], length: f64, inputs: &[AngleInput], prefer: Prefer, no_repeat: bool) -> Vec<(f64, f64, usize, String)> {
    let n = inputs.iter().map(|i| i.per_angle.len()).max().unwrap_or(1).max(1);
    let total_w: f32 = inputs.iter().map(|i| i.weight.max(0.0)).sum::<f32>().max(1e-6);
    let mut bounds = vec![0.0];
    bounds.extend(cuts.iter().copied().filter(|t| *t > 0.0 && *t < length));
    bounds.push(length);
    let mut out = Vec::new();
    let mut prev: Option<usize> = None;
    for (k, w) in bounds.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        let mean = |s: &Signal| {
            let steps = (((b - a) / s.hop).ceil() as usize).max(1);
            (0..steps).map(|i| s.at(a + i as f64 * s.hop)).sum::<f32>() / steps as f32
        };
        // per input, per angle: the input's mean over the shot
        let parts: Vec<Vec<f32>> = inputs.iter().map(|inp| (0..n).map(|ang| inp.per_angle.get(ang).map(mean).unwrap_or(0.0)).collect()).collect();
        let score: Vec<f32> = (0..n).map(|ang| inputs.iter().zip(&parts).map(|(inp, p)| inp.weight.max(0.0) / total_w * p[ang]).sum()).collect();
        let allowed: Vec<usize> = (0..n).filter(|i| !(no_repeat && Some(*i) == prev && n > 1)).collect();
        let best_of = |set: &[usize], most: bool| -> usize {
            let cmp = |x: &usize, y: &usize| score[*x].partial_cmp(&score[*y]).unwrap_or(std::cmp::Ordering::Equal);
            if most { *set.iter().max_by(|x, y| cmp(x, y)).unwrap() } else { *set.iter().min_by(|x, y| cmp(x, y)).unwrap() }
        };
        let all: Vec<usize> = (0..n).collect();
        let (pick, unrestricted) = match prefer {
            Prefer::RoundRobin => { let p = allowed.iter().copied().find(|i| *i == k % n).unwrap_or(allowed[0]); (p, p) }
            Prefer::MostMotion => (best_of(&allowed, true), best_of(&all, true)),
            Prefer::LeastMotion => (best_of(&allowed, false), best_of(&all, false)),
        };
        let why = match prefer {
            Prefer::RoundRobin => format!("angle {} (turns)", pick + 1),
            _ => {
                let detail = inputs.iter().zip(&parts).filter(|(inp, _)| inp.weight > 0.0)
                    .map(|(inp, p)| format!("{} {}", inp.name, p.iter().enumerate()
                        .map(|(i, v)| format!("{}{:.2}", if i == pick { "*" } else { "" }, v)).collect::<Vec<_>>().join("/")))
                    .collect::<Vec<_>>().join(", ");
                format!("angle {}: {detail}{}", pick + 1,
                        if unrestricted != pick { " (the pick was already on screen)" } else { "" })
            }
        };
        out.push((a, b, pick, why));
        prev = Some(pick);
    }
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
    fn the_angle_follows_the_action_and_never_jump_cuts() {
        // angle 1 moves in the first half, angle 2 in the second
        let n = (8.0 / HOP) as usize;
        let a1 = Signal::new("motion: a1", HOP, (0..n).map(|i| if (i as f64 * HOP) < 4.0 { 0.9 } else { 0.1 }).collect());
        let a2 = Signal::new("motion: a2", HOP, (0..n).map(|i| if (i as f64 * HOP) < 4.0 { 0.1 } else { 0.8 }).collect());
        let lanes = [AngleInput { name: "motion".into(), per_angle: vec![a1, a2], weight: 1.0 }];
        let shots = choose_angles(&[2.0, 4.0, 6.0], 8.0, &lanes, Prefer::MostMotion, false);
        assert_eq!(shots.iter().map(|s| s.2).collect::<Vec<_>>(), vec![0, 0, 1, 1], "follows the action");
        assert!(shots[2].3.contains("motion 0.10/*0.80"), "and says why: {}", shots[2].3);
        let calm = choose_angles(&[2.0, 4.0, 6.0], 8.0, &lanes, Prefer::LeastMotion, false);
        assert_eq!(calm.iter().map(|s| s.2).collect::<Vec<_>>(), vec![1, 1, 0, 0]);
        // no repeats: a cut never lands on the angle already showing
        let nr = choose_angles(&[2.0, 4.0, 6.0], 8.0, &lanes, Prefer::MostMotion, true);
        assert!(nr.windows(2).all(|w| w[0].2 != w[1].2), "no jump cuts: {:?}", nr.iter().map(|s| s.2).collect::<Vec<_>>());
        assert!(nr[1].3.contains("already on screen"), "when no-repeat overrides the rule, the reason says so: {}", nr[1].3);
        assert!(!shots[1].3.contains("already on screen"), "and only then: {}", shots[1].3);
        let rr = choose_angles(&[2.0, 4.0, 6.0], 8.0, &lanes, Prefer::RoundRobin, true);
        assert_eq!(rr.iter().map(|s| s.2).collect::<Vec<_>>(), vec![0, 1, 0, 1]);
        assert_eq!((shots[0].0, shots.last().unwrap().1), (0.0, 8.0), "the shots cover the span");
    }

    #[test]
    fn the_angle_nearest_the_voice_wins_when_voice_is_weighted() {
        // two mics: angle 1's is 12 dB louder in the first half (its actor is
        // speaking), angle 2's in the second half
        let n = (8.0 / HOP) as usize;
        let e1: Vec<f32> = (0..n).map(|i| if (i as f64 * HOP) < 4.0 { -12.0 } else { -24.0 }).collect();
        let e2: Vec<f32> = (0..n).map(|i| if (i as f64 * HOP) < 4.0 { -24.0 } else { -12.0 }).collect();
        let voice = relative_voice(&[e1, e2], HOP, 12.0);
        assert!((voice[0].at(1.0) - 1.0).abs() < 1e-6 && voice[1].at(1.0) < 0.01, "the loudest mic reads 1, 12 dB down reads 0");
        // motion says the opposite (angle 2 busy early): the weights decide who wins
        let m1 = Signal::new("m1", HOP, (0..n).map(|i| if (i as f64 * HOP) < 4.0 { 0.1 } else { 0.9 }).collect());
        let m2 = Signal::new("m2", HOP, (0..n).map(|i| if (i as f64 * HOP) < 4.0 { 0.9 } else { 0.1 }).collect());
        let inputs = |wm: f32, wv: f32| vec![
            AngleInput { name: "motion".into(), per_angle: vec![m1.clone(), m2.clone()], weight: wm },
            AngleInput { name: "voice".into(), per_angle: voice.clone(), weight: wv },
        ];
        let by_voice = choose_angles(&[4.0], 8.0, &inputs(0.2, 1.0), Prefer::MostMotion, false);
        assert_eq!(by_voice.iter().map(|s| s.2).collect::<Vec<_>>(), vec![0, 1], "follow the voice: {:?}", by_voice);
        assert!(by_voice[0].3.contains("voice *1.00/0.00"), "{}", by_voice[0].3);
        let by_motion = choose_angles(&[4.0], 8.0, &inputs(1.0, 0.2), Prefer::MostMotion, false);
        assert_eq!(by_motion.iter().map(|s| s.2).collect::<Vec<_>>(), vec![1, 0], "follow the action instead");
        // silence everywhere is no evidence
        let quiet = relative_voice(&[vec![-70.0; 10], vec![-65.0; 10]], HOP, 12.0);
        assert!(quiet.iter().all(|s| s.values.iter().all(|v| *v == 0.0)));
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
