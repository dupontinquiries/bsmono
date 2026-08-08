//! upscale — choosing a reduced render/decode resolution and the factor to
//! magnify it by.
//!
//! Lifted out of `unendlich-engine`'s settings model, where it was two
//! functions beside a game's graphics options. It is not game-specific: any
//! pipeline that renders or decodes below output resolution and magnifies has
//! to answer the same two questions, and answer them the same way twice — once
//! where the buffer is allocated, once where the magnifier is configured. Two
//! places deriving a factor from a scale is exactly the drift this kit exists
//! to prevent.
//!
//! ## The clamp is the load-bearing part
//!
//! `upscale_factor` is `1.0 / render_scale`, and the comment in the original
//! says why it is clamped: *"so a stray scale can't request an unbounded
//! factor"*. A scale that arrives as zero — a settings file edited by hand, a
//! slider that underflowed, a config default that never got written — turns
//! that division into infinity, and the thing downstream allocates a buffer
//! from it. The clamp is not tidiness; it is the difference between a bad
//! setting and an out-of-memory.
//!
//! ## Modes are discrete because implementations are
//!
//! `upscale_mode` returns 4, 2 or 1 rather than the continuous factor, because
//! a magnifier is built for integer ratios — its weights, its tap counts, its
//! shader variants. A 2.7× request has to become one of them anyway, and the
//! honest place to make that choice is here rather than in whatever silently
//! rounds it later.

/// Lowest supported scale: quarter resolution, magnified 4×.
pub const SCALE_MIN: f32 = 0.25;
/// Highest: supersampling at 2×, magnified 0.5× — i.e. downsampled.
pub const SCALE_MAX: f32 = 2.0;

fn clamp_scale(s: f32) -> f32 {
    // NaN would pass both comparisons of a naive clamp and propagate into the
    // buffer size. Treated as "no idea", which is native.
    if !s.is_finite() { return 1.0; }
    s.clamp(SCALE_MIN, SCALE_MAX)
}

/// How much the magnifier must enlarge the rendered target.
pub fn upscale_factor(render_scale: f32) -> f32 { 1.0 / clamp_scale(render_scale) }

/// The discrete mode the pipeline selects: 4 for quarter-res, 2 for half, else 1.
pub fn upscale_mode(render_scale: f32) -> u8 {
    let rs = clamp_scale(render_scale);
    if rs <= 0.25 + 1e-4 { 4 } else if rs <= 0.5 + 1e-4 { 2 } else { 1 }
}

/// The buffer size to render or decode into, for a given output size.
///
/// Rounded UP and to an even number. Odd dimensions break chroma subsampling —
/// a 4:2:0 plane is half the luma size in each axis — and a magnifier that
/// receives a buffer one pixel short of what it computed reads past the end or
/// renders a seam.
pub fn source_size(out_w: u32, out_h: u32, render_scale: f32) -> (u32, u32) {
    let s = clamp_scale(render_scale);
    let round_even = |v: u32| {
        let scaled = (v as f32 * s).ceil().max(2.0) as u32;
        scaled + (scaled & 1)
    };
    (round_even(out_w), round_even(out_h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_factor_is_the_inverse_of_the_scale() {
        assert_eq!(upscale_factor(0.25), 4.0);
        assert_eq!(upscale_factor(0.5), 2.0);
        assert_eq!(upscale_factor(1.0), 1.0);
    }

    #[test]
    fn a_zero_scale_cannot_request_an_unbounded_factor() {
        // The original's own reason for the clamp. A settings file edited by
        // hand, a slider that underflowed, a default that never got written —
        // and the thing downstream allocates a buffer from the result.
        assert_eq!(upscale_factor(0.0), 4.0);
        assert!(upscale_factor(0.0).is_finite());
    }

    #[test]
    fn a_nonsense_scale_falls_back_to_native_rather_than_to_a_limit() {
        // NaN passes both comparisons of a naive clamp and propagates into the
        // buffer size. "No idea" should mean native, not quarter-res.
        assert_eq!(upscale_factor(f32::NAN), 1.0);
        assert_eq!(upscale_mode(f32::NAN), 1);
        assert_eq!(upscale_factor(f32::INFINITY), 1.0);
    }

    #[test]
    fn modes_are_discrete_because_magnifiers_are() {
        // Weights, tap counts and shader variants are built for integer ratios.
        // A 2.7x request becomes one of them anyway; better here than wherever
        // silently rounds it later.
        assert_eq!(upscale_mode(0.25), 4);
        assert_eq!(upscale_mode(0.4), 2);
        assert_eq!(upscale_mode(0.5), 2);
        assert_eq!(upscale_mode(0.75), 1);
        assert_eq!(upscale_mode(1.0), 1);
    }

    #[test]
    fn the_mode_boundaries_have_slack_for_float_error() {
        // 0.5 arriving as 0.50000006 from a slider must not silently drop to
        // native and halve the resolution the user asked for.
        assert_eq!(upscale_mode(0.5 + 5e-5), 2);
        assert_eq!(upscale_mode(0.25 + 5e-5), 4);
    }

    #[test]
    fn the_source_buffer_is_always_even() {
        // Odd dimensions break 4:2:0 chroma, whose planes are half the luma
        // size in each axis.
        for (w, h, s) in [(1920, 1080, 0.5), (1919, 1081, 0.5), (640, 480, 0.33)] {
            let (sw, sh) = source_size(w, h, s);
            assert_eq!(sw % 2, 0, "{w}x{h} @ {s} gave width {sw}");
            assert_eq!(sh % 2, 0);
        }
    }

    #[test]
    fn the_source_rounds_up_so_the_magnifier_is_never_short() {
        // A buffer one pixel short of what the magnifier computed reads past
        // the end or renders a seam.
        let (w, _) = source_size(1921, 1080, 0.5);
        assert!(w as f32 >= 1921.0 * 0.5);
    }

    #[test]
    fn a_tiny_output_still_produces_a_usable_buffer() {
        // Rounding a 1x1 output down to zero would allocate nothing and divide
        // by zero downstream.
        let (w, h) = source_size(1, 1, 0.25);
        assert!(w >= 2 && h >= 2);
    }

    #[test]
    fn supersampling_is_supported_as_well_as_upscaling() {
        // Scale above 1 renders LARGER and shrinks — the same policy, and
        // clamping it away would silently disable a quality setting.
        assert_eq!(upscale_factor(2.0), 0.5);
        let (w, _) = source_size(960, 540, 2.0);
        assert_eq!(w, 1920);
    }
}
