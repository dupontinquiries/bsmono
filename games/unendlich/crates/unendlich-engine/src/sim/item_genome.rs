//! `sim::item_genome` — the hidden genetic profile every crafted item carries,
//! from `docs/design_library/06_Crafting_and_Items/crafting_infusion_genetics.docx`
//! §2. Two identical swords from identical materials on the same forge have
//! identical base stats but *different genomes* if one was forged in a storm —
//! and that genome decides what the item can later become (which sway marks it
//! absorbs, which enchantments take cleanly, how infusions amplify or resist).
//!
//! This is the pure genome model: the five parameters, creation at craft time,
//! and the documented genome-INHERITANCE rules when two items are recombined at
//! a forge. It builds on `sim::craft_sway` (`SwayElement` / `SwayMark` /
//! `imprint`) and stays deterministic — the caller supplies the mutation roll
//! (a 0..1 value from its own RNG), so results are reproducible and testable.

use crate::sim::craft_sway::{imprint, CraftContext, SwayElement, SwayMark};

/// The hidden genome of a crafted item (doc §2.1). Uses primitive stand-ins for
/// the doc's `Fixed`/`ForgeId` so the model stays engine-agnostic and testable.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemGenome {
    /// Primary element alignment — which sway marks/enchantments take cleanly.
    pub element_affinity: SwayElement,
    /// Up to two trace affinities (recessive elements from combination).
    pub trace_affinities: Vec<SwayElement>,
    /// Both parents shared the primary element → doubled depth on it.
    pub strong_primary: bool,
    /// How deeply bending is embedded (0..1). High = techniques interact strongly.
    pub resonance_depth: f32,
    /// Base material quality (0..1). Set by ingredients; can only DROP in
    /// combination, never rise (bar a small forge offset). Caps other genes.
    pub material_purity: f32,
    /// Which forge produced this item (its signature).
    pub craft_signature: u16,
    /// Remaining infusion slots (0..4), consumed by infusions/enchantments.
    pub infusion_capacity: u8,
    /// Accumulated sway marks (≤3). The third is dominant — it overrides the
    /// primary affinity for enchantment-compatibility purposes.
    pub sway_marks: [Option<SwayMark>; 3],
}

/// A forge's character for genome purposes (doc §1, §2.2 mutation table).
#[derive(Clone, Copy, Debug)]
pub struct ForgeProfile {
    pub id: u16,
    pub element: Option<SwayElement>,
    /// Purity offset the forge can add (Ancient Shrine = +0.05; most = 0).
    pub purity_bonus: f32,
    /// Extra infusion slots this forge grants.
    pub infusion_bonus: u8,
    /// Mutation chance per combination (0.08 standard, 0.15 shrine, 0.20 roost,
    /// 0.04 seismic — doc §2.2).
    pub mutation_rate: f32,
}

impl ForgeProfile {
    /// A plain village forge: no element, no bonuses, standard 8% mutation.
    pub fn village(id: u16) -> Self {
        ForgeProfile { id, element: None, purity_bonus: 0.0, infusion_bonus: 0, mutation_rate: 0.08 }
    }
}

/// Create a fresh genome at craft time (doc §2). `material_purity` comes from
/// ingredient quality (immutable hereafter). `resonance_depth` starts from the
/// craft's sway intensity (a strongly-marked craft embeds bending deeper), and
/// `infusion_capacity = round(material_purity * (2 + forge.infusion_bonus))`,
/// capped at 4. The first sway mark (if any) is `imprint(ctx)`.
pub fn create(
    base_element: SwayElement,
    material_purity: f32,
    forge: &ForgeProfile,
    ctx: &CraftContext,
) -> ItemGenome {
    let purity = material_purity.clamp(0.0, 1.0);
    let mark = imprint(ctx);
    // Sway intensity deepens resonance; a calm craft leaves it shallow.
    let resonance_depth = mark.map(|m| 0.2 + 0.6 * m.strength).unwrap_or(0.15).clamp(0.0, 1.0);
    let slots = (purity * (2.0 + forge.infusion_bonus as f32)).round();
    let infusion_capacity = slots.clamp(0.0, 4.0) as u8;
    ItemGenome {
        element_affinity: base_element,
        trace_affinities: Vec::new(),
        strong_primary: false,
        resonance_depth,
        material_purity: purity,
        craft_signature: forge.id,
        infusion_capacity,
        sway_marks: [mark, None, None],
    }
}

