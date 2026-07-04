//! audiomix-kit — pure audio channel routing + stream mixing.
//!
//! Extracted and generalized from bo-suite's soundscape router (bo-mux's channel_layout / mux),
//! so the mechanism can be shared across the fleet — voiceterm's audio, snapclone's sessions,
//! ambient-learner, av1tuner — instead of living inside one app. Everything here is pure f32
//! sample math: no hardware, no codec (Opus lives in the feature-gated `opus-kit`).
//!
//! Three pieces: [`Layout`]/[`Channel`] (what channels a stream has), [`RouteMatrix`] (map an
//! input layout to an output layout with per-pair gains, including a standard downmix), and the
//! [`mix`] helpers (merge many streams into one).

use std::collections::HashMap;

// ── channels + layouts ───────────────────────────────────────────────────────

/// A speaker channel (canonical set, mirrors bo-suite).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// Front-left
    FL,
    /// Front-right
    FR,
    /// Front-center (dialog / lead)
    C,
    /// Low-frequency effects
    LFE,
    /// Rear-left
    RL,
    /// Rear-right
    RR,
    /// Side-left
    SL,
    /// Side-right
    SR,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::FL => "FL",
            Channel::FR => "FR",
            Channel::C => "C",
            Channel::LFE => "LFE",
            Channel::RL => "RL",
            Channel::RR => "RR",
            Channel::SL => "SL",
            Channel::SR => "SR",
        }
    }
    pub fn parse(s: &str) -> Option<Channel> {
        Some(match s {
            "FL" => Channel::FL,
            "FR" => Channel::FR,
            "C" => Channel::C,
            "LFE" => Channel::LFE,
            "RL" => Channel::RL,
            "RR" => Channel::RR,
            "SL" => Channel::SL,
            "SR" => Channel::SR,
            _ => return None,
        })
    }
    /// Stereo side: -1 = left, 0 = center/LFE, +1 = right.
    pub fn side(self) -> i8 {
        match self {
            Channel::FL | Channel::RL | Channel::SL => -1,
            Channel::FR | Channel::RR | Channel::SR => 1,
            Channel::C | Channel::LFE => 0,
        }
    }
}

/// A channel layout — the ordered set of channels in a stream/frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Mono,
    Stereo,
    Quad,
    FivePointOne,
    SevenPointOne,
}

impl Layout {
    /// Channels in canonical order (a frame is one sample per channel, this order).
    pub fn channels(self) -> Vec<Channel> {
        use Channel::*;
        match self {
            Layout::Mono => vec![C],
            Layout::Stereo => vec![FL, FR],
            Layout::Quad => vec![FL, FR, RL, RR],
            Layout::FivePointOne => vec![FL, FR, C, LFE, RL, RR],
            Layout::SevenPointOne => vec![FL, FR, C, LFE, RL, RR, SL, SR],
        }
    }
    pub fn count(self) -> usize {
        self.channels().len()
    }
    pub fn contains(self, ch: Channel) -> bool {
        self.channels().contains(&ch)
    }
    pub fn label(self) -> &'static str {
        match self {
            Layout::Mono => "Mono",
            Layout::Stereo => "Stereo",
            Layout::Quad => "Quad",
            Layout::FivePointOne => "5.1",
            Layout::SevenPointOne => "7.1",
        }
    }
    pub fn parse(s: &str) -> Option<Layout> {
        Some(match s {
            "Mono" | "mono" | "1.0" => Layout::Mono,
            "Stereo" | "stereo" | "2.0" => Layout::Stereo,
            "Quad" | "quad" | "4.0" => Layout::Quad,
            "5.1" | "FivePointOne" => Layout::FivePointOne,
            "7.1" | "SevenPointOne" => Layout::SevenPointOne,
            _ => return None,
        })
    }
    /// Index of a channel within this layout, if present.
    pub fn index_of(self, ch: Channel) -> Option<usize> {
        self.channels().iter().position(|&c| c == ch)
    }
}

// ── routing matrix ───────────────────────────────────────────────────────────

/// The standard −3 dB pan/fold coefficient.
const FOLD: f32 = 0.707_106_77;

