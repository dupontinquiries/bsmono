//! r689: dragon roosts — from
//! `SPECULATIVE/SPEC-011_experimental_whisper_roosts.docx`.
//!
//! A dragon that keeps returning to the same crag turns it into a **roost**: over
//! in-game days the location acquires the dragon's elemental property signature,
//! the surrounding environment shifts toward that element, and the dragon claims
//! a territory around it. The doc fixes three residence milestones:
//!
//! | Residence          | Stage    | What happens                                   |
//! |--------------------|----------|------------------------------------------------|
//! | > 30 days          | Claimed  | element signature acquired; 20-block territory |
//! | > 100 days         | Marked   | physical marks (scale/scorch/arc); det. @40    |
//! | > 500 days (elder) | Elder    | crystal growth; 30-block POI property zone; named |
//!
//! This is the pure timeline + property model: given how long a dragon has been
//! resident and its element, it yields the roost stage, territory/detection
//! radii, a growing signature strength, and the per-property deltas the roost
//! imprints on the local environment (feeding `sim::bending_field` /
//! `affinity_access`, exactly like `sim::waterfall_poi` but keyed on the dragon's
//! element). Deterministic, std-only; no world or entity deps.

/// Residence-day thresholds (doc).
pub const CLAIM_DAYS: u32 = 30;
pub const MARK_DAYS: u32 = 100;
pub const ELDER_DAYS: u32 = 500;

/// Property index order `[H, R, W, P, S, L, B]` (matches `waterfall_poi`):
/// humidity, radiant-heat, wind, pressure, seismic, luminosity, brine/mineral.
pub const H: usize = 0;
pub const R: usize = 1;
pub const W: usize = 2;
pub const P: usize = 3;
pub const S: usize = 4;
pub const L: usize = 5;
pub const B: usize = 6;

/// How established a roost is (by residence time).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoostStage {
    /// < 30 days — a spot the dragon merely uses; not yet a roost.
    Transient,
    /// > 30 days — element signature acquired, 20-block territory.
    Claimed,
    /// > 100 days — physical marks accumulate; detectable at 40 blocks.
    Marked,
    /// > 500 days — crystal growth, a 30-block POI property zone, a named place.
    Elder,
}

/// The roost stage for a given residence time.
pub fn roost_stage(residence_days: u32) -> RoostStage {
    if residence_days > ELDER_DAYS {
        RoostStage::Elder
    } else if residence_days > MARK_DAYS {
        RoostStage::Marked
    } else if residence_days > CLAIM_DAYS {
        RoostStage::Claimed
    } else {
        RoostStage::Transient
    }
}

impl RoostStage {
    /// Territory radius in blocks: none until claimed (20), widening to the
    /// elder POI zone (30).
    pub fn territory_radius(self) -> f32 {
        match self {
            RoostStage::Transient => 0.0,
            RoostStage::Claimed | RoostStage::Marked => 20.0,
            RoostStage::Elder => 30.0,
        }
    }

    /// Range (blocks) at which a player with the relevant bending sense can
    /// detect the roost. 0 until the physical marks accumulate (Marked+).
    pub fn detection_range(self) -> f32 {
        match self {
            RoostStage::Transient | RoostStage::Claimed => 0.0,
            RoostStage::Marked | RoostStage::Elder => 40.0,
        }
    }

    /// An elder roost is a named world location.
    pub fn is_named(self) -> bool {
        matches!(self, RoostStage::Elder)
    }

    /// The physical marks (scale deposits, claw gouges, scorch/frost/arc burns)
    /// have accumulated (Marked+).
    pub fn has_marks(self) -> bool {
        matches!(self, RoostStage::Marked | RoostStage::Elder)
    }
}

/// How strongly the local environment has shifted toward the dragon's element,
/// in `[0, 1]`. Ramps from first claim (30 days) to full saturation at elder
/// status (500 days) — the doc's "shift over weeks … crystal formations from the
/// sustained field". Zero before a claim.
pub fn signature_strength(residence_days: u32) -> f32 {
    if residence_days <= CLAIM_DAYS {
        0.0
    } else {
        let t = (residence_days - CLAIM_DAYS) as f32 / (ELDER_DAYS - CLAIM_DAYS) as f32;
        t.clamp(0.0, 1.0)
    }
}

