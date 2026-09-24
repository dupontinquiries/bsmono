//! `sim::dragon_ears` — what a moving player sounds like, and how sharply a
//! dragon hears it. Built for the thing that gives an armoured player away:
//! **metal clanking**.
//!
//! ## Emission ([`emit`])
//! Every step makes a footfall (low, dull, ~300 Hz). Worn armour adds a
//! clank on each footfall and a louder one on landing: plate and scale bang
//! (bright, ~3 kHz), chain jingles higher, leather creaks, cloth rustles.
//! Sprinting is louder, sneaking much quieter, standing still is silent.
//!
//! ## Hearing ([`hear`])
//! The sound reaching the dragon loses 6 dB per doubling of distance, more
//! for every block of terrain in the way, and **air absorbs high frequencies
//! faster** — a clank's shimmer dies before its thud does. What it is heard
//! *against* is the ambient: wind roars at low frequencies and rain is
//! broadband, so wind masks footfalls far more than it masks clanks. A dragon's
//! ear acuity (genome) lowers its threshold; attention (hunting, alert) lowers
//! it a little more. A sharp transient localises better than a steady sound.
//!
//! Ear acuity is this module's genome mapping (the genome doc has no hearing
//! gene): a tall crest channels sound; a light frame lives by its ears.
//!
//! Levels are dB SPL at 1 m. Pure.

use crate::sim::dragon_archetype::Genome;
use std::f32::consts::PI;

/// What a piece is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    Cloth,
    Leather,
    Chain,
    Scale,
    Plate,
}

impl Material {
    /// Clank of one piece per footfall at walking pace (dB at 1 m), and its
    /// spectral centroid (Hz).
    pub const fn clank(self) -> (f32, f32) {
        match self {
            Material::Plate => (64.0, 3200.0),
            Material::Scale => (60.0, 2800.0),
            Material::Chain => (58.0, 5000.0),
            Material::Leather => (42.0, 900.0),
            Material::Cloth => (30.0, 500.0),
        }
    }
    pub fn metal(self) -> bool {
        matches!(self, Material::Plate | Material::Scale | Material::Chain)
    }
    pub fn parse(s: &str) -> Option<Material> {
        match s.trim().to_ascii_lowercase().as_str() {
            "cloth" => Some(Material::Cloth),
            "leather" => Some(Material::Leather),
            "chain" | "mail" | "chainmail" => Some(Material::Chain),
            "scale" => Some(Material::Scale),
            "plate" | "iron" | "steel" => Some(Material::Plate),
            _ => None,
        }
    }
}

/// Worn pieces by slot, `armor_set::Slot` order: head, chest, arms, hands,
/// legs, feet.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Worn(pub [Option<Material>; 6]);

/// How much each slot rattles per step: chest and legs move most, sabatons
/// strike the ground.
const SLOT_WEIGHT: [f32; 6] = [0.35, 1.0, 0.6, 0.35, 0.8, 0.7];

impl Worn {
    pub const NONE: Worn = Worn([None; 6]);
    pub fn full(m: Material) -> Worn {
        Worn([Some(m); 6])
    }
    /// `"plate"` (all six) or `"head=plate,chest=chain,feet=leather"`.
    pub fn parse(s: &str) -> Worn {
        let s = s.trim();
        if let Some(m) = Material::parse(s) {
            return Worn::full(m);
        }
        let mut w = Worn::NONE;
        for part in s.split(',') {
            if let Some((slot, m)) = part.split_once('=') {
                let i = match slot.trim() {
                    "head" => 0,
                    "chest" => 1,
                    "arms" => 2,
                    "hands" => 3,
                    "legs" => 4,
                    "feet" => 5,
                    _ => continue,
                };
                w.0[i] = Material::parse(m);
            }
        }
        w
    }
    pub fn any_metal(&self) -> bool {
        self.0.iter().flatten().any(|m| m.metal())
    }
}

/// How the player is moving this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gait {
    /// Horizontal speed (m/s).
    pub speed: f32,
    pub crouched: bool,
    /// Vertical speed at touchdown this tick (m/s, 0 if it did not land).
    pub landing: f32,
}

/// One tick's worth of noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emission {
    /// Footfall level (dB at 1 m), `-inf` when silent.
    pub foot_db: f32,
    /// Armour clank level (dB at 1 m), `-inf` with no armour or no movement.
    pub clank_db: f32,
    /// Clank spectral centroid (Hz).
    pub clank_hz: f32,
    pub metal: bool,
}