/// The gain for routing input channel `src` into output channel `dst` in a standard downmix.
fn downmix_gain(src: Channel, dst: Channel) -> f32 {
    use Channel::*;
    if src == dst {
        return 1.0;
    }
    match (src, dst) {
        // center folds into both fronts
        (C, FL) | (C, FR) => FOLD,
        // rears/sides fold into same-side front
        (RL, FL) | (SL, FL) => FOLD,
        (RR, FR) | (SR, FR) => FOLD,
        // LFE is dropped unless the output has its own LFE (handled by src==dst above)
        (LFE, _) => 0.0,
        // anything into a mono centre
        (_, C) => 0.5,
        _ => 0.0,
    }
}

/// A routing matrix mapping an input layout to an output layout. `gains[o][i]` is the gain
/// applied from input channel `i` to output channel `o`. Generalizes bo-suite's
/// `ResolvedRouting`; [`apply`](RouteMatrix::apply) routes one frame.
#[derive(Debug, Clone)]
pub struct RouteMatrix {
    pub input: Layout,
    pub output: Layout,
    gains: Vec<Vec<f32>>, // [out][in]
}

impl RouteMatrix {
    /// A zero matrix (silence) for the given layouts.
    pub fn zeros(input: Layout, output: Layout) -> RouteMatrix {
        RouteMatrix { input, output, gains: vec![vec![0.0; input.count()]; output.count()] }
    }

    /// Pass-through: each channel present in both layouts routes 1:1.
    pub fn identity(layout: Layout) -> RouteMatrix {
        let mut m = RouteMatrix::zeros(layout, layout);
        for i in 0..layout.count() {
            m.gains[i][i] = 1.0;
        }
        m
    }

    /// A standard downmix/upmix from `input` to `output` (5.1→stereo, stereo→mono, quad→stereo,
    /// stereo→5.1 fronts, …) using conventional fold coefficients.
    pub fn downmix(input: Layout, output: Layout) -> RouteMatrix {
        let mut m = RouteMatrix::zeros(input, output);
        let ins = input.channels();
        let outs = output.channels();
        for (oi, &oc) in outs.iter().enumerate() {
            for (ii, &ic) in ins.iter().enumerate() {
                let g = downmix_gain(ic, oc);
                if g != 0.0 {
                    m.gains[oi][ii] += g;
                }
            }
        }
        m
    }

    /// Set the gain from an input channel to an output channel (no-op if either is absent).
    pub fn set_gain(&mut self, from: Channel, to: Channel, gain: f32) -> bool {
        match (self.input.index_of(from), self.output.index_of(to)) {
            (Some(i), Some(o)) => {
                self.gains[o][i] = gain;
                true
            }
            _ => false,
        }
    }

    pub fn gain(&self, from: Channel, to: Channel) -> f32 {
        match (self.input.index_of(from), self.output.index_of(to)) {
            (Some(i), Some(o)) => self.gains[o][i],
            _ => 0.0,
        }
    }

    /// Route one frame (`input.count()` samples, in the input layout's channel order) to an
    /// output frame (`output.count()` samples).
    pub fn apply(&self, frame_in: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; self.output.count()];
        for (o, row) in self.gains.iter().enumerate() {
            let mut acc = 0.0;
            for (i, &g) in row.iter().enumerate() {
                acc += g * frame_in.get(i).copied().unwrap_or(0.0);
            }
            out[o] = acc;
        }
        out
    }

    /// As a channel→gains map for one output channel (interop with bo-suite's ResolvedRouting).
    pub fn contributions_to(&self, out_ch: Channel) -> HashMap<Channel, f32> {
        let mut m = HashMap::new();
        if let Some(o) = self.output.index_of(out_ch) {
            for (i, ic) in self.input.channels().iter().enumerate() {
                let g = self.gains[o][i];
                if g != 0.0 {
                    m.insert(*ic, g);
                }
            }
        }
        m
    }
}

// ── stream mixing / merging ──────────────────────────────────────────────────

/// Soft-clip a sample to (−1, 1) — gentle saturation instead of a hard clamp, so summed
/// streams don't click when they briefly exceed full scale.
pub fn soft_clip(x: f32) -> f32 {
    // rational approximation of tanh, cheap and monotonic
    let x = x.clamp(-3.0, 3.0);
    x * (27.0 + x * x) / (27.0 + 9.0 * x * x)
}

