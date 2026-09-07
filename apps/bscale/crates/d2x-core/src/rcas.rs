//! rcas — Robust Contrast-Adaptive Sharpening, ported from AMD's FSR SDK.
//!
//! ## Why this belongs in a video upscaler
//!
//! Every upscaler in this pipeline produces output that is slightly soft relative to a
//! native-resolution frame: a CNN trained on an L1/perceptual mix regresses toward the mean,
//! bicubic and the flat-bypass path are soft by construction, and a REUSED block is a copy
//! of an earlier upscale and inherits whatever softness that had. AMD ship a sharpening pass
//! at the end of FSR for exactly this reason, and it is the one part of the FSR chain that
//! transfers to offline video unchanged, because it needs no motion vectors, no jitter and
//! no history — only the finished frame.
//!
//! (What does NOT transfer, for the record: FSR2/3's temporal accumulation and FSR4's ML
//! upscaler both want per-pixel motion vectors, a depth buffer and a sub-pixel jitter
//! sequence from the renderer. A decoded video has none of those. DLSS 5, announced at GTC
//! 2026 and shipped in September, is further still from this problem — it is neural
//! RELIGHTING driven by a game's G-buffer, not an upscaler.)
//!
//! ## Why RCAS and not a plain unsharp mask
//!
//! An unsharp mask amplifies whatever is there, including the ringing an upscaler already
//! introduced and the block-boundary discontinuities this pipeline's reuse gate can leave.
//! RCAS is built to avoid exactly that:
//!
//! * **It cannot overshoot.** The sharpening lobe is derived from the min and max of the
//!   four-tap ring around each pixel, so the result stays inside the range its neighbours
//!   already occupy. No new white halo, no crushed black rim — which matters here because a
//!   halo along a block seam is the artifact that makes reuse visible.
//! * **It backs off on noise.** A luma-based detector measures how much the centre pixel
//!   departs from its ring relative to the ring's own spread; where that ratio is high (film
//!   grain, AV1 quantisation noise) the lobe is scaled down. Sharpening compressed video
//!   without this is how you turn ringing artifacts into permanent texture.
//!
//! ## Faithfulness, and the one deliberate difference
//!
//! The kernel below is the FSR1 `FsrRcasF` recipe: the same 5-tap cross, the same luma
//! weights (`0.5·R + G + 0.5·B`, i.e. luma times two), the same per-channel hit-min/hit-max
//! lobe, the same `FSR_RCAS_LIMIT` of `0.25 - 1/16`, and the same noise scaling. It is f32
//! throughout rather than FSR's optional f16 path, and it runs on 8-bit RGB in the display
//! domain — which is where this pipeline's frames live. FSR runs RCAS on linear-ish values
//! after its own resolve; on gamma-encoded 8-bit the effect is slightly stronger in
//! shadows, which is why the default sharpness here is conservative.

use crate::frame::Frame;

/// FSR's own limit on the sharpening lobe.
pub const RCAS_LIMIT: f32 = 0.25 - (1.0 / 16.0);

/// Sharpness, in FSR's units: **stops**. 0.0 is maximum sharpening, 1.0 is half, 2.0 a
/// quarter, and so on. This is AMD's parameterisation, kept rather than inverted so a value
/// tuned against any FSR implementation means the same thing here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sharpness(pub f32);

impl Sharpness {
    /// The multiplier the kernel uses: `2^-stops`.
    pub fn scale(self) -> f32 {
        (-self.0.max(0.0)).exp2()
    }
    /// Off — the kernel becomes a copy.
    pub const OFF: Sharpness = Sharpness(f32::INFINITY);
}

impl Default for Sharpness {
    /// Conservative on purpose: this runs on gamma-encoded 8-bit, where a given lobe bites
    /// harder in the shadows than it does in the linear-ish domain FSR expects.
    fn default() -> Self {
        Sharpness(0.8)
    }
}

/// FSR's luma proxy: `0.5·R + G + 0.5·B` — luma times two, unnormalised, which is fine
/// because every use of it below is a ratio or a difference.
fn luma2(p: [f32; 3]) -> f32 {
    p[2] * 0.5 + (p[0] * 0.5 + p[1])
}