pub const SILENT: f32 = f32::NEG_INFINITY;

fn db_sum(levels: impl Iterator<Item = f32>) -> f32 {
    let p: f32 = levels.filter(|l| l.is_finite()).map(|l| 10f32.powf(l / 10.0)).sum();
    if p > 0.0 { 10.0 * p.log10() } else { SILENT }
}

/// Noise a player makes this tick.
pub fn emit(worn: &Worn, g: &Gait) -> Emission {
    let moving = g.speed.is_finite() && g.speed > 0.3;
    let landing = if g.landing.is_finite() { g.landing.max(0.0) } else { 0.0 };
    if !moving && landing < 1.0 {
        return Emission { foot_db: SILENT, clank_db: SILENT, clank_hz: 0.0, metal: false };
    }
    // Pace: +6 dB per doubling over a 1.4 m/s walk; sneaking -14 dB.
    let pace = if moving { 20.0 * (g.speed / 1.4).max(0.25).log10() } else { 0.0 };
    let sneak = if g.crouched { -14.0 } else { 0.0 };
    let land = 20.0 * (1.0 + landing / 3.0).log10();
    let foot_db = 45.0 + pace + sneak + land;
    let pieces: Vec<(f32, f32, bool)> = worn
        .0
        .iter()
        .zip(SLOT_WEIGHT)
        .filter_map(|(m, w)| m.map(|m| {
            let (db, hz) = m.clank();
            (db + 10.0 * w.log10(), hz, m.metal())
        }))
        .collect();
    let clank_db = db_sum(pieces.iter().map(|p| p.0)) + pace + sneak + land;
    let clank_hz = if pieces.is_empty() {
        0.0
    } else {
        let wsum: f32 = pieces.iter().map(|p| 10f32.powf(p.0 / 10.0)).sum();
        pieces.iter().map(|p| p.1 * 10f32.powf(p.0 / 10.0)).sum::<f32>() / wsum
    };
    Emission { foot_db, clank_db: if pieces.is_empty() { SILENT } else { clank_db }, clank_hz, metal: pieces.iter().any(|p| p.2) }
}

/// Air absorption (dB per metre) at `hz`: ~0.005 at 1 kHz rising to ~0.1 at
/// 8 kHz (ISO 9613-ish, mild day).
pub fn air_absorption_db_per_m(hz: f32) -> f32 {
    let k = (hz.max(50.0) / 1000.0).powf(1.5);
    0.005 * k
}

/// The ambient a sound must beat.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ambient {
    /// Wind speed at the dragon (m/s).
    pub wind: f32,
    /// 0..1 rain.
    pub rain: f32,
}

/// Masking level (dB) at frequency `hz`: a quiet floor, wind noise that
/// falls off with frequency, rain across the band.
pub fn masking_db(a: &Ambient, hz: f32) -> f32 {
    let floor = 18.0;
    let w = a.wind.max(0.0);
    let wind = if w > 0.1 { 22.0 + 30.0 * (w / 8.0).min(2.0) - 12.0 * (hz.max(50.0) / 250.0).log2().max(0.0) } else { SILENT };
    let rain = if a.rain > 0.0 { 20.0 + 25.0 * a.rain.min(1.0) } else { SILENT };
    db_sum([floor, wind, rain].into_iter())
}

/// 0..1. Tall crests channel sound; a light frame relies on its ears.
pub fn ear_acuity(g: &Genome) -> f32 {
    (0.55 + 0.3 * g.crest.clamp(0.0, 1.0) + 0.15 * (1.0 - g.bulk.clamp(0.0, 1.0))).clamp(0.0, 1.0)
}

/// What the dragon makes of one sound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heard {
    /// dB above its threshold (negative = inaudible).
    pub margin_db: f32,
    pub audible: bool,
    /// Heard it as metal (clank audible, not just footsteps).
    pub metal: bool,
    /// World yaw toward the sound.
    pub bearing: f32,
    /// ± uncertainty on that bearing (rad).
    pub bearing_err: f32,
}

/// Diffraction loss (dB) over a terrain crest, Maekawa's approximation:
/// `10·log10(3 + 20N)` with Fresnel number `N = 2δ/λ`, capped at 25 dB.
/// `δ` is the detour (m) from `land_echo::barrier_path_diff`. High
/// frequencies bend round a hill worse: behind a ridge the clank's ring is
/// lost before the footfall's thud.
pub fn diffraction_db(detour_m: f32, hz: f32) -> f32 {
    if !(detour_m > 0.0) {
        return 0.0;
    }
    let n = 2.0 * detour_m * hz.max(20.0) / 343.0;
    (10.0 * (3.0 + 20.0 * n).log10()).min(25.0)
}