/// Mix `stream` into `acc` in place with `gain` (lengths need not match; the overlap is summed).
pub fn mix_into(acc: &mut [f32], stream: &[f32], gain: f32) {
    for (a, s) in acc.iter_mut().zip(stream.iter()) {
        *a += s * gain;
    }
}

/// Merge several equal-length mono streams with per-stream gains into one, soft-clipped.
/// (Stream merging — the "combine sources into a channel" primitive.)
pub fn mix(streams: &[&[f32]], gains: &[f32]) -> Vec<f32> {
    let len = streams.iter().map(|s| s.len()).max().unwrap_or(0);
    let mut out = vec![0.0f32; len];
    for (si, s) in streams.iter().enumerate() {
        let g = gains.get(si).copied().unwrap_or(1.0);
        mix_into(&mut out, s, g);
    }
    for v in &mut out {
        *v = soft_clip(*v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_report_channels() {
        assert_eq!(Layout::Stereo.count(), 2);
        assert_eq!(Layout::Stereo.channels(), vec![Channel::FL, Channel::FR]);
        assert_eq!(Layout::FivePointOne.count(), 6);
        assert!(Layout::FivePointOne.contains(Channel::LFE));
        assert!(!Layout::Stereo.contains(Channel::C));
        assert_eq!(Layout::parse("5.1"), Some(Layout::FivePointOne));
    }

    #[test]
    fn identity_passes_through() {
        let m = RouteMatrix::identity(Layout::Stereo);
        assert_eq!(m.apply(&[0.5, 0.8]), vec![0.5, 0.8]);
    }

    #[test]
    fn downmix_51_to_stereo_folds_center_and_rears() {
        let m = RouteMatrix::downmix(Layout::FivePointOne, Layout::Stereo);
        // frame order FL,FR,C,LFE,RL,RR
        let out = m.apply(&[1.0, 0.0, 1.0, 1.0, 1.0, 0.0]);
        // L = FL(1) + C(0.707) + RL(0.707); LFE dropped
        assert!((out[0] - (1.0 + FOLD + FOLD)).abs() < 1e-4);
        // R = FR(0) + C(0.707) + RR(0) = 0.707
        assert!((out[1] - FOLD).abs() < 1e-4);
        // LFE contributes to nothing
        assert_eq!(m.gain(Channel::LFE, Channel::FL), 0.0);
    }

    #[test]
    fn downmix_stereo_to_mono_averages() {
        let m = RouteMatrix::downmix(Layout::Stereo, Layout::Mono);
        let out = m.apply(&[1.0, 0.0]); // FL=1, FR=0
        assert!((out[0] - 0.5).abs() < 1e-6, "mono = 0.5*(L+R)");
    }

    #[test]
    fn set_gain_and_contributions() {
        let mut m = RouteMatrix::zeros(Layout::Stereo, Layout::Stereo);
        assert!(m.set_gain(Channel::FL, Channel::FR, 0.25));
        assert_eq!(m.gain(Channel::FL, Channel::FR), 0.25);
        let c = m.contributions_to(Channel::FR);
        assert_eq!(c.get(&Channel::FL), Some(&0.25));
        // absent channel → no-op
        assert!(!m.set_gain(Channel::C, Channel::FR, 1.0));
    }

    #[test]
    fn mixing_sums_streams_and_clips() {
        let a = [0.5, 0.5, 0.5];
        let b = [0.5, -0.5, 0.5];
        let out = mix(&[&a, &b], &[1.0, 1.0]);
        assert!((out[0] - soft_clip(1.0)).abs() < 1e-6);
        assert!((out[1] - 0.0).abs() < 1e-6);
        // soft clip keeps things in range even when summed hot
        let hot = mix(&[&[2.0f32][..], &[2.0f32][..]], &[1.0, 1.0]);
        assert!(hot[0] <= 1.0 && hot[0] > 0.5, "summed-hot stays in range (got {})", hot[0]);
    }

    #[test]
    fn mix_into_accumulates_with_gain() {
        let mut acc = [1.0, 1.0];
        mix_into(&mut acc, &[1.0, 1.0], 0.5);
        assert_eq!(acc, [1.5, 1.5]);
    }
}
