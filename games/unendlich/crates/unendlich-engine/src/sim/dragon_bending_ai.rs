//! `sim::dragon_bending_ai` — the dragon's equivalent of the player's gesture
//! arcs, but chosen by genetics and aimed by its eyes instead of by precise
//! input. A dragon can only bend at what it is *looking at* (`dragon_observe`'s
//! attended target), it winds up before releasing (a creature tell, not an
//! instant cast), and its aim carries natural error that grows when it's calm or
//! the target is far — so it feels alive, not like a turret.
//!
//! It reuses the player's technique grammar (`gesture_resolve`) so dragons and
//! players share one system: the dragon's genome picks the (stance, gesture,
//! directional, element) quadruple — its *signature arc* — and the released
//! [`TechniqueShape`] is scaled by `dragon_bending::potency` (genetics ×
//! environment × resonance). The element archetypes are the abilities: Earth's
//! (Rooted, ChargeRelease, Radial) is the pillar; a Fire dragon with a sweeping
//! tail throws an ash cloud. Pure, deterministic, panic-free.

use crate::sim::affinity_access::Fields;
use crate::sim::bending_focus::FocusMods;
use crate::sim::dragon_archetype::Genome;
use crate::sim::dragon_bending;
use crate::sim::gesture_resolve::{resolve, Directional, Element as GElement, Gesture, Stance, TechniqueShape};

/// Genome dominant element (Wood,Water,Air,Dust,Fire,Storm) → the four combat
/// elements of the gesture grammar.
pub fn combat_element(g: &Genome) -> GElement {
    let mut idx = 0usize;
    let mut best = f32::MIN;
    for (i, &w) in g.element.iter().enumerate() {
        if w > best {
            best = w;
            idx = i;
        }
    }
    match idx {
        1 => GElement::Water,         // Water
        2 | 5 => GElement::Air,       // Air, Storm
        4 => GElement::Fire,          // Fire
        _ => GElement::Earth,         // Wood, Dust
    }
}

/// The element's signature triple (matches `gesture_resolve`'s archetypes, so the
/// dragon earns the signature amplification when its body doesn't override it).
fn archetype(e: GElement) -> (Stance, Gesture, Directional) {
    match e {
        GElement::Earth => (Stance::Rooted, Gesture::ChargeRelease, Directional::Radial),
        GElement::Water => (Stance::Flow, Gesture::Sustain, Directional::Pull),
        GElement::Fire => (Stance::Surge, Gesture::ChargeRelease, Directional::Push),
        GElement::Air => (Stance::Aerial, Gesture::ChargeRelease, Directional::Lift),
    }
}

/// The dragon's genetically-favoured gesture arc. Body proportions tilt the
/// stance and projection, so two Fire dragons bend differently: a winged one
/// casts from an Aerial stance, a heavy one Surges, a long-tailed one Sweeps its
/// fire into a wide cloud, a long-legged one drives it into the Ground (a pillar).
pub fn signature_arc(g: &Genome) -> (Stance, Gesture, Directional, GElement) {
    let e = combat_element(g);
    let (mut stance, gesture, mut dir) = archetype(e);
    if g.wing_span > 0.66 {
        stance = Stance::Aerial;
    } else if g.bulk > 0.66 {
        stance = Stance::Surge;
    }
    if g.tail_len > 0.66 {
        dir = Directional::Sweep;
    } else if g.leg_len > 0.66 {
        dir = Directional::Ground;
    }
    (stance, gesture, dir, e)
}

/// What the dragon perceives this tick — built from `dragon_observe` (the eyes)
/// and `dragon_activity` (its willingness/alertness).
#[derive(Clone, Copy, Debug)]
pub struct Perception {
    /// The eyes are locked on something (observe `attending.is_some()`).
    pub has_target: bool,
    /// Gaze bearing to that target, relative to the body heading (rad).
    pub target_bearing: f32,
    pub target_dist2: f32,
    /// Willingness to bend now (0..1) — threat / aggression / command.
    pub willing: f32,
    /// Alertness (0..1) — sharper dragons aim truer.
    pub alertness01: f32,
}