fn max3(a: f32, b: f32, c: f32) -> f32 {
    a.max(b).max(c)
}
fn min3(a: f32, b: f32, c: f32) -> f32 {
    a.min(b).min(c)
}

/// FSR's noise detector, as a multiplier on the sharpening lobe: `1.0` where the centre
/// looks like part of an edge, falling to `0.5` where it looks like an isolated outlier.
///
/// The measure is how far the centre sits from the mean of its ring RELATIVE to the ring's
/// own spread. Grain is a lone pixel disagreeing with neighbours that agree with each other,
/// so the ratio is near 1; an edge pixel disagrees with a ring that is already split, so the
/// ratio is near 0.5 or below.
///
/// It is a scale on the lobe, not a veto — the lobe's own contrast limiter still dominates
/// in places (a mid-grey ring permits a larger lobe than a high-contrast one, whatever the
/// noise reading), so this factor is worth testing directly rather than inferring from two
/// different pixels' outputs.
pub fn noise_scale(b: [f32; 3], d: [f32; 3], e: [f32; 3], f: [f32; 3], h: [f32; 3]) -> f32 {
    let (bl, dl, el, fl, hl) = (luma2(b), luma2(d), luma2(e), luma2(f), luma2(h));
    let spread = max3(max3(bl, dl, el), fl, hl) - min3(min3(bl, dl, el), fl, hl);
    let nz = 0.25 * bl + 0.25 * dl + 0.25 * fl + 0.25 * hl - el;
    let nz = if spread > 0.0 { (nz.abs() / spread).clamp(0.0, 1.0) } else { 0.0 };
    -0.5 * nz + 1.0
}

/// Sharpen one pixel from its 5-tap cross: centre `e`, and `b`/`d`/`f`/`h` above, left,
/// right and below. Values in 0..=1.
///
/// Exposed so the arithmetic can be tested against hand-worked cases without a Frame.
pub fn rcas_pixel(b: [f32; 3], d: [f32; 3], e: [f32; 3], f: [f32; 3], h: [f32; 3], sharpness: Sharpness) -> [f32; 3] {
    let scale = sharpness.scale();
    if !scale.is_finite() || scale <= 0.0 {
        return e;
    }
    let nz = noise_scale(b, d, e, f, h);

    // The ring's range, per channel.
    let mut lobe = f32::NEG_INFINITY;
    for c in 0..3 {
        let mn4 = min3(b[c], d[c], f[c]).min(h[c]);
        let mx4 = max3(b[c], d[c], f[c]).max(h[c]);
        // peakC = (1.0, -4.0) in FSR; the reciprocals are guarded because an all-black or
        // all-white ring makes both denominators zero, and FSR's approximate reciprocal
        // quietly returns a huge number where this would produce a NaN.
        let hit_min = if mx4 > 0.0 { mn4 / (4.0 * mx4) } else { 0.0 };
        let hit_max = {
            let den = 4.0 * mn4 + -4.0;
            if den.abs() > f32::EPSILON { (1.0 - mx4) / den } else { 0.0 }
        };
        lobe = lobe.max((-hit_min).max(hit_max));
    }
    // Clamp to the limit and apply sharpness, then the noise scaling.
    let lobe = (lobe.min(0.0).max(-RCAS_LIMIT)) * scale * nz;

    let rcp = 1.0 / (4.0 * lobe + 1.0);
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        out[c] = ((lobe * b[c] + lobe * d[c] + lobe * h[c] + lobe * f[c] + e[c]) * rcp).clamp(0.0, 1.0);
    }
    out
}

