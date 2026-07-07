//! Emblems — a symbolic identity for every skill and element the player
//! encounters, so a skill slot or an elemental effect reads at a glance. Each
//! emblem is renderer-agnostic data: a motif (the shape family the art is built
//! from), a primary colour, and a representative glyph. The render layer turns
//! these into the actual icons / shimmering slot art; this just guarantees every
//! skill and element has one, and that they're distinct.

use crate::sim::elemental::Element;
use crate::sim::skills::Skill;

/// The shape family an emblem is drawn from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Motif {
    // Elements
    Flame,
    Droplet,
    Bolt,
    Gust,
    Stone,
    Mote,
    Leaf,
    // Skills
    Sword,
    Rune,
    Shield,
    Heart,
    Wing,
    Anvil,
}

/// A symbol for a skill or element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Emblem {
    pub motif: Motif,
    pub rgb: (u8, u8, u8),
    pub glyph: char,
}

/// Emblem for an element — warm/cool/earthy palettes match the feel.
pub fn for_element(e: Element) -> Emblem {
    match e {
        Element::Fire => Emblem { motif: Motif::Flame, rgb: (255, 90, 30), glyph: '▲' },
        Element::Water => Emblem { motif: Motif::Droplet, rgb: (40, 120, 230), glyph: '▼' },
        Element::Lightning => Emblem { motif: Motif::Bolt, rgb: (240, 220, 60), glyph: 'ϟ' },
        Element::Air => Emblem { motif: Motif::Gust, rgb: (200, 230, 235), glyph: '≈' },
        Element::Earth => Emblem { motif: Motif::Stone, rgb: (130, 100, 70), glyph: '◆' },
        Element::Dust => Emblem { motif: Motif::Mote, rgb: (180, 160, 120), glyph: '⋆' },
        Element::Wood => Emblem { motif: Motif::Leaf, rgb: (80, 150, 60), glyph: '♣' },
    }
}

/// Emblem for a skill.
pub fn for_skill(s: Skill) -> Emblem {
    match s {
        Skill::Might => Emblem { motif: Motif::Sword, rgb: (200, 60, 50), glyph: '⚔' },
        Skill::Attunement => Emblem { motif: Motif::Rune, rgb: (150, 80, 220), glyph: '✶' },
        Skill::Warding => Emblem { motif: Motif::Shield, rgb: (80, 140, 220), glyph: '⛨' },
        Skill::Vigor => Emblem { motif: Motif::Heart, rgb: (220, 60, 90), glyph: '❤' },
        Skill::Fleetness => Emblem { motif: Motif::Wing, rgb: (120, 210, 180), glyph: '➳' },
        Skill::Artisan => Emblem { motif: Motif::Anvil, rgb: (190, 140, 70), glyph: '⚒' },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_element_has_a_distinct_emblem() {
        let elements = [
            Element::Fire,
            Element::Water,
            Element::Lightning,
            Element::Air,
            Element::Earth,
            Element::Dust,
            Element::Wood,
        ];
        let motifs: HashSet<_> = elements.iter().map(|&e| for_element(e).motif).collect();
        let glyphs: HashSet<_> = elements.iter().map(|&e| for_element(e).glyph).collect();
        assert_eq!(motifs.len(), elements.len(), "each element a unique motif");
        assert_eq!(glyphs.len(), elements.len(), "each element a unique glyph");
    }

    #[test]
    fn every_skill_has_a_distinct_emblem() {
        let motifs: HashSet<_> = Skill::ALL.iter().map(|&s| for_skill(s).motif).collect();
        let glyphs: HashSet<_> = Skill::ALL.iter().map(|&s| for_skill(s).glyph).collect();
        assert_eq!(motifs.len(), Skill::ALL.len());
        assert_eq!(glyphs.len(), Skill::ALL.len());
    }

    #[test]
    fn skill_and_element_motifs_never_collide() {
        let elem_motifs: HashSet<_> =
            [Element::Fire, Element::Water, Element::Wood].iter().map(|&e| for_element(e).motif).collect();
        for &s in &Skill::ALL {
            assert!(!elem_motifs.contains(&for_skill(s).motif), "skill motifs are their own family");
        }
    }

    #[test]
    fn elemental_palettes_match_temperature() {
        let fire = for_element(Element::Fire).rgb;
        let water = for_element(Element::Water).rgb;
        assert!(fire.0 > fire.2, "fire reads warm (more red than blue)");
        assert!(water.2 > water.0, "water reads cool (more blue than red)");
    }
}