/// Level at the listener after distance, terrain and air.
pub fn arriving_db(level_1m: f32, hz: f32, dist: f32, detour_m: f32) -> f32 {
    if !level_1m.is_finite() {
        return SILENT;
    }
    let d = if dist.is_finite() { dist.max(1.0) } else { return SILENT };
    level_1m - 20.0 * d.log10() - air_absorption_db_per_m(hz) * d - diffraction_db(detour_m, hz)
}

/// Hear one tick's emission. `attention01`: 0 idle, 1 hunting/alert.
pub fn hear(
    e: &Emission,
    src: [f32; 3],
    ear: [f32; 3],
    detour_m: f32,
    amb: &Ambient,
    acuity: f32,
    attention01: f32,
) -> Heard {
    let d = ((src[0] - ear[0]).powi(2) + (src[1] - ear[1]).powi(2) + (src[2] - ear[2]).powi(2)).sqrt();
    let bearing = (src[2] - ear[2]).atan2(src[0] - ear[0]);
    // Sharp ears pull a signal out from under the masking.
    let bonus = 14.0 * acuity.clamp(0.0, 1.0) + 6.0 * attention01.clamp(0.0, 1.0);
    let foot = arriving_db(e.foot_db, 300.0, d, detour_m) - (masking_db(amb, 300.0) - bonus);
    let clank = if e.clank_db.is_finite() {
        arriving_db(e.clank_db, e.clank_hz, d, detour_m) - (masking_db(amb, e.clank_hz) - bonus)
    } else {
        SILENT
    };
    let metal = e.metal && clank > 0.0;
    let margin = foot.max(clank);
    let margin = if margin.is_finite() { margin } else { -120.0 };
    // A transient localises well; a faint one barely.
    let sharp = if metal { 0.6 } else { 1.0 };
    let bearing_err = (0.9 * sharp / (1.0 + margin.max(0.0) / 6.0)).clamp(0.03, PI);
    Heard { margin_db: margin, audible: margin > 0.0, metal, bearing, bearing_err }
}

/// Where it last heard you, kept after the noise stops. A dragon that heard
/// a clank goes to investigate; how long it keeps the place in mind is its
/// mind's business (cunning). Seen in a soak: without this, a player who
/// stood still vanished from a dragon's world the instant they fell silent.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct EarMemory {
    /// Estimated position of the last heard sound, its age (s), and whether
    /// it was metal.
    pub last: Option<([f32; 3], f32, bool)>,
}

/// Seconds a dragon keeps a heard position in mind.
pub fn ear_memory_s(cunning: f32) -> f32 {
    5.0 + 20.0 * cunning.clamp(0.0, 1.0)
}

impl EarMemory {
    /// Feed this tick's hearing: `est_pos` is where it judged the sound to be
    /// (only read when audible). Returns the remembered place to investigate,
    /// how sure it still is (0..1, fading with age), and whether it was metal.
    /// Forgets on arrival (`self_pos` within `arrive_m`) or when too old.
    pub fn update(
        &mut self,
        heard: &Heard,
        est_pos: [f32; 3],
        self_pos: [f32; 3],
        cunning: f32,
        arrive_m: f32,
        dt: f32,
    ) -> Option<([f32; 3], f32, bool)> {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        if heard.audible && est_pos.iter().all(|v| v.is_finite()) {
            self.last = Some((est_pos, 0.0, heard.metal));
        } else if let Some((p, age, m)) = self.last {
            self.last = Some((p, age + dt, m));
        }
        let keep = ear_memory_s(cunning);
        match self.last {
            Some((p, age, m)) => {
                let d = ((p[0] - self_pos[0]).powi(2) + (p[2] - self_pos[2]).powi(2)).sqrt();
                if age > keep || (age > 0.0 && d < arrive_m) {
                    self.last = None;
                    None
                } else {
                    Some((p, (1.0 - age / keep).clamp(0.0, 1.0), m))
                }
            }
            None => None,
        }
    }
}

