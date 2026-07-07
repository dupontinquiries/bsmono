//! Effects — visual-effect descriptors emitted by the novel systems.
//!
//! Collapse, erosion, and the elemental verbs all want feedback: dust when a
//! block lands, a crumble puff when a cell erodes away, an element burst at a
//! verb's epicenter. This is the pure *emission* layer — it turns those
//! systems' outputs into render-agnostic [`Effect`] descriptors (kind, where,
//! how many, color, spread, lifetime). A particle renderer consumes them; the
//! emission rules are testable on their own. Same "pure unit, wired later"
//! pattern as `elemental` / `field_brush`.
//!
//! It composes with [`crate::world::collapse`] (settle / floating moves),
//! [`crate::world::erosion`] (spent cells), and
//! [`crate::world::elemental::ElementalVerb`] (the active item effect).

use crate::world::elemental::ElementalVerb;

/// A world cell.
pub type Pos = (i32, i32, i32);

/// The visual family of an effect. The renderer maps these to particle
/// systems; the gameplay layer only chooses the family + parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectKind {
    /// Settling debris, landing impact, erosion crumble.
    Dust,
    /// Water displacement.
    Splash,
    /// Heat / fire.
    Embers,
    /// Earth / stone shatter.
    Debris,
    /// Charged / magical shimmer.
    Sparkle,
    /// A field-brush perturbation pulse.
    Ripple,
}

/// One emitted effect: a burst of `count` particles of `kind` at `pos`, jittered
/// over `spread` blocks, living `lifetime` seconds, tinted `rgb`.
#[derive(Debug, Clone, Copy)]
pub struct Effect {
    pub kind: EffectKind,
    pub pos: Pos,
    pub count: u32,
    pub spread: f32,
    pub lifetime: f32,
    pub rgb: [f32; 3],
}

impl Effect {
    pub fn new(kind: EffectKind, pos: Pos, count: u32, spread: f32, lifetime: f32, rgb: [f32; 3]) -> Self {
        Effect { kind, pos, count, spread, lifetime, rgb }
    }
}

/// Dust at each landing from a collapse, with more particles for a longer fall.
/// `moves` are the `(from, to)` pairs from `collapse::settle` / floating drops.
pub fn collapse_effects(moves: &[(Pos, Pos)]) -> Vec<Effect> {
    moves
        .iter()
        .map(|&(from, to)| {
            let fall = (from.1 - to.1).max(0) as u32;
            Effect::new(EffectKind::Dust, to, 4 + fall * 2, 0.4, 0.6, [0.62, 0.55, 0.45])
        })
        .collect()
}

/// A small crumble puff at each cell that fully eroded away this step (the
/// `spent` indices resolved to world positions by the caller).
pub fn erosion_effects(spent: &[Pos]) -> Vec<Effect> {
    spent
        .iter()
        .map(|&p| Effect::new(EffectKind::Dust, p, 6, 0.5, 0.8, [0.55, 0.50, 0.42]))
        .collect()
}

/// An element burst at a manipulation verb's epicenter, keyed to the verb:
/// earth verbs throw debris, water makes a splash, heat throws embers (cool
/// blue when it's a chill burst, warm orange when it heats).
pub fn verb_effects(verb: ElementalVerb, center: Pos) -> Vec<Effect> {
    let (kind, count, spread, rgb) = match verb {
        ElementalVerb::RaiseWall { width, height, .. } => {
            (EffectKind::Debris, ((width * height).max(1)) as u32, 0.6, [0.50, 0.45, 0.40])
        }
        ElementalVerb::RaisePillar { height, .. } => {
            (EffectKind::Debris, ((height * 3).max(1)) as u32, 0.5, [0.50, 0.45, 0.40])
        }
        ElementalVerb::Smash { radius } => {
            (EffectKind::Debris, ((radius * radius * 4).max(8)) as u32, 1.0, [0.45, 0.40, 0.35])
        }
        ElementalVerb::Sphere { radius } => {
            (EffectKind::Splash, ((radius * radius * 6).max(8)) as u32, 0.8, [0.30, 0.50, 0.85])
        }
        ElementalVerb::HeatBurst { radius, peak } => {
            let n = (((radius * radius * 8.0) as i32).max(8)) as u32;
            let rgb = if peak < 0.0 {
                [0.60, 0.80, 0.95] // chill: cool blue
            } else {
                [0.95, 0.55, 0.20] // heat: warm orange
            };
            (EffectKind::Embers, n, radius.max(1.0), rgb)
        }
    };
    vec![Effect::new(kind, center, count, spread, 0.7, rgb)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_dust_scales_with_fall_height() {
        let moves = vec![((0, 5, 0), (0, 3, 0)), ((1, 4, 1), (1, 4, 1))];
        let fx = collapse_effects(&moves);
        assert_eq!(fx.len(), 2);
        assert_eq!(fx[0].kind, EffectKind::Dust);
        assert_eq!(fx[0].pos, (0, 3, 0));
        assert_eq!(fx[0].count, 4 + 2 * 2); // fell 2 -> 8
        assert_eq!(fx[1].count, 4); // no fall -> base
    }

    #[test]
    fn erosion_emits_one_puff_per_spent_cell() {
        let spent = vec![(0, 0, 0), (1, 0, 0), (2, 0, 0)];
        let fx = erosion_effects(&spent);
        assert_eq!(fx.len(), 3);
        assert!(fx.iter().all(|e| e.kind == EffectKind::Dust));
    }

    #[test]
    fn verb_bursts_map_to_element_family() {
        let earth = verb_effects(ElementalVerb::Smash { radius: 3 }, (0, 0, 0));
        assert_eq!(earth[0].kind, EffectKind::Debris);

        let water = verb_effects(ElementalVerb::Sphere { radius: 2 }, (0, 0, 0));
        assert_eq!(water[0].kind, EffectKind::Splash);

        let heat = verb_effects(ElementalVerb::HeatBurst { radius: 2.0, peak: 1.0 }, (0, 0, 0));
        assert_eq!(heat[0].kind, EffectKind::Embers);
        assert!(heat[0].rgb[0] > heat[0].rgb[2], "heat reads warm (more red than blue)");

        let chill = verb_effects(ElementalVerb::HeatBurst { radius: 2.0, peak: -1.0 }, (0, 0, 0));
        assert!(chill[0].rgb[2] > chill[0].rgb[0], "chill reads cool (more blue than red)");
    }
}
