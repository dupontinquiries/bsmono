//! Breathing and flame in bad air (`cave_biomes.docx`, "Deep CO₂ Basins",
//! Survival Hazards). Pure: the game feeds the O₂ fraction at the player's
//! head and at the flame (from `world::cave_gas`) and applies what comes back.
//!
//! The docx thresholds, as written:
//! * below 16% O₂: Weakness I (fatigue, slowed movement);
//! * below 12%: Weakness II, Slowness I, vision darkening at the edges;
//! * below 8%: unconsciousness (the player cannot move);
//! * below 5%: death within 30 seconds.
//!
//! "A torch entering a CO₂ zone sputters and dies within 2 seconds. This is
//! the ONLY reliable in-game warning." Fire cannot burn below 16%.

/// How the air at the player's head is going for them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Breath {
    Normal,
    /// < 16%: Weakness I.
    Weak,
    /// < 12%: Weakness II, Slowness I, vision darkening.
    Faint,
    /// < 8%: unconscious, cannot move.
    Unconscious,
    /// < 5%: dying (dead within 30 s).
    Dying,
}

pub fn breath_for(o2: f32) -> Breath {
    if o2 < 0.05 {
        Breath::Dying
    } else if o2 < 0.08 {
        Breath::Unconscious
    } else if o2 < 0.12 {
        Breath::Faint
    } else if o2 < 0.16 {
        Breath::Weak
    } else {
        Breath::Normal
    }
}

/// What a breath stage does to the player this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Effects {
    /// Movement speed multiplier (0 = cannot move).
    pub move_mult: f32,
    /// Multiplier on stamina recovery (fatigue).
    pub stamina_regen_mult: f32,
    /// Vitality (0..1) lost per second.
    pub vitality_drain: f32,
    /// 0..1 darkening at the screen edges.
    pub vignette: f32,
}

/// Vitality drain while dying: the whole bar in 30 seconds, as the docx says.
pub const DYING_DRAIN: f32 = 1.0 / 30.0;

pub fn effects(b: Breath) -> Effects {
    match b {
        Breath::Normal => Effects { move_mult: 1.0, stamina_regen_mult: 1.0, vitality_drain: 0.0, vignette: 0.0 },
        Breath::Weak => Effects { move_mult: 0.85, stamina_regen_mult: 0.4, vitality_drain: 0.0, vignette: 0.0 },
        Breath::Faint => Effects { move_mult: 0.6, stamina_regen_mult: 0.1, vitality_drain: 0.004, vignette: 0.45 },
        Breath::Unconscious => Effects { move_mult: 0.0, stamina_regen_mult: 0.0, vitality_drain: 0.012, vignette: 0.8 },
        Breath::Dying => Effects { move_mult: 0.0, stamina_regen_mult: 0.0, vitality_drain: DYING_DRAIN, vignette: 0.95 },
    }
}

/// Below this, fire cannot burn (docx: "cannot sustain combustion below 16%").
pub const FLAME_O2_MIN: f32 = 0.16;
/// Seconds a flame sputters in bad air before it dies.
pub const SPUTTER_S: f32 = 2.0;
/// Seconds in good air before an ember rekindles. (There is no flint and
/// steel yet; a dead torch comes back by itself once it is carried out.)
pub const REKINDLE_S: f32 = 3.0;
/// Air a dead flame needs before it can rekindle.
pub const REKINDLE_O2: f32 = 0.18;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FlameState {
    Lit,
    /// In bad air for `t` seconds.
    Sputtering { t: f32 },
    /// Out; `t` seconds in good air so far.
    Out { t: f32 },
}

/// A carried flame (a torch in hand): the canary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flame {
    pub state: FlameState,
}

impl Default for Flame {
    fn default() -> Self {
        Flame { state: FlameState::Lit }
    }
}

/// What changed in one tick, for the log line and a sound cue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlameEvent {
    None,
    StartedSputtering,
    Recovered,
    Died,
    Rekindled,
}

