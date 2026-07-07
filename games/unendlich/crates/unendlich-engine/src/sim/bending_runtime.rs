//! Bending runtime — one tick of the bender's loop.
//!
//! This is the *wiring* that makes the four pure bending cores run together as a
//! single per-frame update the input/entity layer calls:
//!   - [`affinity_access`] turns the live fields into which disciplines are
//!     usable and how strongly,
//!   - [`resonance_state`] advances the coherence resource (and its tier),
//!   - [`hybrid_form`] gates dual-element forms off those same affinity
//!     strengths plus the tier,
//!   - [`gesture_resolve`] turns a player gesture into a technique, charged by
//!     the current coherence.
//!
//! One field poll, four consumers — exactly the "one field, two consumers"
//! contract from the spec, realised as a tick. Pure: state in → state + output
//! out, deterministic, no I/O, no clock.

use crate::sim::affinity_access::{self, Access, Fields};
use crate::sim::gesture_resolve::{self, Directional, Element, Gesture, Stance, TechniqueShape};
use crate::sim::hybrid_form::{self, Affinities, HybridForm};
use crate::sim::resonance_state::{self, Inputs, Resonance, MAX_COHERENCE};

/// A gesture the player performed this tick. `None` to [`BenderState::update`]
/// means no technique was attempted.
#[derive(Clone, Copy, Debug)]
pub struct GestureInput {
    pub stance: Stance,
    pub gesture: Gesture,
    pub directional: Directional,
    pub element: Element,
}

/// Persistent bender state carried between ticks (just the coherence resource;
/// everything else is derived fresh from the live fields each tick).
#[derive(Clone, Copy, Debug)]
pub struct BenderState {
    pub resonance: Resonance,
}

impl Default for BenderState {
    fn default() -> Self {
        Self { resonance: Resonance { coherence: 0.0 } }
    }
}

/// Everything the renderer/UI/combat layer needs out of one bending tick.
#[derive(Clone, Copy, Debug)]
pub struct BenderTick {
    /// Which disciplines are unlocked + their per-discipline strength.
    pub access: Access,
    /// Current coherence (0..=MAX_COHERENCE).
    pub coherence: f32,
    /// Resonance tier 0..=3.
    pub tier: u8,
    /// Available dual-element hybrid form (inert if `!active`).
    pub hybrid: HybridForm,
    /// The technique a gesture resolved to this tick, if any.
    pub technique: Option<TechniqueShape>,
}