/// The peak per-property deltas `[H,R,W,P,S,L,B]` a fully-saturated roost of a
/// given element imprints (element index: 0=Earth 1=Water 2=Fire 3=Air 4=Storm
/// 5=Brine 6=Information). Each element pushes the properties characteristic of
/// its bending toward higher access. Tunable; grounded in the elements' nature.
pub fn element_signature(element_idx: u8) -> [f32; 7] {
    match element_idx {
        0 => [0.00, 0.00, 0.00, 0.15, 0.30, 0.00, 0.05], // Earth  → seismic + pressure
        1 => [0.35, -0.10, 0.00, 0.00, 0.00, 0.00, 0.00], // Water  → humidity
        2 => [-0.10, 0.35, 0.00, 0.00, 0.00, 0.05, 0.00], // Fire   → radiant heat (scorch)
        3 => [0.00, 0.00, 0.30, -0.15, 0.00, 0.00, 0.00], // Air    → wind, low pressure
        4 => [0.00, 0.10, 0.35, 0.00, 0.00, 0.05, 0.00],  // Storm  → wind + arc glow
        5 => [0.15, 0.00, 0.00, 0.00, 0.05, 0.00, 0.35],  // Brine  → mineral + humidity
        6 => [0.00, 0.00, 0.00, 0.00, 0.00, 0.35, 0.00],  // Information → luminosity
        _ => [0.0; 7],
    }
}

/// Distance falloff in `[0,1]` for a roost's property field: full within the
/// stage's territory radius, quadratic decay to zero at twice that radius.
pub fn falloff(stage: RoostStage, dist_blocks: f32) -> f32 {
    let peak = stage.territory_radius();
    if peak <= 0.0 {
        return 0.0;
    }
    let zero = peak * 2.0;
    if dist_blocks <= peak {
        1.0
    } else if dist_blocks >= zero {
        0.0
    } else {
        let t = (zero - dist_blocks) / (zero - peak);
        t * t
    }
}

/// The property deltas a roost contributes at `dist_blocks`, combining the
/// element signature, the time-based `signature_strength`, and the distance
/// `falloff`. Feed these onto the ambient `bending_field` sample like a
/// `waterfall_poi`.
pub fn roost_modifiers_at(element_idx: u8, residence_days: u32, dist_blocks: f32) -> [f32; 7] {
    let stage = roost_stage(residence_days);
    let k = signature_strength(residence_days) * falloff(stage, dist_blocks);
    let sig = element_signature(element_idx);
    let mut out = [0.0f32; 7];
    for i in 0..7 {
        out[i] = sig[i] * k;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_cross_at_the_doc_thresholds() {
        assert_eq!(roost_stage(0), RoostStage::Transient);
        assert_eq!(roost_stage(30), RoostStage::Transient); // needs to EXCEED 30
        assert_eq!(roost_stage(31), RoostStage::Claimed);
        assert_eq!(roost_stage(100), RoostStage::Claimed);
        assert_eq!(roost_stage(101), RoostStage::Marked);
        assert_eq!(roost_stage(500), RoostStage::Marked);
        assert_eq!(roost_stage(501), RoostStage::Elder);
    }

    #[test]
    fn territory_and_detection_grow_with_stage() {
        assert_eq!(RoostStage::Transient.territory_radius(), 0.0);
        assert_eq!(RoostStage::Claimed.territory_radius(), 20.0);
        assert_eq!(RoostStage::Elder.territory_radius(), 30.0);
        assert_eq!(RoostStage::Claimed.detection_range(), 0.0); // no marks yet
        assert_eq!(RoostStage::Marked.detection_range(), 40.0);
        assert!(RoostStage::Elder.is_named() && !RoostStage::Marked.is_named());
        assert!(RoostStage::Marked.has_marks() && !RoostStage::Claimed.has_marks());
    }

    #[test]
    fn signature_strength_ramps_from_claim_to_elder() {
        assert_eq!(signature_strength(CLAIM_DAYS), 0.0);
        assert!(signature_strength(100) > 0.0 && signature_strength(100) < 1.0);
        assert_eq!(signature_strength(ELDER_DAYS), 1.0);
        assert_eq!(signature_strength(10_000), 1.0); // saturates
        // monotone
        assert!(signature_strength(200) > signature_strength(100));
    }

    #[test]
    fn fire_roost_raises_heat_storm_raises_wind() {
        assert!(element_signature(2)[R] > 0.0, "fire → radiant heat");
        assert!(element_signature(4)[W] > 0.0, "storm → wind");
        assert!(element_signature(1)[H] > 0.0, "water → humidity");
        assert!(element_signature(6)[L] > 0.0, "information → luminosity");
    }

    #[test]
    fn modifiers_scale_with_time_and_distance() {
        // an elder fire roost at its centre imprints strong heat…
        let near = roost_modifiers_at(2, 600, 0.0)[R];
        // …weaker at the territory edge…
        let far = roost_modifiers_at(2, 600, 40.0)[R];
        assert!(near > far && far >= 0.0);
        // a fresh (pre-claim) roost imprints nothing
        assert_eq!(roost_modifiers_at(2, 20, 0.0), [0.0; 7]);
        // beyond 2× territory radius → nothing
        assert_eq!(roost_modifiers_at(2, 600, 100.0)[R], 0.0);
    }
}
