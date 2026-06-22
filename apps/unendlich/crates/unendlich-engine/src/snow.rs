//! Snow — not one material but a spectrum, selected per column by how wet it is.
//!
//! Wet, packed snow (humid, near melting) is cohesive: it holds steep piles and,
//! when it does move, *slumps* slowly — a viscous flow with lots of internal
//! friction. Dry powder (cold, low humidity) has almost no cohesion: it behaves
//! granularly, like sand, sliding off at a shallow angle of repose and
//! avalanching readily. So one settling rule, parameterised by a `wetness` the
//! caller derives from the local humidity (a `sumtree::region_sum` over the
//! humidity field) and temperature:
//!
//! - **repose threshold** — the height difference a column holds before shedding
//!   to a lower neighbour — *grows* with wetness (wet snow holds steep, powder
//!   sheds early);
//! - **mobility** — the fraction of the excess that actually moves per step —
//!   *falls* with wetness (powder avalanches fast, wet snow oozes).
//!
//! Snow is stored as a per-column depth heightfield (snow piles on top of
//! terrain; a 2-D field is the cheap, natural representation), settled by a
//! volume-conserving relaxation in the spirit of `fluid::flow_step`. Deposition
//! is gated by `season::precipitation` (is it snowing?) and `sampling::exposure`
//! (does the sky reach this column, or is it sheltered?). Pure, deterministic,
//! integer depths + fixed-point wetness; unit-tested.

use crate::season::Precipitation;
use crate::sim::fixed::Fixed;

/// Depth units per full block of snow.
pub const DEPTH_PER_BLOCK: u16 = 16;

/// Per-column snow depth over an `nx × nz` footprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnowField {
    pub nx: usize,
    pub nz: usize,
    /// Row-major (`z * nx + x`) depth in units (`DEPTH_PER_BLOCK` = one block).
    pub depth: Vec<u16>,
}

impl SnowField {
    pub fn new(nx: usize, nz: usize) -> SnowField {
        SnowField { nx, nz, depth: vec![0; nx * nz] }
    }
    pub fn get(&self, x: usize, z: usize) -> u16 {
        self.depth.get(z * self.nx + x).copied().unwrap_or(0)
    }
    pub fn set(&mut self, x: usize, z: usize, v: u16) {
        if x < self.nx && z < self.nz {
            self.depth[z * self.nx + x] = v;
        }
    }
    /// Total snow volume (units) — the conserved quantity.
    pub fn total(&self) -> u64 {
        self.depth.iter().map(|&d| d as u64).sum()
    }
    /// Depth at a column in blocks.
    pub fn depth_blocks(&self, x: usize, z: usize) -> f32 {
        self.get(x, z) as f32 / DEPTH_PER_BLOCK as f32
    }

    /// Add snow per column. `amount_at(x,z)` is the depth to add (usually
    /// [`snowfall_amount`] folded with that column's sky exposure), saturating.
    pub fn deposit(&mut self, amount_at: impl Fn(usize, usize) -> u16) {
        for z in 0..self.nz {
            for x in 0..self.nx {
                let i = z * self.nx + x;
                self.depth[i] = self.depth[i].saturating_add(amount_at(x, z));
            }
        }
    }
}

/// Wetness of freshly-settling snow in `[0,1]`: 0 = dry powder (granular),
/// 1 = sodden packed snow (viscous). Humidity is the main driver (you need
/// moisture to pack), warmth nudges it wetter (snow near melting is slushy);
/// cold, dry air gives powder. Both inputs normalised `[0,1]`.
pub fn snow_wetness(humidity: Fixed, temperature: Fixed) -> Fixed {
    let h = humidity.clamp(Fixed::ZERO, Fixed::ONE);
    let t = temperature.clamp(Fixed::ZERO, Fixed::ONE);
    // humidity scaled by a warmth factor in [0.5, 1.0].
    (h * (Fixed::from_frac(1, 2) + t * Fixed::from_frac(1, 2))).clamp(Fixed::ZERO, Fixed::ONE)
}

