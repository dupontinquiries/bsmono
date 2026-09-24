//! A frosted-glass blur that a CPU can afford.
//!
//! Not for the lock-screen wash — that needs no blur at all. A Gaussian
//! followed by heavy downsampling IS downsampling: the average of a cell is
//! the low pass, so [`crate::camera::glass`] plus the lattice already
//! produces what a large-sigma blur would, and blurring first would compute
//! the same answer twice. A blurred image at big sigma has no content above
//! its cutoff, so a low-resolution buffer is the blur's entire information
//! content.
//!
//! This is for the other case: a frosted panel over arbitrary content, where
//! the source is whatever happens to be underneath and cannot be chosen.
//!
//! ## The recipe, and why each step
//!
//! 1. **Downsample first.** Sigma comes free in proportion to the factor,
//!    and every later pass costs 16x or 64x less. Blurring at full
//!    resolution and then throwing the detail away is the common mistake.
//! 2. **Three box passes, not a Gaussian kernel.** Three boxes converge on a
//!    Gaussian to within about a percent
//!    ([`box_widths`]), and a box pass is a running sum — two adds per
//!    pixel, independent of radius. A true Gaussian is a multiply-add per
//!    tap per pixel.
//! 3. **Separable.** Horizontal then vertical, so a WxH blur is 2(W+H) work
//!    rather than W*H.
//! 4. **Upsample bilinearly, then dither.** The dither goes LAST, and after
//!    any dimming: a veil multiplies the image toward black, which amplifies
//!    banding rather than hiding it, so grain applied before the veil is
//!    grain that has been dimmed away exactly where it was needed.

/// Widths of three successive box passes that together approximate a
/// Gaussian of standard deviation `sigma`.
///
/// Accurate to a few percent from about sigma 2 upward. Below that the
/// available widths are too coarse — the choice is between 1 and 3 — and the
/// result can be off by a fifth. That is fine for frost, which is never
/// subtle, and it is why the test starts at 2.
///
/// From the standard result that n boxes of width w have variance
/// n(w^2 - 1)/12: solve for w, then split the budget between the two
/// nearest odd widths so the total variance lands on target instead of
/// rounding three times in the same direction.
pub fn box_widths(sigma: f32) -> [usize; 3] {
    if sigma <= 0.0 {
        return [1, 1, 1];
    }
    let n = 3.0_f32;
    let ideal = ((12.0 * sigma * sigma / n) + 1.0).sqrt();
    let mut wl = ideal.floor() as i32;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    // How many of the three passes take the smaller width.
    let m = ((12.0 * sigma * sigma - (n * wl as f32 * wl as f32) - (4.0 * n * wl as f32) - (3.0 * n)) / (-4.0 * wl as f32 - 4.0)).round();
    let m = m.clamp(0.0, n) as usize;
    let mut out = [1usize; 3];
    for (i, o) in out.iter_mut().enumerate() {
        *o = if i < m { wl.max(1) as usize } else { wu.max(1) as usize };
    }
    out
}

/// One separable box pass over an RGBA buffer, in place, using a running
/// sum: cost per pixel does not grow with the radius.
pub fn box_pass(px: &mut [u8], w: usize, h: usize, radius: usize) {
    if radius == 0 || w == 0 || h == 0 || px.len() < w * h * 4 {
        return;
    }
    let mut tmp = vec![0u8; w * h * 4];
    blur_rows(px, &mut tmp, w, h, radius);
    // Vertical pass is the horizontal one on the transpose; done in place by
    // striding rather than actually transposing.
    blur_cols(&tmp, px, w, h, radius);
}

fn blur_rows(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let span = (2 * r + 1) as u32;
    for y in 0..h {
        let row = y * w * 4;
        for c in 0..4 {
            // Prime the window with CLAMPED reads on both sides, so it is
            // always 2r+1 wide. Priming with only the pixels that exist
            // leaves a short window divided by the full span, which darkens
            // every row narrower than the radius — the whole image, for a
            // small surface.
            let mut sum: u32 = src[row + c] as u32 * (r as u32 + 1);
            for x in 1..=r {
                sum += src[row + x.min(w - 1) * 4 + c] as u32;
            }
            for x in 0..w {
                let add = src[row + (x + r).min(w - 1) * 4 + c] as u32;
                let sub = src[row + x.saturating_sub(r + 1).min(w - 1) * 4 + c] as u32;
                dst[row + x * 4 + c] = (sum / span) as u8;
                sum = sum + add - sub;
            }
        }
    }
}