impl Flame {
    /// Advance by `dt` seconds with `o2` at the flame.
    pub fn tick(&mut self, o2: f32, dt: f32) -> FlameEvent {
        let dt = dt.max(0.0);
        match self.state {
            FlameState::Lit => {
                if o2 < FLAME_O2_MIN {
                    self.state = FlameState::Sputtering { t: 0.0 };
                    return FlameEvent::StartedSputtering;
                }
            }
            FlameState::Sputtering { t } => {
                if o2 >= FLAME_O2_MIN {
                    self.state = FlameState::Lit;
                    return FlameEvent::Recovered;
                }
                let t = t + dt;
                if t >= SPUTTER_S {
                    self.state = FlameState::Out { t: 0.0 };
                    return FlameEvent::Died;
                }
                self.state = FlameState::Sputtering { t };
            }
            FlameState::Out { t } => {
                let t = if o2 >= REKINDLE_O2 { t + dt } else { 0.0 };
                if t >= REKINDLE_S {
                    self.state = FlameState::Lit;
                    return FlameEvent::Rekindled;
                }
                self.state = FlameState::Out { t };
            }
        }
        FlameEvent::None
    }

    /// Light output 0..1 at `time` seconds: a steady flicker when lit, a
    /// failing stutter while sputtering, nothing when out.
    pub fn intensity(&self, time: f32) -> f32 {
        let flicker = 0.9 + 0.06 * (time * 13.0).sin() + 0.04 * (time * 29.0 + 1.3).sin();
        match self.state {
            FlameState::Lit => flicker,
            FlameState::Sputtering { t } => {
                let fade = 1.0 - (t / SPUTTER_S).clamp(0.0, 1.0);
                let stutter = if (time * 11.0).sin() > 0.2 { 1.0 } else { 0.25 };
                (flicker * fade * stutter).max(0.0)
            }
            FlameState::Out { .. } => 0.0,
        }
    }

    pub fn is_lit(&self) -> bool {
        !matches!(self.state, FlameState::Out { .. })
    }
}

/// Heat stress from the air temperature (`cave_biomes.docx`, Geothermal
/// Deep: "Heat II at 60°C, Heat III at 75°C+"; Steam Caverns: "At 40°C:
/// Heat I (fatigue, reduced sprint)").
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Heat {
    None,
    /// >= 40 °C: fatigue, reduced sprint.
    I,
    /// >= 60 °C.
    II,
    /// >= 75 °C: sprint disabled, vision impaired by heat haze.
    III,
}

pub fn heat_for(air_c: f32) -> Heat {
    if air_c >= 75.0 {
        Heat::III
    } else if air_c >= 60.0 {
        Heat::II
    } else if air_c >= 40.0 {
        Heat::I
    } else {
        Heat::None
    }
}

/// What a heat stage does. No vitality term on purpose: the body-temperature
/// model (sim::survival_exposure) already drains vitality in hot air, and the
/// docx's per-stage HP drain on top of it would count the same heat twice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeatEffects {
    pub move_mult: f32,
    /// Sprint speed multiplier applies only when this is true.
    pub sprint: bool,
    pub stamina_regen_mult: f32,
    /// 0..1 edge darkening (the haze).
    pub vignette: f32,
}

pub fn heat_effects(h: Heat) -> HeatEffects {
    match h {
        Heat::None => HeatEffects { move_mult: 1.0, sprint: true, stamina_regen_mult: 1.0, vignette: 0.0 },
        Heat::I => HeatEffects { move_mult: 1.0, sprint: true, stamina_regen_mult: 0.5, vignette: 0.0 },
        Heat::II => HeatEffects { move_mult: 0.9, sprint: true, stamina_regen_mult: 0.25, vignette: 0.2 },
        Heat::III => HeatEffects { move_mult: 0.8, sprint: false, stamina_regen_mult: 0.0, vignette: 0.45 },
    }
}

/// Share of the world's sound the gas swallows at full concentration (the
/// docx's `sound_absorption`).
pub const SOUND_ABSORPTION: f32 = 0.85;

