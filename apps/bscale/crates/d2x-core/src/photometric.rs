//! photometric — reusing a block through a fade, a grade, or a lighting change.
//!
//! ## The idea, and why this pipeline can bank it
//!
//! Some things a frame does to the previous one are **invertible per pixel**: a fade is a
//! multiply, a colour grade is a per-channel gain and offset, an exposure ramp is a gain, a
//! dissolve to a flat colour is a lerp toward a constant. In every case the CONTENT is
//! unchanged and only its photometry moved. The block matcher does not know that, so it sees
//! every block fail the SSE gate and sends the whole frame through the network — for as long
//! as the fade lasts.
//!
//! That is the opposite of the case the block cache lost on. `xref`'s measured negative was
//! that the probe set is content genuinely new to the shot, which has no twin anywhere. A
//! fade is the reverse: the probe set is the ENTIRE FRAME, and every block of it has a twin
//! one frame back, related by two numbers per channel. The work is redundant and the
//! redundancy is cheap to describe. That is why this is worth doing where the cache was not.
//!
//! ## What already exists, and what it misses
//!
//! `predict::fade_scalar` estimates ONE ADDITIVE SCALAR per block, across all three
//! channels together, and `residual::merge` applies it to the upscaled previous frame with
//! `add_scalar_block`. The machinery — estimate low-res, verify against the gate's own SSE
//! limit, apply at output resolution — is right, and this module keeps all of it.
//!
//! What the model misses:
//!
//! * **A fade is multiplicative.** `out = a·in`, not `out = in + b`. An additive offset fits
//!   it only near the value where the two curves cross: at a = 0.9, a mid-grey pixel moves
//!   13 levels and a near-white one moves 26. One offset cannot cover a block spanning both,
//!   and across a frame it cannot cover anything. So a real fade still blows the gate on most
//!   blocks, which is exactly when the frame is most redundant.
//! * **A grade is per channel.** Teal-and-orange pushes blue up and red down.
//!   `fade_scalar` sums over all three channels, so a channel-differential grade averages
//!   toward zero and is invisible to it.
//!
//! [`Photometric`] is `a·x + b` per channel — six numbers per block, still tiny next to the
//! block's own pixels — fitted by least squares and verified by the same bounded SSE.
//!
//! ## The honest catch: gain and upscale do not commute
//!
//! Reuse serves a block from the PREVIOUS UPSCALED OUTPUT. Applying a gain to an upscaled
//! block is not the same as upscaling a gained block, because the network is nonlinear:
//! `N(a·x + b) ≠ a·N(x) + b`. The gate verifies the fit at LOW resolution, so it does not see
//! this error at all.
//!
//! Three things bound it, and they are the design rather than a caveat:
//!
//! 1. **Gain is clamped to a narrow window** ([`MAX_GAIN_DEVIATION`]). A CNN is close to
//!    locally linear over a small brightness change; over a large one it is not, and a fade
//!    that steep is a frame worth redrawing anyway.
//! 2. **The gate keeps headroom.** [`verify`] takes the cell's SSE budget and spends only
//!    part of it ([`HEADROOM`]), leaving the rest to cover the commutation error it cannot
//!    measure. A compensated reuse is therefore held to a STRICTER standard than an
//!    uncompensated one, which is the right way round: the uncertain path should not get the
//!    same rope as the certain one.
//! 3. **Offset-only is exempt.** A pure `+b` on the full-range signal is what the existing
//!    code already does and ships; it gets the full budget, so this cannot regress what
//!    works today.
//!
//! What this does NOT attempt: geometric transforms. A zoom, a rotation or a warp needs
//! resampling, and resampling an already-upscaled block is a quality question that a
//! low-resolution gate is not qualified to answer. Global translation is already handled by
//! the matcher and `phase_corr`.

use crate::frame::Frame;

/// How far the gain may stray from 1.0 before a block is redrawn instead.
///
/// 0.25 is a factor of 1.33 in either direction — steeper than most of a fade's duration
/// and far inside where a CNN stops behaving locally linearly. A fade steeper than this per
/// frame is a handful of frames at the very end of the fade, where the picture is nearly
/// flat and redrawing is cheap.
pub const MAX_GAIN_DEVIATION: f32 = 0.25;

/// The fraction of a cell's SSE budget a GAINED reuse may spend. The remainder covers the
/// commutation error the low-resolution gate cannot see. Offset-only fits spend the lot.
pub const HEADROOM: f64 = 0.6;

