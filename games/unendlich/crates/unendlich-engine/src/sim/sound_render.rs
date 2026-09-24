//! `sim::sound_render` — play a dry mono sound through a `land_echo` plan.
//!
//! Every path (direct + each reflection) is the dry signal, low-passed to that
//! path's cutoff, placed at its arrival time with its left/right gains. The
//! tail is a small stereo Schroeder reverb (four combs per ear with different
//! lengths so the ears decorrelate, then two all-passes) fed from the direct
//! path and tuned to the plan's decay.
//!
//! Output is interleaved stereo f32, starting at emission time: a roar 170 m
//! away begins with half a second of silence, because that is when it arrives.
//! A player hears the dragon open its jaws before the sound reaches them.
//!
//! Pure. Cost is `O(len × (paths + 12))`; a 3 s roar with six echoes at 24 kHz
//! is ~1.5 M multiply-adds. Callers on a phone render off the frame thread.

use crate::sim::land_echo::EchoPlan;
use crate::sim::roar_synth::one_pole_coeff;

/// Samples of silence cut from the front: if `trim_lead` is set, the output
/// starts `lead_keep_s` before the direct sound instead of at emission. The
/// game keeps the lead (arrival delay is the point); tests and the render
/// tool can trim it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderOpts {
    pub trim_lead: bool,
    pub lead_keep_s: f32,
    /// Master gain on everything.
    pub gain: f32,
    /// Silence before emission (s): a caller staggering several sounds that
    /// were triggered together (a pack roaring at once) delays the later ones
    /// so they answer each other instead of stacking.
    pub pre_delay_s: f32,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts { trim_lead: false, lead_keep_s: 0.0, gain: 1.0, pre_delay_s: 0.0 }
    }
}

fn lowpassed(dry: &[f32], hz: f32, rate: f32) -> Vec<f32> {
    let k = one_pole_coeff(hz, rate);
    let mut y = 0.0f32;
    // Two poles in series: a one-pole alone barely dulls anything.
    let mut y2 = 0.0f32;
    dry.iter()
        .map(|&x| {
            y += (x - y) * k;
            y2 += (y - y2) * k;
            y2
        })
        .collect()
}

struct Comb {
    buf: Vec<f32>,
    i: usize,
    fb: f32,
    damp: f32,
    lp: f32,
}
impl Comb {
    fn new(len: usize, fb: f32, damp: f32) -> Comb {
        Comb { buf: vec![0.0; len.max(1)], i: 0, fb, damp, lp: 0.0 }
    }
    fn run(&mut self, x: f32) -> f32 {
        let y = self.buf[self.i];
        self.lp = y + (self.lp - y) * self.damp;
        self.buf[self.i] = x + self.lp * self.fb;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    i: usize,
}
impl Allpass {
    fn new(len: usize) -> Allpass {
        Allpass { buf: vec![0.0; len.max(1)], i: 0 }
    }
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        let y = -x + b;
        self.buf[self.i] = x + b * 0.5;
        self.i = (self.i + 1) % self.buf.len();
        y
    }
}