/// How muffled the world is (0 = not at all) for the O₂ at the listener's
/// head: the docx's "dead silence" below the surface.
pub fn hush_for(o2: f32) -> f32 {
    SOUND_ABSORPTION * crate::world::cave_gas::density_for_o2(o2)
}

/// The bending field caps in a CO₂ basin (docx property signature: W 0.02,
/// H 0.55, L 0.02, B 0.05, R 0.15), blended in by the gas density at `o2`.
/// Below [`FLAME_O2_MIN`] fire has nothing at all: "Fire bending is
/// completely blocked at 15% and below." P and S are left alone (earth still
/// works; pressure is modelled elsewhere).
pub fn gas_fields(f: &mut crate::sim::affinity_access::Fields, o2: f32) {
    let d = crate::world::cave_gas::density_for_o2(o2);
    if d <= 0.0 {
        return;
    }
    let cap = |v: f32, c: f32| v + (v.min(c) - v) * d;
    f.w = cap(f.w, 0.02);
    f.h = cap(f.h, 0.55);
    f.l = cap(f.l, 0.02);
    f.b = cap(f.b, 0.05);
    f.r = if o2 < FLAME_O2_MIN { 0.0 } else { cap(f.r, 0.15) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_follow_the_docx_thresholds() {
        assert_eq!(breath_for(0.209), Breath::Normal);
        assert_eq!(breath_for(0.16), Breath::Normal);
        assert_eq!(breath_for(0.159), Breath::Weak);
        assert_eq!(breath_for(0.119), Breath::Faint);
        assert_eq!(breath_for(0.079), Breath::Unconscious);
        assert_eq!(breath_for(0.049), Breath::Dying);
        // Worse air is never a better stage.
        let mut prev = Breath::Normal;
        for k in (0..=210).rev() {
            let b = breath_for(k as f32 / 1000.0);
            assert!(b >= prev, "{k}");
            prev = b;
        }
    }

    #[test]
    fn effects_get_worse_with_each_stage_and_dying_kills_in_thirty_seconds() {
        let order = [Breath::Normal, Breath::Weak, Breath::Faint, Breath::Unconscious, Breath::Dying];
        for w in order.windows(2) {
            let (a, b) = (effects(w[0]), effects(w[1]));
            assert!(b.move_mult <= a.move_mult && b.vitality_drain >= a.vitality_drain && b.vignette >= a.vignette);
        }
        assert_eq!(effects(Breath::Unconscious).move_mult, 0.0, "cannot move");
        assert!((effects(Breath::Dying).vitality_drain * 30.0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_torch_dies_two_seconds_into_bad_air_and_not_before() {
        let mut f = Flame::default();
        assert_eq!(f.tick(0.20, 0.1), FlameEvent::None);
        assert_eq!(f.tick(0.10, 0.1), FlameEvent::StartedSputtering);
        let mut t = 0.0;
        let mut died_at = None;
        while t < 3.0 {
            t += 0.1;
            if f.tick(0.10, 0.1) == FlameEvent::Died {
                died_at = Some(t);
                break;
            }
            assert!(f.intensity(t) <= 1.0);
        }
        let d = died_at.expect("the flame died");
        assert!((1.9..=2.15).contains(&d), "died after {d} s");
        assert_eq!(f.intensity(5.0), 0.0);
    }

    #[test]
    fn stepping_back_out_saves_a_sputtering_flame_and_a_dead_one_rekindles_in_good_air() {
        let mut f = Flame::default();
        f.tick(0.10, 0.1);
        f.tick(0.10, 1.0);
        assert_eq!(f.tick(0.20, 0.1), FlameEvent::Recovered);
        assert!(f.is_lit());
        // Kill it, then carry it out.
        f.tick(0.05, 0.1);
        for _ in 0..30 {
            f.tick(0.05, 0.1);
        }
        assert!(!f.is_lit());
        // Stale air (between 16 and 18%) does not rekindle it.
        for _ in 0..100 {
            f.tick(0.17, 0.1);
        }
        assert!(!f.is_lit());
        let mut ev = FlameEvent::None;
        for _ in 0..40 {
            ev = f.tick(0.20, 0.1);
            if ev == FlameEvent::Rekindled {
                break;
            }
        }
        assert_eq!(ev, FlameEvent::Rekindled);
    }

    #[test]
    fn the_basin_is_silent_and_the_open_cave_is_not() {
        assert_eq!(hush_for(crate::world::cave_gas::O2_AIR), 0.0);
        assert!((hush_for(crate::world::cave_gas::o2_for_density(1.0)) - SOUND_ABSORPTION).abs() < 1e-5);
        let half = hush_for(crate::world::cave_gas::o2_for_density(0.5));
        assert!((0.3..0.55).contains(&half), "{half}");
    }

    #[test]
    fn fire_and_wind_are_gone_in_the_gas_and_untouched_outside_it() {
        use crate::sim::affinity_access::{access, Fields, THRESHOLDS};
        use crate::world::cave_gas::{o2_for_density, O2_AIR};
        let rich = Fields { h: 0.9, r: 0.9, w: 0.9, p: 0.8, l: 0.9, s: 0.7, b: 0.9 };
        let mut f = rich;
        gas_fields(&mut f, O2_AIR);
        assert_eq!(f, rich, "clean air changes nothing");
        let mut f = rich;
        gas_fields(&mut f, o2_for_density(1.0));
        assert_eq!(f.r, 0.0, "no fire");
        assert!(f.w <= 0.02 + 1e-6 && f.l <= 0.02 + 1e-6 && f.b <= 0.05 + 1e-6 && f.h <= 0.55 + 1e-6);
        assert_eq!((f.p, f.s), (rich.p, rich.s), "earth and pressure untouched");
        // The access gate agrees: fire (channel 1) and wind (2) are locked,
        // earth (5) is not.
        let a = access(&f);
        assert!(f.r < THRESHOLDS[1] && a.disciplines & (1 << 1) == 0 && a.disciplines & (1 << 2) == 0);
        assert!(a.disciplines & (1 << 5) != 0, "earth still answers");
        // Stale but burnable air (above 16%) only weakens fire.
        let mut f = rich;
        gas_fields(&mut f, 0.18);
        assert!(f.r > 0.15 && f.r < rich.r, "{}", f.r);
    }

    #[test]
    fn heat_stages_follow_the_docx_and_only_worsen() {
        assert_eq!(heat_for(39.9), Heat::None);
        assert_eq!(heat_for(40.0), Heat::I);
        assert_eq!(heat_for(59.9), Heat::I);
        assert_eq!(heat_for(60.0), Heat::II);
        assert_eq!(heat_for(74.9), Heat::II);
        assert_eq!(heat_for(75.0), Heat::III);
        assert!(!heat_effects(Heat::III).sprint, "Heat III: no sprint");
        assert!(heat_effects(Heat::II).sprint);
        assert!(heat_effects(Heat::I).stamina_regen_mult < 1.0, "Heat I: fatigue");
        let order = [Heat::None, Heat::I, Heat::II, Heat::III];
        for w in order.windows(2) {
            let (a, b) = (heat_effects(w[0]), heat_effects(w[1]));
            assert!(b.move_mult <= a.move_mult && b.stamina_regen_mult <= a.stamina_regen_mult && b.vignette >= a.vignette);
        }
        let mut prev = Heat::None;
        for c in 0..120 {
            let h = heat_for(c as f32);
            assert!(h >= prev, "{c}");
            prev = h;
        }
    }

    #[test]
    fn a_sputtering_flame_is_dimmer_than_a_lit_one() {
        let lit = Flame::default();
        let sp = Flame { state: FlameState::Sputtering { t: 1.5 } };
        let (mut a, mut b) = (0.0, 0.0);
        for k in 0..100 {
            let t = k as f32 * 0.05;
            a += lit.intensity(t);
            b += sp.intensity(t);
        }
        assert!(b < a * 0.5, "sputtering {b} vs lit {a}");
    }
}