/// Max neighbour depth difference (units) a column holds before shedding. Powder
/// sheds early (low), wet packed snow holds steep (high).
fn repose_threshold(wetness: Fixed) -> i32 {
    2 + (wetness.clamp(Fixed::ZERO, Fixed::ONE) * Fixed::from_int(10)).floor_int() as i32
}

/// Fraction (percent) of the over-threshold excess that moves per step. Powder
/// is mobile (avalanches), wet snow sluggish (slumps). 100 → settles to repose
/// in one step; lower → relaxes over many.
fn mobility_pct(wetness: Fixed) -> i32 {
    100 - (wetness.clamp(Fixed::ZERO, Fixed::ONE) * Fixed::from_int(75)).floor_int() as i32
}

/// One settle relaxation. A column higher than a neighbour by more than its
/// repose threshold sheds part of the excess downhill; how much depends on
/// mobility (powder fast, wet slow). Volume-conserving. `wetness_at(x,z)` gives
/// each column's snow wetness; `is_blocked(x,z)` marks columns snow can't enter
/// (walls), so it piles against them. Deterministic (fixed +x/+z visit order).
pub fn settle_step(
    f: &SnowField,
    wetness_at: impl Fn(usize, usize) -> Fixed,
    is_blocked: impl Fn(usize, usize) -> bool,
) -> SnowField {
    let mut out = f.clone();
    for z in 0..f.nz {
        for x in 0..f.nx {
            if x + 1 < f.nx {
                relax(&mut out, (x, z), (x + 1, z), &wetness_at, &is_blocked);
            }
            if z + 1 < f.nz {
                relax(&mut out, (x, z), (x, z + 1), &wetness_at, &is_blocked);
            }
        }
    }
    out
}

fn relax(
    out: &mut SnowField,
    a: (usize, usize),
    b: (usize, usize),
    wetness_at: &impl Fn(usize, usize) -> Fixed,
    is_blocked: &impl Fn(usize, usize) -> bool,
) {
    let da = out.get(a.0, a.1) as i32;
    let db = out.get(b.0, b.1) as i32;
    // Order high → low.
    let (hi, hd, lo, ld) = if da >= db { (a, da, b, db) } else { (b, db, a, da) };
    if is_blocked(lo.0, lo.1) {
        return; // snow can't slide into a wall; it stacks against it
    }
    let w = wetness_at(hi.0, hi.1);
    let excess = (hd - ld) - repose_threshold(w);
    if excess <= 0 {
        return;
    }
    let mv = (excess * mobility_pct(w)) / 200; // ≤ excess/2 → stable
    if mv <= 0 {
        return;
    }
    out.set(hi.0, hi.1, (hd - mv) as u16);
    out.set(lo.0, lo.1, (ld + mv) as u16);
}

