//! `sim::player_bending` — the player's bending controller (the spine that turns
//! gesture input into a resolved technique + effect intent).
//!
//! Spec: `docs/specs/resonance_bending_spec.docx` (v2.0). Resonance Bending
//! replaces the action-hotbar: no chi bar, no cooldown counter, no slot binding —
//! every technique is available from the start, gated only by **resonance**
//! (the environment's [`affinity_access::Fields`]) and the **correct gesture**.
//! This module composes the two pure cores rather than duplicating them:
//! - [`super::gesture_resolve`] turns `(stance, gesture, directional, element)`
//!   into a [`gesture_resolve::TechniqueShape`];
//! - [`super::affinity_access`] gates which discipline the local field unlocks.
//!
//! ## Discipline mapping (this module's reading of the v2.0 discipline set)
//! The spec's four named disciplines plus the generic elemental bend map onto
//! `affinity_access`'s field-indexed disciplines (`h,r,w,p,l,s,b = 0..6`):
//!
//! ```text
//!   Information Bending  ← the Echo (probe/listen) gesture, any element  → b (6)
//!   Stormstrike          ← Air element                                   → l (4, lightning)
//!   Heavy Brine          ← Water element                                 → h (0, humidity)
//!   Thermal Phase        ← Fire element                                  → r (1, heat)
//!   Elemental(e)         ← any other element (Earth)                     → e's field
//! ```
//!
//! A gesture **commits exactly once** per activation: [`step`](PlayerBending::step)
//! emits an [`EffectIntent`] on the rising edge of a new gesture key and returns
//! `None` while the same gesture is held (no per-tick spam) or when the discipline
//! is inaccessible. `power01` scales with the resonance charge handle
//! (`charge01`, which r66-02 will drive; the local fallback regenerates it toward
//! full) times the technique's normalised magnitude.
//!
//! Pure, deterministic, panic-free. No `crate::render`, no `ash`/`vk`, no IO, no
//! clock, no RNG, no `.unwrap()`/`.expect()` in non-test code; compiles clean
//! under `cargo check --release` (no `cfg(test)`/`cfg(debug_assertions)` gated
//! helpers).

use super::affinity_access;
use super::gesture_resolve;

/// A bending discipline the player can express. `Elemental` carries the raw
/// element for the generic (non-named) bend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Discipline {
    Stormstrike,
    HeavyBrine,
    ThermalPhase,
    Information,
    Elemental(gesture_resolve::Element),
}

/// One tick of player bending input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BendInput {
    pub stance: gesture_resolve::Stance,
    pub gesture: gesture_resolve::Gesture,
    pub dir: gesture_resolve::Directional,
    pub element: gesture_resolve::Element,
    pub aim: [f32; 3],
}

/// The resolved intent emitted when a technique commits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectIntent {
    pub discipline: Discipline,
    pub shape: gesture_resolve::TechniqueShape,
    pub power01: f32,
    pub aim: [f32; 3],
    pub committed: bool,
}

/// The player's bending controller.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerBending {
    /// Encoded key of the last gesture seen (for rising-edge commit).
    last_key: Option<u32>,
    /// Resonance charge `0..1` (r66-02 drives this; default full).
    pub charge01: f32,
}

/// Normalisation divisor for `TechniqueShape::scale` → `0..1`. The
/// `gesture_resolve` scale peaks around `1.5 (Surge) · 1.5 (ChargeRelease at
/// full) · 1.5 (signature) ≈ 3.375`.
const SCALE_NORM: f32 = 3.4;
/// Local resonance regen per second (fallback until r66-02 wires the real charge).
const CHARGE_REGEN: f32 = 0.5;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

fn norm_scale(scale: f32) -> f32 {
    clamp01(scale / SCALE_NORM)
}

