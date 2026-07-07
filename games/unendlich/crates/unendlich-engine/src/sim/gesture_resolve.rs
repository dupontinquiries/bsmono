//! `sim::gesture_resolve` — resolve a bending technique purely from the gesture
//! grammar, with **no named-ability registry**.
//!
//! The Resonance Bending spec (§3.2, §10.2) says techniques are composed at
//! runtime from a grammar — `[stance] + [gesture] + [directional] + [element]`
//! — rather than looked up in a table of hand-authored abilities. This module
//! is that composition: four enums in, one [`TechniqueShape`] out, computed by
//! straight arithmetic over the grammar (no map, no lookup table, no I/O).
//!
//! How each axis contributes:
//! - **Stance** sets the base scale and gross shape. Bigger stances hit harder
//!   (`Surge > Rooted > Flow > Open > Aerial` on scale); `Rooted` is inherently
//!   `radial`; `Aerial` is gravity-interacting, modelled as extended range.
//! - **Gesture** sets the emission mode. `Strike` is a point impulse
//!   (charge-independent); `Sustain` and `ChargeRelease` scale with `charge01`
//!   (monotonically); `Echo`/`Draw`/`Redirect`/`Release` are charge-independent
//!   shapes.
//! - **Directional** sets the projection, modelled as a range multiplier
//!   (`Push`/`Pull` long, `Sweep` wide-medium, `Lift`/`Ground` short vertical,
//!   `Radial` omnidirectional → `radial`).
//! - **Element** tags the substance via `kind` (`Earth=0 Water=1 Fire=2 Air=3`).
//!
//! ## Emergent signature techniques (the point of having no registry)
//! Each element has one *archetype* triple `(stance, gesture, directional)`.
//! When the cast matches its element's archetype, `signature = true` and the
//! scale is amplified — so a "named technique" *feel* emerges from the grammar
//! with no ability list to maintain. Documented archetypes:
//!
//! | element | stance  | gesture        | directional | feel             |
//! |---------|---------|----------------|-------------|------------------|
//! | Earth   | Rooted  | ChargeRelease  | Radial      | seismic bloom    |
//! | Water   | Flow    | Sustain        | Pull        | undertow         |
//! | Fire    | Surge   | ChargeRelease  | Push        | flame lance      |
//! | Air     | Aerial  | ChargeRelease  | Lift        | updraft burst    |
//!
//! Pure, deterministic, finite. No `crate::render`, no `ash`/`vk`, no I/O, no
//! clock, no RNG, no `.unwrap()`/`.expect()` in non-test code.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stance {
    Open,
    Rooted,
    Flow,
    Surge,
    Aerial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Strike,
    Sustain,
    ChargeRelease,
    Echo,
    Draw,
    Redirect,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Directional {
    Push,
    Pull,
    Sweep,
    Ground,
    Lift,
    Radial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element {
    Earth,
    Water,
    Fire,
    Air,
}

/// The resolved technique: a substance `kind`, a `scale` (magnitude), a `range`
/// (projection reach), whether it is `radial` (omnidirectional), and whether it
/// is a `signature` (archetype-matching) technique.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TechniqueShape {
    pub kind: u8,
    pub scale: f32,
    pub range: f32,
    pub radial: bool,
    pub signature: bool,
}

/// Scale amplification applied when the cast matches its element's archetype.
const SIGNATURE_AMP: f32 = 1.5;

fn clamp01(v: f32) -> f32 {
    // r344 dedup: branch clamp w/o NaN-sanitise == mathx::clampf(_,0,1).
    crate::util::mathx::clampf(v, 0.0, 1.0)
}

/// Base scale per stance (bigger stance ⇒ bigger technique). `Surge > Open`.
fn stance_scale(s: Stance) -> f32 {
    match s {
        Stance::Aerial => 0.9,
        Stance::Open => 1.0,
        Stance::Flow => 1.05,
        Stance::Rooted => 1.15,
        Stance::Surge => 1.5,
    }
}

/// Range multiplier per stance. `Aerial` is gravity-interacting → reaches far.
fn stance_range_mult(s: Stance) -> f32 {
    match s {
        Stance::Rooted => 0.9, // grounded, close
        Stance::Open => 1.0,
        Stance::Flow => 1.1,
        Stance::Surge => 1.2,
        Stance::Aerial => 1.4, // gravity-interacting, travels
    }
}

/// Scale factor for the gesture's emission mode. `Sustain`/`ChargeRelease`
/// increase strictly with `charge01`; all others are charge-independent.
fn gesture_scale(g: Gesture, charge01: f32) -> f32 {
    let c = clamp01(charge01);
    match g {
        Gesture::Strike => 1.0,             // point impulse
        Gesture::Sustain => 0.6 + 0.8 * c,  // beam, grows with charge (0.6..1.4)
        Gesture::ChargeRelease => 0.5 + 1.0 * c, // burst, grows with charge (0.5..1.5)
        Gesture::Echo => 1.0,               // paired emission
        Gesture::Draw => 1.0,
        Gesture::Redirect => 1.0,
        Gesture::Release => 1.0,
    }
}