/// `out = a·x + b`, per channel, in 0..=255 units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Photometric {
    pub gain: [f32; 3],
    pub offset: [f32; 3],
}

impl Default for Photometric {
    fn default() -> Self {
        Photometric::IDENTITY
    }
}

impl Photometric {
    pub const IDENTITY: Photometric = Photometric { gain: [1.0; 3], offset: [0.0; 3] };

    /// The existing model: one additive scalar across all channels.
    pub fn offset_only(b: i32) -> Photometric {
        Photometric { gain: [1.0; 3], offset: [b as f32; 3] }
    }

    /// Is this the identity, to within a level? Then there is nothing to apply and nothing
    /// to pay headroom for.
    pub fn is_identity(&self) -> bool {
        (0..3).all(|c| (self.gain[c] - 1.0).abs() < 1e-3 && self.offset[c].abs() < 0.5)
    }

    /// Does this use gain at all? An offset-only fit is the shipped model and gets the full
    /// SSE budget; see the module docs.
    pub fn uses_gain(&self) -> bool {
        (0..3).any(|c| (self.gain[c] - 1.0).abs() >= 1e-3)
    }

    /// Within the window a locally-linear network can be trusted over?
    pub fn gain_is_sane(&self) -> bool {
        (0..3).all(|c| {
            let g = self.gain[c];
            g.is_finite() && (g - 1.0).abs() <= MAX_GAIN_DEVIATION && g > 0.0
        })
    }

    /// Apply to one pixel value.
    pub fn map(&self, c: usize, v: u8) -> u8 {
        (self.gain[c] * v as f32 + self.offset[c]).round().clamp(0.0, 255.0) as u8
    }

    /// The inverse transform, when there is one. `None` for a gain that has collapsed the
    /// signal — a fade that has reached black is not invertible, and pretending otherwise
    /// would amplify quantisation noise into bands.
    pub fn inverse(&self) -> Option<Photometric> {
        let mut gain = [0.0f32; 3];
        let mut offset = [0.0f32; 3];
        for c in 0..3 {
            if self.gain[c].abs() < 1e-3 {
                return None;
            }
            gain[c] = 1.0 / self.gain[c];
            offset[c] = -self.offset[c] / self.gain[c];
        }
        Some(Photometric { gain, offset })
    }
}

/// Least-squares fit of `next ≈ a·prev + b`, per channel, over one block.
///
/// Returns [`Photometric::IDENTITY`] when the block has no contrast to fit against: with
/// every pixel at one value, `a` and `b` trade off freely and the fit is arbitrary. That is
/// the flat-region case — sky, letterbox, a fade that has already finished — where an
/// offset-only answer is both sufficient and stable.
pub fn fit_block(prev: &Frame, next: &Frame, x: u32, y: u32, size: u32) -> Photometric {
    let mut gain = [1.0f32; 3];
    let mut offset = [0.0f32; 3];
    for c in 0..3 {
        let (mut sx, mut sy, mut sxx, mut sxy, mut n) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for j in 0..size {
            let rp = prev.row(y + j, x, x + size);
            let rn = next.row(y + j, x, x + size);
            for k in (c..rp.len()).step_by(3) {
                let (p, q) = (rp[k] as f64, rn[k] as f64);
                sx += p;
                sy += q;
                sxx += p * p;
                sxy += p * q;
                n += 1.0;
            }
        }
        if n < 2.0 {
            continue;
        }
        let var = sxx - sx * sx / n;
        // No contrast to fit a slope against: fall back to the mean difference, which is
        // the existing model and the stable answer here.
        if var < n * 4.0 {
            offset[c] = ((sy - sx) / n) as f32;
            continue;
        }
        let a = (sxy - sx * sy / n) / var;
        let b = (sy - a * sx) / n;
        gain[c] = a as f32;
        offset[c] = b as f32;
    }
    Photometric { gain, offset }
}

/// The SSE between `next`'s block and `prev`'s block put through `p`, stopping early once
/// `limit` is exceeded. Mirrors `predict::shifted_sse_bounded`, generalised.
pub fn compensated_sse_bounded(
    prev: &Frame,
    next: &Frame,
    x: u32,
    y: u32,
    size: u32,
    p: &Photometric,
    limit: u64,
) -> Option<u64> {
    let mut sum: u64 = 0;
    for j in 0..size {
        let rp = prev.row(y + j, x, x + size);
        let rn = next.row(y + j, x, x + size);
        let mut s: u64 = 0;
        for (k, (&a, &b)) in rp.iter().zip(rn).enumerate() {
            let d = p.map(k % 3, a) as i32 - b as i32;
            s += (d * d) as u64;
        }
        sum += s;
        if sum > limit {
            return None;
        }
    }
    Some(sum)
}