fn blur_cols(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize) {
    let span = (2 * r + 1) as u32;
    for x in 0..w {
        for c in 0..4 {
            let at = |y: usize| src[y * w * 4 + x * 4 + c] as u32;
            let mut sum: u32 = at(0) * (r as u32 + 1);
            for y in 1..=r {
                sum += at(y.min(h - 1));
            }
            for y in 0..h {
                let add = at((y + r).min(h - 1));
                let sub = at(y.saturating_sub(r + 1).min(h - 1));
                dst[y * w * 4 + x * 4 + c] = (sum / span) as u8;
                sum = sum + add - sub;
            }
        }
    }
}

/// Blur `px` (RGBA, `w` x `h`) to approximately `sigma`, three box passes.
pub fn frost(px: &mut [u8], w: usize, h: usize, sigma: f32) {
    for width in box_widths(sigma) {
        box_pass(px, w, h, width / 2);
    }
}

/// The sigma a downsample by `factor` provides for free, so callers can ask
/// for the blur they want and subtract what the scaling already did.
pub fn sigma_from_downsample(factor: usize) -> f32 {
    // A box average over `factor` pixels has variance (factor^2 - 1)/12.
    if factor <= 1 {
        0.0
    } else {
        (((factor * factor - 1) as f32) / 12.0).sqrt()
    }
}

/// What is left to do after downsampling by `factor`, in the SMALL image's
/// own pixels. Returns 0 when the scaling already did enough.
pub fn remaining_sigma(target: f32, factor: usize) -> f32 {
    let have = sigma_from_downsample(factor);
    let small_target = target / factor.max(1) as f32;
    (small_target * small_target - (have / factor.max(1) as f32).powi(2)).max(0.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, v: u8) -> Vec<u8> {
        vec![v; w * h * 4]
    }

    #[test]
    fn three_boxes_land_on_the_requested_sigma() {
        for sigma in [2.0f32, 2.5, 8.0, 20.0] {
            let ws = box_widths(sigma);
            // Variance of n boxes: sum (w^2 - 1)/12.
            let var: f32 = ws.iter().map(|w| ((*w * *w) as f32 - 1.0) / 12.0).sum();
            let got = var.sqrt();
            assert!((got - sigma).abs() / sigma < 0.12, "sigma {sigma}: widths {ws:?} give {got}");
            assert!(ws.iter().all(|w| w % 2 == 1), "box widths must be odd to stay centred: {ws:?}");
        }
        assert_eq!(box_widths(0.0), [1, 1, 1]);
    }

    #[test]
    fn a_flat_image_survives_untouched() {
        // The classic blur bug is a darkened border, from a window that
        // reads zeros past the edge. It reads as a vignette.
        let (w, h) = (16, 12);
        let mut px = flat(w, h, 200);
        frost(&mut px, w, h, 4.0);
        assert!(px.iter().all(|v| *v >= 199 && *v <= 200), "edges darkened: min {}", px.iter().min().unwrap());
    }

    #[test]
    fn energy_moves_but_is_not_lost() {
        let (w, h) = (32, 32);
        let mut px = flat(w, h, 0);
        // One bright block in the middle.
        for y in 14..18 {
            for x in 14..18 {
                for c in 0..4 {
                    px[(y * w + x) * 4 + c] = 255;
                }
            }
        }
        let before: u64 = px.iter().map(|v| *v as u64).sum();
        frost(&mut px, w, h, 3.0);
        let after: u64 = px.iter().map(|v| *v as u64).sum();
        // Integer division loses a little; it must not lose much.
        assert!(after as f64 > before as f64 * 0.9, "{before} -> {after}");
        // And it actually spread: the centre is dimmer, a neighbour brighter.
        assert!(px[(16 * w + 16) * 4] < 255);
        assert!(px[(16 * w + 22) * 4] > 0);
    }

    #[test]
    fn downsampling_is_counted_as_blur_already_done() {
        // Blurring at full resolution and then scaling down is the mistake
        // this exists to prevent.
        assert_eq!(sigma_from_downsample(1), 0.0);
        assert!(sigma_from_downsample(4) > sigma_from_downsample(2));
        // Asking for less than the scaling already gives needs no pass.
        assert_eq!(remaining_sigma(1.0, 8), 0.0);
        // Asking for much more still needs work, in the small image's units.
        let r = remaining_sigma(64.0, 4);
        assert!(r > 0.0 && r < 64.0, "{r}");
    }

    #[test]
    fn degenerate_inputs_do_not_panic_or_corrupt() {
        let mut empty: Vec<u8> = Vec::new();
        frost(&mut empty, 0, 0, 5.0);
        let mut one = flat(1, 1, 77);
        frost(&mut one, 1, 1, 9.0);
        assert_eq!(one, flat(1, 1, 77), "a single pixel is its own blur");
        let mut small = flat(2, 2, 10);
        frost(&mut small, 2, 2, 50.0);
        assert!(small.iter().all(|v| *v == 10));
        // A buffer shorter than it claims is refused rather than read past.
        let mut short = vec![0u8; 8];
        box_pass(&mut short, 100, 100, 3);
    }
}
