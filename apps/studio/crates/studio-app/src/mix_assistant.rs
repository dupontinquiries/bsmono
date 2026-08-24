//! mix_assistant — the mixing/mastering pass, called from the master strip.
//!
//! `studio_fx::assistant` has done the hard part since before this session:
//! findings-first analysis (a build-up, a harshness, a phase risk — each
//! with a confidence), a `propose` that is strength-capped and defaults to
//! 30% of a full correction, and the two rules its own tests enforce —
//! strength 0 changes nothing, and a balanced mix yields NO findings.
//! `studio_copilot::mix_pass` maps those findings onto real ops against a
//! chosen bus. Neither was reachable from the app: the analysis had no
//! caller and the proposal had no button.
//!
//! This module is the caller. It measures the actual mixdown (the thing
//! being mastered, not a guess), asks for a proposal, and — this is the
//! part worth being careful about — COMPOSES the proposed chain onto the
//! bus's existing one rather than replacing it, which is the simplification
//! `mix_pass` documents and leaves to its caller. The result applies as ONE
//! undo group: a mastering pass that takes six presses of `u` to reverse is
//! a pass nobody dares try.
//!
//! The order stays diagnosis → proposal → apply, with the findings readable
//! and the strength adjustable BEFORE anything touches the project. That is
//! the assistant module's own thesis; the UI would betray it by applying on
//! click.

use studio_core::{BusId, Effect, Op};
use studio_fx::analysis::{self, Spectrum};

/// What the analysis measured, kept beside the proposal so the panel can
/// show the numbers a mastering engineer would actually check.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub crest_db: f32,
    pub correlation: f32,
    pub low_width: f32,
    pub peak_db: f32,
}

/// Measure an interleaved-stereo mixdown: the spectrum of its loudest
/// second (a mix's character lives in the loud part, not in a fade-in that
/// happens to be first), plus crest, correlation and low-end width.
pub fn measure(interleaved: &[f32], rate: u32) -> Option<(Spectrum, Measured)> {
    if interleaved.len() < 4 { return None; }
    let frames = interleaved.len() / 2;
    let (mut left, mut right) = (Vec::with_capacity(frames), Vec::with_capacity(frames));
    for f in 0..frames {
        left.push(interleaved[f * 2]);
        right.push(interleaved[f * 2 + 1]);
    }
    let mono: Vec<f32> = (0..frames).map(|i| (left[i] + right[i]) * 0.5).collect();

    // The loudest window, by RMS over non-overlapping windows — one FFT
    // block from a REPRESENTATIVE place beats an average over silence.
    // Sized to a POWER OF TWO at or below a second: Spectrum::analyze is
    // radix-2 and panics otherwise, which a plain `rate` window (48000)
    // duly triggered.
    let win = {
        let cap = (rate as usize).min(frames.max(1));
        let mut w = 1usize;
        while w * 2 <= cap { w *= 2; }
        w
    };
    if frames < win { return None; }
    let mut best_at = 0usize;
    let mut best_rms = -1.0f32;
    let mut at = 0usize;
    while at + win <= frames {
        let r: f32 = mono[at..at + win].iter().map(|s| s * s).sum::<f32>() / win as f32;
        if r > best_rms { best_rms = r; best_at = at; }
        at += win;
    }
    let block = &mono[best_at..(best_at + win).min(frames)];
    // Silence is judged on the CHANNELS, not on their sum: a fully
    // out-of-phase mix mono-sums to zero, and bailing there would skip
    // analysis on precisely the mix most worth warning about. (The first
    // draft did exactly that; the phase-inversion test caught it.)
    let quiet = left[best_at..(best_at + win).min(frames)].iter()
        .chain(right[best_at..(best_at + win).min(frames)].iter())
        .all(|s| s.abs() < 1e-7);
    if quiet { return None; }

    // When the sum has collapsed (heavy phase cancellation), the mono
    // block's spectrum describes the cancellation, not the mix — read one
    // channel instead, and let the correlation finding carry the warning.
    let mono_rms = (block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32).sqrt();
    let side = &left[best_at..(best_at + win).min(frames)];
    let side_rms = (side.iter().map(|s| s * s).sum::<f32>() / side.len() as f32).sqrt();
    let block: &[f32] = if mono_rms < side_rms * 0.1 { side } else { block };
    let spec = Spectrum::analyze(block, rate as f32);
    let rms = (block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32).sqrt();
    let peak = block.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let crest_db = 20.0 * (peak.max(1e-9) / rms.max(1e-9)).log10();
    let lo = best_at;
    let hi = (best_at + win).min(frames);
    let correlation = analysis::correlation(&left[lo..hi], &right[lo..hi]);
    // Low-end width from a crudely low-passed copy of each side: the
    // finding is about the BASS being wide, not the mix being wide.
    let lp = |v: &[f32]| {
        let k = 1.0 - (-2.0 * std::f32::consts::PI * 120.0 / rate as f32).exp();
        let mut st = 0.0f32;
        v.iter().map(|s| { st += k * (s - st); st }).collect::<Vec<f32>>()
    };
    let low_width = analysis::width(&lp(&left[lo..hi]), &lp(&right[lo..hi]));
    Some((spec, Measured {
        crest_db, correlation, low_width,
        peak_db: 20.0 * peak.max(1e-9).log10(),
    }))
}

