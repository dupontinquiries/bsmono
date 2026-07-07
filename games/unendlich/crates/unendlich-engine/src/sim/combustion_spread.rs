//! `sim::combustion_spread` — tree combustion duration + fire spread.
//!
//! Implements **`docs/specs/flora_update_genetics.docx` §A** (Combustion & Dead
//! Wood). The old flat 45 s burn is replaced by §A.1's *revised burn-duration
//! table*: a tree is a big wet organism, so a full mature tree burns for about
//! one in-game day, scaling up with size and fire-resistance (FR) allele tier.
//! A 1-day burn is a fire *event* the player watches unfold, not an instant
//! pop. Pure logic only — the bin owns the voxel grid and the wall clock.
//!
//! Time scale: the table is authored in **in-game hours**; [`burn_duration`]
//! returns real seconds via [`SECONDS_PER_IGAME_HOUR`] (1 in-game day = 1200 s
//! ≈ 20 real minutes). Only the *shape*/ratios are load-bearing — the bin owns
//! the authoritative day length and can rescale that one constant.
//!
//! Note on §A.1's FR-3 column: the spec marks high-FR cases qualitatively
//! ("green sapling does not ignite", "near-immune", "smoulders for days"). The
//! *does-not-ignite* behaviour is modelled where it belongs — in
//! [`ignition_chance`], which collapses toward 0 at high FR / low dryness — so
//! [`burn_duration`] stays a clean monotone "how long it burns once lit".
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()`. Inputs sanitised; never panics.

/// Real seconds per in-game hour (1 in-game day = 24 h = 1200 s ≈ 20 min).
pub const SECONDS_PER_IGAME_HOUR: f32 = 50.0;

/// Tree size/state categories (§A.1). Burn time grows with size; `DeadWood`
/// is the dry exception that burns fast and hot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeCategory {
    Sapling,
    Standard, // a full-sized mature tree — the spec's "1 in-game day" baseline
    Ancient,
    Mega,
    DeadWood,
}

#[inline]
fn san(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}
#[inline]
fn clamp01(x: f32) -> f32 {
    san(x).max(0.0).min(1.0)
}

/// Burn duration in **seconds**, from §A.1's revised table.
///
/// In-game-hour grid by category × FR tier (`fr_tier` clamped to `0..=3`):
///
/// ```text
///            FR0    FR1    FR2    FR3
/// DeadWood    1.0    1.2    1.5    2.0     (dry — fast & hot)
/// Sapling     6      10     18     30
/// Standard    24     36     72     120     (1d / 1.5d / 3d / 5d smoulder)
/// Ancient     36     48     96     156     (1.5d / 2d / 4d / 6.5d)
/// Mega        72     120    192    300     (3d / 5d / 8d / 12.5d)
/// ```
///
/// Strictly increases with both category size and FR tier; `DeadWood` is the
/// shortest. Result is finite and `> 0`.
pub fn burn_duration(cat: TreeCategory, fr_tier: u8) -> f32 {
    let fr = fr_tier.min(3) as usize;
    let hours: [f32; 4] = match cat {
        TreeCategory::DeadWood => [1.0, 1.2, 1.5, 2.0],
        TreeCategory::Sapling => [6.0, 10.0, 18.0, 30.0],
        TreeCategory::Standard => [24.0, 36.0, 72.0, 120.0],
        TreeCategory::Ancient => [36.0, 48.0, 96.0, 156.0],
        TreeCategory::Mega => [72.0, 120.0, 192.0, 300.0],
    };
    hours[fr] * SECONDS_PER_IGAME_HOUR
}

/// Heat units at which the heat term is ~63% saturated (tunable).
const HEAT_K: f32 = 1.0;
/// Wind-alignment contribution floor: at zero alignment the wind factor is this.
const WIND_BASE: f32 = 0.5;
/// FR resistance penalty per tier in the ignition denominator.
const FR_PENALTY: f32 = 1.0;

/// Chance in `[0, 1]` that a neighbouring tree catches this tick.
///
/// `chance = dryness01 * heat_term * wind_factor / (1 + FR_PENALTY*fr_tier)`
/// where `heat_term = 1 - exp(-max(0,heat)/HEAT_K)` and
/// `wind_factor = WIND_BASE + (1-WIND_BASE)*wind_align01`. Rises with dryness,
/// heat, and wind alignment; falls with FR tier. Wet wood (`dryness01 == 0`)
/// gives `0`. Clamped to `[0, 1]`.
pub fn ignition_chance(dryness01: f32, incoming_heat: f32, wind_align01: f32, fr_tier: u8) -> f32 {
    let dry = clamp01(dryness01);
    let heat = san(incoming_heat).max(0.0);
    let wind = clamp01(wind_align01);
    let fr = fr_tier.min(3) as f32;

    let heat_term = 1.0 - (-heat / HEAT_K).exp(); // [0,1), increasing in heat
    let wind_factor = WIND_BASE + (1.0 - WIND_BASE) * wind; // [WIND_BASE, 1]
    let resist = 1.0 + FR_PENALTY * fr; // >= 1

    (dry * heat_term * wind_factor / resist).max(0.0).min(1.0)
}