/// Which discipline the given input expresses (see module docs).
pub fn discipline_for(input: &BendInput) -> Discipline {
    use gesture_resolve::{Element, Gesture};
    if matches!(input.gesture, Gesture::Echo) {
        return Discipline::Information;
    }
    match input.element {
        Element::Air => Discipline::Stormstrike,
        Element::Water => Discipline::HeavyBrine,
        Element::Fire => Discipline::ThermalPhase,
        Element::Earth => Discipline::Elemental(Element::Earth),
    }
}

/// The `affinity_access` field index that gates a discipline (`h,r,w,p,l,s,b`).
pub fn governing_field_index(d: Discipline) -> u8 {
    use gesture_resolve::Element;
    match d {
        Discipline::Stormstrike => 4,  // l — lightning
        Discipline::HeavyBrine => 0,   // h — humidity / water
        Discipline::ThermalPhase => 1, // r — heat
        Discipline::Information => 6,  // b — biome richness (ambient information)
        Discipline::Elemental(e) => match e {
            Element::Water => 0, // h
            Element::Fire => 1,  // r
            Element::Air => 2,   // w — wind
            Element::Earth => 3, // p — pressure
        },
    }
}

/// Stable encoding of the gesture identity (rising-edge detection). `aim` is
/// deliberately excluded — re-aiming a held gesture is not a new commit.
fn key_id(i: &BendInput) -> u32 {
    use gesture_resolve::{Directional, Element, Gesture, Stance};
    let s = match i.stance {
        Stance::Open => 0,
        Stance::Rooted => 1,
        Stance::Flow => 2,
        Stance::Surge => 3,
        Stance::Aerial => 4,
    };
    let g = match i.gesture {
        Gesture::Strike => 0,
        Gesture::Sustain => 1,
        Gesture::ChargeRelease => 2,
        Gesture::Echo => 3,
        Gesture::Draw => 4,
        Gesture::Redirect => 5,
        Gesture::Release => 6,
    };
    let d = match i.dir {
        Directional::Push => 0,
        Directional::Pull => 1,
        Directional::Sweep => 2,
        Directional::Ground => 3,
        Directional::Lift => 4,
        Directional::Radial => 5,
    };
    let e = match i.element {
        Element::Earth => 0,
        Element::Water => 1,
        Element::Fire => 2,
        Element::Air => 3,
    };
    s * 1000 + g * 100 + d * 10 + e
}

impl PlayerBending {
    pub fn new() -> PlayerBending {
        PlayerBending { last_key: None, charge01: 1.0 }
    }

    /// Resolve the current input into an [`EffectIntent`], or `None` if the
    /// discipline is inaccessible or the gesture is being held (already
    /// committed). See module docs.
    pub fn step(
        &mut self,
        input: &BendInput,
        fields: &affinity_access::Fields,
        dt: f32,
    ) -> Option<EffectIntent> {
        // Local resonance regen (no-op at full; r66-02 drives charge01).
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        self.charge01 = clamp01(self.charge01 + dt * CHARGE_REGEN);
        let charge = clamp01(self.charge01);

        let shape =
            gesture_resolve::resolve(input.stance, input.gesture, input.dir, input.element, charge);
        let discipline = discipline_for(input);
        let accessible = affinity_access::can_access(fields, governing_field_index(discipline));

        let id = key_id(input);
        let is_new = self.last_key != Some(id);
        self.last_key = Some(id);

        if !accessible || !is_new {
            return None; // gated out, or held (no per-tick spam)
        }

        let power01 = clamp01(charge * norm_scale(shape.scale));
        Some(EffectIntent { discipline, shape, power01, aim: input.aim, committed: true })
    }
}