/// The strongest sway mark of a genome (highest strength), if any.
fn strongest_mark(g: &ItemGenome) -> Option<SwayMark> {
    g.sway_marks
        .iter()
        .flatten()
        .copied()
        .fold(None, |acc: Option<SwayMark>, m| match acc {
            Some(a) if a.strength >= m.strength => Some(a),
            _ => Some(m),
        })
}

/// Combine two item genomes at a forge (doc §2.2). `roll01` is the caller's
/// mutation roll in `0..1`; mutation fires when `roll01 < forge.mutation_rate`.
/// Deterministic given its inputs.
pub fn combine(a: &ItemGenome, b: &ItemGenome, forge: &ForgeProfile, roll01: f32) -> ItemGenome {
    // element_affinity: the deeper-resonance parent is dominant; the other's
    // affinity becomes a trace. Shared affinity → strong_primary (doubled depth).
    let (dom, rec) = if a.resonance_depth >= b.resonance_depth { (a, b) } else { (b, a) };
    let shared = a.element_affinity == b.element_affinity;
    let element_affinity = dom.element_affinity;
    let mut trace_affinities = Vec::new();
    if !shared {
        trace_affinities.push(rec.element_affinity);
    }

    // resonance_depth: averaged, shifted +0.05 toward the forge element, with an
    // 8%-ish mutation → the higher parent's value + 0.10 (capped).
    let mut resonance_depth = (a.resonance_depth + b.resonance_depth) * 0.5;
    if let Some(fe) = forge.element {
        if fe == element_affinity {
            resonance_depth += 0.05;
        }
    }
    let mutated = roll01 < forge.mutation_rate;
    if mutated {
        resonance_depth = a.resonance_depth.max(b.resonance_depth) + 0.10;
    }
    let resonance_depth = resonance_depth.clamp(0.0, 1.0);

    // material_purity: the LOWER parent (can't raise through combination), plus
    // the forge's small offset.
    let material_purity = (a.material_purity.min(b.material_purity) + forge.purity_bonus).clamp(0.0, 1.0);

    // infusion_capacity: (sum / 2, floored) + forge bonus, capped at 4.
    let infusion_capacity =
        (((a.infusion_capacity as u16 + b.infusion_capacity as u16) / 2) as u8)
            .saturating_add(forge.infusion_bonus)
            .min(4);

    // sway_marks: the strongest from each parent; matching elements merge into a
    // stronger single mark; the third slot stays open for sway during THIS craft.
    let mut sway_marks: [Option<SwayMark>; 3] = [None, None, None];
    let ma = strongest_mark(a);
    let mb = strongest_mark(b);
    match (ma, mb) {
        (Some(x), Some(y)) if x.element == y.element => {
            // merge: strengths add, capped at 1.0
            sway_marks[0] = Some(SwayMark { element: x.element, strength: (x.strength + y.strength).min(1.0) });
        }
        (Some(x), Some(y)) => {
            sway_marks[0] = Some(x);
            sway_marks[1] = Some(y);
        }
        (Some(x), None) | (None, Some(x)) => {
            sway_marks[0] = Some(x);
        }
        (None, None) => {}
    }

    ItemGenome {
        element_affinity,
        trace_affinities,
        strong_primary: shared,
        resonance_depth,
        material_purity,
        craft_signature: forge.id, // the combination forge overwrites both
        infusion_capacity,
        sway_marks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(e: SwayElement, s: f32) -> SwayMark {
        SwayMark { element: e, strength: s }
    }

    fn genome(el: SwayElement, res: f32, pur: f32, cap: u8, m0: Option<SwayMark>) -> ItemGenome {
        ItemGenome {
            element_affinity: el,
            trace_affinities: Vec::new(),
            strong_primary: false,
            resonance_depth: res,
            material_purity: pur,
            craft_signature: 1,
            infusion_capacity: cap,
            sway_marks: [m0, None, None],
        }
    }

    #[test]
    fn create_sets_purity_and_caps_slots() {
        let forge = ForgeProfile::village(7);
        let ctx = CraftContext::default(); // calm craft → no mark
        let g = create(SwayElement::Fire, 0.9, &forge, &ctx);
        assert_eq!(g.material_purity, 0.9);
        assert_eq!(g.craft_signature, 7);
        assert_eq!(g.sway_marks[0], None, "calm craft leaves no mark");
        // slots = round(0.9 * 2) = 2
        assert_eq!(g.infusion_capacity, 2);
        // shallow resonance with no sway
        assert!(g.resonance_depth < 0.2);
    }

    #[test]
    fn create_deepens_resonance_under_sway() {
        let forge = ForgeProfile::village(1);
        let ctx = CraftContext { weather: Some(SwayElement::Storm), ..Default::default() };
        let g = create(SwayElement::Fire, 1.0, &forge, &ctx);
        assert!(g.sway_marks[0].is_some(), "storm imprints a mark");
        assert!(g.resonance_depth > 0.2, "sway deepens resonance");
    }

    #[test]
    fn combine_purity_takes_the_weaker_parent() {
        let a = genome(SwayElement::Fire, 0.5, 0.8, 2, None);
        let b = genome(SwayElement::Water, 0.4, 0.6, 2, None);
        let forge = ForgeProfile::village(9);
        let out = combine(&a, &b, &forge, 0.99); // no mutation
        assert!((out.material_purity - 0.6).abs() < 1e-6, "lower parent purity");
        assert_eq!(out.craft_signature, 9, "combination forge overwrites signature");
    }

    #[test]
    fn combine_dominant_affinity_wins_recessive_becomes_trace() {
        let a = genome(SwayElement::Fire, 0.7, 0.9, 2, None); // deeper → dominant
        let b = genome(SwayElement::Water, 0.3, 0.9, 2, None);
        let out = combine(&a, &b, &ForgeProfile::village(1), 0.99);
        assert_eq!(out.element_affinity, SwayElement::Fire);
        assert_eq!(out.trace_affinities, vec![SwayElement::Water]);
        assert!(!out.strong_primary);
    }

    #[test]
    fn combine_shared_affinity_sets_strong_primary() {
        let a = genome(SwayElement::Earth, 0.6, 0.9, 2, None);
        let b = genome(SwayElement::Earth, 0.5, 0.9, 2, None);
        let out = combine(&a, &b, &ForgeProfile::village(1), 0.99);
        assert!(out.strong_primary);
        assert!(out.trace_affinities.is_empty());
    }

    #[test]
    fn combine_infusion_capacity_is_half_sum_plus_bonus() {
        let a = genome(SwayElement::Fire, 0.5, 0.9, 3, None);
        let b = genome(SwayElement::Fire, 0.5, 0.9, 1, None);
        let mut forge = ForgeProfile::village(1);
        forge.infusion_bonus = 1;
        let out = combine(&a, &b, &forge, 0.99);
        // (3+1)/2 = 2, +1 bonus = 3
        assert_eq!(out.infusion_capacity, 3);
    }

    #[test]
    fn combine_matching_sway_marks_merge_stronger() {
        let a = genome(SwayElement::Storm, 0.5, 0.9, 2, Some(mark(SwayElement::Storm, 0.5)));
        let b = genome(SwayElement::Storm, 0.5, 0.9, 2, Some(mark(SwayElement::Storm, 0.4)));
        let out = combine(&a, &b, &ForgeProfile::village(1), 0.99);
        let m = out.sway_marks[0].expect("merged mark");
        assert_eq!(m.element, SwayElement::Storm);
        assert!((m.strength - 0.9).abs() < 1e-6, "strengths add");
        assert_eq!(out.sway_marks[1], None, "merged into one slot");
    }

    #[test]
    fn combine_mutation_boosts_resonance() {
        let a = genome(SwayElement::Fire, 0.6, 0.9, 2, None);
        let b = genome(SwayElement::Fire, 0.4, 0.9, 2, None);
        let forge = ForgeProfile::village(1);
        // roll below mutation_rate → mutated: max(0.6,0.4)+0.10 = 0.70
        let out = combine(&a, &b, &forge, 0.01);
        assert!((out.resonance_depth - 0.70).abs() < 1e-6);
    }
}
