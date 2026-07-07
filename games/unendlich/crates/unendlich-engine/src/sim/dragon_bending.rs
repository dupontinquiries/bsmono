//! `sim::dragon_bending` — genetics-derived elemental bending. A dragon's innate
//! element affinities (its genome) project onto the seven ambient field channels
//! that gate bending disciplines (`affinity_access`); blended with the live
//! environment they decide what it can bend and how strongly. Sustained
//! channelling builds power through `resonance_state`, and equipped focus items
//! (`bending_focus`) tilt the result. Pure, deterministic, panic-free.
//!
//! Element → channel map (genome `element` is Wood,Water,Air,Dust,Fire,Storm;
//! channels are h,r,w,p,l,s,b = humidity,heat,wind,pressure,lightning,storm,biome):
//!   Water→humidity, Fire→heat, Air→wind, Dust→pressure, Storm→lightning,
//!   Storm+Dust→storm/particulate, Wood→biome.

use crate::sim::affinity_access::{access, Access, Fields, DISCIPLINES};
use crate::sim::bending_focus::FocusMods;
use crate::sim::dragon_archetype::Genome;
use crate::sim::resonance_state::{self, Inputs, Resonance};

#[inline]
fn c01(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.max(0.0).min(1.0) }
}
#[inline]
fn chans(f: &Fields) -> [f32; 7] {
    [f.h, f.r, f.w, f.p, f.l, f.s, f.b]
}
#[inline]
fn from_chans(c: [f32; 7]) -> Fields {
    Fields { h: c[0], r: c[1], w: c[2], p: c[3], l: c[4], s: c[5], b: c[6] }
}

/// The dragon's innate field affinities, projected from its genome elements.
pub fn innate_fields(g: &Genome) -> Fields {
    let e = &g.element;
    Fields {
        h: c01(e[1]),                    // Water → humidity
        r: c01(e[4]),                    // Fire  → heat
        w: c01(e[2]),                    // Air   → wind
        p: c01(e[3]),                    // Dust  → pressure
        l: c01(e[5]),                    // Storm → lightning
        s: c01(0.7 * e[5] + 0.5 * e[3]), // Storm + Dust → storm/particulate
        b: c01(e[0]),                    // Wood  → biome
    }
}

/// Innate affinity blended with the live ambient fields (`env_weight01` = how
/// much the environment matters; 0 = innate only, 1 = ambient only).
pub fn effective_fields(g: &Genome, ambient: Fields, env_weight01: f32) -> Fields {
    let w = c01(env_weight01);
    let inn = chans(&innate_fields(g));
    let amb = chans(&ambient);
    let mut c = [0.0f32; 7];
    for i in 0..7 {
        c[i] = c01(inn[i] * (1.0 - w) + amb[i] * w);
    }
    from_chans(c)
}

/// Which disciplines the dragon can bend now (genetics × environment).
pub fn bender_access(g: &Genome, ambient: Fields, env_weight01: f32) -> Access {
    access(&effective_fields(g, ambient, env_weight01))
}

/// Can the dragon bend `discipline` (0..6) right now?
pub fn can_bend(g: &Genome, ambient: Fields, env_weight01: f32, discipline: u8) -> bool {
    let d = discipline as usize;
    d < DISCIPLINES && (bender_access(g, ambient, env_weight01).disciplines & (1 << d)) != 0
}

/// The dragon's signature discipline: its strongest innate channel.
pub fn dominant_discipline(g: &Genome) -> u8 {
    let c = chans(&innate_fields(g));
    let mut bi = 0usize;
    let mut bv = f32::MIN;
    for (i, &v) in c.iter().enumerate() {
        if v > bv {
            bv = v;
            bi = i;
        }
    }
    bi as u8
}

/// Potency of bending `discipline` right now: 0 if locked, else the access
/// strength (with focus field-projection + strength bonus) scaled by `intent01`
/// and the resonance tier (0..3 → ×1.0..×2.5).
pub fn potency(
    g: &Genome,
    ambient: Fields,
    env_weight01: f32,
    discipline: u8,
    intent01: f32,
    res_tier: u8,
    focus: &FocusMods,
) -> f32 {
    let d = discipline as usize;
    if d >= DISCIPLINES {
        return 0.0;
    }
    // focus items can project extra field (e.g. an imbued artifact) before access
    let eff = chans(&effective_fields(g, ambient, env_weight01));
    let mut c = [0.0f32; 7];
    for i in 0..7 {
        c[i] = c01(eff[i] + focus.field_delta[i]);
    }
    let acc = access(&from_chans(c));
    if acc.disciplines & (1 << d) == 0 {
        return 0.0;
    }
    let base = acc.strength[d] + focus.strength_add[d].max(0.0);
    let res_bonus = 1.0 + (res_tier.min(3) as f32) * 0.5;
    (base * c01(intent01) * res_bonus).max(0.0)
}

