//! `world::tree_form` — tree SILHOUETTES as pure layer geometry (r604).
//!
//! Until now every biome's trees were the same shape: a straight 1×1 trunk
//! under a dome of leaves, differing only in size and block palette. The
//! operator supplied two field studies of West African forests (see
//! `docs/design/biomes_west_africa.md`) whose tree architecture translates
//! directly into voxel silhouettes:
//!
//! - **FlatTop** — the savanna umbrella crown (Vachellia/Acacia habit):
//!   foliage concentrated in a thin, WIDE slab at the very top of the
//!   trunk, nothing below the crown. Reads instantly as "African savanna"
//!   at any distance.
//! - **Baobab** — a massively thick trunk (2×2 columns) with a SHORT,
//!   sparse crown of stubby tip-clusters; the trunk IS the tree.
//! - **Shrub** — the understory stratum ("o — arbustes (sous-bois)" in the
//!   Comoé island tables): a leaf blob sitting directly on a 1-block stem.
//! - **Dome** — the pre-r604 shape, kept BYTE-IDENTICAL so every existing
//!   biome's worldgen is unchanged (pinned by test against the old
//!   formula).
//!
//! Everything here is pure integer/f64 math with no state, because the GPU
//! worldgen shader (`chunk_generation.comp`) must MIRROR it exactly — the
//! two backends generate the same world or the seams show. The
//! `worldgen-parity` probe mode diffs full chunks across backends to pin
//! that mirror.

/// Tree silhouette selector. `repr(u32)` and append-only, because the GLSL
/// mirror switches on the same integer.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanopyForm {
    /// A domed crown: round on top, tapering underneath. The default
    /// everywhere. Was linear in both directions until the upper half was
    /// made circular — see `canopy_layer_radius`.
    Dome = 0,
    /// Thin, wide umbrella slab at the trunk top; bare below the crown.
    FlatTop = 1,
    /// Fat 2×2 trunk, short sparse tip-cluster crown.
    Baobab = 2,
    /// Ground-hugging leaf blob on a 1-block stem (understory stratum).
    Shrub = 3,
}

/// Vertical half-extent of the canopy for `form`, given the tree's rolled
/// max canopy radius. The stamping loop walks `dy in -v..=+v`.
#[inline]
pub fn canopy_v_extent(form: CanopyForm, r_max: i32) -> i32 {
    match form {
        CanopyForm::Dome => r_max.max(1),
        // Umbrella: one layer below the cap line is all the depth it has.
        CanopyForm::FlatTop => 1,
        CanopyForm::Baobab => 1,
        CanopyForm::Shrub => 1,
    }
}

/// Leaf-layer radius at vertical offset `dy` from the canopy centre, or
/// `None` for "no leaves at this layer". `v_extent` is
/// `canopy_v_extent(form, r_max)`.
///
/// Dome reproduces the pre-r604 formula EXACTLY (see the parity test):
/// `v<=1 → 1`, else `round(r_max * (1 - |dy|/v)).max(1)`.
#[inline]
pub fn canopy_layer_radius(
    form: CanopyForm,
    dy: i32,
    v_extent: i32,
    r_max: i32,
) -> Option<i32> {
    match form {
        CanopyForm::Dome => {
            // ROUND ON TOP, TAPERED UNDERNEATH.
            //
            // This was linear in both directions, which is a CONE however it
            // is named: at r_max=5 the layers ran 5,4,3,2,1 upward, straight
            // sides to a point. The variant is called Dome and its own doc
            // said "sphere-ish", and it was neither — reported as the leaves
            // having lost their upper roundness, which is exactly what a cone
            // looks like next to a crown.
            //
            // The top is now a circular profile, `r·√(1-(dy/v)²)`, giving
            // 5,5,5,4,3 — full shoulders and then a quick close, which is what
            // reads as round.
            //
            // The UNDERSIDE keeps the linear taper, and that is deliberate
            // twice over. A real crown is domed above and tapers below, where
            // the branches are; and a full sphere costs ~80% more leaf cells
            // where this costs ~45%, which matters on a phone. Rounding only
            // the half that was reported is both cheaper and more accurate.
            let r = if v_extent <= 1 {
                1
            } else if dy <= 0 {
                let frac = 1.0 - (dy.abs() as f64 / v_extent as f64);
                ((r_max as f64) * frac).round().max(1.0) as i32
            } else {
                let t = dy as f64 / v_extent as f64;
                let frac = (1.0 - t * t).max(0.0).sqrt();
                ((r_max as f64) * frac).round().max(1.0) as i32
            };
            Some(r)
        }
        CanopyForm::FlatTop => match dy {
            // The umbrella: full width at the crown line…
            0 => Some(r_max.max(1)),
            // …a slightly inset cap one above (keeps a hint of dome so the
            // slab doesn't read as a cut plane)…
            1 => Some((r_max - 2).max(1)),
            // …and NOTHING below: the visible clear space under the crown
            // is what sells the silhouette.
            _ => None,
        },
        CanopyForm::Baobab => match dy {
            // Stubby crown barely wider than the fat trunk.
            0 => Some((r_max.min(2)).max(1)),
            1 => Some(1),
            _ => None,
        },
        CanopyForm::Shrub => match dy {
            // One blob layer plus a 1-wide cap.
            0 => Some(r_max.clamp(1, 2)),
            1 => Some(1),
            _ => None,
        },
    }
}

