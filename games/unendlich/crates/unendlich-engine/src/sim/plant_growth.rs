//! `sim::plant_growth` — advance a plant's growth one tick, with genome variation.
//!
//! The middle of the plant loop: R34 `germination` decides *if/when* a seed
//! sprouts, R38 `flora_harvest` yields from a grown plant, R35
//! `canopy_field_shift` edits the field once grown — this is the part between,
//! advancing `growth01` over time under the local field.
//!
//! Each tick `growth01 += dt · rate`, where `rate` rises with warmth (`r`),
//! moisture (`h`), light (`l`) and the plant's `vigor`, and goes **negative**
//! (the plant wilts) when conditions collapse — specifically when the survival
//! driver `(h + r)/2` falls below [`WILT_POINT`]. `stage` is recomputed from
//! `growth01` at fixed thresholds, emitting [`GrowthEvent::Sprouted`] on first
//! emergence, [`GrowthEvent::StageAdvanced`] on the middle thresholds, and
//! [`GrowthEvent::Matured`] once at the top (`growth01 = 1`). A plant that was
//! alive and wilts past 0 emits [`GrowthEvent::Wilted`] (and resets to a dead
//! seed).
//!
//! **Variation:** `vigor` (genome) scales rate and an `element` bias nudges it,
//! so two seeds with different genomes in the *same* field diverge over time.
//!
//! `s` (storm/particulate) and `b` (biome richness) are carried on [`Fields`]
//! for the canopy/field consumers but do not drive growth rate here (the spec
//! ties rate to warmth/moisture/light/vigor).
//!
//! Pure, deterministic, clamped, panic-free. No `crate::render`, no `ash`/`vk`,
//! no IO, no clock, no RNG, no `.unwrap()`/`.expect()` in non-test code; compiles
//! clean under `cargo check --release` (no `cfg(test)`/`cfg(debug_assertions)`
//! gated helpers).

/// A plant being grown. `growth01` 0..1; `stage` 0..=[`FINAL_STAGE`]; `element`
/// 0..5 (read mod 6); `vigor` 0..1 genome trait (clamped).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlantState {
    pub growth01: f32,
    pub stage: u8,
    pub element: u8,
    pub vigor: f32,
}

/// Local field at the plant. Each 0..1 (clamped). `h`=moisture, `r`=warmth,
/// `l`=light drive growth; `s`,`b` are reserved for canopy/field consumers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fields {
    pub h: f32,
    pub r: f32,
    pub l: f32,
    pub s: f32,
    pub b: f32,
}

/// What happened to the plant on this tick.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum GrowthEvent {
    None,
    Sprouted,
    StageAdvanced,
    Matured,
    Wilted,
}

/// One advanced tick: the new state plus the event (if any) it triggered.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GrowthStep {
    pub state: PlantState,
    pub event: GrowthEvent,
}

/// The mature (final) stage index. Stages: 0 seed, 1 sprout, 2 juvenile,
/// 3 adult, 4 mature.
pub const FINAL_STAGE: u8 = 4;
/// Growth thresholds at which `stage` increments (to 1, 2, 3, 4 respectively).
const STAGE_THRESHOLDS: [f32; 4] = [0.05, 0.35, 0.65, 1.0];
/// Survival driver `(h+r)/2` below this ⇒ negative rate (wilting).
pub const WILT_POINT: f32 = 0.25;
/// Base rate scalar.
const GROWTH_K: f32 = 0.6;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Stage implied by a growth value (count of thresholds reached). Unconditional
/// (release-safe).
pub fn stage_for(growth01: f32) -> u8 {
    let g = clamp01(growth01);
    let mut st = 0u8;
    let mut i = 0;
    while i < STAGE_THRESHOLDS.len() {
        if g >= STAGE_THRESHOLDS[i] {
            st += 1;
        }
        i += 1;
    }
    st
}

/// Instantaneous growth rate (signed). Positive in good fields, negative when
/// the survival driver drops below [`WILT_POINT`]. Rises with `h`,`r`,`l`,`vigor`
/// and is nudged by `element` so genomes diverge.
fn growth_rate(f: &Fields, element: u8, vigor: f32) -> f32 {
    let h = clamp01(f.h);
    let r = clamp01(f.r);
    let l = clamp01(f.l);
    let v = clamp01(vigor);

    let mw = 0.5 * (h + r); // survival driver
    let surplus = mw - WILT_POINT; // signed
    // light accelerates growth only while surviving; below WILT_POINT it decays.
    let drive = if surplus >= 0.0 {
        surplus * (0.5 + 0.5 * l)
    } else {
        surplus
    };
    let vigor_factor = 0.5 + 1.5 * v; // 0.5..2.0 — higher vigor grows faster
    let element_factor = 0.90 + 0.04 * (element % 6) as f32; // 0.90..1.10, distinct
    GROWTH_K * vigor_factor * element_factor * drive
}