/// Render `dry` (mono, `rate` Hz) through `plan`. Interleaved stereo f32.
pub fn render(dry: &[f32], rate: u32, plan: &EchoPlan, opts: RenderOpts) -> Vec<f32> {
    let rate_f = rate.max(8000) as f32;
    let at = |s: f32| (s.max(0.0) * rate_f) as usize;
    let last_path = plan
        .reflections
        .iter()
        .map(|r| r.delay_s)
        .fold(plan.direct.delay_s, f32::max);
    let tail_s = if plan.tail.level > 1e-5 { plan.tail.decay_s.clamp(0.1, 6.0) } else { 0.0 };
    let frames = at(last_path) + dry.len() + at(tail_s) + 1;
    let mut out = vec![0.0f32; frames * 2];

    let mut add = |path: &crate::sim::land_echo::Path| {
        let sig = lowpassed(dry, path.lowpass_hz, rate_f);
        let o = at(path.delay_s);
        for (j, s) in sig.iter().enumerate() {
            let f = o + j;
            out[f * 2] += s * path.left;
            out[f * 2 + 1] += s * path.right;
        }
    };
    add(&plan.direct);
    for r in &plan.reflections {
        add(r);
    }

    // Tail: stereo Schroeder fed by the direct path, dulled.
    if tail_s > 0.0 {
        let feed = lowpassed(dry, plan.direct.lowpass_hz.min(3500.0), rate_f);
        let scale = rate_f / 44_100.0;
        let lens_l = [1557, 1617, 1491, 1422];
        let lens_r = [1580, 1640, 1514, 1445];
        let mk = |lens: [usize; 4]| -> Vec<Comb> {
            lens.iter()
                .map(|&l| {
                    let len = ((l as f32 * scale) as usize).max(8);
                    // RT60: gain per pass so the loop falls 60 dB in decay_s.
                    let fb = 10f32.powf(-3.0 * (len as f32 / rate_f) / tail_s).min(0.97);
                    Comb::new(len, fb, 0.3)
                })
                .collect()
        };
        let (mut cl, mut cr) = (mk(lens_l), mk(lens_r));
        let mut al = [Allpass::new((556.0 * scale) as usize), Allpass::new((225.0 * scale) as usize)];
        let mut ar = [Allpass::new((579.0 * scale) as usize), Allpass::new((248.0 * scale) as usize)];
        let o = at(plan.direct.delay_s);
        let n = frames - o;
        let lvl = plan.tail.level * 0.25;
        for j in 0..n {
            let x = feed.get(j).copied().unwrap_or(0.0);
            let mut l: f32 = cl.iter_mut().map(|c| c.run(x)).sum();
            let mut r: f32 = cr.iter_mut().map(|c| c.run(x)).sum();
            for a in al.iter_mut() {
                l = a.run(l);
            }
            for a in ar.iter_mut() {
                r = a.run(r);
            }
            out[(o + j) * 2] += l * lvl;
            out[(o + j) * 2 + 1] += r * lvl;
        }
    }

    let g = if opts.gain.is_finite() { opts.gain.max(0.0) } else { 0.0 };
    for s in out.iter_mut() {
        // Soft knee above 0.8: a roar next to you saturates, never wraps.
        let x = *s * g;
        let a = x.abs();
        *s = if a <= 0.8 { x } else { x.signum() * (0.8 + 0.2 * ((a - 0.8) / 0.2).tanh()) };
    }
    if opts.trim_lead {
        let cut = at(plan.direct.delay_s - opts.lead_keep_s).min(frames);
        out.drain(..cut * 2);
    }
    let pre = at(if opts.pre_delay_s.is_finite() { opts.pre_delay_s.min(10.0) } else { 0.0 });
    if pre > 0 {
        out.splice(0..0, std::iter::repeat(0.0).take(pre * 2));
    }
    out
}

/// Staggers sounds triggered together so they answer each other: each new
/// one waits until `gap` after the previous one's start. A request that would
/// wait longer than `max_wait` is refused (the chorus is already full).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stagger {
    free_at: f32,
}

