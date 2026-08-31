//! cv — primitives that produce tags from pixels.
//!
//! ## Why this exists
//!
//! `Entry::scan` accepts tags and produces none. So the tag half of the
//! search — the half that knows what a thing IS, and the half weighted
//! highest — only works for photos somebody labelled by hand. In a real
//! library that is approximately none of them.
//!
//! These are the cheap, explainable measurements: edge density,
//! brightness, contrast, the distribution of mass across the frame.
//! Each produces a tag a person could have written, and each is
//! computed the same way every time.
//!
//! ## What these are NOT
//!
//! Not object recognition. Nothing here knows what a dog is. The names
//! are about the IMAGE — `dark`, `busy`, `centred`, `portrait` — not
//! about its subject, and the module is careful to name them that way
//! so nobody reads `busy` as a claim about content.
//!
//! That matters for the search. `similarity` weights tags highest
//! because a tag is the only signal that knows what a thing is — but
//! these tags do not, so an auto-tagged library gets composition
//! matching with a semantic weighting. `auto_weight` exists for exactly
//! that reason: derived tags are worth less than written ones and the
//! caller has to be able to say so.
//!
//! ## On faces
//!
//! There is no face detector here and there is not going to be a fake
//! one. A skin-tone-and-oval heuristic finds faces in about the same
//! proportion of photos as it finds wooden doors, and a library
//! confidently mislabelled is worse than one not labelled at all —
//! because the mislabelling is invisible until someone searches for a
//! person and gets furniture. Faces want a real model behind
//! `Entry::scan`'s seam.

use crate::Gray;

/// Measurements of one image. All 0..1 unless stated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Features {
    /// Mean brightness.
    pub brightness: f32,
    /// Spread of brightness — RMS around the mean, normalised.
    pub contrast: f32,
    /// Fraction of pixels sitting on an edge.
    pub edges: f32,
    /// How much of the visual mass is in the middle third.
    pub centrality: f32,
    /// Width / height. Not 0..1.
    pub aspect: f32,
}

/// A pixel differing from its right or lower neighbour by more than
/// this counts as an edge.
///
/// 24 of 255. Below about 16 the sensor noise in a dark photo reads as
/// texture and every night shot scores as "busy"; well above 30 and a
/// soft-focus portrait registers as empty. Tuned to the property that
/// matters — that `busy` and `plain` land on different photos — rather
/// than to a reference image.
pub const EDGE_THRESHOLD: i32 = 24;

pub fn features(img: &Gray) -> Features {
    let n = (img.w * img.h) as f32;
    let sum: f32 = img.px.iter().map(|p| *p as f32).sum();
    let mean = sum / n;

    let var: f32 = img.px.iter().map(|p| {
        let d = *p as f32 - mean;
        d * d
    }).sum::<f32>() / n;
    // Divided by 128 rather than 255: a real photograph never spans the
    // full range, so normalising by 255 would leave every contrast
    // score bunched near zero and unable to separate anything.
    let contrast = (var.sqrt() / 128.0).min(1.0);

    let mut edge = 0u32;
    let mut centre_mass = 0f32;
    let mut total_mass = 0f32;
    let (x_lo, x_hi) = (img.w / 3, img.w * 2 / 3);
    let (y_lo, y_hi) = (img.h / 3, img.h * 2 / 3);

    for y in 0..img.h {
        for x in 0..img.w {
            let p = img.px[y * img.w + x] as i32;
            if x + 1 < img.w {
                let r = img.px[y * img.w + x + 1] as i32;
                if (p - r).abs() > EDGE_THRESHOLD {
                    edge += 1;
                }
            }
            if y + 1 < img.h {
                let d = img.px[(y + 1) * img.w + x] as i32;
                if (p - d).abs() > EDGE_THRESHOLD {
                    edge += 1;
                }
            }
            // Mass is measured as DEVIATION from the mean, not
            // brightness. A photo of a dark subject on a light
            // background has its subject in the middle and its
            // brightness at the edges; scoring by brightness would call
            // that composition edge-heavy.
            let m = (p as f32 - mean).abs();
            total_mass += m;
            if (x_lo..x_hi).contains(&x) && (y_lo..y_hi).contains(&y) {
                centre_mass += m;
            }
        }
    }

    Features {
        brightness: mean / 255.0,
        contrast,
        // Two comparisons per pixel, so the denominator is 2n.
        edges: (edge as f32 / (2.0 * n)).min(1.0),
        centrality: if total_mass > 0.0 { centre_mass / total_mass } else { 0.0 },
        aspect: img.w as f32 / img.h as f32,
    }
}

