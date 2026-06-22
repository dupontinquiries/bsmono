//! `sim::dragon_blueprint_from_genome` — express BODY genes into a dragon part blueprint.
//!
//! Maps the seven DRAGON_GENOME BODY genes (each normalised `0..1`) to a
//! deterministic, fixed-order list of [`BlueprintPart`]s (role + body-local
//! offset + half-extents). The list mirrors the eight-part shape that
//! `spawn.rs` hard-codes for `CreatureKind::Dragon`; the coordinator wires
//! the output into `PartState::new` — this module only produces the geometry.
//!
//! ## Gene -> geometry contract (see `DRAGON_GENOME.md §BODY`)
//!
//! | gene         | effect                                              |
//! |--------------|-----------------------------------------------------|
//! | `bulk`       | torso half-extents (XYZ scale factor of body AABB) |
//! | `neck_len`   | jaw/eye X-offset (head reach forward)               |
//! | `neck_arch`  | jaw/eye Y-offset (head carried high vs low)         |
//! | `tail_len`   | tail X-offset magnitude + tail X half-extent        |
//! | `leg_len`    | foot Y-offset (how far legs reach down)             |
//! | `wing_span`  | wing Z-offset + wing X half-extent (span)           |
//! | `wing_spread`| wing Y-offset (how high wings are set on the back)  |
//!
//! Each gene lerps its target dimension between a documented `[MIN, MAX]` pair
//! via a clamped linear interpolation: gene=0 -> MIN phenotype, gene=1 -> MAX.
//! All outputs are positive and finite by construction.
//!
//! ## Part order (fixed, 8 parts)
//! 0 Jaw, 1 Eye-R, 2 Eye-L, 3 Wing-R, 4 Wing-L, 5 Foot-R, 6 Foot-L, 7 Tail

/// An anatomical role tag. Mirrors `creature::Part`; re-declared here so the
/// module stays fully pure (no crate import). Coordinator maps this to `Part`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartRole {
    Jaw,
    Eye,
    Wing,
    Foot,
    Tail,
}

/// A single body-part box in the blueprint.
/// Body-local convention: X forward, Y up, Z right (matches `spawn.rs`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlueprintPart {
    pub role: PartRole,
    /// Body-local AABB centre offset [x, y, z].
    pub offset: [f32; 3],
    /// Body-local AABB half-extents [x, y, z]. All values > 0.
    pub half_extents: [f32; 3],
}

// ---------------------------------------------------------------------------
// Per-gene lerp ranges: gene 0 -> MIN, gene 1 -> MAX.
// Neutral (0.5) reproduces the spawn.rs hardcoded values.
// ---------------------------------------------------------------------------
const BULK_MIN:         f32 = 0.6;
const BULK_MAX:         f32 = 1.4;
const NECK_LEN_MIN:     f32 = 1.8;
const NECK_LEN_MAX:     f32 = 4.5;
const NECK_ARCH_MIN:    f32 = -0.3;
const NECK_ARCH_MAX:    f32 = 1.2;
const TAIL_LEN_MIN:     f32 = 1.5;
const TAIL_LEN_MAX:     f32 = 5.0;
const LEG_LEN_MIN:      f32 = 0.6;
const LEG_LEN_MAX:      f32 = 2.0;
const WING_SPAN_MIN:    f32 = 1.2;
const WING_SPAN_MAX:    f32 = 4.0;
const WING_SPREAD_MIN:  f32 = 0.2;
const WING_SPREAD_MAX:  f32 = 1.8;

/// Genome BODY genes — each normalised to [0, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyGenes {
    pub bulk:        f32,
    pub neck_len:    f32,
    pub neck_arch:   f32,
    pub tail_len:    f32,
    pub leg_len:     f32,
    pub wing_span:   f32,
    pub wing_spread: f32,
}

/// Clamped linear interpolation. `t` outside [0,1] is clamped first.
#[inline(always)]
fn lerp(min: f32, max: f32, t: f32) -> f32 {
    let t = t.max(0.0).min(1.0);
    min + (max - min) * t
}