/// Live burn state for one burning tree/section.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BurnState {
    /// Seconds of burn left.
    pub remaining: f32,
    /// Current fire intensity in `[0, 1]` (ramps up, holds, then tapers).
    pub intensity01: f32,
    /// True once the burn has finished (becomes dead wood / ash).
    pub spent: bool,
}

/// Seconds over which a fresh fire ramps to full intensity.
const IGNITE_SECONDS: f32 = 3.0;
/// Tail seconds over which intensity tapers to 0 as the burn ends (embers).
const EMBER_SECONDS: f32 = 5.0;

/// Advance a burn by `dt` seconds: count `remaining` down toward 0 (never up),
/// ramp intensity up early and taper it as the end nears, and set `spent` once
/// `remaining` hits 0. `dt <= 0` (or non-finite) leaves the state unchanged.
/// Intensity stays finite in `[0, 1]`.
pub fn tick_burn(prev: BurnState, dt: f32) -> BurnState {
    let cur_remaining = san(prev.remaining).max(0.0);
    let cur_intensity = clamp01(prev.intensity01);

    // already finished, or no time advance -> unchanged (sanitised).
    let step = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
    if step == 0.0 || prev.spent || cur_remaining <= 0.0 {
        let spent = prev.spent || cur_remaining <= 0.0;
        return BurnState {
            remaining: cur_remaining,
            intensity01: if spent { 0.0 } else { cur_intensity },
            spent,
        };
    }

    let remaining = (cur_remaining - step).max(0.0);
    if remaining <= 0.0 {
        return BurnState { remaining: 0.0, intensity01: 0.0, spent: true };
    }

    // ramp toward 1 early (the fire builds), and a taper that pulls intensity
    // down over the final EMBER_SECONDS — intensity = min(ramp, taper).
    let ramp = (cur_intensity + step / IGNITE_SECONDS).min(1.0);
    let taper = (remaining / EMBER_SECONDS).max(0.0).min(1.0);
    let intensity01 = ramp.min(taper).max(0.0).min(1.0);

    BurnState { remaining, intensity01, spent: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDER: [TreeCategory; 5] = [
        TreeCategory::DeadWood,
        TreeCategory::Sapling,
        TreeCategory::Standard,
        TreeCategory::Ancient,
        TreeCategory::Mega,
    ];

    #[test]
    fn burn_duration_increases_with_size_and_fr() {
        // size monotonic at each FR tier (DeadWood fastest .. Mega longest)
        for fr in 0..=3u8 {
            for w in ORDER.windows(2) {
                assert!(burn_duration(w[1], fr) > burn_duration(w[0], fr),
                        "size not increasing at fr {}: {:?} vs {:?}", fr, w[0], w[1]);
            }
        }
        // FR monotonic within each category
        for &cat in &ORDER {
            for fr in 0..3u8 {
                assert!(burn_duration(cat, fr + 1) > burn_duration(cat, fr),
                        "fr not increasing for {:?} at {}", cat, fr);
            }
        }
        // DeadWood is the fastest overall; all finite and > 0
        for &cat in &ORDER {
            for fr in 0..=4u8 {
                let d = burn_duration(cat, fr);
                assert!(d.is_finite() && d > 0.0);
            }
            assert!(burn_duration(TreeCategory::DeadWood, 3) < burn_duration(TreeCategory::Sapling, 0));
        }
        // fr_tier clamps at 3
        assert_eq!(burn_duration(TreeCategory::Standard, 7), burn_duration(TreeCategory::Standard, 3));
        // spec anchor: mature/Standard FR0 == one in-game day
        assert!((burn_duration(TreeCategory::Standard, 0) - 24.0 * SECONDS_PER_IGAME_HOUR).abs() < 1e-3);
    }

    #[test]
    fn ignition_rises_and_falls_correctly_and_clamped() {
        // wet -> ~0
        assert_eq!(ignition_chance(0.0, 100.0, 1.0, 0), 0.0);
        // rises with dryness
        assert!(ignition_chance(0.8, 2.0, 0.5, 0) > ignition_chance(0.3, 2.0, 0.5, 0));
        // rises with heat
        assert!(ignition_chance(0.6, 4.0, 0.5, 0) > ignition_chance(0.6, 0.5, 0.5, 0));
        // rises with wind alignment
        assert!(ignition_chance(0.6, 2.0, 1.0, 0) > ignition_chance(0.6, 2.0, 0.0, 0));
        // falls with FR tier
        assert!(ignition_chance(0.9, 5.0, 1.0, 0) > ignition_chance(0.9, 5.0, 1.0, 3));
        // clamped [0,1] incl. extreme/degenerate inputs
        for &(d, h, w, fr) in &[(2.0f32, 1e9f32, 2.0f32, 0u8), (-1.0, -5.0, -1.0, 9), (f32::NAN, f32::INFINITY, 0.5, 1)] {
            let c = ignition_chance(d, h, w, fr);
            assert!(c.is_finite() && (0.0..=1.0).contains(&c), "chance {} out of range", c);
        }
    }

    #[test]
    fn tick_burn_counts_down_peaks_then_spent() {
        let total = burn_duration(TreeCategory::Sapling, 0); // 300 s
        let mut s = BurnState { remaining: total, intensity01: 0.0, spent: false };
        let dt = 1.0;
        let mut prev_remaining = s.remaining;
        let mut peak = 0.0f32;
        let mut samples: Vec<(f32, f32)> = Vec::new(); // (elapsed, intensity)
        let mut elapsed = 0.0;
        let mut guard = 0;
        while !s.spent && guard < 100_000 {
            s = tick_burn(s, dt);
            assert!(s.remaining <= prev_remaining + 1e-6, "remaining went up");
            assert!((0.0..=1.0).contains(&s.intensity01) && s.intensity01.is_finite());
            prev_remaining = s.remaining;
            elapsed += dt;
            if s.intensity01 > peak { peak = s.intensity01; }
            samples.push((elapsed, s.intensity01));
            guard += 1;
        }
        assert!(s.spent && s.remaining == 0.0, "did not finish");
        assert!(s.intensity01 == 0.0, "intensity not 0 when spent");
        // peaks then tapers: peak is high, and it both rose from the start and
        // fell before the end.
        assert!(peak > 0.9, "never reached high intensity: {}", peak);
        let start_i = samples[0].1;
        // an early sample is below the peak (rose) ...
        assert!(start_i < peak);
        // ... and a late sample (just before spent) is below the peak (tapered)
        let late = samples[samples.len().saturating_sub(2)].1;
        assert!(late < peak, "did not taper: late {} peak {}", late, peak);
    }

    #[test]
    fn tick_burn_dt_nonpositive_unchanged() {
        let s0 = BurnState { remaining: 50.0, intensity01: 0.4, spent: false };
        for &dt in &[0.0f32, -2.0, f32::NAN, f32::INFINITY] {
            let s = tick_burn(s0, dt);
            assert_eq!(s.remaining, 50.0);
            assert_eq!(s.spent, false);
            assert!(s.intensity01.is_finite());
        }
        // already spent stays spent, intensity 0
        let sp = tick_burn(BurnState { remaining: 0.0, intensity01: 0.5, spent: true }, 1.0);
        assert!(sp.spent && sp.intensity01 == 0.0 && sp.remaining == 0.0);
    }

    #[test]
    fn deterministic_and_seeded_sweep() {
        assert_eq!(burn_duration(TreeCategory::Ancient, 2), burn_duration(TreeCategory::Ancient, 2));
        assert_eq!(ignition_chance(0.5, 2.0, 0.5, 1), ignition_chance(0.5, 2.0, 0.5, 1));

        // deterministic seeded sweep (R20): all outputs finite & in range
        let mut x: u64 = 0xF13E_0058_u64.wrapping_add(1);
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        let span = |v: u32, lo: f32, hi: f32| lo + (v % 1000) as f32 / 1000.0 * (hi - lo);
        for _ in 0..5000 {
            let c = ignition_chance(span(next(), -0.2, 1.2), span(next(), -1.0, 50.0), span(next(), -0.2, 1.2), (next() % 6) as u8);
            assert!(c.is_finite() && (0.0..=1.0).contains(&c));
            let s = tick_burn(
                BurnState { remaining: span(next(), -5.0, 500.0), intensity01: span(next(), -0.5, 1.5), spent: next() % 5 == 0 },
                span(next(), -0.1, 3.0),
            );
            assert!(s.remaining.is_finite() && s.remaining >= 0.0);
            assert!(s.intensity01.is_finite() && (0.0..=1.0).contains(&s.intensity01));
        }
    }
}
