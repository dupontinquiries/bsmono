//! format — channel layouts, and the matrices between them.
//!
//! A studio that only knows stereo cannot deliver a 5.1 stem or fold a quad
//! bus into the mix bus. This module is the algebra for both directions:
//! which speakers a layout has, how a wider layout FOLDS to stereo (the
//! operation a `5.1 bus → 2.0 mix bus → 2.0 master` chain performs), and how
//! stereo EXPANDS to a wider one for delivery.
//!
//! ## Scope, honestly
//!
//! `mixdown` renders stereo internally — its buffers are interleaved pairs
//! in a dozen places, and widening them is a real refactor of the most
//! heavily tested code in this workspace, not a parameter. So today this
//! module serves the EXPORT stage (render stereo, deliver in any layout) and
//! stands as the tested foundation the future N-channel mixdown will fold
//! its busses with — the same matrices, applied one layer earlier. What it
//! does NOT do is let the model claim a bus is 5.1 while the renderer sums
//! it in stereo; a declared format nothing honours is worse than none.
//!
//! ## Coefficients
//!
//! The fold uses the ITU-R BS.775 coefficients every consumer decoder
//! implements: centre and surrounds at -3 dB (0.707) into both sides,
//! surrounds carried at that level rather than dropped, and LFE excluded
//! from the fold by default — the LFE channel is a bass-management artefact
//! and folding it back in double-counts energy already present in the mains.

use std::fmt;

/// A speaker layout. Ordering follows the WAV/ITU convention (L, R, C, LFE,
/// then surrounds), which is what every decoder and player expects — a
/// correct mix in the wrong channel order is a broken mix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChannelFormat {
    Mono,
    #[default]
    Stereo,
    /// L R LFE.
    Stereo21,
    /// L R Ls Rs — quad, no centre, no LFE.
    Quad40,
    /// L R C LFE Ls Rs.
    Surround51,
    /// L R C LFE Ls Rs Lb Rb.
    Surround71,
}

impl ChannelFormat {
    pub const ALL: [ChannelFormat; 6] = [
        ChannelFormat::Mono, ChannelFormat::Stereo, ChannelFormat::Stereo21,
        ChannelFormat::Quad40, ChannelFormat::Surround51, ChannelFormat::Surround71,
    ];

    pub fn channels(self) -> usize {
        match self {
            ChannelFormat::Mono => 1,
            ChannelFormat::Stereo => 2,
            ChannelFormat::Stereo21 => 3,
            ChannelFormat::Quad40 => 4,
            ChannelFormat::Surround51 => 6,
            ChannelFormat::Surround71 => 8,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ChannelFormat::Mono => "1.0",
            ChannelFormat::Stereo => "2.0",
            ChannelFormat::Stereo21 => "2.1",
            ChannelFormat::Quad40 => "4.0",
            ChannelFormat::Surround51 => "5.1",
            ChannelFormat::Surround71 => "7.1",
        }
    }

    /// Index of the LFE channel, if the layout has one.
    pub fn lfe_index(self) -> Option<usize> {
        match self {
            ChannelFormat::Stereo21 => Some(2),
            ChannelFormat::Surround51 | ChannelFormat::Surround71 => Some(3),
            _ => None,
        }
    }

    /// Per-channel (left, right) fold gains — the matrix that collapses this
    /// layout to stereo. LFE folds at zero; see the module docs.
    pub fn fold_gains(self) -> Vec<(f32, f32)> {
        const M3DB: f32 = std::f32::consts::FRAC_1_SQRT_2;
        match self {
            ChannelFormat::Mono => vec![(M3DB, M3DB)],
            ChannelFormat::Stereo => vec![(1.0, 0.0), (0.0, 1.0)],
            ChannelFormat::Stereo21 => vec![(1.0, 0.0), (0.0, 1.0), (0.0, 0.0)],
            ChannelFormat::Quad40 => vec![
                (1.0, 0.0), (0.0, 1.0), (M3DB, 0.0), (0.0, M3DB),
            ],
            ChannelFormat::Surround51 => vec![
                (1.0, 0.0), (0.0, 1.0), (M3DB, M3DB), (0.0, 0.0),
                (M3DB, 0.0), (0.0, M3DB),
            ],
            ChannelFormat::Surround71 => vec![
                (1.0, 0.0), (0.0, 1.0), (M3DB, M3DB), (0.0, 0.0),
                (M3DB, 0.0), (0.0, M3DB), (M3DB, 0.0), (0.0, M3DB),
            ],
        }
    }
}