/// What a fit was allowed to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    /// Reuse this block, compensated by the transform.
    Accept(Photometric),
    /// The fit does not hold: redraw.
    Redraw,
}

/// Decide whether a block may be reused through `p`, against the cell's own SSE budget.
///
/// The budget is the gate's — the same number the reference ring is tested against — so a
/// compensated reuse is never allowed to be worse than `--quality` says. A fit that uses
/// gain spends only [`HEADROOM`] of it, leaving the rest to cover the error the
/// low-resolution gate cannot measure (see the module docs).
pub fn verify(
    prev: &Frame,
    next: &Frame,
    x: u32,
    y: u32,
    size: u32,
    p: &Photometric,
    budget_sse: u64,
) -> Verdict {
    if p.uses_gain() && !p.gain_is_sane() {
        return Verdict::Redraw;
    }
    let allowed = if p.uses_gain() {
        (budget_sse as f64 * HEADROOM) as u64
    } else {
        budget_sse
    };
    match compensated_sse_bounded(prev, next, x, y, size, p, allowed) {
        Some(_) => Verdict::Accept(*p),
        None => Verdict::Redraw,
    }
}

/// Fit and verify in one call: the whole decision for one block.
pub fn compensate(
    prev: &Frame,
    next: &Frame,
    x: u32,
    y: u32,
    size: u32,
    budget_sse: u64,
) -> Verdict {
    verify(prev, next, x, y, size, &fit_block(prev, next, x, y, size), budget_sse)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame filled with a deterministic pattern that has real contrast in every channel.
    fn patterned(w: u32, h: u32) -> Frame {
        let mut f = Frame::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = [
                    ((x * 7 + y * 3) % 256) as u8,
                    ((x * 3 + y * 11) % 256) as u8,
                    ((x * 13 + y * 5) % 256) as u8,
                ];
                f.set_px(x, y, v);
            }
        }
        f
    }

    fn mapped(src: &Frame, p: &Photometric) -> Frame {
        let mut out = src.clone();
        for y in 0..src.height {
            for x in 0..src.width {
                let s = src.px(x, y);
                out.set_px(x, y, [p.map(0, s[0]), p.map(1, s[1]), p.map(2, s[2])]);
            }
        }
        out
    }

    /// The case the shipped model cannot express: a multiplicative fade. An additive offset
    /// fits it only where the two curves cross, so on a block with range it fails outright.
    #[test]
    fn a_multiplicative_fade_is_recovered_where_an_offset_cannot_be() {
        let prev = patterned(32, 32);
        let fade = Photometric { gain: [0.85; 3], offset: [0.0; 3] };
        let next = mapped(&prev, &fade);

        let fit = fit_block(&prev, &next, 0, 0, 32);
        for c in 0..3 {
            assert!((fit.gain[c] - 0.85).abs() < 0.02, "channel {c} gain {}", fit.gain[c]);
            assert!(fit.offset[c].abs() < 2.0, "channel {c} offset {}", fit.offset[c]);
        }
        // Residual after compensation is tiny — rounding only.
        let err = compensated_sse_bounded(&prev, &next, 0, 0, 32, &fit, u64::MAX).unwrap();
        assert!(err < 32 * 32 * 3, "compensated residual {err} is more than a level per subpixel");

        // The SHIPPED model — one additive scalar — cannot get close on the same block.
        let mean_diff = {
            let mut s = 0i64;
            for y in 0..32 {
                for x in 0..32 {
                    let (a, b) = (prev.px(x, y), next.px(x, y));
                    for c in 0..3 {
                        s += b[c] as i64 - a[c] as i64;
                    }
                }
            }
            (s as f64 / (32.0 * 32.0 * 3.0)).round() as i32
        };
        let offset_err =
            compensated_sse_bounded(&prev, &next, 0, 0, 32, &Photometric::offset_only(mean_diff), u64::MAX).unwrap();
        assert!(
            offset_err > err * 20,
            "the additive model was expected to do far worse on a multiply: {offset_err} vs {err}"
        );
    }

    /// A colour grade pushes channels in different directions. The shipped model sums over
    /// all three, so a grade that lifts blue and drops red averages to nothing and is
    /// invisible to it.
    #[test]
    fn a_per_channel_grade_is_fitted_per_channel() {
        let prev = patterned(32, 32);
        // Chosen NOT to clip: `map` clamps at 0 and 255, and a transform that clips is no
        // longer linear over the clipped range, so no linear fit can recover it. That is a
        // real property of the model, tested on its own below.
        let grade = Photometric { gain: [0.92, 1.0, 0.94], offset: [-6.0, 0.0, 8.0] };
        let next = mapped(&prev, &grade);
        let fit = fit_block(&prev, &next, 0, 0, 32);
        for c in 0..3 {
            assert!((fit.gain[c] - grade.gain[c]).abs() < 0.02, "channel {c}: {:?}", fit.gain);
            assert!((fit.offset[c] - grade.offset[c]).abs() < 2.0, "channel {c}: {:?}", fit.offset);
        }
        let err = compensated_sse_bounded(&prev, &next, 0, 0, 32, &fit, u64::MAX).unwrap();
        assert!(err < 32 * 32 * 3 * 2, "{err}");
    }

    /// A gained reuse is held to a STRICTER standard than an uncompensated one, because the
    /// gate cannot see the error introduced by applying the gain at output resolution to a
    /// block the network produced. The uncertain path must not get the same rope.
    #[test]
    fn gain_spends_only_part_of_the_budget_and_offset_only_spends_it_all() {
        let prev = patterned(16, 16);
        // A fade PLUS a little noise, so the linear fit has a real residual to spend the
        // budget on. A noiseless fade fits to within rounding, and a test whose residual is
        // zero cannot tell a strict budget from a generous one — which is how the first
        // version of this test passed while measuring nothing.
        let fade = Photometric { gain: [0.9; 3], offset: [0.0; 3] };
        let mut next = mapped(&prev, &fade);
        for y in 0..16u32 {
            for x in 0..16u32 {
                let v = next.px(x, y);
                let n = (((x * 5 + y * 7) % 7) as i32) - 3;
                next.set_px(x, y, [
                    (v[0] as i32 + n).clamp(0, 255) as u8,
                    (v[1] as i32 - n).clamp(0, 255) as u8,
                    (v[2] as i32 + n).clamp(0, 255) as u8,
                ]);
            }
        }
        let fit = fit_block(&prev, &next, 0, 0, 16);
        let err = compensated_sse_bounded(&prev, &next, 0, 0, 16, &fit, u64::MAX).unwrap();

        // A budget generous enough for the raw error but not for HEADROOM of it.
        assert!(err > 100, "the noise should leave a real residual to test against: {err}");
        // Generous enough for the raw error, not for HEADROOM of it.
        let budget = (err as f64 / HEADROOM) as u64 - 1;
        assert!(matches!(verify(&prev, &next, 0, 0, 16, &fit, budget), Verdict::Redraw),
                "a gained fit spent the whole budget");
        assert!(matches!(verify(&prev, &next, 0, 0, 16, &fit, budget * 3), Verdict::Accept(_)));

        // The shipped offset-only path is unaffected: it gets the full budget, so this
        // cannot regress what works today.
        let off = Photometric::offset_only(3);
        let off_err = compensated_sse_bounded(&prev, &next, 0, 0, 16, &off, u64::MAX).unwrap();
        assert!(matches!(verify(&prev, &next, 0, 0, 16, &off, off_err), Verdict::Accept(_)),
                "offset-only was denied its full budget");
    }

    /// A gain outside the window is refused outright rather than verified, because the
    /// low-resolution gate is not evidence about a network that is no longer locally linear.
    #[test]
    fn an_extreme_gain_is_refused_however_well_it_fits() {
        let prev = patterned(16, 16);
        let steep = Photometric { gain: [0.4; 3], offset: [0.0; 3] };
        let next = mapped(&prev, &steep);
        let fit = fit_block(&prev, &next, 0, 0, 16);
        assert!(!fit.gain_is_sane(), "0.4 should be outside the window: {:?}", fit.gain);
        // It fits the LOW-RES data almost perfectly, and is still refused.
        let err = compensated_sse_bounded(&prev, &next, 0, 0, 16, &fit, u64::MAX).unwrap();
        assert!(err < 16 * 16 * 3 * 2, "the fit really is good: {err}");
        assert_eq!(verify(&prev, &next, 0, 0, 16, &fit, u64::MAX), Verdict::Redraw);
    }

    /// A flat block has no contrast to fit a slope against — `a` and `b` trade off freely
    /// and the fit is arbitrary. Fall back to the mean difference, which is stable and is
    /// what ships today.
    #[test]
    fn a_flat_block_falls_back_to_an_offset() {
        let mut prev = Frame::new(16, 16);
        prev.data.iter_mut().for_each(|b| *b = 128);
        let mut next = Frame::new(16, 16);
        next.data.iter_mut().for_each(|b| *b = 138);
        let fit = fit_block(&prev, &next, 0, 0, 16);
        assert!(!fit.uses_gain(), "a slope was fitted to a flat block: {:?}", fit.gain);
        for c in 0..3 {
            assert!((fit.offset[c] - 10.0).abs() < 0.6, "{:?}", fit.offset);
        }
        assert!(matches!(verify(&prev, &next, 0, 0, 16, &fit, 64), Verdict::Accept(_)));
    }

    /// Round trip, and the case that has no inverse: a fade that reached black cannot be
    /// undone, and pretending otherwise would amplify quantisation into bands.
    #[test]
    fn the_transform_inverts_except_where_it_has_collapsed() {
        let p = Photometric { gain: [0.9, 1.1, 1.0], offset: [4.0, -3.0, 0.0] };
        let inv = p.inverse().expect("invertible");
        // Values that do not hit the clamp under this transform — see `clipping_is_not_invertible`.
        for v in [10u8, 40, 128, 200] {
            for c in 0..3 {
                let back = inv.map(c, p.map(c, v)) as i32;
                assert!((back - v as i32).abs() <= 2, "channel {c} at {v} came back {back}");
            }
        }
        assert!(Photometric { gain: [0.0; 3], offset: [0.0; 3] }.inverse().is_none());
        assert!(Photometric::IDENTITY.is_identity());
        assert!(!Photometric::offset_only(5).uses_gain());
    }

    /// CLIPPING BREAKS THE MODEL, and the verification is what notices.
    ///
    /// `map` clamps at 0 and 255. A grade that pushes a channel past white is no longer
    /// `a·x + b` over the clipped range — the information is gone — so no linear fit can
    /// recover it and no inverse exists for those pixels. Found by a test of mine that
    /// asserted a fitted gain of 1.08 and got 1.05, because a third of the block had
    /// saturated.
    #[test]
    fn clipping_is_not_invertible_and_the_gate_catches_it() {
        let prev = patterned(32, 32);
        let clipping = Photometric { gain: [1.6; 3], offset: [0.0; 3] };
        let next = mapped(&prev, &clipping);
        let fit = fit_block(&prev, &next, 0, 0, 32);
        // The fit is pulled well below the true gain by the saturated pixels.
        assert!(fit.gain[0] < 1.5, "the clipped data should drag the slope down: {:?}", fit.gain);
        // And it is refused — on the RESIDUAL, against a realistic budget. (Not on the
        // gain window: the saturated pixels drag the fitted slope back inside it, which is
        // the trap. `u64::MAX` here would accept anything, which is what my first version
        // of this assertion did.)
        let budget = 32 * 32 * 3 * 4; // ~2 levels per subpixel, generous
        assert_eq!(verify(&prev, &next, 0, 0, 32, &fit, budget), Verdict::Redraw);
        let err = compensated_sse_bounded(&prev, &next, 0, 0, 32, &fit, u64::MAX).unwrap();
        assert!(err > budget, "clipping should leave a residual the gate refuses: {err} vs {budget}");

        // A genuine, non-clipping fade of similar magnitude passes the same budget — so
        // this is refusing the CLIPPING and not just refusing brightness changes.
        let clean = Photometric { gain: [0.8; 3], offset: [0.0; 3] };
        let next = mapped(&prev, &clean);
        let fit = fit_block(&prev, &next, 0, 0, 32);
        assert!(matches!(verify(&prev, &next, 0, 0, 32, &fit, budget), Verdict::Accept(_)));
    }

    /// An unrelated frame is not a photometric change, and must be redrawn rather than
    /// fitted. Two numbers per channel cannot describe different content — but least squares
    /// will always return SOMETHING, so the verification is what refuses it.
    #[test]
    fn different_content_is_redrawn_not_fitted() {
        let prev = patterned(32, 32);
        let mut next = Frame::new(32, 32);
        for y in 0..32u32 {
            for x in 0..32u32 {
                next.set_px(x, y, [((x * 29 + y * 17) % 256) as u8, ((y * 23) % 256) as u8, 30]);
            }
        }
        // A budget that a genuine fade of this block would pass comfortably.
        let budget = 32 * 32 * 3 * 16;
        assert_eq!(compensate(&prev, &next, 0, 0, 32, budget), Verdict::Redraw);
    }
}