impl Default for PlayerBending {
    fn default() -> Self {
        PlayerBending::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::affinity_access::Fields;
    use crate::sim::gesture_resolve::{Directional, Element, Gesture, Stance};

    fn fields_all(v: f32) -> Fields {
        Fields { h: v, r: v, w: v, p: v, l: v, s: v, b: v }
    }
    fn input(stance: Stance, gesture: Gesture, dir: Directional, element: Element) -> BendInput {
        BendInput { stance, gesture, dir, element, aim: [0.0, 1.0, 0.0] }
    }

    #[test]
    fn accessible_yields_intent_inaccessible_none() {
        let mut pb = PlayerBending::new();
        let inp = input(Stance::Surge, Gesture::Strike, Directional::Push, Element::Air); // Stormstrike → l
        // full fields: l well over threshold → accessible
        let got = pb.step(&inp, &fields_all(1.0), 0.016);
        assert!(got.is_some());
        assert_eq!(got.unwrap().discipline, Discipline::Stormstrike);
        assert!(got.unwrap().committed);

        // all-zero fields: nothing accessible → None
        let mut pb2 = PlayerBending::new();
        assert!(pb2.step(&inp, &fields_all(0.0), 0.016).is_none());
    }

    #[test]
    fn commit_once_per_hold() {
        let mut pb = PlayerBending::new();
        let inp = input(Stance::Flow, Gesture::Sustain, Directional::Pull, Element::Water); // HeavyBrine → h
        let f = fields_all(1.0);
        // first tick commits
        assert!(pb.step(&inp, &f, 0.016).is_some());
        // holding the same gesture: no further commits
        for _ in 0..20 {
            assert!(pb.step(&inp, &f, 0.016).is_none(), "held gesture must not re-commit");
        }
        // a different gesture commits again
        let inp2 = input(Stance::Surge, Gesture::Strike, Directional::Push, Element::Fire);
        assert!(pb.step(&inp2, &f, 0.016).is_some());
        // and re-issuing the first (after the change) is a new edge → commits
        assert!(pb.step(&inp, &f, 0.016).is_some());
    }

    #[test]
    fn power01_in_range_and_aim_passthrough() {
        let f = fields_all(1.0);
        let elems = [Element::Earth, Element::Water, Element::Fire, Element::Air];
        let gests = [Gesture::Strike, Gesture::Sustain, Gesture::ChargeRelease, Gesture::Echo, Gesture::Draw];
        let dirs = [Directional::Push, Directional::Pull, Directional::Sweep, Directional::Ground, Directional::Lift, Directional::Radial];
        let stances = [Stance::Open, Stance::Rooted, Stance::Flow, Stance::Surge, Stance::Aerial];
        let mut k = 0u32;
        for &st in &stances {
            for &g in &gests {
                for &d in &dirs {
                    for &e in &elems {
                        let mut bi = input(st, g, d, e);
                        bi.aim = [k as f32 * 0.5, -(k as f32), 3.0];
                        // fresh controller each combo so every one is a rising edge
                        let mut p = PlayerBending::new();
                        if let Some(intent) = p.step(&bi, &f, 0.016) {
                            assert!(intent.power01 >= 0.0 && intent.power01 <= 1.0, "power {}", intent.power01);
                            assert_eq!(intent.aim, bi.aim, "aim must pass through");
                        }
                        k += 1;
                    }
                }
            }
        }
    }

    #[test]
    fn discipline_mapping_and_gating() {
        // Echo → Information (gated on b, idx 6)
        assert_eq!(governing_field_index(Discipline::Information), 6);
        let echo = input(Stance::Open, Gesture::Echo, Directional::Sweep, Element::Fire);
        assert_eq!(discipline_for(&echo), Discipline::Information);

        // element → discipline
        assert_eq!(discipline_for(&input(Stance::Open, Gesture::Strike, Directional::Push, Element::Air)), Discipline::Stormstrike);
        assert_eq!(discipline_for(&input(Stance::Open, Gesture::Strike, Directional::Push, Element::Water)), Discipline::HeavyBrine);
        assert_eq!(discipline_for(&input(Stance::Open, Gesture::Strike, Directional::Push, Element::Fire)), Discipline::ThermalPhase);
        assert_eq!(discipline_for(&input(Stance::Open, Gesture::Strike, Directional::Push, Element::Earth)), Discipline::Elemental(Element::Earth));

        // gating: ThermalPhase needs r (idx1) >= 0.55. Set only r high.
        let mut only_r = fields_all(0.0);
        only_r.r = 0.9;
        let mut pb = PlayerBending::new();
        let fire = input(Stance::Open, Gesture::Strike, Directional::Push, Element::Fire);
        assert!(pb.step(&fire, &only_r, 0.016).is_some(), "ThermalPhase accessible when r high");
        // but Stormstrike (needs l) is NOT accessible with only r high
        let mut pb2 = PlayerBending::new();
        let air = input(Stance::Open, Gesture::Strike, Directional::Push, Element::Air);
        assert!(pb2.step(&air, &only_r, 0.016).is_none(), "Stormstrike needs lightning field");
    }

    #[test]
    fn deterministic() {
        let f = fields_all(0.8);
        let seq = [
            input(Stance::Surge, Gesture::Strike, Directional::Push, Element::Air),
            input(Stance::Surge, Gesture::Strike, Directional::Push, Element::Air), // hold
            input(Stance::Flow, Gesture::Sustain, Directional::Pull, Element::Water),
            input(Stance::Open, Gesture::Echo, Directional::Sweep, Element::Fire),
        ];
        let run = || {
            let mut pb = PlayerBending::new();
            let mut outs = Vec::new();
            for s in &seq {
                outs.push(pb.step(s, &f, 0.016));
            }
            (pb, outs)
        };
        let (a, oa) = run();
        let (b, ob) = run();
        assert_eq!(a, b);
        assert_eq!(oa, ob);
    }

    #[test]
    fn no_panic_on_bad_dt_and_fields() {
        let mut pb = PlayerBending::new();
        let inp = input(Stance::Surge, Gesture::Strike, Directional::Push, Element::Air);
        let weird = Fields { h: f32::NAN, r: f32::INFINITY, w: -5.0, p: 1e9, l: f32::NAN, s: -0.0, b: 2.0 };
        for &dt in &[f32::NAN, -1.0, 0.0, 1e9, 0.016] {
            let r = pb.step(&inp, &weird, dt);
            if let Some(i) = r {
                assert!(i.power01 >= 0.0 && i.power01 <= 1.0);
            }
            assert!(pb.charge01 >= 0.0 && pb.charge01 <= 1.0);
        }
    }
}

// ── service_registry wiring (S2) ───────────────────────────────────
// Wires player_bending's OWN controller (PlayerBending), distinct from
// bending_runtime's BenderState — no shared state is double-registered.
// Neutral inputs (no gesture, empty fields) ⇒ step() gates out and
// returns None, which proves the wiring ticks panic-free.
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct PlayerBendingService {
    state: PlayerBending,
    input: BendInput,
    fields: crate::sim::affinity_access::Fields,
}
impl Service for PlayerBendingService {
    fn name(&self) -> &'static str {
        "player_bending"
    }
    fn tick(&mut self, ctx: &ServiceCtx) {
        let _ = self.state.step(&self.input, &self.fields, ctx.dt);
    }
}
#[distributed_slice(SERVICES)]
static PLAYER_BENDING_SERVICE: ServiceEntry = ServiceEntry {
    name: "player_bending",
    make: || {
        use crate::sim::gesture_resolve::{Directional, Element, Gesture, Stance};
        Box::new(PlayerBendingService {
            state: PlayerBending::new(),
            input: BendInput {
                stance: Stance::Open,
                gesture: Gesture::Sustain,
                dir: Directional::Push,
                element: Element::Earth,
                aim: [0.0, 0.0, 0.0],
            },
            // all-zero ambient fields ⇒ no discipline accessible (neutral)
            fields: crate::sim::affinity_access::Fields {
                h: 0.0, r: 0.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0,
            },
        })
    },
};