impl fmt::Display for ChannelFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Fold interleaved `from`-layout audio down to interleaved stereo.
///
/// This is the `wide bus → 2.0 mix bus` operation. Gains are NOT normalised
/// afterwards: a fold that quietly renormalises makes A/B against the stereo
/// mix a lie, and the standard coefficients are chosen to sit right.
pub fn fold_to_stereo(samples: &[f32], from: ChannelFormat) -> Vec<f32> {
    let n = from.channels();
    if n == 2 { return samples.to_vec(); }
    let gains = from.fold_gains();
    let frames = samples.len() / n;
    let mut out = Vec::with_capacity(frames * 2);
    for f in 0..frames {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for (c, (gl, gr)) in gains.iter().enumerate() {
            let s = samples[f * n + c];
            l += s * gl;
            r += s * gr;
        }
        out.push(l);
        out.push(r);
    }
    out
}

/// Expand interleaved stereo into `to`, for delivery.
///
/// Deliberately conservative: mains stay where they are, the centre gets the
/// correlated (mono) content at -3 dB with that much removed from the sides
/// so the phantom centre is not doubled, surrounds get the DIFFERENCE signal
/// (the classic matrix trick — decorrelated content is what belongs behind
/// you) at a modest level, and LFE is a low-passed sum. This is a delivery
/// convenience, not a mixing decision: a real surround mix is authored, and
/// this module's docs should be believed over any marketing word like
/// "upmix".
pub fn expand_from_stereo(samples: &[f32], to: ChannelFormat, rate: u32) -> Vec<f32> {
    let n = to.channels();
    if n == 2 { return samples.to_vec(); }
    const M3DB: f32 = std::f32::consts::FRAC_1_SQRT_2;
    let frames = samples.len() / 2;
    let mut out = vec![0.0f32; frames * n];
    // LFE: 120 Hz low-passed sum, the standard bass-management corner.
    let mut lfe_filter = studio_fx::Biquad::design(
        studio_fx::BandKind::LowPass, rate as f32, 120.0, 0.707, 0.0);
    let has_centre = matches!(to, ChannelFormat::Surround51 | ChannelFormat::Surround71);
    for f in 0..frames {
        let l = samples[f * 2];
        let r = samples[f * 2 + 1];
        let mid = (l + r) * 0.5;
        let side = (l - r) * 0.5;
        let base = f * n;
        if has_centre {
            // Centre takes the correlated content; the mains give up the
            // same amount, so summing L+C+R reproduces the original energy
            // instead of a louder, centre-heavy version of it.
            let c = mid * M3DB;
            out[base] = l - c * 0.5;
            out[base + 1] = r - c * 0.5;
            out[base + 2] = c;
        } else {
            out[base] = l;
            out[base + 1] = r;
        }
        if let Some(i) = to.lfe_index() {
            out[base + i] = lfe_filter.process(mid);
        }
        // Surrounds: the difference signal, quietly, out of phase per side.
        let surround_at = match to {
            ChannelFormat::Quad40 => Some(2),
            ChannelFormat::Surround51 | ChannelFormat::Surround71 => Some(4),
            _ => None,
        };
        if let Some(s) = surround_at {
            out[base + s] = side * 0.5;
            out[base + s + 1] = -side * 0.5;
            if to == ChannelFormat::Surround71 {
                // Rears repeat the sides at a further -3 dB: without real
                // rear content, pretending otherwise would invent detail.
                out[base + 6] = side * 0.5 * M3DB;
                out[base + 7] = -side * 0.5 * M3DB;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(frames: usize, l: f32, r: f32) -> Vec<f32> {
        (0..frames).flat_map(|_| [l, r]).collect()
    }

    #[test]
    fn every_layout_reports_the_channel_count_its_name_promises() {
        assert_eq!(ChannelFormat::Stereo21.channels(), 3);
        assert_eq!(ChannelFormat::Quad40.channels(), 4);
        assert_eq!(ChannelFormat::Surround51.channels(), 6);
        assert_eq!(ChannelFormat::Surround71.channels(), 8);
        // And the fold matrix covers every channel — a short matrix would
        // silently drop the last speaker on the way down.
        for f in ChannelFormat::ALL {
            assert_eq!(f.fold_gains().len(), f.channels(), "{f} matrix is short");
        }
    }

    #[test]
    fn folding_stereo_is_the_identity_and_costs_nothing() {
        let s = stereo(64, 0.3, -0.7);
        assert_eq!(fold_to_stereo(&s, ChannelFormat::Stereo), s);
        assert_eq!(expand_from_stereo(&s, ChannelFormat::Stereo, 48_000), s);
    }

    #[test]
    fn a_five_one_fold_puts_centre_in_both_sides_and_leaves_lfe_out() {
        // One frame, one channel hot at a time, so the matrix is read
        // directly rather than inferred from a mix.
        let probe = |c: usize| {
            let mut fr = vec![0.0f32; 6];
            fr[c] = 1.0;
            let out = fold_to_stereo(&fr, ChannelFormat::Surround51);
            (out[0], out[1])
        };
        assert_eq!(probe(0), (1.0, 0.0), "L stays left");
        assert_eq!(probe(1), (0.0, 1.0), "R stays right");
        let (cl, cr) = probe(2);
        assert!((cl - 0.707).abs() < 0.001 && (cr - 0.707).abs() < 0.001,
                "centre folds -3dB into both: {cl},{cr}");
        assert_eq!(probe(3), (0.0, 0.0), "LFE is excluded — it double-counts bass");
        let (sl, _) = probe(4);
        assert!((sl - 0.707).abs() < 0.001, "surrounds are carried, not dropped");
    }

    #[test]
    fn expanding_then_folding_returns_the_music_not_a_smear() {
        // The round trip a delivery workflow actually performs: author in
        // stereo, expand for a 5.1 deliverable, and let a consumer decoder
        // fold it back. The result must resemble the original — the centre
        // compensation exists exactly so this does not come back loud and
        // mono-heavy.
        let src: Vec<f32> = (0..2000).flat_map(|i| {
            let t = i as f32 / 480.0;
            [(t).sin() * 0.5, (t * 1.5).sin() * 0.5] // decorrelated-ish
        }).collect();
        let wide = expand_from_stereo(&src, ChannelFormat::Surround51, 48_000);
        assert_eq!(wide.len(), src.len() / 2 * 6);
        let back = fold_to_stereo(&wide, ChannelFormat::Surround51);
        assert_eq!(back.len(), src.len());
        let energy = |v: &[f32]| v.iter().map(|s| s * s).sum::<f32>().sqrt();
        let (a, b) = (energy(&src), energy(&back));
        assert!((b / a - 1.0).abs() < 0.35,
                "round trip stays in the same ballpark: {a} then {b}");
        // And it stays STEREO: a fold that collapsed the image would make
        // both sides equal.
        let l: f32 = back.chunks(2).map(|f| f[0].abs()).sum();
        let r: f32 = back.chunks(2).map(|f| f[1].abs()).sum();
        let width: f32 = back.chunks(2).map(|f| (f[0] - f[1]).abs()).sum();
        assert!(width > 0.05 * (l + r), "the image survived: width {width}");
    }

    #[test]
    fn lfe_carries_bass_and_not_treble() {
        // A 4 kHz tone must not reach the subwoofer; a 40 Hz one must.
        let tone = |hz: f32| -> Vec<f32> {
            (0..4800).flat_map(|i| {
                let v = (i as f32 / 48_000.0 * hz * std::f32::consts::TAU).sin() * 0.8;
                [v, v]
            }).collect::<Vec<f32>>()
        };
        let lfe_energy = |src: &[f32]| {
            let w = expand_from_stereo(src, ChannelFormat::Surround51, 48_000);
            // Skip the filter's settling, then measure channel 3.
            w.chunks(6).skip(480).map(|f| f[3].abs()).sum::<f32>()
        };
        let low = lfe_energy(&tone(40.0));
        let high = lfe_energy(&tone(4000.0));
        assert!(low > high * 20.0, "bass {low} vs treble {high} in the LFE");
    }

    #[test]
    fn a_quad_layout_has_no_centre_or_lfe_to_write_into() {
        assert_eq!(ChannelFormat::Quad40.lfe_index(), None);
        let out = expand_from_stereo(&stereo(10, 1.0, -1.0), ChannelFormat::Quad40, 48_000);
        assert_eq!(out.len(), 40);
        // Mains untouched (no centre to compensate for), surrounds carry the
        // difference — which for a hard-panned pair is most of the signal.
        assert_eq!(out[0], 1.0);
        assert_eq!(out[1], -1.0);
        assert!(out[2].abs() > 0.1 && out[3].abs() > 0.1);
    }
}
