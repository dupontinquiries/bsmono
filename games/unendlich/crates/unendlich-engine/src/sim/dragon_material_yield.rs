//! `sim::dragon_material_yield` — what a harvested dragon material yields.
//!
//! Spec §14.7: a dragon's harvested material is a natural imbued artifact. Its
//! **Field Amplification** profile follows its element & attunement (the same
//! element→channel palette as [`super::artifact_imbue`]), and disassembly yields
//! **essence** of that element scaling with attunement & rarity (feeding
//! `essence_refine`). This composes the creature→item §14.7 loop: a legendary
//! lineage drops legendary essence and a scale that projects the right field.
//!
//! Pure, deterministic, finite (R1/R6/R11); standalone `rustc --test`-able.
//!
//! Element palette (mirrors `artifact_imbue` / `elemental_multiplier`):
//! `0=Wood 1=Water 2=Air 3=Dust 4=Fire 5=Storm`, read modulo 6.
//! Wood→`b,h`  Water→`h`  Air→`w`  Dust→`s,p`  Fire→`r`  Storm→`w,r`.
//! The `l` channel is governed by no element and is always zero.

/// A harvested dragon material.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DragonMaterial {
    /// Element `0..5` (palette above); read modulo 6 so out-of-range is safe.
    pub element: u8,
    /// Attunement strength `0..1` — scales both profile and essence.
    pub attunement01: f32,
    /// Rarity tier (0 = common … 5+ = legendary), capped at legendary.
    pub rarity_tier: u8,
}

/// Field-amplification profile: one channel per field; only the element's
/// governed channels are non-zero.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct FieldProfile {
    pub h: f32,
    pub r: f32,
    pub w: f32,
    pub p: f32,
    pub l: f32,
    pub s: f32,
    pub b: f32,
}

/// What disassembling the material produces.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Yield {
    /// Field-amplification profile (the natural artifact's imbue).
    pub profile: FieldProfile,
    /// Per-element essence units `[Wood, Water, Air, Dust, Fire, Storm]`.
    pub essence: [u32; 6],
}