impl BenderState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance one tick.
    ///
    /// `stillness01` / `match01` drive resonance gain; `spent` is coherence
    /// consumed by a held technique or sustained form this tick; `damage` is
    /// incoming damage; `dt` seconds. `gesture` resolves into a technique when
    /// present, charged by the current (post-update) coherence.
    pub fn update(
        &mut self,
        fields: &Fields,
        stillness01: f32,
        match01: f32,
        spent: f32,
        damage: f32,
        dt: f32,
        gesture: Option<GestureInput>,
    ) -> BenderTick {
        // 1. Which disciplines the live fields unlock, and how strongly.
        let access = affinity_access::access(fields);

        // 2. Advance the resonance resource off stillness/match vs spend/damage.
        self.resonance = resonance_state::update(
            self.resonance,
            &Inputs { stillness01, match01, spent, damage, dt },
        );
        let tier = resonance_state::tier(&self.resonance);

        // 3. Affinity strengths feed straight into hybrid-form gating (both are
        //    the same 7-discipline strength vector — no adapter needed).
        let hybrid = hybrid_form::resolve(&Affinities { strength: access.strength }, tier);

        // 4. Resolve a technique if a gesture was performed; charge is the
        //    normalised coherence so a fuller bender hits harder.
        let charge01 = (self.resonance.coherence / MAX_COHERENCE).clamp(0.0, 1.0);
        let technique = gesture.map(|g| {
            gesture_resolve::resolve(g.stance, g.gesture, g.directional, g.element, charge01)
        });

        BenderTick {
            access,
            coherence: self.resonance.coherence,
            tier,
            hybrid,
            technique,
        }
    }

    /// Drive one tick straight from raw player input: detect the gesture
    /// quadruple (`gesture_detect`), and if the player is casting, feed it into
    /// [`update`](Self::update) as the `GestureInput`; otherwise tick with no
    /// gesture (resonance still decays/settles from the fields). This is the
    /// input → grammar → bending bridge the player controller calls each frame.
    #[allow(clippy::too_many_arguments)]
    pub fn update_from_input(
        &mut self,
        fields: &Fields,
        input: &crate::sim::gesture_detect::InputState,
        stillness01: f32,
        match01: f32,
        spent: f32,
        damage: f32,
        dt: f32,
    ) -> BenderTick {
        let d = crate::sim::gesture_detect::detect(input);
        let gesture = if d.active {
            Some(GestureInput {
                stance: d.stance,
                gesture: d.gesture,
                directional: d.directional,
                element: d.element,
            })
        } else {
            None
        };
        self.update(fields, stillness01, match01, spent, damage, dt, gesture)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dry_fields() -> Fields {
        Fields { h: 0.0, r: 0.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0 }
    }
    fn rich_fields() -> Fields {
        Fields { h: 0.9, r: 0.9, w: 0.9, p: 0.9, l: 0.9, s: 0.9, b: 0.9 }
    }
    fn a_gesture() -> GestureInput {
        GestureInput {
            stance: Stance::Surge,
            gesture: Gesture::ChargeRelease,
            directional: Directional::Push,
            element: Element::Fire,
        }
    }

    fn casting_input() -> crate::sim::gesture_detect::InputState {
        use crate::sim::gesture_detect::{InputState, PressPhase};
        InputState {
            look: [0.0, 0.0, 1.0],
            velocity: [0.0, 0.0, 7.0], // surge
            airborne: false,
            sprint: false,
            press: PressPhase::Released,
            charge01: 0.9, // charge-release
            double_tap: false,
            element: Element::Fire,
        }
    }

    #[test]
    fn update_from_input_active_produces_technique() {
        let mut b = BenderState::new();
        let f = rich_fields();
        // settle some coherence first so a technique can form
        for _ in 0..8 {
            b.update_from_input(&f, &casting_input(), 1.0, 1.0, 0.0, 0.0, 0.1);
        }
        let tick = b.update_from_input(&f, &casting_input(), 1.0, 1.0, 0.0, 0.0, 0.1);
        assert!(tick.technique.is_some(), "an active cast should resolve a technique");
    }

    #[test]
    fn update_from_input_idle_has_no_technique() {
        use crate::sim::gesture_detect::PressPhase;
        let mut b = BenderState::new();
        let f = rich_fields();
        let mut idle = casting_input();
        idle.press = PressPhase::Idle;
        idle.double_tap = false;
        let tick = b.update_from_input(&f, &idle, 1.0, 1.0, 0.0, 0.0, 0.1);
        assert!(tick.technique.is_none(), "idle input must not emit a technique");
    }

    #[test]
    fn stillness_and_match_raise_coherence_over_ticks() {
        let mut b = BenderState::new();
        let f = rich_fields();
        let c0 = b.resonance.coherence;
        for _ in 0..10 {
            b.update(&f, 1.0, 1.0, 0.0, 0.0, 0.1, None);
        }
        assert!(b.resonance.coherence > c0, "coherence should rise with stillness+match");
    }

    #[test]
    fn damage_and_spend_lower_coherence() {
        let mut b = BenderState::new();
        let f = rich_fields();
        for _ in 0..20 {
            b.update(&f, 1.0, 1.0, 0.0, 0.0, 0.1, None);
        }
        let charged = b.resonance.coherence;
        b.update(&f, 0.0, 0.0, 1.0, 1.0, 0.1, None);
        assert!(b.resonance.coherence < charged, "spend+damage should lower coherence");
    }

    #[test]
    fn rich_fields_unlock_more_than_dry() {
        let mut b = BenderState::new();
        let dry = b.update(&dry_fields(), 0.0, 0.0, 0.0, 0.0, 0.1, None);
        let rich = b.update(&rich_fields(), 0.0, 0.0, 0.0, 0.0, 0.1, None);
        assert!(
            rich.access.disciplines.count_ones() >= dry.access.disciplines.count_ones(),
            "richer fields unlock at least as many disciplines"
        );
    }

    #[test]
    fn gesture_present_yields_technique_absent_yields_none() {
        let mut b = BenderState::new();
        let f = rich_fields();
        let none = b.update(&f, 1.0, 1.0, 0.0, 0.0, 0.1, None);
        assert!(none.technique.is_none());
        let some = b.update(&f, 1.0, 1.0, 0.0, 0.0, 0.1, Some(a_gesture()));
        let t = some.technique.expect("gesture should resolve to a technique");
        assert!(t.scale.is_finite() && t.scale > 0.0);
    }

    #[test]
    fn tier_tracks_coherence() {
        let mut b = BenderState::new();
        let f = rich_fields();
        let early = b.update(&f, 1.0, 1.0, 0.0, 0.0, 0.1, None).tier;
        for _ in 0..40 {
            b.update(&f, 1.0, 1.0, 0.0, 0.0, 0.1, None);
        }
        let late = b.update(&f, 1.0, 1.0, 0.0, 0.0, 0.1, None).tier;
        assert!(late >= early, "tier should not drop while charging");
    }

    #[test]
    fn deterministic() {
        let run = || {
            let mut b = BenderState::new();
            let f = rich_fields();
            let mut last = b.update(&f, 0.7, 0.5, 0.1, 0.0, 0.05, Some(a_gesture()));
            for _ in 0..5 {
                last = b.update(&f, 0.7, 0.5, 0.1, 0.0, 0.05, Some(a_gesture()));
            }
            (last.coherence, last.tier, last.hybrid.active, last.technique.map(|t| t.kind))
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn hybrid_is_well_formed() {
        let mut b = BenderState::new();
        let t = b.update(&rich_fields(), 1.0, 1.0, 0.0, 0.0, 0.1, None);
        // When inactive, form is inert; when active, cost is positive.
        if t.hybrid.active {
            assert!(t.hybrid.cost > 0.0);
        } else {
            assert_eq!(t.hybrid.cost, 0.0);
        }
    }
}
