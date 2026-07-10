//! `sim::craft_sway` — the elemental "sway mark" a crafting context stamps into an
//! item's genome.
//!
//! From `06_Crafting_and_Items/crafting_infusion_genetics.docx`. Every item you
//! craft has a hidden genome, and that genome is shaped not only by its materials
//! but by *where* and *how* it was made: the weather at the forge, whether a dragon
//! was watching, and whether a bender channelled chi through the work. Two identical
//! swords from identical materials on the same forge have identical base stats but
//! *different genomes* if one was forged in a thunderstorm and the other in calm —
//! the storm-made blade carries a **Storm sway mark**. That mark biases what the
//! item can later become. This module derives the mark from the context; it pairs
//! with `sim::item_genetics` (the base genome / breeding) rather than modifying it.

/// An elemental influence that can leave a sway mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SwayElement {
    Storm,
    Fire,
    Water,
    Earth,
    Air,
    Wood,
}

/// The circumstances of a craft. Each present influence pushes the item's genome
/// toward its element; the strongest becomes the sway mark.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CraftContext {
    /// Weather element during the craft (a thunderstorm is a strong Storm push).
    pub weather: Option<SwayElement>,
    /// The element of a dragon watching the work, if any.
    pub dragon_watching: Option<SwayElement>,
    /// Element of the forge's own character, if it has one.
    pub forge_element: Option<SwayElement>,
    /// Element of chi a bender is channelling through the work, if any …
    pub chi_element: Option<SwayElement>,
    /// … and how strongly (0..1). Chi is a deliberate act, so it weighs heavily.
    pub chi_strength: f32,
}

/// A stamped sway mark: an element and how strongly it took (0..1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwayMark {
    pub element: SwayElement,
    pub strength: f32,
}

// Per-source base weights: a channelled chi and a raging storm bias hardest; an
// idle forge character or a watching dragon nudge more gently.
const W_WEATHER: f32 = 0.45;
const W_DRAGON: f32 = 0.30;
const W_FORGE: f32 = 0.20;
const W_CHI_MAX: f32 = 0.60;

fn add(scores: &mut [f32; 6], e: SwayElement, w: f32) {
    scores[e as usize] += w;
}

const ELEMS: [SwayElement; 6] = [
    SwayElement::Storm,
    SwayElement::Fire,
    SwayElement::Water,
    SwayElement::Earth,
    SwayElement::Air,
    SwayElement::Wood,
];

/// Derive the sway mark for a craft, or `None` if the context was wholly neutral
/// (a calm craft leaves no mark — the item's genome is shaped by materials alone).
/// The mark is the highest-scoring element; strength is its share of the total
/// elemental pressure, so a single strong influence marks harder than several
/// competing ones.
pub fn imprint(ctx: &CraftContext) -> Option<SwayMark> {
    let mut scores = [0.0f32; 6];
    if let Some(e) = ctx.weather {
        add(&mut scores, e, W_WEATHER);
    }
    if let Some(e) = ctx.dragon_watching {
        add(&mut scores, e, W_DRAGON);
    }
    if let Some(e) = ctx.forge_element {
        add(&mut scores, e, W_FORGE);
    }
    if let Some(e) = ctx.chi_element {
        add(&mut scores, e, W_CHI_MAX * ctx.chi_strength.clamp(0.0, 1.0));
    }

    let total: f32 = scores.iter().sum();
    if total <= 0.0 {
        return None;
    }
    // Highest-scoring element wins the mark.
    let mut best = 0usize;
    for i in 1..6 {
        if scores[i] > scores[best] {
            best = i;
        }
    }
    Some(SwayMark {
        element: ELEMS[best],
        strength: (scores[best] / total).clamp(0.0, 1.0),
    })
}

impl SwayMark {
    /// Does this mark dominate the item (a decisive, single-element genome), versus
    /// merely tint it? Threshold at 0.6 of the total elemental pressure.
    pub fn is_dominant(&self) -> bool {
        self.strength >= 0.6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calm_craft_leaves_no_mark() {
        assert_eq!(imprint(&CraftContext::default()), None);
    }

    #[test]
    fn storm_craft_gets_a_storm_mark() {
        let ctx = CraftContext { weather: Some(SwayElement::Storm), ..Default::default() };
        let m = imprint(&ctx).unwrap();
        assert_eq!(m.element, SwayElement::Storm);
        // Sole influence → full strength → dominant.
        assert!((m.strength - 1.0).abs() < 1e-6);
        assert!(m.is_dominant());
    }

    #[test]
    fn channelled_chi_outweighs_a_watching_dragon() {
        // A dragon watching pushes Fire; a bender channels strong Water chi.
        let ctx = CraftContext {
            dragon_watching: Some(SwayElement::Fire),
            chi_element: Some(SwayElement::Water),
            chi_strength: 1.0,
            ..Default::default()
        };
        let m = imprint(&ctx).unwrap();
        assert_eq!(m.element, SwayElement::Water, "strong chi should win the mark");
    }

    #[test]
    fn competing_influences_dilute_strength() {
        // Storm weather vs an Earth forge vs a Fire dragon — Storm still wins, but
        // spread across three elements it no longer dominates.
        let ctx = CraftContext {
            weather: Some(SwayElement::Storm),
            forge_element: Some(SwayElement::Earth),
            dragon_watching: Some(SwayElement::Fire),
            ..Default::default()
        };
        let m = imprint(&ctx).unwrap();
        assert_eq!(m.element, SwayElement::Storm); // 0.45 is the largest single source
        assert!(m.strength < 1.0 && !m.is_dominant(), "diluted by competitors: {}", m.strength);
    }

    #[test]
    fn same_element_from_two_sources_stacks() {
        // Storm weather AND a Storm dragon → strongly dominant Storm.
        let ctx = CraftContext {
            weather: Some(SwayElement::Storm),
            dragon_watching: Some(SwayElement::Storm),
            ..Default::default()
        };
        let m = imprint(&ctx).unwrap();
        assert_eq!(m.element, SwayElement::Storm);
        assert!(m.is_dominant());
    }
}