/// Range multiplier per directional projection.
fn directional_range_mult(d: Directional) -> f32 {
    match d {
        Directional::Ground => 0.8, // short, downward
        Directional::Lift => 0.9,   // short, upward
        Directional::Radial => 1.0, // omnidirectional radius
        Directional::Sweep => 1.2,  // wide arc
        Directional::Push => 1.5,   // long, forward
        Directional::Pull => 1.5,   // long, drawn in
    }
}

fn element_kind(e: Element) -> u8 {
    match e {
        Element::Earth => 0,
        Element::Water => 1,
        Element::Fire => 2,
        Element::Air => 3,
    }
}

/// The archetype triple `(stance, gesture, directional)` for an element.
fn archetype(e: Element) -> (Stance, Gesture, Directional) {
    match e {
        Element::Earth => (Stance::Rooted, Gesture::ChargeRelease, Directional::Radial),
        Element::Water => (Stance::Flow, Gesture::Sustain, Directional::Pull),
        Element::Fire => (Stance::Surge, Gesture::ChargeRelease, Directional::Push),
        Element::Air => (Stance::Aerial, Gesture::ChargeRelease, Directional::Lift),
    }
}

/// Resolve a technique from the grammar. See module docs.
pub fn resolve(
    stance: Stance,
    gesture: Gesture,
    dir: Directional,
    element: Element,
    charge01: f32,
) -> TechniqueShape {
    let signature = archetype(element) == (stance, gesture, dir);
    let amp = if signature { SIGNATURE_AMP } else { 1.0 };

    let scale = stance_scale(stance) * gesture_scale(gesture, charge01) * amp;
    let range = stance_range_mult(stance) * directional_range_mult(dir);
    let radial = matches!(stance, Stance::Rooted) || matches!(dir, Directional::Radial);

    TechniqueShape {
        kind: element_kind(element),
        scale,
        range,
        radial,
        signature,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STANCES: [Stance; 5] = [
        Stance::Open,
        Stance::Rooted,
        Stance::Flow,
        Stance::Surge,
        Stance::Aerial,
    ];
    const GESTURES: [Gesture; 7] = [
        Gesture::Strike,
        Gesture::Sustain,
        Gesture::ChargeRelease,
        Gesture::Echo,
        Gesture::Draw,
        Gesture::Redirect,
        Gesture::Release,
    ];
    const DIRS: [Directional; 6] = [
        Directional::Push,
        Directional::Pull,
        Directional::Sweep,
        Directional::Ground,
        Directional::Lift,
        Directional::Radial,
    ];
    const ELEMENTS: [Element; 4] = [Element::Earth, Element::Water, Element::Fire, Element::Air];

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn surge_scale_exceeds_open() {
        // same non-signature context, only stance differs
        let surge = resolve(Stance::Surge, Gesture::Strike, Directional::Sweep, Element::Water, 0.5);
        let open = resolve(Stance::Open, Gesture::Strike, Directional::Sweep, Element::Water, 0.5);
        assert!(surge.scale > open.scale, "Surge {} !> Open {}", surge.scale, open.scale);
    }

    #[test]
    fn charge_release_and_sustain_scale_monotone_in_charge() {
        // non-signature context (Open + ... + Water, whose archetype differs)
        for &g in &[Gesture::ChargeRelease, Gesture::Sustain] {
            let mut prev = resolve(Stance::Open, g, Directional::Sweep, Element::Water, 0.0).scale;
            let mut c = 0.05;
            while c <= 1.0001 {
                let now = resolve(Stance::Open, g, Directional::Sweep, Element::Water, c).scale;
                assert!(now > prev, "{:?} not increasing at charge {}: {} !> {}", g, c, now, prev);
                prev = now;
                c += 0.05;
            }
        }
    }

    #[test]
    fn other_gestures_are_charge_independent() {
        for &g in &[Gesture::Strike, Gesture::Echo, Gesture::Draw, Gesture::Redirect, Gesture::Release] {
            let a = resolve(Stance::Open, g, Directional::Sweep, Element::Water, 0.0).scale;
            let b = resolve(Stance::Open, g, Directional::Sweep, Element::Water, 1.0).scale;
            assert_eq!(a, b, "{:?} should not depend on charge", g);
        }
    }

    #[test]
    fn rooted_is_radial() {
        for &g in &GESTURES {
            for &d in &DIRS {
                if d == Directional::Radial {
                    continue; // would be radial anyway; test the stance alone
                }
                for &e in &ELEMENTS {
                    assert!(
                        resolve(Stance::Rooted, g, d, e, 0.5).radial,
                        "Rooted must be radial ({:?},{:?},{:?})", g, d, e
                    );
                }
            }
        }
        // Radial directional also forces radial regardless of stance
        assert!(resolve(Stance::Open, Gesture::Strike, Directional::Radial, Element::Air, 0.0).radial);
    }

    #[test]
    fn directional_sets_projection() {
        let r = |d| resolve(Stance::Open, Gesture::Strike, d, Element::Earth, 0.0).range;
        // Open stance range_mult == 1.0, so range == directional_range_mult
        let ground = r(Directional::Ground);
        let lift = r(Directional::Lift);
        let sweep = r(Directional::Sweep);
        let push = r(Directional::Push);
        let pull = r(Directional::Pull);
        assert!(ground < lift, "Ground {} !< Lift {}", ground, lift);
        assert!(lift < sweep, "Lift {} !< Sweep {}", lift, sweep);
        assert!(sweep < push, "Sweep {} !< Push {}", sweep, push);
        assert_eq!(push, pull, "Push/Pull are both long-range projections");
        assert!((ground - 0.8).abs() < 1e-6 && (push - 1.5).abs() < 1e-6);
    }

    #[test]
    fn kind_tags_element() {
        assert_eq!(resolve(Stance::Open, Gesture::Strike, Directional::Sweep, Element::Earth, 0.0).kind, 0);
        assert_eq!(resolve(Stance::Open, Gesture::Strike, Directional::Sweep, Element::Water, 0.0).kind, 1);
        assert_eq!(resolve(Stance::Open, Gesture::Strike, Directional::Sweep, Element::Fire, 0.0).kind, 2);
        assert_eq!(resolve(Stance::Open, Gesture::Strike, Directional::Sweep, Element::Air, 0.0).kind, 3);
    }

    #[test]
    fn signature_true_exactly_for_archetypes() {
        // exhaustive over all 5*7*6*4 = 840 combinations
        for &st in &STANCES {
            for &g in &GESTURES {
                for &d in &DIRS {
                    for &e in &ELEMENTS {
                        let shape = resolve(st, g, d, e, 0.7);
                        let want = archetype(e) == (st, g, d);
                        assert_eq!(
                            shape.signature, want,
                            "signature mismatch for {:?}+{:?}+{:?}+{:?}", st, g, d, e
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn documented_archetypes_are_signature_and_amplified() {
        // Each archetype is signature; a different element on the same triple is
        // not, and the signature scale is exactly SIGNATURE_AMP× the plain one
        // (element doesn't affect scale, only the signature amp does).
        for &e in &ELEMENTS {
            let (st, g, d) = archetype(e);
            let sig = resolve(st, g, d, e, 0.7);
            assert!(sig.signature, "{:?} archetype should be signature", e);

            // pick a different element whose archetype != this triple
            let other = ELEMENTS.iter().copied().find(|&o| archetype(o) != (st, g, d)).unwrap();
            let plain = resolve(st, g, d, other, 0.7);
            assert!(!plain.signature);
            assert!(
                (sig.scale - SIGNATURE_AMP * plain.scale).abs() < 1e-4,
                "{:?}: signature scale {} != {}x plain {}", e, sig.scale, SIGNATURE_AMP, plain.scale
            );
            assert!(sig.scale > plain.scale);
        }
    }

    #[test]
    fn all_outputs_finite_incl_out_of_range_charge() {
        let mut s = 0x6E57_0000u32;
        for &st in &STANCES {
            for &g in &GESTURES {
                for &d in &DIRS {
                    for &e in &ELEMENTS {
                        // include out-of-range charges
                        for &c in &[-5.0f32, -0.0, 0.0, 0.33, 1.0, 7.5, (lcg(&mut s) % 1000) as f32] {
                            let shape = resolve(st, g, d, e, c);
                            assert!(shape.scale.is_finite() && shape.scale > 0.0, "scale {}", shape.scale);
                            assert!(shape.range.is_finite() && shape.range > 0.0, "range {}", shape.range);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn deterministic() {
        let mut s = 0x6E57_0001u32;
        for _ in 0..5000 {
            let st = STANCES[(lcg(&mut s) as usize) % STANCES.len()];
            let g = GESTURES[(lcg(&mut s) as usize) % GESTURES.len()];
            let d = DIRS[(lcg(&mut s) as usize) % DIRS.len()];
            let e = ELEMENTS[(lcg(&mut s) as usize) % ELEMENTS.len()];
            let c = (lcg(&mut s) % 1001) as f32 / 1000.0;
            assert_eq!(resolve(st, g, d, e, c), resolve(st, g, d, e, c));
        }
    }
}