/// The proposal's ops, rewritten so each `SetBusChain` EXTENDS the bus's
/// current chain instead of replacing it — `mix_pass` builds each move as a
/// standalone one-effect chain (it is pure and cannot see the bus), and
/// applying those verbatim would delete the user's own effects and then
/// each other. Composed here, in the caller that does have the chain, and
/// folded to ONE `SetBusChain` so several EQ moves are one op.
pub fn compose(existing: &[Effect], ops: &[Op], bus: BusId) -> Vec<Op> {
    let mut chain = existing.to_vec();
    let mut out = Vec::new();
    let mut touched_chain = false;
    for op in ops {
        match op {
            Op::SetBusChain { chain: added, .. } => {
                chain.extend(added.iter().cloned());
                touched_chain = true;
            }
            other => out.push(other.clone()),
        }
    }
    if touched_chain {
        out.insert(0, Op::SetBusChain { id: bus, chain });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn stereo(frames: usize, f: impl Fn(usize) -> (f32, f32)) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * 2);
        for i in 0..frames { let (l, r) = f(i); v.push(l); v.push(r); }
        v
    }

    #[test]
    fn measurement_reads_the_loud_part_and_reports_real_stereo_facts() {
        let rate = 48_000;
        // Two seconds: near-silence first, then a loud correlated tone. A
        // mean over the whole thing would call this quiet; the loudest
        // window is what a mastering read wants.
        let x = stereo(rate * 2, |i| {
            let t = i as f32 / rate as f32;
            let a = if i < rate { 0.0005 } else { 0.5 };
            let s = (t * 220.0 * std::f32::consts::TAU).sin() * a;
            (s, s)
        });
        let (_, m) = measure(&x, rate as u32).expect("measures");
        assert!(m.peak_db > -12.0, "found the loud second, not the quiet one: {}", m.peak_db);
        assert!(m.correlation > 0.95, "a mono-identical mix correlates: {}", m.correlation);
        // An out-of-phase mix must report the risk rather than average it away.
        let flipped = stereo(rate, |i| {
            let t = i as f32 / rate as f32;
            let s = (t * 220.0 * std::f32::consts::TAU).sin() * 0.5;
            (s, -s)
        });
        let (_, m2) = measure(&flipped, rate as u32).unwrap();
        assert!(m2.correlation < -0.9, "phase inversion is visible: {}", m2.correlation);
        assert!(measure(&vec![0.0; 4800], 48_000).is_none(),
                "silence has nothing to say, and says so as None");
    }

    #[test]
    fn composing_extends_the_users_chain_instead_of_deleting_it() {
        // The bug this function exists to prevent: mix_pass emits one
        // standalone SetBusChain per move, so applying them verbatim would
        // wipe the user's own effects and then each other, leaving only the
        // last move.
        let mine = Effect { kind: "reverb".into(), params: BTreeMap::new(), enabled: true };
        let eq = |hz: f32| Op::SetBusChain { id: BusId(1), chain: vec![
            Effect { kind: "eq".into(),
                     params: [("freq0".to_string(), hz)].into_iter().collect(),
                     enabled: true } ] };
        let composed = compose(&[mine.clone()], &[
            eq(250.0), eq(3000.0),
            Op::SetBusGain { id: BusId(1), gain: 0.9 },
        ], BusId(1));
        // One chain op carrying everything, plus the gain op untouched.
        assert_eq!(composed.len(), 2);
        let Op::SetBusChain { chain, .. } = &composed[0] else { panic!("chain first") };
        assert_eq!(chain.len(), 3, "the user's reverb survived and both EQs landed");
        assert_eq!(chain[0], mine, "and the user's effect keeps its place at the head");
        assert!(matches!(composed[1], Op::SetBusGain { .. }));
        // No chain moves: nothing invented.
        let only_gain = compose(&[mine], &[Op::SetBusGain { id: BusId(1), gain: 0.5 }], BusId(1));
        assert_eq!(only_gain.len(), 1);
        assert!(matches!(only_gain[0], Op::SetBusGain { .. }));
    }
}
