//! temporal — the arithmetic behind a temporal upscale, without a GPU.
//!
//! A temporal upscaler renders below the output resolution, jitters the sample
//! position every frame, and accumulates. Two things fall out of that:
//!
//! * **Fill drops by the square of the scale.** Half-res is a quarter of the
//!   pixels, which at 4K is the whole reason to do it.
//! * **It anti-aliases.** Jittering and accumulating IS supersampling spread
//!   over time — the AA is not a second pass, it is the same machinery. On a
//!   phone panel, where the pixels are small and the aliasing is what you
//!   actually notice, this is the better half of the deal.
//!
//! ## What is hard here is not the network
//!
//! There is no network in this module and there does not need to be one to
//! start. What decides whether a temporal upscale looks right is:
//!
//! 1. the jitter sequence — badly distributed offsets converge to a blur
//!    rather than to detail;
//! 2. reprojection — where this pixel was last frame;
//! 3. **when to throw history away** — the hard one. Reused history that
//!    should have been rejected is ghosting, and ghosting is worse than
//!    aliasing because it looks like a bug rather than like a cheap setting.
//!
//! All three are arithmetic, all three are where the subtle errors live, and
//! none of them needs a device to test.

/// Radical-inverse (van der Corput) in `base`.
///
/// Halton, not random: a random sequence clumps, and a clump means several
/// frames sampling nearly the same sub-pixel position, which converges to a
/// blurred average instead of to the detail between samples.
fn radical_inverse(mut i: u32, base: u32) -> f32 {
    let mut f = 1.0f32;
    let mut r = 0.0f32;
    let b = base as f32;
    while i > 0 {
        f /= b;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}

/// Sub-pixel jitter for frame `n`, in **pixels**, centred on zero.
///
/// Halton(2,3), the standard choice, shifted to [-0.5, 0.5] so the offsets
/// straddle the pixel centre rather than all pushing one way — an unshifted
/// [0,1) sequence biases the whole image half a pixel down-right, which reads
/// as the picture being subtly misaligned rather than as jitter.
///
/// `n` starts at 1: Halton's index 0 is (0,0), and a first frame with no
/// jitter at all is a frame that contributes nothing to the accumulation.
pub fn jitter(n: u32, phase: u32) -> (f32, f32) {
    let i = (n % phase.max(1)) + 1;
    (radical_inverse(i, 2) - 0.5, radical_inverse(i, 3) - 0.5)
}

/// How many frames before the jitter pattern repeats.
///
/// Scaled by the pixel count being reconstructed: recovering 4× the pixels
/// needs more distinct sub-pixel positions than recovering 2×. The `* 2` is
/// slack so the sequence is not exactly the number of pixels — a phase equal
/// to the upscale ratio squared visits each target pixel once and then repeats
/// forever with identical offsets, which stops accumulating.
pub fn jitter_phase(scale: f32) -> u32 {
    let ratio = (1.0 / scale.clamp(0.25, 1.0)).round() as u32;
    (ratio * ratio * 2).clamp(2, 32)
}

/// Convert a pixel-space jitter to the clip-space offset a projection matrix
/// wants.
///
/// TWO PIXELS PER NDC UNIT, not one: clip space spans -1..1. Halving this is
/// the classic error and produces jitter at half the intended amplitude, which
/// looks like the upscaler simply working badly rather than like a bug.
///
/// The Y sign is the caller's: whether clip-space Y is up or down depends on
/// the graphics API, and guessing here would invert the jitter on one of them.
pub fn jitter_to_clip(jitter_px: (f32, f32), render_w: u32, render_h: u32) -> (f32, f32) {
    let w = render_w.max(1) as f32;
    let h = render_h.max(1) as f32;
    (2.0 * jitter_px.0 / w, 2.0 * jitter_px.1 / h)
}

/// The render resolution for an output size and a scale.
///
/// Rounded to even numbers. An odd render width against an even output makes
/// the 2× mapping land between texels on one edge, and the artefact is a
/// shimmering column at the screen edge that looks like a driver bug.
pub fn render_size(out_w: u32, out_h: u32, scale: f32) -> (u32, u32) {
    let s = scale.clamp(0.25, 1.0);
    let w = ((out_w as f32 * s).round() as u32).max(2) & !1;
    let h = ((out_h as f32 * s).round() as u32).max(2) & !1;
    (w.max(2), h.max(2))
}

/// Fill saved against rendering at full resolution, as a fraction.
pub fn fill_saved(scale: f32) -> f32 {
    let s = scale.clamp(0.25, 1.0);
    1.0 - s * s
}

/// Why a pixel's history was not reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reject {
    /// Reprojected outside the previous frame — a camera cut, or the edge of
    /// the screen. There is no history to reuse.
    OffScreen,
    /// The surface under this pixel changed too much: a disocclusion, where
    /// something moved away and revealed what was behind it. Reusing here is
    /// the classic trail-behind-a-moving-object ghost.
    Disoccluded,
    /// History is reused.
    Accept,
}

