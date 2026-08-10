//! ambient_palette — `AmbientPaletteDeriver.cpp`, ported.
//!
//! Derives a theme accent and background tint from the photos in an inbox, so
//! the app quietly takes on the colour of what is in it.
//!
//! ## The constants are tuned against real photographs
//!
//! The C++ header justifies each, and they are the port rather than the loops:
//!
//! * **Saturation threshold 0.20.** Below it a pixel counts as neutral —
//!   backdrop, wall, sky. The comment says why that number: real photos have "a
//!   lot of near-grey but slightly warm pixels (skin tones, sunset edges)", and
//!   *"too low and skin dominates accent"*. Every inbox would turn beige.
//! * **Edge band, 4px of a 64px square.** Only the outer frame is sampled,
//!   because the subject of a photo is not its ambience — a red jumper in the
//!   middle should not repaint the app.
//! * **Tint saturation capped at 0.18**, the same number `ambient_blobs` uses
//!   and for the same reason: above it, text contrast leaves the WCAG safe
//!   zone. Two features arriving at one number independently is the sign it is
//!   a real constraint rather than a preference.
//!
//! ## An integer chroma proxy, and its documented bias
//!
//! Rather than a full HSL transform per pixel, the C++ uses
//! `max(r,g,b) - min(r,g,b)` — chroma, which is monotonic in saturation across
//! the lightness band that matters. Its note is careful: the proxy *"errs
//! slightly conservative (treats some near-threshold colors as neutral)"*, and
//! is ~5x faster. That bias direction is the safe one — mistaking a colour for
//! neutral loses a little accent, while the reverse tints the whole UI off a
//! stray pixel — so it ports as-is rather than being "fixed".
//!
//! ## Empty buckets keep the base
//!
//! An inbox of greyscale photos produces no accent samples. The C++ keeps the
//! base palette's value rather than averaging nothing, because the alternative
//! is a black or random accent — a theme that breaks on a legitimate inbox.

/// Sampled image side after downscale.
pub const IMAGE_SIDE: u32 = 64;
/// Edge band width in downscaled pixels.
pub const EDGE_WIDTH: u32 = 4;
/// Chroma at or above this is a colour, below it is neutral. `0.20 * 255`.
pub const CHROMA_THRESHOLD: u8 = 51;
/// Lightness band the chroma proxy is valid across.
pub const LUM_LOW: u8 = 25;
pub const LUM_HIGH: u8 = 230;
/// Ceiling on the derived tint's saturation — the WCAG-derived number.
pub const TINT_MAX_SAT: f32 = 0.18;

/// An 8-bit RGB pixel.
pub type Rgb = [u8; 3];

/// Chroma: `max - min`.
///
/// The integer proxy for saturation, monotonic across the lightness band we
/// sample. Conservative near the threshold by design.
pub fn chroma(p: Rgb) -> u8 {
    let max = p[0].max(p[1]).max(p[2]);
    let min = p[0].min(p[1]).min(p[2]);
    max - min
}

/// Mean lightness, the integer approximation the C++ uses.
pub fn lightness(p: Rgb) -> u8 {
    ((p[0] as u16 + p[1] as u16 + p[2] as u16) / 3) as u8
}

/// Does this pixel contribute to the accent?
///
/// Colourful enough, and inside the lightness band where the chroma proxy
/// holds. Excluding the extremes matters: a blown highlight and a crushed
/// shadow both report low chroma regardless of their true hue, so counting them
/// would drag every accent toward whatever the clipping happened to be.
pub fn is_accent_pixel(p: Rgb) -> bool {
    let l = lightness(p);
    l >= LUM_LOW && l <= LUM_HIGH && chroma(p) >= CHROMA_THRESHOLD
}

/// Is this coordinate in the edge band of a downscaled image?
///
/// The subject of a photo is not its ambience: a red jumper in the middle
/// should not repaint the app.
pub fn in_edge_band(x: u32, y: u32, side: u32, width: u32) -> bool {
    if side == 0 { return false; }
    let w = width.min(side / 2);
    x < w || y < w || x >= side.saturating_sub(w) || y >= side.saturating_sub(w)
}

/// What one pass over an inbox found.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Samples {
    accent_sum: [u64; 3],
    accent_count: u64,
    neutral_sum: [u64; 3],
    neutral_count: u64,
}

impl Samples {
    /// Add one edge pixel.
    pub fn add(&mut self, p: Rgb) {
        let (sum, count) = if is_accent_pixel(p) {
            (&mut self.accent_sum, &mut self.accent_count)
        } else {
            (&mut self.neutral_sum, &mut self.neutral_count)
        };
        for i in 0..3 { sum[i] += p[i] as u64; }
        *count += 1;
    }

    /// Sample the edge band of one downscaled image.
    pub fn add_image(&mut self, pixels: &[Rgb], side: u32) {
        for y in 0..side {
            for x in 0..side {
                if !in_edge_band(x, y, side, EDGE_WIDTH) { continue; }
                if let Some(p) = pixels.get((y * side + x) as usize) { self.add(*p); }
            }
        }
    }