/// Wind-up state carried between ticks (the creature tells before a cast).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BendIntent {
    pub charge01: f32,
}
impl BendIntent {
    pub fn idle() -> Self {
        BendIntent { charge01: 0.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Release {
    pub technique: TechniqueShape,
    /// Where it actually aims (relative to body heading) — eyes + natural error.
    pub aim_bearing: f32,
    pub potency: f32,
    pub element: GElement,
}

#[derive(Clone, Copy, Debug)]
pub struct BendAiStep {
    pub intent: BendIntent,
    /// `Some` only on the tick it fires.
    pub release: Option<Release>,
}

const WINDUP_BASE: f32 = 1.2; // charge/sec
const RELAX_RATE: f32 = 1.5;

fn hash_signed(seed: u64) -> f32 {
    let mut z = seed ^ 0x9E37_79B9_7F4A_7C15;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    ((z >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0 // [-1,1)
}

/// Aim error (rad): grows as alertness falls and as the target gets farther — a
/// calm or distant dragon misses by more. Deterministic in `seed`.
pub fn aim_error(seed: u64, alertness01: f32, target_dist2: f32) -> f32 {
    let calm = 1.0 - alertness01.clamp(0.0, 1.0);
    let spread = 0.40 * calm + 0.0006 * target_dist2.max(0.0).min(2500.0);
    hash_signed(seed) * spread
}

/// One bending-AI tick. The dragon charges only while it's looking at a target it
/// wants to hit; once charged it releases a genetics-chosen technique aimed (with
/// error) where its eyes are. Returns `release: None` while charging or idle.
#[allow(clippy::too_many_arguments)]
pub fn step(
    intent: BendIntent,
    g: &Genome,
    ambient: Fields,
    env_weight01: f32,
    res_tier: u8,
    p: &Perception,
    seed: u64,
    dt: f32,
) -> BendAiStep {
    let dt = dt.max(0.0);
    let mut intent = intent;
    let disc = dragon_bending::dominant_discipline(g);
    let capable = dragon_bending::can_bend(g, ambient, env_weight01, disc);

    // Relax if it has nothing to bend at, can't bend here, or isn't willing.
    if !p.has_target || !capable || p.willing < 0.2 {
        intent.charge01 = (intent.charge01 - RELAX_RATE * dt).max(0.0);
        return BendAiStep { intent, release: None };
    }

    // Wind up — faster the more willing it is and the more combative its build.
    let combat_build = ((g.teeth + g.horns) * 0.5).clamp(0.0, 1.0);
    let rate = WINDUP_BASE * (0.5 + p.willing.clamp(0.0, 1.0)) * (0.6 + 0.8 * combat_build);
    intent.charge01 = (intent.charge01 + rate * dt).min(1.0);
    if intent.charge01 < 1.0 {
        return BendAiStep { intent, release: None }; // still telegraphing the cast
    }

    // Release: the genetics-chosen arc, charged by the wind-up, aimed by the eyes.
    let (stance, gesture, dir, elem) = signature_arc(g);
    let technique = resolve(stance, gesture, dir, elem, intent.charge01);
    let potency = dragon_bending::potency(
        g, ambient, env_weight01, disc, p.willing.clamp(0.0, 1.0), res_tier, &FocusMods::identity());
    let aim_bearing = p.target_bearing + aim_error(seed, p.alertness01, p.target_dist2);
    intent.charge01 = 0.0;
    BendAiStep {
        intent,
        release: Some(Release { technique, aim_bearing, potency, element: elem }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fire() -> Genome {
        let mut g = Genome::from_seed(1);
        g.element = [0.1, 0.1, 0.1, 0.1, 0.95, 0.1];
        g.teeth = 0.8;
        g.horns = 0.6;
        g.wing_span = 0.3;
        g.bulk = 0.3;
        g.tail_len = 0.3;
        g.leg_len = 0.3;
        g
    }
    fn hot() -> Fields { Fields { h: 0.0, r: 1.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0 } }
    fn target() -> Perception {
        Perception { has_target: true, target_bearing: 0.5, target_dist2: 100.0, willing: 0.9, alertness01: 0.8 }
    }

    #[test]
    fn maps_genome_to_combat_element() {
        assert_eq!(combat_element(&fire()), GElement::Fire);
        let mut g = Genome::from_seed(1);
        g.element = [0.9, 0.1, 0.1, 0.1, 0.1, 0.1]; // Wood
        assert_eq!(combat_element(&g), GElement::Earth);
    }

    #[test]
    fn body_tilts_the_signature_arc() {
        let mut winged = fire();
        winged.wing_span = 0.9;
        assert_eq!(signature_arc(&winged).0, Stance::Aerial, "winged ⇒ aerial stance");
        let mut sweeper = fire();
        sweeper.tail_len = 0.9;
        assert_eq!(signature_arc(&sweeper).2, Directional::Sweep, "long tail ⇒ sweeping cloud");
        let mut pillarist = fire();
        pillarist.leg_len = 0.9;
        pillarist.tail_len = 0.2;
        assert_eq!(signature_arc(&pillarist).2, Directional::Ground, "long legs ⇒ ground-driven");
    }

    #[test]
    fn winds_up_then_releases() {
        let g = fire();
        let mut intent = BendIntent::idle();
        let mut fired = None;
        for i in 0..200 {
            let s = step(intent, &g, hot(), 0.4, 1, &target(), i as u64, 0.05);
            intent = s.intent;
            if let Some(r) = s.release {
                fired = Some(r);
                break;
            }
        }
        let r = fired.expect("a willing, capable dragon eventually casts");
        assert_eq!(r.technique.kind, 2, "fires its Fire technique");
        assert!(r.potency > 0.0);
    }

    #[test]
    fn only_casts_at_what_it_sees() {
        let g = fire();
        let mut blind = target();
        blind.has_target = false;
        let mut intent = BendIntent { charge01: 0.9 };
        for i in 0..50 {
            let s = step(intent, &g, hot(), 0.4, 1, &blind, i, 0.05);
            intent = s.intent;
            assert!(s.release.is_none(), "won't bend with nothing in its sights");
        }
        assert_eq!(intent.charge01, 0.0, "and it relaxes the wind-up");
    }

    #[test]
    fn unwilling_does_not_cast() {
        let g = fire();
        let mut p = target();
        p.willing = 0.0;
        let s = step(BendIntent { charge01: 0.9 }, &g, hot(), 0.4, 1, &p, 7, 0.1);
        assert!(s.release.is_none());
    }

    #[test]
    fn aim_follows_the_eyes_with_creature_error() {
        // higher alertness ⇒ tighter aim spread (over many seeds)
        let mut sharp_max = 0.0f32;
        let mut dull_max = 0.0f32;
        for seed in 0..200u64 {
            sharp_max = sharp_max.max(aim_error(seed, 0.95, 100.0).abs());
            dull_max = dull_max.max(aim_error(seed, 0.1, 100.0).abs());
        }
        assert!(dull_max > sharp_max, "calm dragons aim looser");
        // farther target ⇒ looser aim
        let near = aim_error(42, 0.5, 10.0).abs();
        let far = aim_error(42, 0.5, 2000.0).abs();
        assert!(far > near, "distance widens the miss");
    }

    #[test]
    fn deterministic() {
        let g = fire();
        let a = step(BendIntent { charge01: 1.0 }, &g, hot(), 0.4, 2, &target(), 99, 0.05);
        let b = step(BendIntent { charge01: 1.0 }, &g, hot(), 0.4, 2, &target(), 99, 0.05);
        assert_eq!(a.release.map(|r| r.aim_bearing), b.release.map(|r| r.aim_bearing));
    }
}