/// Should this pixel reuse its reprojected history?
///
/// `prev_uv` is where the pixel was last frame, in 0..1. `depth_delta` is how
/// far the reprojected depth is from the current one, relative to the current
/// depth — a ratio rather than an absolute, because a 1cm discrepancy is
/// nothing at 100m and everything at 30cm.
pub fn history_decision(prev_uv: (f32, f32), depth_delta: f32) -> Reject {
    if !(0.0..=1.0).contains(&prev_uv.0) || !(0.0..=1.0).contains(&prev_uv.1) {
        return Reject::OffScreen;
    }
    // NOT `> TOLERANCE`. Every comparison against NaN is false, so a
    // degenerate depth -- which is exactly what a reprojection through a
    // singular matrix produces -- would fall through to Accept and ghost,
    // silently, in the one case where the data is known to be broken.
    //
    // Written the wrong way first and caught by the test below: the inverted
    // form rejects NaN because `NaN <= x` is also false.
    if !(depth_delta.abs() <= DEPTH_TOLERANCE) {
        return Reject::Disoccluded;
    }
    Reject::Accept
}

/// Relative depth discrepancy past which history is a different surface.
///
/// 5%: loose enough to survive the precision of a reprojected depth buffer,
/// tight enough that a foreground object's edge rejects rather than smears.
pub const DEPTH_TOLERANCE: f32 = 0.05;