/// The share of the frame the middle third occupies.
///
/// A uniform image has this much of its mass in the centre by geometry
/// alone, so `centrality` is only interesting relative to it — calling
/// a flat grey photo "centred" because 1/9th of it is in the middle
/// would tag every photo in the library.
pub const UNIFORM_CENTRALITY: f32 = 1.0 / 9.0;

/// Tags derived from the pixels.
///
/// Every name describes the IMAGE, never its subject. Sorted and
/// deduplicated so they drop straight into `Entry`.
pub fn auto_tags(f: &Features) -> Vec<String> {
    let mut t: Vec<String> = Vec::new();

    // Thresholds are deliberately not symmetric around 0.5: photographs
    // cluster in the middle, and a symmetric split would tag half the
    // library "dark" and the other half "bright", which distinguishes
    // nothing.
    if f.brightness < 0.25 {
        t.push("dark".into());
    } else if f.brightness > 0.75 {
        t.push("bright".into());
    }

    if f.contrast < 0.10 {
        t.push("flat".into());
    } else if f.contrast > 0.45 {
        t.push("high-contrast".into());
    }

    if f.edges > 0.20 {
        t.push("busy".into());
    } else if f.edges < 0.03 {
        t.push("plain".into());
    }

    // Twice the uniform share before it counts as composed.
    if f.centrality > UNIFORM_CENTRALITY * 2.0 {
        t.push("centred".into());
    }

    if f.aspect > 1.2 {
        t.push("landscape".into());
    } else if f.aspect < 0.85 {
        t.push("portrait".into());
    } else {
        t.push("square".into());
    }

    t.sort();
    t.dedup();
    t
}

/// What a derived tag is worth against a written one.
///
/// Half. `similarity` weights tags highest because a tag knows what a
/// thing IS — and these do not, they know what the image LOOKS like. A
/// library auto-tagged at full weight gets composition matching
/// presented as semantic matching, which is the failure mode this whole
/// module has to avoid.
pub const AUTO_TAG_WEIGHT: f32 = 0.5;

/// Prefix marking a tag as machine-derived.
///
/// So `tag_similarity` can never confuse "somebody said this is a
/// portrait of Ada" with "the frame is taller than it is wide". Same
/// word, entirely different evidence.
pub const AUTO_PREFIX: &str = "auto:";

/// Auto tags, prefixed and ready for `Entry::scan`.
pub fn auto_tags_prefixed(f: &Features) -> Vec<String> {
    auto_tags(f).into_iter().map(|t| format!("{AUTO_PREFIX}{t}")).collect()
}