/// Distance at which a steady walk in `worn` stops being audible to a dragon
/// with `acuity` in `amb` (m), by bisection. For logs and tuning.
pub fn hearing_range(worn: &Worn, speed: f32, amb: &Ambient, acuity: f32) -> f32 {
    let e = emit(worn, &Gait { speed, crouched: false, landing: 0.0 });
    let audible = |d: f32| hear(&e, [d, 0.0, 0.0], [0.0; 3], 0.0, amb, acuity, 0.0).audible;
    if !audible(1.0) {
        return 0.0;
    }
    let (mut lo, mut hi) = (1.0f32, 2000.0f32);
    if audible(hi) {
        return hi;
    }
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if audible(mid) { lo = mid } else { hi = mid }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;

    const CALM: Ambient = Ambient { wind: 0.0, rain: 0.0 };
    const WINDY: Ambient = Ambient { wind: 8.0, rain: 0.0 };
    fn walk() -> Gait {
        Gait { speed: 1.4, crouched: false, landing: 0.0 }
    }

    #[test]
    fn standing_still_is_silent_moving_is_not() {
        let still = emit(&Worn::full(Material::Plate), &Gait { speed: 0.0, crouched: false, landing: 0.0 });
        assert!(!still.foot_db.is_finite() && !still.clank_db.is_finite());
        let e = emit(&Worn::full(Material::Plate), &walk());
        assert!(e.foot_db.is_finite() && e.clank_db > e.foot_db);
        assert!(e.metal && e.clank_hz > 2000.0);
    }

    #[test]
    fn metal_clanks_louder_than_leather_louder_than_nothing() {
        let plate = emit(&Worn::full(Material::Plate), &walk()).clank_db;
        let leather = emit(&Worn::full(Material::Leather), &walk()).clank_db;
        let none = emit(&Worn::NONE, &walk());
        assert!(plate > leather + 20.0);
        assert!(!none.clank_db.is_finite() && !none.metal);
    }

    #[test]
    fn sprinting_is_louder_sneaking_quieter_landing_loud() {
        let w = Worn::full(Material::Chain);
        let walk_db = emit(&w, &walk()).clank_db;
        let run = emit(&w, &Gait { speed: 5.6, ..walk() }).clank_db;
        let sneak = emit(&w, &Gait { crouched: true, ..walk() }).clank_db;
        let land = emit(&w, &Gait { speed: 0.0, crouched: false, landing: 9.0 }).clank_db;
        assert!((run - walk_db - 12.0).abs() < 0.5, "two doublings of pace");
        assert!(sneak < walk_db - 10.0);
        assert!(land.is_finite() && land > walk_db);
    }

    #[test]
    fn a_clank_carries_farther_than_footsteps_and_cuts_through_wind() {
        let acu = 0.8;
        let plate = hearing_range(&Worn::full(Material::Plate), 1.4, &CALM, acu);
        let bare = hearing_range(&Worn::NONE, 1.4, &CALM, acu);
        assert!(plate > bare * 3.0, "plate {plate:.0} m vs bare {bare:.0} m");
        // Wind swamps the footfalls far more than the clank.
        let plate_w = hearing_range(&Worn::full(Material::Plate), 1.4, &WINDY, acu);
        let bare_w = hearing_range(&Worn::NONE, 1.4, &WINDY, acu);
        assert!(plate_w / plate > bare_w / bare, "wind: plate keeps {:.2}, bare keeps {:.2}", plate_w / plate, bare_w / bare);
    }

    #[test]
    fn sharp_ears_hear_farther_and_attention_helps() {
        let w = Worn::full(Material::Scale);
        assert!(hearing_range(&w, 1.4, &CALM, 1.0) > hearing_range(&w, 1.4, &CALM, 0.2) * 2.0);
        let e = emit(&w, &walk());
        let idle = hear(&e, [150.0, 0.0, 0.0], [0.0; 3], 0.0, &CALM, 0.6, 0.0);
        let hunting = hear(&e, [150.0, 0.0, 0.0], [0.0; 3], 0.0, &CALM, 0.6, 1.0);
        assert!(hunting.margin_db > idle.margin_db + 5.0);
    }

    #[test]
    fn terrain_between_muffles() {
        let e = emit(&Worn::full(Material::Plate), &walk());
        let open = hear(&e, [60.0, 0.0, 0.0], [0.0; 3], 0.0, &CALM, 0.7, 0.0);
        let hill = hear(&e, [60.0, 0.0, 0.0], [0.0; 3], 3.0, &CALM, 0.7, 0.0);
        assert!(open.margin_db - hill.margin_db > 15.0, "{} vs {}", open.margin_db, hill.margin_db);
        // The clank's ring suffers more behind a hill than the footfall's thud.
        assert!(diffraction_db(1.0, 3200.0) > diffraction_db(1.0, 300.0) + 8.0);
        assert!(diffraction_db(1000.0, 5000.0) <= 25.0);
        assert_eq!(diffraction_db(0.0, 3000.0), 0.0);
    }

    #[test]
    fn high_frequencies_die_faster_in_air() {
        assert!(air_absorption_db_per_m(5000.0) > air_absorption_db_per_m(300.0) * 20.0);
        let near = arriving_db(70.0, 5000.0, 10.0, 0.0) - arriving_db(70.0, 300.0, 10.0, 0.0);
        let far = arriving_db(70.0, 5000.0, 300.0, 0.0) - arriving_db(70.0, 300.0, 300.0, 0.0);
        assert!(far < near - 5.0);
    }

    #[test]
    fn a_clank_localises_better_than_a_footfall() {
        let e_plate = emit(&Worn::full(Material::Plate), &walk());
        let e_bare = emit(&Worn::NONE, &walk());
        // Same distance where each is just clearly audible.
        let p = hear(&e_plate, [0.0, 0.0, 200.0], [0.0; 3], 0.0, &CALM, 0.8, 0.0);
        let b = hear(&e_bare, [0.0, 0.0, 20.0], [0.0; 3], 0.0, &CALM, 0.8, 0.0);
        assert!(p.audible && b.audible && p.metal && !b.metal);
        assert!((p.bearing - std::f32::consts::FRAC_PI_2).abs() < 1e-4);
        assert!(p.bearing_err < b.bearing_err * 1.5, "{} vs {}", p.bearing_err, b.bearing_err);
    }

    #[test]
    fn worn_parses_both_forms() {
        assert_eq!(Worn::parse("plate"), Worn::full(Material::Plate));
        let w = Worn::parse("chest=chain, feet=plate, head=bogus, tail=plate");
        assert_eq!(w.0[1], Some(Material::Chain));
        assert_eq!(w.0[5], Some(Material::Plate));
        assert_eq!(w.0[0], None);
        assert!(w.any_metal());
        assert_eq!(Worn::parse(""), Worn::NONE);
    }

    #[test]
    fn ear_acuity_follows_the_crest() {
        let mut g = Genome::from_seed(1);
        g.crest = 1.0;
        let a = ear_acuity(&g);
        g.crest = 0.0;
        assert!(a > ear_acuity(&g) + 0.25);
    }

    #[test]
    fn it_remembers_where_it_heard_you_then_forgets() {
        let heard = Heard { margin_db: 10.0, audible: true, metal: true, bearing: 0.0, bearing_err: 0.1 };
        let silent = Heard { audible: false, margin_db: -5.0, ..heard };
        let run = |cunning: f32| {
            let mut m = EarMemory::default();
            assert!(m.update(&heard, [80.0, 0.0, 0.0], [0.0; 3], cunning, 6.0, 0.1).is_some());
            let mut t = 0.0f32;
            let mut conf_prev = 1.1f32;
            while let Some((p, conf, metal)) = m.update(&silent, [0.0; 3], [0.0; 3], cunning, 6.0, 0.1) {
                assert_eq!(p, [80.0, 0.0, 0.0]);
                assert!(metal && conf <= conf_prev);
                conf_prev = conf;
                t += 0.1;
            }
            t
        };
        let (dull, sharp) = (run(0.0), run(1.0));
        assert!((dull - 5.0).abs() < 0.2 && (sharp - 25.0).abs() < 0.2, "{dull} {sharp}");
        // Arriving at the spot and finding nothing: forgets.
        let mut m = EarMemory::default();
        m.update(&heard, [10.0, 0.0, 0.0], [0.0; 3], 0.5, 6.0, 0.1);
        assert!(m.update(&silent, [0.0; 3], [8.0, 0.0, 0.0], 0.5, 6.0, 0.1).is_none());
    }

    #[test]
    fn garbage_in_does_not_hear_anything() {
        let e = emit(&Worn::full(Material::Plate), &Gait { speed: f32::NAN, crouched: false, landing: f32::NAN });
        let h = hear(&e, [f32::NAN, 0.0, 0.0], [0.0; 3], 0.0, &CALM, 0.8, 0.0);
        assert!(!h.audible);
    }
}