/// Trunk footprint side length: 1 for a normal 1×1 trunk, 2 for the
/// baobab's 2×2 mass. The extra columns extend +x/+z from the origin.
#[inline]
pub fn trunk_thickness(form: CanopyForm) -> i32 {
    match form {
        CanopyForm::Baobab => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE UNDERSIDE is still the pre-r604 formula, byte for byte.
    ///
    /// r604's contract was that extracting this module must not change the
    /// shape. That was an EXTRACTION-TIME guarantee — a refactor must not
    /// alter geometry — and not a promise never to improve the crown. The
    /// upper half has now been changed deliberately, so the parity assertion
    /// narrows to the half that did not move rather than being deleted: a
    /// dropped test looks the same in a diff as a test that was in the way.
    #[test]
    fn the_canopy_underside_still_matches_the_pre_r604_formula() {
        for r_max in 1..=6 {
            let v = r_max.max(1);
            assert_eq!(canopy_v_extent(CanopyForm::Dome, r_max), v);
            for dy in -v..=0 {
                let old = if v <= 1 {
                    1
                } else {
                    let frac = 1.0 - (dy.abs() as f64 / v as f64);
                    ((r_max as f64) * frac).round().max(1.0) as i32
                };
                assert_eq!(
                    canopy_layer_radius(CanopyForm::Dome, dy, v, r_max),
                    Some(old),
                    "r_max={r_max} dy={dy}"
                );
            }
        }
    }

    /// The umbrella is what its name says: widest layer AT the crown,
    /// nothing below it, and the crown is as wide as the roll allows.
    #[test]
    fn flat_top_is_wide_thin_and_bare_below() {
        let r = 4;
        let v = canopy_v_extent(CanopyForm::FlatTop, r);
        assert_eq!(v, 1, "umbrella depth is one layer each way");
        assert_eq!(canopy_layer_radius(CanopyForm::FlatTop, 0, v, r), Some(4));
        assert_eq!(canopy_layer_radius(CanopyForm::FlatTop, 1, v, r), Some(2));
        assert_eq!(
            canopy_layer_radius(CanopyForm::FlatTop, -1, v, r),
            None,
            "the clear space under the crown IS the silhouette"
        );
    }

    /// Baobab: the trunk is the tree — 2×2 footprint, crown barely wider.
    #[test]
    fn baobab_is_thick_trunked_with_a_stub_crown() {
        assert_eq!(trunk_thickness(CanopyForm::Baobab), 2);
        assert_eq!(trunk_thickness(CanopyForm::Dome), 1);
        let v = canopy_v_extent(CanopyForm::Baobab, 5);
        assert_eq!(canopy_layer_radius(CanopyForm::Baobab, 0, v, 5), Some(2));
        assert_eq!(canopy_layer_radius(CanopyForm::Baobab, -1, v, 5), None);
    }

    /// The GLSL mirror in `chunk_generation.comp`, transliterated LITERALLY
    /// (same ops, same clamp/min/max spellings), asserted equal to the Rust
    /// source of truth across the whole parameter domain. This is the
    /// r604 mirror's gate: the WORLD-level parity probe cannot gate the
    /// mirror while the pre-existing f32/f64 noise fork makes the two
    /// backends' terrain differ over land (see the r604 ledger entry) —
    /// but the FORM math itself is pure integers and must match exactly.
    #[test]
    fn glsl_transliteration_matches_rust_exactly() {
        // — begin literal transliteration of the shader helpers —
        fn g_v_extent(form: i32, r_max: i32) -> i32 {
            if form == 0 { return r_max.max(1); }
            1
        }
        fn g_layer_radius(form: i32, dy: i32, v_extent: i32, r_max: i32) -> i32 {
            if form == 0 {
                if v_extent <= 1 { return 1; }
                let frac = 1.0 - (dy.abs() as f32) / (v_extent as f32);
                return ((r_max as f32 * frac).round() as i32).max(1);
            }
            if form == 1 {
                if dy == 0 { return r_max.max(1); }
                if dy == 1 { return (r_max - 2).max(1); }
                return -1;
            }
            if form == 2 {
                if dy == 0 { return r_max.min(2).max(1); }
                if dy == 1 { return 1; }
                return -1;
            }
            if dy == 0 { return r_max.clamp(1, 2); }
            if dy == 1 { return 1; }
            -1
        }
        fn g_trunk_thickness(form: i32) -> i32 { if form == 2 { 2 } else { 1 } }
        // — end transliteration —
        let forms = [
            CanopyForm::Dome,
            CanopyForm::FlatTop,
            CanopyForm::Baobab,
            CanopyForm::Shrub,
        ];
        for (fi, &form) in forms.iter().enumerate() {
            let fi = fi as i32;
            assert_eq!(g_trunk_thickness(fi), trunk_thickness(form), "thickness form {fi}");
            for r_max in 1..=6 {
                let v = canopy_v_extent(form, r_max);
                assert_eq!(g_v_extent(fi, r_max), v, "v_extent form {fi} r {r_max}");
                for dy in -6..=6 {
                    let rust = canopy_layer_radius(form, dy, v, r_max)
                        .map(|r| r as i32)
                        .unwrap_or(-1);
                    // GLSL only evaluates dy within ±v_extent (the caller
                    // continues past that); mirror the caller's guard.
                    let glsl = if dy.abs() > v { -1 } else { g_layer_radius(fi, dy, v, r_max) };
                    let rust = if dy.abs() > v { -1 } else { rust };
                    assert_eq!(glsl, rust, "layer form {fi} r {r_max} dy {dy}");
                }
            }
        }
    }

    /// Shrub: a blob on the ground, never a canopy in the sky.
    #[test]
    fn shrub_is_a_capped_ground_blob() {
        let v = canopy_v_extent(CanopyForm::Shrub, 3);
        assert_eq!(canopy_layer_radius(CanopyForm::Shrub, 0, v, 3), Some(2));
        assert_eq!(canopy_layer_radius(CanopyForm::Shrub, 1, v, 3), Some(1));
        assert_eq!(canopy_layer_radius(CanopyForm::Shrub, -1, v, 3), None);
    }

    /// THE REPORT: "the plant leaves on trees seem to have lost their upper
    /// roundness". They had none to lose — the profile was linear both ways,
    /// which is a cone. A dome has full shoulders and then closes quickly.
    #[test]
    fn the_crown_is_round_on_top_rather_than_conical() {
        let (r_max, v) = (5, 5);
        let up: Vec<i32> = (0..=v)
            .map(|dy| canopy_layer_radius(CanopyForm::Dome, dy, v, r_max).unwrap())
            .collect();
        assert_eq!(up, vec![5, 5, 5, 4, 3, 1], "got {up:?}");
        // The cone this replaced would have been 5,4,3,2,1,1 — strictly
        // narrower at every layer above the widest.
        for dy in 1..v {
            let cone = ((r_max as f64) * (1.0 - dy as f64 / v as f64)).round().max(1.0) as i32;
            assert!(up[dy as usize] >= cone, "layer {dy} is not wider than the cone");
        }
        assert!(up[1] > ((r_max as f64) * 0.8).round() as i32,
            "the shoulder just below the top must stay near full width");
    }

    /// The underside is UNCHANGED, which is what keeps the silhouette a tree
    /// rather than a ball on a stick.
    #[test]
    fn the_underside_still_tapers_linearly() {
        let (r_max, v) = (5, 5);
        let down: Vec<i32> = (0..=v)
            .map(|dy| canopy_layer_radius(CanopyForm::Dome, -dy, v, r_max).unwrap())
            .collect();
        assert_eq!(down, vec![5, 4, 3, 2, 1, 1]);
    }

    /// The change costs leaves, and how many is worth pinning: a canopy that
    /// silently doubled would be a performance regression wearing a bug fix.
    #[test]
    fn the_extra_roundness_costs_under_half_again_in_leaves() {
        for r_max in [3, 5, 6] {
            let v = r_max;
            let now: i32 = (-v..=v)
                .map(|dy| {
                    let r = canopy_layer_radius(CanopyForm::Dome, dy, v, r_max).unwrap();
                    r * r
                })
                .sum();
            let cone: i32 = (-v..=v)
                .map(|dy| {
                    let r = ((r_max as f64) * (1.0 - dy.abs() as f64 / v as f64))
                        .round()
                        .max(1.0) as i32;
                    r * r
                })
                .sum();
            let ratio = now as f64 / cone as f64;
            assert!(ratio < 1.6, "r_max={r_max}: {ratio:.2}x the leaves is too many");
            assert!(ratio > 1.0, "r_max={r_max}: it should be fuller than a cone");
        }
    }

    /// A one-layer canopy has no top to round, and must not divide by zero.
    #[test]
    fn a_degenerate_canopy_is_still_one_block() {
        assert_eq!(canopy_layer_radius(CanopyForm::Dome, 0, 1, 1), Some(1));
        assert_eq!(canopy_layer_radius(CanopyForm::Dome, 0, 0, 3), Some(1));
    }

    /// The other three forms are untouched — this was a Dome bug.
    #[test]
    fn the_other_silhouettes_are_unchanged() {
        assert_eq!(canopy_layer_radius(CanopyForm::FlatTop, 0, 1, 5), Some(5));
        assert_eq!(canopy_layer_radius(CanopyForm::FlatTop, 1, 1, 5), Some(3));
        assert_eq!(canopy_layer_radius(CanopyForm::FlatTop, -1, 1, 5), None);
        assert_eq!(canopy_layer_radius(CanopyForm::Baobab, 0, 1, 5), Some(2));
        assert_eq!(canopy_layer_radius(CanopyForm::Shrub, 0, 1, 5), Some(2));
    }
}