/// Sharpen a whole frame in place.
///
/// The one-pixel border is left alone rather than clamped-and-sharpened: a cross that reads
/// the edge pixel twice biases the lobe, and a bright rim around a video frame is more
/// visible than a slightly softer outermost row.
pub fn sharpen(frame: &mut Frame, sharpness: Sharpness) {
    if frame.width < 3 || frame.height < 3 || !sharpness.scale().is_finite() || sharpness.scale() <= 0.0 {
        return;
    }
    let src = frame.data.clone();
    let (w, h) = (frame.width as usize, frame.height as usize);
    let at = |x: usize, y: usize| -> [f32; 3] {
        let i = (y * w + x) * 3;
        [src[i] as f32 / 255.0, src[i + 1] as f32 / 255.0, src[i + 2] as f32 / 255.0]
    };
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let out = rcas_pixel(at(x, y - 1), at(x - 1, y), at(x, y), at(x + 1, y), at(x, y + 1), sharpness);
            let i = (y * w + x) * 3;
            for c in 0..3 {
                frame.data[i + c] = (out[c] * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: u32, h: u32, v: u8) -> Frame {
        let mut f = Frame::new(w, h);
        f.data.iter_mut().for_each(|b| *b = v);
        f
    }

    /// A flat field has nothing to sharpen, and must come back bit-identical — the property
    /// that makes this safe to run on every frame including the ones that are pure reuse.
    #[test]
    fn a_flat_field_is_unchanged() {
        for v in [0u8, 17, 128, 255] {
            let mut f = flat(16, 16, v);
            let before = f.data.clone();
            sharpen(&mut f, Sharpness::default());
            assert_eq!(f.data, before, "a flat field at {v} was altered");
        }
    }

    /// RCAS's defining property: the output stays inside the range the neighbours already
    /// occupy. An unsharp mask does not do this, and the halo it produces along a block seam
    /// is exactly what would make this pipeline's reuse visible.
    #[test]
    fn sharpening_never_overshoots_the_ring() {
        let cases: [([f32; 3], [f32; 3], [f32; 3], [f32; 3], [f32; 3]); 4] = [
            ([0.0; 3], [0.0; 3], [1.0; 3], [0.0; 3], [0.0; 3]),   // a bright dot
            ([1.0; 3], [1.0; 3], [0.0; 3], [1.0; 3], [1.0; 3]),   // a dark dot
            ([0.2, 0.4, 0.6], [0.9, 0.1, 0.5], [0.5; 3], [0.3, 0.7, 0.2], [0.6, 0.6, 0.9]),
            ([0.51; 3], [0.49; 3], [0.50; 3], [0.52; 3], [0.48; 3]), // a near-flat patch
        ];
        for (b, d, e, f, h) in cases {
            let out = rcas_pixel(b, d, e, f, h, Sharpness(0.0)); // maximum sharpening
            for c in 0..3 {
                let lo = min3(b[c], d[c], f[c]).min(h[c]).min(e[c]);
                let hi = max3(b[c], d[c], f[c]).max(h[c]).max(e[c]);
                assert!(
                    out[c] >= lo - 1e-4 && out[c] <= hi + 1e-4,
                    "channel {c} left the ring: {} not in {lo}..={hi}",
                    out[c]
                );
                assert!(out[c].is_finite(), "NaN out of the kernel");
            }
        }
    }

    /// An edge gets sharpened: the centre moves AWAY from the ring's mean, which is what
    /// sharpening is.
    #[test]
    fn an_edge_gains_contrast() {
        // A step the centre sits ON: ring mean 0.5, centre 0.7. A centre that already equals
        // the ring mean is a FIXED POINT of the kernel — the cross is symmetric, so
        // `(lobe·4·0.5 + 0.5)/(4·lobe + 1)` is 0.5 whatever the lobe is. Picking one of
        // those by accident is how this test first "passed" while measuring nothing.
        let (b, d, e, f, h) = ([0.8; 3], [0.2; 3], [0.7; 3], [0.8; 3], [0.2; 3]);
        let out = rcas_pixel(b, d, e, f, h, Sharpness(0.0));
        let ring_mean = (0.8 + 0.2 + 0.8 + 0.2) / 4.0;
        let before = (e[0] - ring_mean).abs();
        let after = (out[0] - ring_mean).abs();
        assert!(after > before, "the edge got softer, not sharper: {before} → {after}");
    }

    /// Sharpness is in STOPS, AMD's units: each whole stop halves the effect, and OFF is a
    /// copy. Keeping their parameterisation means a value tuned against any FSR
    /// implementation means the same thing here.
    #[test]
    fn sharpness_is_in_stops_and_off_is_a_copy() {
        assert!((Sharpness(0.0).scale() - 1.0).abs() < 1e-6);
        assert!((Sharpness(1.0).scale() - 0.5).abs() < 1e-6);
        assert!((Sharpness(2.0).scale() - 0.25).abs() < 1e-6);
        let (b, d, e, f, h) = ([0.8; 3], [0.2; 3], [0.7; 3], [0.8; 3], [0.2; 3]);
        assert_eq!(rcas_pixel(b, d, e, f, h, Sharpness::OFF), e, "OFF must be a pure copy");
        // and more stops means less movement
        let strong = (rcas_pixel(b, d, e, f, h, Sharpness(0.0))[0] - e[0]).abs();
        let weak = (rcas_pixel(b, d, e, f, h, Sharpness(2.0))[0] - e[0]).abs();
        assert!(weak < strong, "two stops down did not weaken the effect: {weak} vs {strong}");
    }

    /// The noise gate is the reason this is usable on compressed video: where the centre
    /// departs from its ring in a way that looks like grain rather than an edge, the lobe is
    /// scaled down. Without it, sharpening AV1 quantisation noise makes it permanent.
    ///
    /// Asserted on the FACTOR, not on two pixels' outputs. The first version of this test
    /// compared a grain-like pixel's movement with an edge-like one's and failed — correctly:
    /// the lobe's contrast limiter is more generous on a mid-grey ring than on a
    /// high-contrast one, so the grain pixel moved further despite a smaller noise factor.
    /// Both things are true, and only one of them is this gate.
    #[test]
    fn the_noise_factor_backs_off_on_an_isolated_outlier() {
        // Grain: a ring that agrees with itself, one pixel that does not.
        let grain = noise_scale([0.5; 3], [0.5; 3], [0.2; 3], [0.5; 3], [0.5; 3]);
        // An edge: the ring is already split, so the centre's disagreement is structure.
        let edge = noise_scale([0.8; 3], [0.2; 3], [0.2; 3], [0.8; 3], [0.2; 3]);
        assert!(grain < edge, "grain {grain} was not damped more than an edge {edge}");
        assert!((0.5..=1.0).contains(&grain) && (0.5..=1.0).contains(&edge), "{grain}, {edge}");
        // A centre sitting exactly on its ring's mean is pure structure: no damping at all.
        assert!((noise_scale([0.8; 3], [0.2; 3], [0.5; 3], [0.8; 3], [0.2; 3]) - 1.0).abs() < 1e-6);
        // A flat cross has no spread to divide by, and must not produce a NaN.
        assert!(noise_scale([0.4; 3], [0.4; 3], [0.4; 3], [0.4; 3], [0.4; 3]).is_finite());
    }

    /// The border is left alone: a cross that reads the edge pixel twice biases the lobe,
    /// and a bright rim around the frame is more visible than a soft outermost row.
    #[test]
    fn the_one_pixel_border_is_untouched_and_tiny_frames_are_safe() {
        let mut f = Frame::new(5, 5);
        for (i, b) in f.data.iter_mut().enumerate() {
            *b = (i * 7 % 251) as u8;
        }
        let before = f.data.clone();
        sharpen(&mut f, Sharpness(0.0));
        let w = 5usize;
        for y in 0..5usize {
            for x in 0..5usize {
                let i = (y * w + x) * 3;
                if x == 0 || y == 0 || x == 4 || y == 4 {
                    assert_eq!(&f.data[i..i + 3], &before[i..i + 3], "border pixel ({x},{y}) moved");
                }
            }
        }
        // a frame too small to have an interior is a no-op, not a panic
        let mut tiny = flat(2, 2, 40);
        let t = tiny.data.clone();
        sharpen(&mut tiny, Sharpness(0.0));
        assert_eq!(tiny.data, t);
    }
}
