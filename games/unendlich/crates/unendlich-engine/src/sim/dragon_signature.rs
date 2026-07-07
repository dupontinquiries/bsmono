//! dragon_signature — map a dragon's genetics, its element, and its temperament
//! to the parameters of the emission it leaves on the world (the breath/heat
//! plume the `sim::emission` system deposits). This is the pure core; the
//! coordinator wires `SignatureParams` here onto the real
//! `sim::emission::Signature`. Self-contained: a local `Element` and a local
//! `SignatureParams` mirror, deterministic seeded mixing, no clock/rng.

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

/// Local minimal mirror of `sim::emission::Signature` (Fixed → f32 here).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SignatureParams {
    pub element: Element,
    /// Deposited energy per emission (0..~12).
    pub energy: f32,
    /// Reach in blocks (1..~24).
    pub range: f32,
    /// Angular/areal spread, 0 (tight) .. 1 (diffuse).
    pub spread: f32,
    /// Seed for the emission's per-event jitter.
    pub jitter_seed: u64,
}

/// A dragon's heritable emission traits, each in `[0, 1]`.
#[derive(Clone, Copy, Debug)]
pub struct DragonGenome {
    /// Raw output strength.
    pub power: f32,
    /// Focus — high makes a tight, long plume; low makes a wide puff.
    pub focus: f32,
    /// Volatility — adds spread and jitter.
    pub volatility: f32,
    /// Stable per-individual identity, mixed into the jitter seed.
    pub id: u64,
}

/// The temperament knobs that nudge the plume (mirror of `Disposition`).
#[derive(Clone, Copy, Debug)]
pub struct Brain {
    pub aggression: f32,
    pub curiosity: f32,
}

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Per-element character: (energy×, range×, spread+). Fire burns hot and far,
/// water is cool and diffuse, lightning is sharp and long, earth/dust are short
/// and broad, wood is gentle. These make distinct elements read distinctly.
fn element_profile(e: Element) -> (f32, f32, f32) {
    match e {
        Element::Fire => (1.30, 1.10, 0.05),
        Element::Lightning => (1.15, 1.35, -0.05),
        Element::Air => (0.80, 1.20, 0.20),
        Element::Water => (0.85, 0.90, 0.25),
        Element::Earth => (1.00, 0.70, 0.10),
        Element::Dust => (0.75, 0.80, 0.30),
        Element::Wood => (0.70, 0.85, 0.00),
    }
}

fn element_tag(e: Element) -> u64 {
    match e {
        Element::Earth => 1,
        Element::Dust => 2,
        Element::Fire => 3,
        Element::Lightning => 4,
        Element::Air => 5,
        Element::Water => 6,
        Element::Wood => 7,
    }
}

/// Build the emission signature for a dragon. Deterministic in
/// `(genome.id, element)`. Energy rises with power and aggression; range rises
/// with focus and power; spread rises with volatility and curiosity but is pulled
/// tight by focus; the element profile scales all three. All outputs are clamped
/// to documented ranges and are always finite.
pub fn signature_from(genome: &DragonGenome, element: Element, brain: &Brain) -> SignatureParams {
    let power = genome.power.clamp(0.0, 1.0);
    let focus = genome.focus.clamp(0.0, 1.0);
    let volatility = genome.volatility.clamp(0.0, 1.0);
    let aggression = brain.aggression.clamp(0.0, 1.0);
    let curiosity = brain.curiosity.clamp(0.0, 1.0);
    let (e_mul, r_mul, s_add) = element_profile(element);

    let energy = ((1.0 + power * 8.0 + aggression * 3.0) * e_mul).clamp(0.0, 12.0);
    let range = ((1.0 + focus * 14.0 + power * 6.0) * r_mul).clamp(1.0, 24.0);
    let spread = (volatility * 0.6 + curiosity * 0.2 - focus * 0.3 + s_add).clamp(0.0, 1.0);
    let jitter_seed = mix(genome.id ^ element_tag(element).wrapping_mul(0x100_0000_01B3));

    SignatureParams { element, energy, range, spread, jitter_seed }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }
    const ALL: [Element; 7] = [
        Element::Earth,
        Element::Dust,
        Element::Fire,
        Element::Lightning,
        Element::Air,
        Element::Water,
        Element::Wood,
    ];

    #[test]
    fn signature_is_deterministic() {
        let g = DragonGenome { power: 0.7, focus: 0.6, volatility: 0.3, id: 0xABCD };
        let b = Brain { aggression: 0.8, curiosity: 0.4 };
        assert_eq!(signature_from(&g, Element::Fire, &b), signature_from(&g, Element::Fire, &b));
    }

    #[test]
    fn different_elements_give_distinct_signatures() {
        let g = DragonGenome { power: 0.6, focus: 0.5, volatility: 0.4, id: 7 };
        let b = Brain { aggression: 0.5, curiosity: 0.5 };
        let mut sigs = Vec::new();
        for &e in &ALL {
            sigs.push(signature_from(&g, e, &b));
        }
        // every pair differs in at least one of (energy, range, spread, jitter_seed)
        for i in 0..sigs.len() {
            for j in (i + 1)..sigs.len() {
                let a = &sigs[i];
                let c = &sigs[j];
                let differ = (a.energy - c.energy).abs() > 1e-4
                    || (a.range - c.range).abs() > 1e-4
                    || (a.spread - c.spread).abs() > 1e-4
                    || a.jitter_seed != c.jitter_seed;
                assert!(differ, "elements {:?} vs {:?} produced identical signatures", a.element, c.element);
            }
        }
    }

    #[test]
    fn more_aggression_means_more_energy() {
        let g = DragonGenome { power: 0.5, focus: 0.5, volatility: 0.5, id: 1 };
        let calm = signature_from(&g, Element::Fire, &Brain { aggression: 0.1, curiosity: 0.5 });
        let fierce = signature_from(&g, Element::Fire, &Brain { aggression: 0.9, curiosity: 0.5 });
        assert!(fierce.energy > calm.energy, "aggression drives energy");
    }

    #[test]
    fn prop_signature_bounded_and_finite() {
        let mut seed = 0x0D12_A901u32;
        for _ in 0..6000 {
            let g = DragonGenome {
                power: lcg(&mut seed) * 2.0 - 0.5,
                focus: lcg(&mut seed) * 2.0 - 0.5,
                volatility: lcg(&mut seed) * 2.0 - 0.5,
                id: (lcg(&mut seed) * 1e9) as u64,
            };
            let b = Brain { aggression: lcg(&mut seed) * 2.0 - 0.5, curiosity: lcg(&mut seed) * 2.0 - 0.5 };
            let e = ALL[(lcg(&mut seed) * 7.0) as usize % 7];
            let s = signature_from(&g, e, &b);
            assert!(s.energy.is_finite() && s.range.is_finite() && s.spread.is_finite());
            assert!(s.energy >= 0.0 && s.energy <= 12.0, "energy {}", s.energy);
            assert!(s.range >= 1.0 && s.range <= 24.0, "range {}", s.range);
            assert!(s.spread >= 0.0 && s.spread <= 1.0, "spread {}", s.spread);
        }
    }
}