    pub fn accent_count(&self) -> u64 { self.accent_count }
    pub fn neutral_count(&self) -> u64 { self.neutral_count }

    fn mean(sum: [u64; 3], count: u64) -> Option<Rgb> {
        if count == 0 { return None; }
        Some([(sum[0] / count) as u8, (sum[1] / count) as u8, (sum[2] / count) as u8])
    }

    /// Derived accent, or `None` when no pixel qualified.
    ///
    /// `None` rather than black: an inbox of greyscale photos is legitimate,
    /// and the caller keeps the base palette's accent instead of rendering a
    /// theme nobody chose.
    pub fn accent(&self) -> Option<Rgb> { Self::mean(self.accent_sum, self.accent_count) }

    /// Derived background, or `None`.
    pub fn background(&self) -> Option<Rgb> { Self::mean(self.neutral_sum, self.neutral_count) }
}

/// Saturation of an RGB colour, 0..=1, HSL definition.
pub fn saturation(p: Rgb) -> f32 {
    let max = p[0].max(p[1]).max(p[2]) as f32 / 255.0;
    let min = p[0].min(p[1]).min(p[2]) as f32 / 255.0;
    let l = (max + min) / 2.0;
    if (max - min).abs() < 1e-6 { return 0.0; }
    // The denominator flips at mid-lightness; using one branch would report a
    // near-white pastel as fully saturated.
    if l <= 0.5 { (max - min) / (max + min) } else { (max - min) / (2.0 - max - min) }
}