/// Rarity tier treated as legendary (caps the scaling).
pub const RARITY_MAX: u8 = 5;
/// Essence units per rarity tier at full attunement.
const ESSENCE_PER_TIER: f32 = 20.0;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Yield for a harvested dragon material: a field-amplification profile whose
/// non-zero channels are governed by the element, plus per-element essence — both
/// rising with attunement and rarity. Rarity is capped at [`RARITY_MAX`].
pub fn material_yield(m: &DragonMaterial) -> Yield {
    let att = clamp01(m.attunement01);
    let tier = m.rarity_tier.min(RARITY_MAX);
    let r01 = tier as f32 / RARITY_MAX as f32; // 0..1

    // profile magnitude: minimal (but present) at rarity 0, full at legendary,
    // scaled by attunement. Bounded to 0..1.
    let mag = (att * (0.2 + 0.8 * r01)).max(0.0).min(1.0);
    let mut profile = FieldProfile::default();
    match m.element % 6 {
        0 => {
            profile.b = mag;
            profile.h = mag;
        } // Wood
        1 => {
            profile.h = mag;
        } // Water
        2 => {
            profile.w = mag;
        } // Air
        3 => {
            profile.s = mag;
            profile.p = mag;
        } // Dust
        4 => {
            profile.r = mag;
        } // Fire
        _ => {
            profile.w = mag;
            profile.r = mag;
        } // Storm (5)
    }
    // `l` is never governed — it stays at its Default of 0.0.

    // essence: this element's slot, rising with attunement & (tier+1).
    let units = (att * ESSENCE_PER_TIER * (tier as f32 + 1.0)) as u32;
    let mut essence = [0u32; 6];
    essence[(m.element % 6) as usize] = units;

    Yield { profile, essence }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mat(element: u8, att: f32, rarity: u8) -> DragonMaterial {
        DragonMaterial { element, attunement01: att, rarity_tier: rarity }
    }

    // Channels in struct field order, for governance checks.
    fn channels(p: &FieldProfile) -> [f32; 7] {
        [p.h, p.r, p.w, p.p, p.l, p.s, p.b]
    }
    // governed channel indices into the above array, per element.
    fn governed(element: u8) -> &'static [usize] {
        match element % 6 {
            0 => &[6, 0], // Wood: b, h
            1 => &[0],    // Water: h
            2 => &[2],    // Air: w
            3 => &[5, 3], // Dust: s, p
            4 => &[1],    // Fire: r
            _ => &[2, 1], // Storm: w, r
        }
    }

    #[test]
    fn monotone_in_attunement() {
        for e in 0..6u8 {
            let lo = material_yield(&mat(e, 0.2, 3));
            let hi = material_yield(&mat(e, 0.9, 3));
            // governed profile channels rise
            let (cl, ch) = (channels(&lo.profile), channels(&hi.profile));
            for &g in governed(e) {
                assert!(ch[g] > cl[g], "profile not monotone in att (elem {})", e);
            }
            // essence rises
            assert!(hi.essence[(e % 6) as usize] > lo.essence[(e % 6) as usize]);
        }
    }

    #[test]
    fn monotone_in_rarity() {
        for e in 0..6u8 {
            let lo = material_yield(&mat(e, 0.8, 0));
            let mid = material_yield(&mat(e, 0.8, 3));
            let hi = material_yield(&mat(e, 0.8, 5));
            let (cl, cm, ch) = (channels(&lo.profile), channels(&mid.profile), channels(&hi.profile));
            for &g in governed(e) {
                assert!(cm[g] > cl[g] && ch[g] > cm[g], "profile not monotone in rarity");
            }
            let idx = (e % 6) as usize;
            assert!(mid.essence[idx] > lo.essence[idx] && hi.essence[idx] > mid.essence[idx]);
        }
    }

    #[test]
    fn element_governs_which_fields() {
        for e in 0..6u8 {
            let y = material_yield(&mat(e, 1.0, 5));
            let ch = channels(&y.profile);
            let gov = governed(e);
            for (i, &v) in ch.iter().enumerate() {
                if gov.contains(&i) {
                    assert!(v > 0.0, "governed channel {} should be >0 (elem {})", i, e);
                } else {
                    assert_eq!(v, 0.0, "ungoverned channel {} must be 0 (elem {})", i, e);
                }
            }
            // l (index 4) is never governed
            assert_eq!(y.profile.l, 0.0);
            // essence lands only in this element's slot
            for (i, &u) in y.essence.iter().enumerate() {
                if i == (e % 6) as usize {
                    assert!(u > 0, "element slot should have essence");
                } else {
                    assert_eq!(u, 0, "non-element slot must be empty");
                }
            }
        }
    }

    #[test]
    fn rarity_zero_is_minimal() {
        let lo = material_yield(&mat(4, 1.0, 0)); // Fire, common
        let hi = material_yield(&mat(4, 1.0, 5)); // Fire, legendary
        assert!(lo.profile.r < hi.profile.r, "rarity 0 profile should be smaller");
        assert!(lo.essence[4] < hi.essence[4], "rarity 0 essence should be smaller");
        assert!(lo.profile.r > 0.0, "still present, just minimal");
    }

    #[test]
    fn bad_element_handled() {
        // element 200 % 6 == 2 (Air) -> only w governed; essence in slot 2
        let y = material_yield(&mat(200, 1.0, 5));
        assert!(y.profile.w > 0.0);
        assert_eq!(y.profile.h, 0.0);
        assert_eq!(y.essence[2], y.essence.iter().copied().max().unwrap_or(0));
        assert_eq!(y.essence[0], 0);
    }

    #[test]
    fn finite_and_deterministic() {
        let mut s = 0x36FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as f32 / ((1u64 << 24) as f32)
        };
        for _ in 0..3000 {
            let m = mat((rng() * 300.0) as u8, rng() * 4.0 - 2.0, (rng() * 12.0) as u8);
            let y = material_yield(&m);
            for &c in channels(&y.profile).iter() {
                assert!(c.is_finite() && (0.0..=1.0).contains(&c), "profile oob {}", c);
            }
            // determinism
            assert_eq!(material_yield(&m), y);
        }
        // saturation: legendary + full attunement caps profile at 1.0
        let full = material_yield(&mat(4, 1.0, 5));
        assert!((full.profile.r - 1.0).abs() < 1e-6);
    }
}
