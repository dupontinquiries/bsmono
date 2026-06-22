//! dragon_elemental — a distilled situation→element policy: given how far the
//! target is, whether it's behind cover, and how wet the surroundings are, pick
//! which breath element to use. A pure lookup (no search) that's the fast Stage-4
//! "harvest" substrate and ties to `block_yield`'s element. Total over its inputs
//! — every situation maps to exactly one element. Pure, std-only, deterministic.

/// Local mirror of the engine's `Element` (sim::elemental::Element).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element {
    Earth,
    Dust,
    Fire,
    Lightning,
    Air,
    Water,
    Wood,
}

/// Coarse range band to the target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeBand {
    Close,
    Mid,
    Far,
}

/// Pick a breath element. Rules, in order: a wet field douses fire, so when it's
/// wet the dragon switches to **Lightning** (it conducts) at range or **Dust**
/// up close; behind cover it favours area/arcing elements (**Dust** close,
/// **Air** otherwise) rather than a blockable bolt; in the dry open it uses
/// **Fire** up close/mid and **Lightning** at range. `cover` = target shielded by
/// terrain; `wetness01` in `[0, 1]`.
pub fn choose_element(range: RangeBand, cover: bool, wetness01: f32) -> Element {
    let wet = wetness01.clamp(0.0, 1.0) >= 0.5;
    if wet {
        return match range {
            RangeBand::Close => Element::Dust,
            RangeBand::Mid | RangeBand::Far => Element::Lightning,
        };
    }
    if cover {
        return match range {
            RangeBand::Close => Element::Dust,
            RangeBand::Mid | RangeBand::Far => Element::Air,
        };
    }
    match range {
        RangeBand::Close => Element::Fire,
        RangeBand::Mid => Element::Fire,
        RangeBand::Far => Element::Lightning,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }
    const BANDS: [RangeBand; 3] = [RangeBand::Close, RangeBand::Mid, RangeBand::Far];

    #[test]
    fn wet_never_uses_fire() {
        for &r in &BANDS {
            for &cover in &[false, true] {
                assert_ne!(choose_element(r, cover, 1.0), Element::Fire, "wet → no fire");
            }
        }
    }

    #[test]
    fn dry_open_close_is_fire_far_is_lightning() {
        assert_eq!(choose_element(RangeBand::Close, false, 0.0), Element::Fire);
        assert_eq!(choose_element(RangeBand::Far, false, 0.0), Element::Lightning);
    }

    #[test]
    fn cover_avoids_a_straight_bolt() {
        // behind cover at range → arcing Air, not a blockable Lightning bolt
        assert_eq!(choose_element(RangeBand::Far, true, 0.0), Element::Air);
    }

    #[test]
    fn prop_total_and_deterministic() {
        let mut seed = 0xDADD_0001u32;
        for _ in 0..6000 {
            let r = BANDS[(lcg(&mut seed) * 3.0) as usize % 3];
            let cover = lcg(&mut seed) < 0.5;
            let wet = lcg(&mut seed) * 1.5 - 0.25; // sometimes out of [0,1]
            let a = choose_element(r, cover, wet);
            let b = choose_element(r, cover, wet);
            assert_eq!(a, b, "deterministic + total (always returns an element)");
        }
    }
}