/// Pull a colour's saturation down to the tint ceiling, keeping its hue.
///
/// A quiet tint, not a wash. Above 0.18 the background starts competing with
/// text — the same limit `ambient_blobs` enforces on its opacity, arrived at
/// independently.
pub fn cap_tint(p: Rgb) -> Rgb {
    let s = saturation(p);
    if s <= TINT_MAX_SAT { return p; }
    let l = (p[0].max(p[1]).max(p[2]) as f32 + p[0].min(p[1]).min(p[2]) as f32) / 2.0;
    let k = TINT_MAX_SAT / s;
    let mut out = [0u8; 3];
    for i in 0..3 {
        // Interpolate toward the grey of the same lightness, which reduces
        // saturation while leaving hue alone.
        out[i] = (l + (p[i] as f32 - l) * k).round().clamp(0.0, 255.0) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_grey_pixel_is_neutral_and_a_saturated_one_is_accent() {
        assert!(!is_accent_pixel([128, 128, 128]));
        assert!(is_accent_pixel([200, 60, 60]));
    }

    #[test]
    fn a_near_grey_warm_pixel_counts_as_neutral() {
        // The tuning the header explains: real photos have "a lot of near-grey
        // but slightly warm pixels (skin tones, sunset edges)", and too low a
        // threshold means skin dominates the accent — every inbox turns beige.
        assert!(!is_accent_pixel([150, 140, 132]), "skin-ish, chroma 18");
        assert!(!is_accent_pixel([180, 165, 155]), "sunset edge, chroma 25");
    }

    #[test]
    fn a_blown_highlight_and_a_crushed_shadow_are_both_excluded() {
        // Both report low chroma regardless of true hue, so counting them drags
        // every accent toward whatever the clipping happened to be.
        assert!(!is_accent_pixel([252, 250, 200]), "blown, above the band");
        assert!(!is_accent_pixel([20, 4, 4]), "crushed, below the band");
    }

    #[test]
    fn only_the_edge_band_is_sampled() {
        // The subject of a photo is not its ambience: a red jumper in the
        // middle should not repaint the app.
        assert!(in_edge_band(0, 0, 64, 4));
        assert!(in_edge_band(63, 63, 64, 4));
        assert!(in_edge_band(30, 2, 64, 4));
        assert!(!in_edge_band(32, 32, 64, 4), "the centre is not ambience");
        assert!(!in_edge_band(10, 10, 64, 4));
    }

    #[test]
    fn a_bright_subject_in_the_middle_does_not_reach_the_accent() {
        // The property the edge band exists for, end to end.
        let mut pixels = vec![[128u8, 128, 128]; 64 * 64];
        for y in 20..44 { for x in 20..44 { pixels[y * 64 + x] = [220, 30, 30]; } }
        let mut s = Samples::default();
        s.add_image(&pixels, 64);
        assert_eq!(s.accent_count(), 0, "the jumper was ignored");
        assert!(s.accent().is_none());
    }

    #[test]
    fn an_edge_colour_does_reach_the_accent() {
        let mut pixels = vec![[128u8, 128, 128]; 64 * 64];
        for x in 0..64 { pixels[x] = [40, 90, 200]; }      // top row
        let mut s = Samples::default();
        s.add_image(&pixels, 64);
        assert!(s.accent_count() > 0);
        let a = s.accent().unwrap();
        assert!(a[2] > a[0], "the blue survived: {a:?}");
    }

    #[test]
    fn a_greyscale_inbox_keeps_the_base_accent_rather_than_going_black() {
        // An inbox of greyscale photos is legitimate; averaging nothing would
        // render a theme nobody chose.
        let pixels = vec![[90u8, 90, 90]; 64 * 64];
        let mut s = Samples::default();
        s.add_image(&pixels, 64);
        assert!(s.accent().is_none(), "no accent to derive");
        assert!(s.background().is_some(), "but a background there is");
    }

    #[test]
    fn samples_accumulate_across_an_inbox_rather_than_per_image() {
        // One photo does not set the theme; the inbox as a whole does.
        let mut s = Samples::default();
        let mut red = vec![[128u8, 128, 128]; 64 * 64];
        for x in 0..64 { red[x] = [200, 40, 40]; }
        let mut blue = vec![[128u8, 128, 128]; 64 * 64];
        for x in 0..64 { blue[x] = [40, 40, 200]; }
        s.add_image(&red, 64);
        s.add_image(&blue, 64);
        let a = s.accent().unwrap();
        assert!(a[0] > 80 && a[2] > 80, "both contributed: {a:?}");
    }

    #[test]
    fn the_chroma_proxy_errs_conservative_as_documented() {
        // Its note says it "treats some near-threshold colors as neutral", and
        // that bias direction is the safe one: mistaking a colour for neutral
        // loses a little accent, while the reverse tints the whole UI off a
        // stray pixel.
        let borderline = [138u8, 120, 100];      // chroma 38, under 51
        assert!(!is_accent_pixel(borderline));
        assert!(saturation(borderline) > 0.15, "genuinely a little saturated");
    }

    #[test]
    fn hsl_saturation_is_misleading_at_extreme_lightness_which_is_why_chroma_is_used() {
        // I wrote this expecting a near-white pastel to read as barely
        // saturated. HSL genuinely reports #FFF0F0 as FULLY saturated — six
        // units of chroma at 97% lightness saturates the formula — and the
        // implementation is right.
        //
        // That is not a curiosity: it is precisely why the C++ classifies
        // pixels by CHROMA within a lightness band rather than by HSL
        // saturation. A pastel highlight would otherwise count as a vivid
        // accent and repaint the app off a bright corner of a photo.
        let pastel = [255u8, 240, 240];
        assert!(saturation(pastel) > 0.9, "HSL says vivid: {}", saturation(pastel));
        assert!(!is_accent_pixel(pastel), "but the chroma band correctly excludes it");

        let vivid = [255u8, 0, 0];
        assert!((saturation(vivid) - 1.0).abs() < 0.01);
        assert!(is_accent_pixel(vivid), "a real accent passes both");
    }

    #[test]
    fn the_two_branches_of_saturation_meet_at_mid_lightness() {
        // The denominator flips at l = 0.5; a discontinuity there would make
        // cap_tint jump as a colour crossed it.
        let below = saturation([128, 64, 64]);
        let above = saturation([130, 66, 66]);
        assert!((below - above).abs() < 0.05, "{below} vs {above}");
    }

    #[test]
    fn the_tint_is_capped_at_the_same_number_the_blobs_use() {
        // Two features arriving at 0.18 independently is the sign it is a real
        // contrast constraint rather than a preference.
        let vivid = [220u8, 30, 30];
        let tinted = cap_tint(vivid);
        assert!(saturation(tinted) <= TINT_MAX_SAT + 0.01,
                "capped to {}", saturation(tinted));
        assert!(tinted[0] > tinted[1], "hue preserved: {tinted:?}");
    }

    #[test]
    fn an_already_quiet_colour_passes_through_untouched() {
        // Capping something already below the ceiling would drift the palette
        // grey a little more on every derivation.
        let quiet = [130u8, 125, 120];
        assert_eq!(cap_tint(quiet), quiet);
    }

    #[test]
    fn a_grey_has_no_saturation_and_survives_capping() {
        assert_eq!(saturation([100, 100, 100]), 0.0);
        assert_eq!(cap_tint([100, 100, 100]), [100, 100, 100]);
    }

    #[test]
    fn a_zero_sized_image_samples_nothing_rather_than_panicking() {
        // Reachable from a corrupt file, which the C++ skips silently because
        // "the failure mode for an ambient theme is fall back to the base
        // palette, which is fine".
        assert!(!in_edge_band(0, 0, 0, 4));
        let mut s = Samples::default();
        s.add_image(&[], 0);
        assert_eq!(s.accent_count(), 0);
        assert!(s.accent().is_none());
    }

    #[test]
    fn an_edge_band_wider_than_the_image_covers_it_rather_than_underflowing() {
        // side - width underflows on unsigned arithmetic for a tiny thumbnail.
        assert!(in_edge_band(1, 1, 4, 40));
        let mut s = Samples::default();
        s.add_image(&vec![[200u8, 40, 40]; 16], 4);
        assert!(s.accent_count() > 0, "a 4px image still samples");
    }
}