/// Advance resonance while channelling `discipline`: stillness and how well the
/// dragon's effective field matches that discipline drive the coherence gain;
/// `spent`/`damage` drain it. Composes `resonance_state::update`.
pub fn channel_step(
    res: Resonance,
    g: &Genome,
    ambient: Fields,
    env_weight01: f32,
    discipline: u8,
    stillness01: f32,
    spent: f32,
    damage: f32,
    dt: f32,
) -> Resonance {
    let d = (discipline as usize).min(DISCIPLINES - 1);
    let match01 = chans(&effective_fields(g, ambient, env_weight01))[d];
    resonance_state::update(res, &Inputs { stillness01, match01, spent, damage, dt })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::bending_focus::{focus, FocusItem, NUM_DISCIPLINES};
    use crate::sim::resonance_state::{tier, Resonance};

    // a genome with exactly one element expressed
    fn pure(elem: usize, v: f32) -> Genome {
        let mut g = Genome::from_seed(1);
        g.element = [0.0; 6];
        g.element[elem] = v;
        g
    }
    fn no_ambient() -> Fields { Fields { h: 0.0, r: 0.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0 } }

    #[test]
    fn fire_dragon_bends_heat_not_biome() {
        let g = pure(4, 0.95); // Fire
        assert!(can_bend(&g, no_ambient(), 0.0, 1), "fire ⇒ heat discipline (1)");
        assert!(!can_bend(&g, no_ambient(), 0.0, 6), "no Wood ⇒ no biome discipline (6)");
        assert_eq!(dominant_discipline(&g), 1, "signature is heat");
    }

    #[test]
    fn water_and_storm_map_to_their_channels() {
        assert_eq!(dominant_discipline(&pure(1, 0.9)), 0, "Water → humidity");
        assert_eq!(dominant_discipline(&pure(2, 0.9)), 2, "Air → wind");
        assert_eq!(dominant_discipline(&pure(5, 0.9)), 4, "Storm → lightning");
        assert_eq!(dominant_discipline(&pure(0, 0.9)), 6, "Wood → biome");
    }

    #[test]
    fn potency_zero_when_locked_rises_with_intent_and_tier() {
        let g = pure(4, 0.95);
        let id = FocusMods::identity();
        assert_eq!(potency(&g, no_ambient(), 0.0, 6, 1.0, 3, &id), 0.0, "locked ⇒ 0");
        let low = potency(&g, no_ambient(), 0.0, 1, 0.5, 0, &id);
        let high = potency(&g, no_ambient(), 0.0, 1, 1.0, 0, &id);
        assert!(high > low && low > 0.0, "intent raises potency");
        let charged = potency(&g, no_ambient(), 0.0, 1, 1.0, 3, &id);
        assert!(charged > high, "resonance tier raises potency");
    }

    #[test]
    fn environment_can_carry_a_weak_affinity() {
        let g = pure(4, 0.95); // strong fire, no wind innate
        let windy = Fields { w: 1.0, ..no_ambient() };
        assert!(!can_bend(&g, no_ambient(), 0.0, 2), "no innate wind");
        assert!(can_bend(&g, windy, 1.0, 2), "a gale lets it bend wind");
    }

    #[test]
    fn focus_item_can_unlock_a_borderline_discipline() {
        let g = pure(4, 0.95);
        let id = FocusMods::identity();
        assert_eq!(potency(&g, no_ambient(), 0.0, 2, 1.0, 0, &id), 0.0, "wind locked");
        // an imbued focus projecting wind field
        let mut delta = [0.0f32; NUM_DISCIPLINES];
        delta[2] = 1.0;
        let mods = focus(&[FocusItem { discipline: 2, strength_bonus: 0.2, resonance_gain_mul: 1.0, field_delta: delta }]);
        assert!(potency(&g, no_ambient(), 0.0, 2, 1.0, 0, &mods) > 0.0, "focus unlocks wind");
    }

    #[test]
    fn channelling_a_matched_element_builds_resonance() {
        let g = pure(4, 0.95); // fire
        let mut res = Resonance { coherence: 0.0 };
        for _ in 0..30 {
            res = channel_step(res, &g, no_ambient(), 0.0, 1, 1.0, 0.0, 0.0, 0.1);
        }
        assert!(tier(&res) >= 1, "still + matched bending charges coherence");
        // spending drains it
        let drained = channel_step(res, &g, no_ambient(), 0.0, 1, 0.0, 2.0, 0.0, 0.1);
        assert!(drained.coherence < res.coherence);
    }

    #[test]
    fn deterministic_and_bounded() {
        let g = pure(5, 0.9);
        let a = potency(&g, no_ambient(), 0.0, 4, 0.8, 2, &FocusMods::identity());
        let b = potency(&g, no_ambient(), 0.0, 4, 0.8, 2, &FocusMods::identity());
        assert_eq!(a, b);
        assert!(a.is_finite() && a >= 0.0);
        for d in 0..7u8 {
            assert!(potency(&g, no_ambient(), 0.0, d, 1.0, 3, &FocusMods::identity()).is_finite());
        }
    }
}