/// How much of the history to keep for an accepted pixel.
///
/// Higher is smoother and ghostier. This is the knob that decides whether the
/// result reads as anti-aliased or as smeared, and it is deliberately NOT a
/// constant: a pixel that has been stable for many frames can hold more
/// history than one that was just rejected and is rebuilding.
///
/// `age` is frames since this pixel last rejected. The ramp means a
/// disocclusion resolves sharply and then settles, rather than either
/// ghosting immediately or never converging.
pub fn history_weight(age: u32) -> f32 {
    // 0 -> 0.0 (a fresh pixel is entirely current), rising to a 0.9 ceiling.
    // Never 1.0: at 1.0 the current frame contributes nothing and the image
    // freezes, which is a real failure mode of a naive implementation.
    let w = 1.0 - 1.0 / (age as f32 + 1.0);
    w.min(0.9)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JITTER MUST STRADDLE THE PIXEL CENTRE. An unshifted [0,1) Halton biases
    /// the whole image half a pixel down-right, which reads as the picture
    /// being misaligned rather than as jitter.
    #[test]
    fn the_jitter_is_centred_on_zero() {
        let n = 64;
        let (mut sx, mut sy) = (0.0f32, 0.0f32);
        for i in 0..n {
            let (x, y) = jitter(i, 16);
            assert!((-0.5..=0.5).contains(&x), "frame {i}: {x}");
            assert!((-0.5..=0.5).contains(&y), "frame {i}: {y}");
            sx += x;
            sy += y;
        }
        assert!((sx / n as f32).abs() < 0.1, "x biased: {}", sx / n as f32);
        assert!((sy / n as f32).abs() < 0.1, "y biased: {}", sy / n as f32);
    }

    /// The first frame must actually jitter. Halton's index 0 is (0,0), and a
    /// first frame at the pixel centre contributes nothing to the accumulation.
    #[test]
    fn the_first_frame_is_not_unjittered() {
        let (x, y) = jitter(0, 16);
        assert!(x.abs() > 1e-6 || y.abs() > 1e-6, "frame 0 did not jitter: {x},{y}");
    }

    /// Successive offsets must SPREAD. A clump means several frames sampling
    /// nearly the same sub-pixel position, which converges to a blur instead
    /// of to the detail between samples -- the reason for Halton over random.
    #[test]
    fn consecutive_jitters_are_not_clustered() {
        for i in 0..16 {
            let a = jitter(i, 16);
            let b = jitter(i + 1, 16);
            let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
            assert!(d > 0.1, "frames {i},{} are {d} apart", i + 1);
        }
    }

    /// A PHASE THAT VISITS EACH TARGET PIXEL ONCE AND REPEATS STOPS
    /// ACCUMULATING. 4x upscale needs more than 16 distinct positions.
    #[test]
    fn the_phase_grows_with_the_upscale_ratio() {
        assert!(jitter_phase(0.5) > 4, "2x needs more than its 4 target pixels");
        assert!(jitter_phase(0.25) > jitter_phase(0.5), "4x needs more than 2x");
        assert!(jitter_phase(1.0) >= 2, "even native gets AA from jitter");
        assert!(jitter_phase(0.25) <= 32, "and it stays bounded");
    }

    /// TWO PIXELS PER NDC UNIT: clip space spans -1..1. Halving this is the
    /// classic error and gives jitter at half amplitude, which looks like the
    /// upscaler working badly rather than like a bug.
    #[test]
    fn the_clip_offset_accounts_for_the_full_ndc_range() {
        let (cx, _) = jitter_to_clip((0.5, 0.0), 1920, 1080);
        assert!((cx - (2.0 * 0.5 / 1920.0)).abs() < 1e-9, "{cx}");
        // Half a pixel of a 1920-wide target is 1/1920 of NDC, not 1/3840.
        assert!((cx - 1.0 / 1920.0).abs() < 1e-9, "{cx}");
    }

    #[test]
    fn a_zero_sized_target_does_not_divide_by_zero() {
        let (x, y) = jitter_to_clip((0.5, 0.5), 0, 0);
        assert!(x.is_finite() && y.is_finite(), "{x},{y}");
    }

    /// An ODD render width against an even output makes the 2x mapping land
    /// between texels at one edge -- a shimmering column that reads as a
    /// driver bug.
    #[test]
    fn render_sizes_are_even() {
        for (w, h) in [(1920, 1080), (2560, 1440), (3840, 2160), (1224, 2700)] {
            for s in [0.25, 0.5, 0.67, 0.75, 1.0] {
                let (rw, rh) = render_size(w, h, s);
                assert_eq!(rw % 2, 0, "{w}x{h} @ {s} -> {rw}");
                assert_eq!(rh % 2, 0, "{w}x{h} @ {s} -> {rh}");
                assert!(rw >= 2 && rh >= 2);
            }
        }
    }

    /// Fill is the SQUARE of the scale, which is the entire reason to do this.
    #[test]
    fn half_scale_saves_three_quarters_of_the_fill() {
        assert!((fill_saved(0.5) - 0.75).abs() < 1e-6);
        assert!((fill_saved(0.25) - 0.9375).abs() < 1e-6);
        assert!(fill_saved(1.0).abs() < 1e-6, "native saves nothing");
    }

    /// GHOSTING IS WORSE THAN ALIASING because it looks like a bug rather than
    /// like a cheap setting. A disocclusion must reject.
    #[test]
    fn a_disoccluded_pixel_rejects_its_history() {
        assert_eq!(history_decision((0.5, 0.5), 0.0), Reject::Accept);
        assert_eq!(history_decision((0.5, 0.5), 0.5), Reject::Disoccluded);
        // Just inside and just outside the tolerance.
        assert_eq!(history_decision((0.5, 0.5), DEPTH_TOLERANCE * 0.9), Reject::Accept);
        assert_eq!(history_decision((0.5, 0.5), DEPTH_TOLERANCE * 1.1), Reject::Disoccluded);
    }

    /// Reprojecting off-screen is not a disocclusion -- there is simply no
    /// history -- and the two want different words in a debug view.
    #[test]
    fn an_offscreen_reprojection_is_its_own_case() {
        assert_eq!(history_decision((-0.01, 0.5), 0.0), Reject::OffScreen);
        assert_eq!(history_decision((0.5, 1.01), 0.0), Reject::OffScreen);
        assert_eq!(history_decision((0.0, 1.0), 0.0), Reject::Accept, "the edges are in");
    }

    /// A DEGENERATE REPROJECTION MUST NOT ACCEPT. NaN fails the range check,
    /// which is why the range check comes first.
    #[test]
    fn a_nan_reprojection_is_rejected() {
        assert_eq!(history_decision((f32::NAN, 0.5), 0.0), Reject::OffScreen);
        assert_eq!(history_decision((0.5, 0.5), f32::NAN), Reject::Disoccluded);
    }

    /// NEVER 1.0. At full history weight the current frame contributes
    /// nothing and the image freezes -- a real failure mode of a naive
    /// implementation, and one that looks like a hang.
    #[test]
    fn history_never_completely_replaces_the_current_frame() {
        for age in [0u32, 1, 5, 100, 10_000] {
            let w = history_weight(age);
            assert!((0.0..=0.9).contains(&w), "age {age}: {w}");
        }
        assert_eq!(history_weight(0), 0.0, "a fresh pixel is entirely current");
        assert!(history_weight(10) > history_weight(1), "and it ramps");
    }
}