/// Is this tag machine-derived?
pub fn is_auto(tag: &str) -> bool {
    tag.starts_with(AUTO_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        (0..w * h).map(|i| f(i % w, i / w)).collect()
    }

    fn feats(w: usize, h: usize, px: &[u8]) -> Features {
        features(&Gray::new(w, h, px).unwrap())
    }

    #[test]
    fn a_dark_photo_is_dark_and_a_bright_one_is_bright() {
        let dark = img(16, 16, |_, _| 20);
        let bright = img(16, 16, |_, _| 240);
        assert!(auto_tags(&feats(16, 16, &dark)).contains(&"dark".to_string()));
        assert!(auto_tags(&feats(16, 16, &bright)).contains(&"bright".to_string()));
    }

    #[test]
    fn a_mid_grey_photo_is_neither_dark_nor_bright() {
        // The thresholds are asymmetric on purpose: photographs cluster
        // in the middle, and a 0.5 split would tag half the library
        // "dark" and half "bright", which separates nothing.
        let mid = img(16, 16, |_, _| 128);
        let t = auto_tags(&feats(16, 16, &mid));
        assert!(!t.contains(&"dark".to_string()), "{t:?}");
        assert!(!t.contains(&"bright".to_string()), "{t:?}");
    }

    #[test]
    fn a_flat_image_has_no_edges_and_a_checkerboard_is_busy() {
        let flat = img(16, 16, |_, _| 128);
        let check = img(16, 16, |x, y| if (x + y) % 2 == 0 { 0 } else { 255 });
        assert!(auto_tags(&feats(16, 16, &flat)).contains(&"plain".to_string()));
        assert!(auto_tags(&feats(16, 16, &check)).contains(&"busy".to_string()));
    }

    #[test]
    fn a_uniform_image_is_not_called_centred() {
        // A flat photo has a ninth of its mass in the middle third by
        // geometry alone. Tagging that "centred" would tag everything.
        let flat = img(30, 30, |_, _| 100);
        let t = auto_tags(&feats(30, 30, &flat));
        assert!(!t.contains(&"centred".to_string()), "{t:?}");
    }

    #[test]
    fn a_subject_in_the_middle_is_centred() {
        let spot = img(30, 30, |x, y| {
            if (10..20).contains(&x) && (10..20).contains(&y) { 255 } else { 0 }
        });
        let t = auto_tags(&feats(30, 30, &spot));
        assert!(t.contains(&"centred".to_string()), "{t:?}");
    }

    #[test]
    fn a_dark_subject_on_a_light_background_still_reads_as_centred() {
        // Mass is deviation from the mean, not brightness. Measuring
        // brightness would call this composition edge-heavy, because the
        // bright pixels are all round the outside.
        let inv = img(30, 30, |x, y| {
            if (10..20).contains(&x) && (10..20).contains(&y) { 0 } else { 255 }
        });
        let f = feats(30, 30, &inv);
        assert!(f.centrality > UNIFORM_CENTRALITY * 2.0, "{}", f.centrality);
    }

    #[test]
    fn aspect_produces_exactly_one_of_three_tags() {
        // Every photo gets one, and never two — a photo tagged both
        // portrait and square would score oddly against both.
        for (w, h) in [(40usize, 20usize), (20, 40), (30, 30)] {
            let px = img(w, h, |_, _| 128);
            let t = auto_tags(&feats(w, h, &px));
            let n = t.iter().filter(|s| ["landscape", "portrait", "square"]
                .contains(&s.as_str())).count();
            assert_eq!(n, 1, "{w}x{h} gave {t:?}");
        }
    }

    #[test]
    fn derived_tags_are_marked_so_they_cannot_be_mistaken_for_written_ones() {
        // "Somebody said this is a portrait of Ada" and "the frame is
        // taller than it is wide" are the same word and entirely
        // different evidence.
        let px = img(20, 40, |_, _| 128);
        let t = auto_tags_prefixed(&feats(20, 40, &px));
        assert!(t.iter().all(|s| is_auto(s)), "{t:?}");
        assert!(t.contains(&"auto:portrait".to_string()), "{t:?}");
        assert!(!is_auto("portrait"), "a written tag must not read as derived");
    }

    #[test]
    fn features_stay_in_range_for_extreme_images() {
        // A score above 1.0 would sort above a perfect match and read as
        // a ranking bug rather than a measurement one.
        for px in [img(16, 16, |_, _| 0), img(16, 16, |_, _| 255),
                   img(16, 16, |x, y| if (x + y) % 2 == 0 { 0 } else { 255 })] {
            let f = feats(16, 16, &px);
            for (name, v) in [("brightness", f.brightness), ("contrast", f.contrast),
                              ("edges", f.edges), ("centrality", f.centrality)] {
                assert!((0.0..=1.0).contains(&v), "{name} = {v}");
            }
        }
    }

    #[test]
    fn the_same_image_always_produces_the_same_tags() {
        // Search results are only comparable between runs if scanning
        // is deterministic.
        let px = img(24, 24, |x, y| ((x * 7 + y * 13) % 256) as u8);
        assert_eq!(auto_tags(&feats(24, 24, &px)), auto_tags(&feats(24, 24, &px)));
    }
}