/// Express a normalised genome into a fixed-order 8-part dragon blueprint.
///
/// Out-of-range genes are silently clamped to [0, 1] before expression.
/// Pure and deterministic: same genes -> same blueprint, every call.
///
/// Part order: Jaw(0), Eye-R(1), Eye-L(2), Wing-R(3), Wing-L(4),
///             Foot-R(5), Foot-L(6), Tail(7).
pub fn dragon_blueprint_from_genome(g: BodyGenes) -> [BlueprintPart; 8] {
    let bulk        = lerp(BULK_MIN,        BULK_MAX,        g.bulk);
    let neck_len    = lerp(NECK_LEN_MIN,    NECK_LEN_MAX,    g.neck_len);
    let neck_arch   = lerp(NECK_ARCH_MIN,   NECK_ARCH_MAX,   g.neck_arch);
    let tail_len    = lerp(TAIL_LEN_MIN,    TAIL_LEN_MAX,    g.tail_len);
    let leg_len     = lerp(LEG_LEN_MIN,     LEG_LEN_MAX,     g.leg_len);
    let wing_span   = lerp(WING_SPAN_MIN,   WING_SPAN_MAX,   g.wing_span);
    let wing_spread = lerp(WING_SPREAD_MIN, WING_SPREAD_MAX, g.wing_spread);

    let eye_lat   = 0.6 * bulk;       // lateral eye offset scales with body width
    let foot_x    = 1.0 * bulk;       // foot forward offset scales with bulk
    let foot_z    = 1.0 * bulk;       // foot lateral offset scales with bulk
    let tail_x    = -(tail_len * 0.75 + 0.5);  // tail origin behind body centre
    let tail_he_x = (tail_len * 0.55).max(0.05);
    let tail_he_yz = (0.5 * bulk).max(0.05);

    [
        // 0: Jaw
        BlueprintPart {
            role: PartRole::Jaw,
            offset:       [neck_len, neck_arch, 0.0],
            half_extents: [(0.8 * bulk).max(0.05), (0.6 * bulk).max(0.05), (0.8 * bulk).max(0.05)],
        },
        // 1: Eye-R (positive Z = right)
        BlueprintPart {
            role: PartRole::Eye,
            offset:       [neck_len, neck_arch + 0.6 * bulk, eye_lat],
            half_extents: [(0.2 * bulk).max(0.05); 3],
        },
        // 2: Eye-L (negative Z = left)
        BlueprintPart {
            role: PartRole::Eye,
            offset:       [neck_len, neck_arch + 0.6 * bulk, -eye_lat],
            half_extents: [(0.2 * bulk).max(0.05); 3],
        },
        // 3: Wing-R (positive Z side)
        BlueprintPart {
            role: PartRole::Wing,
            offset:       [0.0, wing_spread, wing_span],
            half_extents: [wing_span.max(0.05), (0.2 * bulk).max(0.05), (1.5 * bulk).max(0.05)],
        },
        // 4: Wing-L (negative Z side)
        BlueprintPart {
            role: PartRole::Wing,
            offset:       [0.0, wing_spread, -wing_span],
            half_extents: [wing_span.max(0.05), (0.2 * bulk).max(0.05), (1.5 * bulk).max(0.05)],
        },
        // 5: Foot-R
        BlueprintPart {
            role: PartRole::Foot,
            offset:       [foot_x, -leg_len, foot_z],
            half_extents: [(0.5 * bulk).max(0.05), (0.6 * bulk).max(0.05), (0.5 * bulk).max(0.05)],
        },
        // 6: Foot-L
        BlueprintPart {
            role: PartRole::Foot,
            offset:       [foot_x, -leg_len, -foot_z],
            half_extents: [(0.5 * bulk).max(0.05), (0.6 * bulk).max(0.05), (0.5 * bulk).max(0.05)],
        },
        // 7: Tail
        BlueprintPart {
            role: PartRole::Tail,
            offset:       [tail_x, 0.0, 0.0],
            half_extents: [tail_he_x, tail_he_yz, tail_he_yz],
        },
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn neutral() -> BodyGenes {
        BodyGenes { bulk: 0.5, neck_len: 0.5, neck_arch: 0.5, tail_len: 0.5,
                    leg_len: 0.5, wing_span: 0.5, wing_spread: 0.5 }
    }

    #[test]
    fn part_count_is_always_8() {
        for t in [0.0f32, 0.5, 1.0] {
            let g = BodyGenes { bulk: t, neck_len: t, neck_arch: t, tail_len: t,
                                leg_len: t, wing_span: t, wing_spread: t };
            assert_eq!(dragon_blueprint_from_genome(g).len(), 8);
        }
    }

    #[test]
    fn part_order_is_correct() {
        let bp = dragon_blueprint_from_genome(neutral());
        assert_eq!(bp[0].role, PartRole::Jaw);
        assert_eq!(bp[1].role, PartRole::Eye);
        assert_eq!(bp[2].role, PartRole::Eye);
        assert_eq!(bp[3].role, PartRole::Wing);
        assert_eq!(bp[4].role, PartRole::Wing);
        assert_eq!(bp[5].role, PartRole::Foot);
        assert_eq!(bp[6].role, PartRole::Foot);
        assert_eq!(bp[7].role, PartRole::Tail);
    }

    #[test]
    fn all_half_extents_positive_and_finite() {
        let cases = [
            BodyGenes { bulk: 0.0, neck_len: 0.0, neck_arch: 0.0, tail_len: 0.0,
                        leg_len: 0.0, wing_span: 0.0, wing_spread: 0.0 },
            neutral(),
            BodyGenes { bulk: 1.0, neck_len: 1.0, neck_arch: 1.0, tail_len: 1.0,
                        leg_len: 1.0, wing_span: 1.0, wing_spread: 1.0 },
        ];
        for genes in cases {
            for part in dragon_blueprint_from_genome(genes) {
                for he in part.half_extents {
                    assert!(he > 0.0, "half-extent not positive: {he}");
                    assert!(he.is_finite(), "non-finite half-extent: {he}");
                }
                for off in part.offset {
                    assert!(off.is_finite(), "non-finite offset: {off}");
                }
            }
        }
    }

    #[test]
    fn out_of_range_genes_clamp() {
        let hi  = BodyGenes { bulk: 9.0, neck_len: 5.0, neck_arch: 3.0, tail_len: 9.0,
                              leg_len: 4.0, wing_span: 7.0, wing_spread: 8.0 };
        let lo  = BodyGenes { bulk: -1.0, neck_len: -2.0, neck_arch: -5.0, tail_len: -3.0,
                              leg_len: -1.0, wing_span: -9.0, wing_spread: -4.0 };
        let max = BodyGenes { bulk: 1.0, neck_len: 1.0, neck_arch: 1.0, tail_len: 1.0,
                              leg_len: 1.0, wing_span: 1.0, wing_spread: 1.0 };
        let min = BodyGenes { bulk: 0.0, neck_len: 0.0, neck_arch: 0.0, tail_len: 0.0,
                              leg_len: 0.0, wing_span: 0.0, wing_spread: 0.0 };
        assert_eq!(dragon_blueprint_from_genome(hi), dragon_blueprint_from_genome(max));
        assert_eq!(dragon_blueprint_from_genome(lo), dragon_blueprint_from_genome(min));
    }

    #[test]
    fn deterministic() {
        let g = neutral();
        assert_eq!(dragon_blueprint_from_genome(g), dragon_blueprint_from_genome(g));
    }

    #[test]
    fn bigger_wing_span_wider_wing_box() {
        let lo = dragon_blueprint_from_genome(BodyGenes { wing_span: 0.1, ..neutral() });
        let hi = dragon_blueprint_from_genome(BodyGenes { wing_span: 0.9, ..neutral() });
        // Wing-R index 3: X half-extent encodes chord; Z-offset encodes reach.
        assert!(hi[3].half_extents[0] > lo[3].half_extents[0], "wing X he not monotone");
        assert!(hi[3].offset[2]       > lo[3].offset[2],       "wing Z-offset not monotone");
    }

    #[test]
    fn longer_neck_moves_head_forward() {
        let lo = dragon_blueprint_from_genome(BodyGenes { neck_len: 0.0, ..neutral() });
        let hi = dragon_blueprint_from_genome(BodyGenes { neck_len: 1.0, ..neutral() });
        assert!(hi[0].offset[0] > lo[0].offset[0], "jaw X-offset not monotone");
        assert!(hi[1].offset[0] > lo[1].offset[0], "eye X-offset not monotone");
    }

    #[test]
    fn longer_tail_extends_tail_box() {
        let lo = dragon_blueprint_from_genome(BodyGenes { tail_len: 0.0, ..neutral() });
        let hi = dragon_blueprint_from_genome(BodyGenes { tail_len: 1.0, ..neutral() });
        // Tail X-offset is negative; more tail_len -> more negative -> further back.
        assert!(hi[7].offset[0]       < lo[7].offset[0],       "tail offset not further back");
        assert!(hi[7].half_extents[0] > lo[7].half_extents[0], "tail X he not monotone");
    }

    #[test]
    fn longer_legs_lower_feet() {
        let lo = dragon_blueprint_from_genome(BodyGenes { leg_len: 0.0, ..neutral() });
        let hi = dragon_blueprint_from_genome(BodyGenes { leg_len: 1.0, ..neutral() });
        // Foot Y-offset is negative; more leg_len -> more negative.
        assert!(hi[5].offset[1] < lo[5].offset[1], "foot Y not lower with longer leg");
    }
}