impl Stagger {
    /// `now` on any monotonic clock (s). Returns the pre-delay to use.
    pub fn slot(&mut self, now: f32, gap: f32, max_wait: f32) -> Option<f32> {
        let wait = (self.free_at - now).max(0.0);
        if wait > max_wait {
            return None;
        }
        self.free_at = now + wait + gap.max(0.0);
        Some(wait)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::land_echo::{Path, Tail};

    const R: u32 = 24_000;

    fn click() -> Vec<f32> {
        let mut v = vec![0.0; 240];
        v[0] = 1.0;
        v
    }

    fn plan(direct_delay: f32, echoes: Vec<Path>, tail: f32) -> EchoPlan {
        EchoPlan {
            direct: Path { delay_s: direct_delay, left: 0.5, right: 0.5, lowpass_hz: 20_000.0 },
            occluded: false,
            ridge_m: 0.0,
            reflections: echoes,
            tail: Tail { level: tail, decay_s: 1.0, enclosure: 0.0 },
            distance: direct_delay * 343.0,
        }
    }

    /// Frame index of the first sample with |x| above `thr` on a channel.
    fn onset(out: &[f32], ch: usize, from: usize, thr: f32) -> Option<usize> {
        (from..out.len() / 2).find(|&f| out[f * 2 + ch].abs() > thr)
    }

    #[test]
    fn the_direct_sound_arrives_at_its_delay() {
        let out = render(&click(), R, &plan(0.25, vec![], 0.0), RenderOpts::default());
        let f = onset(&out, 0, 0, 1e-3).unwrap();
        assert!((f as i64 - (0.25 * R as f32) as i64).abs() <= 2, "onset {f}");
    }

    #[test]
    fn an_echo_lands_later_on_its_side() {
        let echo = Path { delay_s: 0.9, left: 0.0, right: 0.3, lowpass_hz: 20_000.0 };
        let out = render(&click(), R, &plan(0.1, vec![echo], 0.0), RenderOpts::default());
        let from = (0.5 * R as f32) as usize;
        let fr = onset(&out, 1, from, 1e-3).unwrap();
        assert!((fr as i64 - (0.9 * R as f32) as i64).abs() <= 2);
        assert!(onset(&out, 0, from, 1e-3).is_none(), "the echo is hard right");
    }

    #[test]
    fn lowpass_dulls_a_path() {
        let noise: Vec<f32> = (0..4800).map(|i| if (i * 7919) % 13 < 6 { 0.5 } else { -0.5 }).collect();
        let bright = Path { delay_s: 0.0, left: 1.0, right: 1.0, lowpass_hz: 18_000.0 };
        let dull = Path { lowpass_hz: 300.0, ..bright };
        let mut pb = plan(0.0, vec![], 0.0);
        pb.direct = bright;
        let mut pd = pb.clone();
        pd.direct = dull;
        let zc = |x: &[f32]| x.chunks(2).collect::<Vec<_>>().windows(2).filter(|w| (w[0][0] >= 0.0) != (w[1][0] >= 0.0)).count();
        let b = render(&noise, R, &pb, RenderOpts::default());
        let d = render(&noise, R, &pd, RenderOpts::default());
        assert!(zc(&d) * 3 < zc(&b), "dull {} bright {}", zc(&d), zc(&b));
    }

    #[test]
    fn the_tail_rings_after_the_sound_and_decays() {
        let dry: Vec<f32> = (0..2400).map(|i| ((i as f32) * 0.3).sin() * 0.5).collect();
        let out = render(&dry, R, &plan(0.0, vec![], 0.5), RenderOpts::default());
        let rms = |a: usize, b: usize| {
            let s = &out[a * 2..b * 2];
            (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
        };
        let early = rms(2400 + 1200, 2400 + 3600); // right after the sound ends
        let late = rms(2400 + 14_000, 2400 + 16_400); // ~0.6 s later
        assert!(early > 1e-4, "no tail at all");
        assert!(late < early * 0.5, "tail did not decay: {early} -> {late}");
        // Ears decorrelated: the tail's L and R differ.
        let diff: f32 = (2400 + 1200..2400 + 3600).map(|f| (out[f * 2] - out[f * 2 + 1]).abs()).sum();
        assert!(diff > 1e-3);
    }

    #[test]
    fn no_tail_means_no_ringing() {
        let out = render(&click(), R, &plan(0.0, vec![], 0.0), RenderOpts::default());
        assert!(out.len() / 2 <= 240 + 2);
    }

    #[test]
    fn trim_lead_keeps_a_little_before_arrival() {
        let o = RenderOpts { trim_lead: true, lead_keep_s: 0.05, ..RenderOpts::default() };
        let out = render(&click(), R, &plan(0.5, vec![], 0.0), o);
        let f = onset(&out, 0, 0, 1e-3).unwrap();
        assert!((f as i64 - (0.05 * R as f32) as i64).abs() <= 2);
    }

    #[test]
    fn pre_delay_shifts_everything_later() {
        let o = RenderOpts { pre_delay_s: 0.4, ..RenderOpts::default() };
        let out = render(&click(), R, &plan(0.1, vec![], 0.0), o);
        let f = onset(&out, 0, 0, 1e-3).unwrap();
        assert!((f as i64 - (0.5 * R as f32) as i64).abs() <= 2, "onset {f}");
    }

    #[test]
    fn stagger_spaces_a_burst_and_caps_it() {
        let mut st = Stagger::default();
        let waits: Vec<Option<f32>> = (0..6).map(|_| st.slot(10.0, 0.9, 3.0)).collect();
        assert_eq!(waits[0], Some(0.0));
        assert!((waits[1].unwrap() - 0.9).abs() < 1e-5);
        assert!((waits[3].unwrap() - 2.7).abs() < 1e-5);
        assert_eq!(waits[4], None, "a fifth in the same instant would wait 3.6 s");
        // Once the burst has passed, the next is immediate.
        assert_eq!(st.slot(20.0, 0.9, 3.0), Some(0.0));
    }

    #[test]
    fn output_is_bounded_even_when_hot() {
        let loud = vec![1.0f32; 2400];
        let mut p = plan(0.0, vec![], 0.0);
        p.direct.left = 5.0;
        p.direct.right = 5.0;
        let out = render(&loud, R, &p, RenderOpts { gain: 4.0, ..RenderOpts::default() });
        assert!(out.iter().all(|s| s.is_finite() && s.abs() <= 1.0 + 1e-6));
    }

}