/// Depth of snow to deposit this tick: zero unless it's snowing, scaled by sky
/// `exposure` (`[0,1]`, from `sampling::exposure` of upward rays) so overhangs
/// and caves stay bare. `rate` is the depth units per tick under fully open sky.
pub fn snowfall_amount(precip: Precipitation, exposure: Fixed, rate: u16) -> u16 {
    if precip != Precipitation::Snow {
        return 0;
    }
    let e = exposure.clamp(Fixed::ZERO, Fixed::ONE);
    (Fixed::from_int(rate as i64) * e).floor_int().max(0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform_wetness(w: Fixed) -> impl Fn(usize, usize) -> Fixed {
        move |_, _| w
    }
    fn no_walls(_: usize, _: usize) -> bool {
        false
    }

    #[test]
    fn wetness_rises_with_humidity_and_warmth() {
        let dry_cold = snow_wetness(Fixed::from_frac(1, 10), Fixed::ZERO);
        let humid_cold = snow_wetness(Fixed::from_frac(9, 10), Fixed::ZERO);
        let humid_warm = snow_wetness(Fixed::from_frac(9, 10), Fixed::ONE);
        assert!(humid_cold > dry_cold, "more humidity → wetter snow");
        assert!(humid_warm > humid_cold, "warmer → wetter snow");
        assert_eq!(snow_wetness(Fixed::ZERO, Fixed::ONE), Fixed::ZERO, "no moisture → powder");
    }

    #[test]
    fn powder_holds_less_and_flows_more_than_wet() {
        let powder = Fixed::ZERO;
        let wet = Fixed::ONE;
        assert!(repose_threshold(powder) < repose_threshold(wet), "powder sheds at a shallower slope");
        assert!(mobility_pct(powder) > mobility_pct(wet), "powder avalanches faster than wet slumps");
    }

    #[test]
    fn settle_conserves_volume() {
        let mut f = SnowField::new(5, 1);
        f.set(0, 0, 200); // a tall spike
        let before = f.total();
        let mut cur = f;
        for _ in 0..30 {
            cur = settle_step(&cur, uniform_wetness(Fixed::ZERO), no_walls);
        }
        assert_eq!(cur.total(), before, "snow is neither created nor destroyed");
    }

    #[test]
    fn powder_avalanches_faster_than_wet_snow() {
        // Same tall spike, one step: powder should shed more than wet.
        let mut spike = SnowField::new(3, 1);
        spike.set(0, 0, 200);
        let powder = settle_step(&spike, uniform_wetness(Fixed::ZERO), no_walls);
        let wet = settle_step(&spike, uniform_wetness(Fixed::ONE), no_walls);
        let powder_moved = 200 - powder.get(0, 0);
        let wet_moved = 200 - wet.get(0, 0);
        assert!(powder_moved > wet_moved, "powder {powder_moved} sheds more than wet {wet_moved}");
    }

    #[test]
    fn a_spike_relaxes_toward_its_repose_angle() {
        // Powder spike eventually flattens to within ~repose of its neighbours.
        let mut cur = SnowField::new(7, 1);
        cur.set(3, 0, 300);
        for _ in 0..200 {
            cur = settle_step(&cur, uniform_wetness(Fixed::ZERO), no_walls);
        }
        // No adjacent pair differs by more than the powder repose threshold.
        let thr = repose_threshold(Fixed::ZERO);
        for x in 0..6 {
            let d = (cur.get(x, 0) as i32 - cur.get(x + 1, 0) as i32).abs();
            // Integer relaxation leaves at most a 1-unit residual over the repose
            // threshold (moving excess/2 floors odd excess).
            assert!(d <= thr + 1, "settled within repose at x={x}: diff {d} > {}", thr + 1);
        }
    }

    #[test]
    fn snow_stacks_against_a_wall() {
        // Column 1 is a wall; snow piled at 0 must not slide into it.
        let mut f = SnowField::new(3, 1);
        f.set(0, 0, 100);
        let blocked = |x: usize, _z: usize| x == 1;
        let out = settle_step(&f, uniform_wetness(Fixed::ZERO), blocked);
        assert_eq!(out.get(1, 0), 0, "no snow enters the wall column");
        assert_eq!(out.get(0, 0), 100, "it stays stacked against the wall");
    }

    #[test]
    fn deposit_only_when_snowing_and_scaled_by_exposure() {
        assert_eq!(snowfall_amount(Precipitation::Rain, Fixed::ONE, 8), 0, "rain leaves no snow");
        assert_eq!(snowfall_amount(Precipitation::Snow, Fixed::ZERO, 8), 0, "sheltered → bare");
        assert_eq!(snowfall_amount(Precipitation::Snow, Fixed::ONE, 8), 8, "open sky → full rate");
        let half = snowfall_amount(Precipitation::Snow, Fixed::from_frac(1, 2), 8);
        assert_eq!(half, 4, "partial exposure → partial fall");

        // End to end: snowing, an open column gains depth, a sheltered one doesn't.
        let mut field = SnowField::new(2, 1);
        let exposure = |x: usize, _z: usize| if x == 0 { Fixed::ONE } else { Fixed::ZERO };
        field.deposit(|x, z| snowfall_amount(Precipitation::Snow, exposure(x, z), 8));
        assert_eq!(field.get(0, 0), 8, "open column collects snow");
        assert_eq!(field.get(1, 0), 0, "sheltered column stays bare");
    }
}