/// Advance the plant one tick of length `dt`. See module docs.
pub fn advance(s: PlantState, f: &Fields, dt: f32) -> GrowthStep {
    // Non-negative, finite dt (NaN/negative ⇒ no time passes).
    let dt = if dt.is_nan() || dt < 0.0 { 0.0 } else { dt };
    let old_growth = clamp01(s.growth01);
    let old_stage = s.stage;

    let rate = growth_rate(f, s.element, s.vigor);
    let pre = old_growth + dt * rate; // may be < 0 (wilting) or > 1
    let wilted = pre < 0.0 && old_growth > 0.0;

    if wilted {
        let state = PlantState { growth01: 0.0, stage: 0, element: s.element, vigor: s.vigor };
        return GrowthStep { state, event: GrowthEvent::Wilted };
    }

    let new_growth = clamp01(pre);
    let new_stage = stage_for(new_growth);
    let event = if new_stage > old_stage {
        if new_stage >= FINAL_STAGE {
            GrowthEvent::Matured
        } else if old_stage == 0 {
            GrowthEvent::Sprouted
        } else {
            GrowthEvent::StageAdvanced
        }
    } else {
        GrowthEvent::None
    };

    let state = PlantState { growth01: new_growth, stage: new_stage, element: s.element, vigor: s.vigor };
    GrowthStep { state, event }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn plant(growth: f32, stage: u8, element: u8, vigor: f32) -> PlantState {
        PlantState { growth01: growth, stage, element, vigor }
    }
    fn fields(h: f32, r: f32, l: f32) -> Fields {
        Fields { h, r, l, s: 0.0, b: 0.0 }
    }
    /// Run up to `max` ticks, returning (final state, events seen).
    fn run(mut st: PlantState, f: &Fields, dt: f32, max: usize) -> (PlantState, Vec<GrowthEvent>) {
        let mut events = Vec::new();
        for _ in 0..max {
            let step = advance(st, f, dt);
            st = step.state;
            events.push(step.event);
        }
        (st, events)
    }
    fn count(events: &[GrowthEvent], e: GrowthEvent) -> usize {
        events.iter().filter(|&&x| x == e).count()
    }

    #[test]
    fn good_fields_grow_and_advance_through_stages() {
        let seed = plant(0.0, 0, 0, 1.0);
        let good = fields(1.0, 1.0, 1.0);
        let (st, ev) = run(seed, &good, 0.05, 400);
        assert_eq!(st.growth01, 1.0, "should reach full growth");
        assert_eq!(st.stage, FINAL_STAGE);
        assert_eq!(count(&ev, GrowthEvent::Sprouted), 1, "sprouts exactly once");
        assert_eq!(count(&ev, GrowthEvent::Matured), 1, "matures exactly once at the top");
        assert!(count(&ev, GrowthEvent::StageAdvanced) >= 1, "advances middle stages");
        assert_eq!(count(&ev, GrowthEvent::Wilted), 0, "good fields never wilt");
    }

    #[test]
    fn growth_is_monotone_increasing_in_good_fields() {
        let mut st = plant(0.0, 0, 2, 0.7);
        let good = fields(0.9, 0.9, 0.8);
        let mut prev = st.growth01;
        for _ in 0..300 {
            st = advance(st, &good, 0.04).state;
            assert!(st.growth01 >= prev - 1e-7, "growth should not decrease in good fields");
            prev = st.growth01;
        }
        assert_eq!(st.growth01, 1.0);
    }

    #[test]
    fn poor_fields_wilt_a_grown_plant() {
        let grown = plant(0.5, 2, 3, 0.8);
        let poor = fields(0.0, 0.0, 0.0);
        let (st, ev) = run(grown, &poor, 0.05, 400);
        assert!(count(&ev, GrowthEvent::Wilted) >= 1, "a grown plant in poor fields must wilt");
        assert_eq!(st.growth01, 0.0);
        assert_eq!(st.stage, 0);
    }

    #[test]
    fn dormant_seed_in_poor_fields_stalls_without_wilting() {
        let seed = plant(0.0, 0, 0, 0.5);
        let poor = fields(0.0, 0.0, 0.0);
        let (st, ev) = run(seed, &poor, 0.05, 200);
        assert_eq!(st.growth01, 0.0, "stays dormant");
        assert_eq!(count(&ev, GrowthEvent::Wilted), 0, "a never-grown seed does not 'wilt'");
        assert!(ev.iter().all(|&e| e == GrowthEvent::None));
    }

    #[test]
    fn higher_vigor_grows_faster() {
        let f = fields(0.9, 0.9, 0.9);
        let lo = advance(plant(0.1, 0, 0, 0.2), &f, 0.1).state.growth01;
        let hi = advance(plant(0.1, 0, 0, 0.9), &f, 0.1).state.growth01;
        assert!(hi > lo, "higher vigor should grow faster: {} vs {}", hi, lo);
    }

    #[test]
    fn genomes_diverge_under_identical_fields() {
        let f = fields(0.8, 0.8, 0.8);
        // different vigor
        let a = run(plant(0.1, 0, 0, 0.3), &f, 0.05, 5).0.growth01;
        let b = run(plant(0.1, 0, 0, 0.9), &f, 0.05, 5).0.growth01;
        assert!((a - b).abs() > 1e-4, "different vigor must diverge: {} vs {}", a, b);
        // different element, same vigor
        let c = run(plant(0.1, 0, 0, 0.6), &f, 0.05, 5).0.growth01;
        let d = run(plant(0.1, 0, 5, 0.6), &f, 0.05, 5).0.growth01;
        assert!((c - d).abs() > 1e-4, "different element must diverge: {} vs {}", c, d);
    }

    #[test]
    fn matured_fires_once_then_silent() {
        let seed = plant(0.0, 0, 1, 1.0);
        let good = fields(1.0, 1.0, 1.0);
        // grow to maturity, then keep ticking — Matured must not refire.
        let (st, ev) = run(seed, &good, 0.05, 200);
        assert_eq!(st.stage, FINAL_STAGE);
        assert_eq!(count(&ev, GrowthEvent::Matured), 1);
        // many more ticks at full growth → only None
        let (_, ev2) = run(st, &good, 0.05, 50);
        assert!(ev2.iter().all(|&e| e == GrowthEvent::None), "no events once mature & saturated");
    }

    #[test]
    fn events_only_fire_at_transitions() {
        // Within a stage, no event; the tick that crosses a threshold fires one.
        let good = fields(1.0, 1.0, 1.0);
        let mut st = plant(0.0, 0, 0, 1.0);
        let mut last_stage = st.stage;
        for _ in 0..400 {
            let step = advance(st, &good, 0.05);
            if step.event == GrowthEvent::None {
                assert_eq!(step.state.stage, last_stage, "None implies no stage change");
            } else if step.event != GrowthEvent::Wilted {
                assert!(step.state.stage > last_stage, "advance event implies stage increase");
            }
            last_stage = step.state.stage;
            st = step.state;
        }
    }

    #[test]
    fn growth_always_clamped() {
        let mut seed = 0x6201_0000u32;
        for _ in 0..20000 {
            let st = plant(
                (lcg(&mut seed) % 2001) as f32 / 1000.0 - 0.5, // incl out of range
                (lcg(&mut seed) % 6) as u8,
                (lcg(&mut seed) % 9) as u8,
                (lcg(&mut seed) % 2001) as f32 / 1000.0 - 0.5,
            );
            let f = Fields {
                h: (lcg(&mut seed) % 2001) as f32 / 1000.0 - 0.5,
                r: (lcg(&mut seed) % 2001) as f32 / 1000.0 - 0.5,
                l: (lcg(&mut seed) % 2001) as f32 / 1000.0 - 0.5,
                s: 0.0,
                b: 0.0,
            };
            let dt = (lcg(&mut seed) % 1000) as f32 / 100.0; // 0..10
            let out = advance(st, &f, dt).state;
            assert!(out.growth01 >= 0.0 && out.growth01 <= 1.0, "growth {}", out.growth01);
            assert!(out.stage <= FINAL_STAGE);
        }
    }

    #[test]
    fn nan_inputs_do_not_panic() {
        let st = plant(f32::NAN, 0, 200, f32::NAN);
        let f = Fields { h: f32::NAN, r: f32::INFINITY, l: -5.0, s: f32::NAN, b: f32::NAN };
        let out = advance(st, &f, f32::NAN);
        assert!(out.state.growth01 >= 0.0 && out.state.growth01 <= 1.0);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x6201_0001u32;
        for _ in 0..5000 {
            let st = plant(
                (lcg(&mut seed) % 1001) as f32 / 1000.0,
                (lcg(&mut seed) % 5) as u8,
                (lcg(&mut seed) % 6) as u8,
                (lcg(&mut seed) % 1001) as f32 / 1000.0,
            );
            let f = fields(
                (lcg(&mut seed) % 1001) as f32 / 1000.0,
                (lcg(&mut seed) % 1001) as f32 / 1000.0,
                (lcg(&mut seed) % 1001) as f32 / 1000.0,
            );
            let dt = (lcg(&mut seed) % 200) as f32 / 1000.0;
            assert_eq!(advance(st, &f, dt), advance(st, &f, dt));
        }
    }
}
