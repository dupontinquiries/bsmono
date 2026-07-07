//! `fire_field` — batch-as-field LOD for spreading fire (EXPERIMENTAL, r239).
//!
//! The grass treatment (`sim::grass_field`) applied to fire. A spreading fire
//! across many burning blocks is, at distance, an aggregate the eye reads as a
//! glowing, advancing front — not individually resolvable flames. So instead
//! of ticking every burning cell's [`combustion_spread::tick_burn`] far away,
//! we batch a fire region into an **intensity / fuel / heat field** and split
//! by distance band:
//!
//! - **Front band** (near): high-res per-cell burn ticking — the flames you
//!   watch up close.
//! - **Back band** (far): the region is a **field** (per-cell `fuel`,
//!   `intensity`, `heat`) **fast-forwarded** by one propagation pass that
//!   compensates for the skipped ticks — fuel burns down, intensity follows
//!   fuel, the fire front spreads to flammable neighbours, and heat radiates &
//!   decays. Same catch-up idea as `world::leaf_decay::advance_n_ticks` /
//!   `grass_field::fast_forward`, but with combustion semantics.
//!
//! Why a field for the back: a wildfire can be thousands of burning cells;
//! ticking each is O(N). A coarse W×H field is O(W·H) « N, and at distance the
//! per-cell flame isn't resolvable — the aggregate (how big/bright the burn,
//! where the front is, how much fuel is left) is what reads. When the player
//! turns to look, the far fire is already at a plausible *present* (advanced
//! front, depleted fuel) rather than frozen.
//!
//! Determinism + gating mirror `grass_field`: the model is pure, always built,
//! and unit-tested; the live tick-path hookup is behind the `fire-field-batch`
//! cargo feature (off by default). Spread/burn *ratios* here echo
//! `combustion_spread` (fuel burns down monotonically; intensity tracks fuel;
//! ignition needs fuel + incoming heat) without re-deriving its authored
//! tables — the field is the coarse aggregate, the per-cell path stays exact.

#![allow(dead_code)]

use crate::world::scheduler::{ChunkTickBand, TickScheduleParams};

/// Aggregate state of one fire **field cell** — a tile of a burning region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FireCell {
    /// Remaining fuel in `[0,1]` (1 = untouched flammable mass, 0 = spent).
    pub fuel: f32,
    /// Current burn intensity in `[0,1]` (0 = not burning, 1 = full flame).
    pub intensity: f32,
    /// Local heat in `[0,1]` — radiated by burning cells, ignites neighbours,
    /// decays when nothing feeds it. Drives spread.
    pub heat: f32,
}

impl FireCell {
    /// Unburnt flammable cell (fuel, cold, not yet lit).
    pub fn fuel(fuel: f32) -> FireCell {
        FireCell { fuel: clamp01(fuel), intensity: 0.0, heat: 0.0 }
    }
    /// A lit cell (full intensity + heat), e.g. the ignition source.
    pub fn lit(fuel: f32) -> FireCell {
        FireCell { fuel: clamp01(fuel), intensity: 1.0, heat: 1.0 }
    }
    pub fn is_burning(&self) -> bool {
        self.intensity > 0.0 && self.fuel > 0.0
    }
}

/// Per-tick combustion rates for the field. Tunable per fuel type/biome.
#[derive(Clone, Copy, Debug)]
pub struct FireFieldParams {
    /// Fuel consumed per tick by a fully-burning cell (intensity-scaled).
    pub burn_per_tick: f32,
    /// Heat each burning cell radiates to its 4 neighbours per tick
    /// (intensity-scaled). Drives the spreading front.
    pub heat_spread_per_tick: f32,
    /// Heat decay per tick (fraction lost when not refed). Keeps fronts thin.
    pub heat_decay_per_tick: f32,
    /// Heat threshold above which a fuelled cell ignites (intensity jumps up).
    pub ignite_heat: f32,
}

impl Default for FireFieldParams {
    fn default() -> Self {
        FireFieldParams {
            burn_per_tick: 0.05,
            heat_spread_per_tick: 0.30,
            heat_decay_per_tick: 0.20,
            ignite_heat: 0.35,
        }
    }
}

/// A coarse fuel/intensity/heat field over a `w × h` tile grid covering a fire
/// region. Row-major.
#[derive(Clone, Debug)]
pub struct FireField {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<FireCell>,
}

impl FireField {
    pub fn new(w: usize, h: usize, fill: FireCell) -> FireField {
        FireField { w, h, cells: vec![fill; w * h] }
    }

    /// Total remaining fuel (Σ fuel) — non-increasing as the fire burns.
    pub fn total_fuel(&self) -> f32 {
        self.cells.iter().map(|c| c.fuel).sum()
    }

    /// Total active burn (Σ intensity) — the size of the blaze right now.
    pub fn total_intensity(&self) -> f32 {
        self.cells.iter().map(|c| c.intensity).sum()
    }

    /// Number of cells currently burning.
    pub fn burning_count(&self) -> usize {
        self.cells.iter().filter(|c| c.is_burning()).count()
    }

    /// **One high-res front tick** (what the player sees up close). Exact:
    /// burning cells consume fuel + radiate heat to neighbours; fuelled cells
    /// over the ignite threshold light up; spent cells go out; heat decays.
    pub fn tick_front(&mut self, p: &FireFieldParams) {
        self.step(1, p);
    }

    /// **Field propagation — fast-forward the back band by `n` ticks.** Runs
    /// the same combustion step `n` times on the coarse field. Unlike grass
    /// (whose growth/sway are linear and collapse to a closed form), fire is a
    /// reaction-diffusion front: fuel burndown and ignition are coupled and
    /// nonlinear, so the honest catch-up is to iterate the aggregate step `n`
    /// times — still O(n · w · h) « O(n · N blades) for a dense region, and
    /// the field is coarse. `n == 0` is a no-op. `n` is capped so a very long
    /// absence can't spin forever; by then the region has burned out anyway
    /// (fuel is monotonically consumed, so the state converges).
    pub fn fast_forward(&mut self, n: u32, p: &FireFieldParams) {
        if n == 0 {
            return;
        }
        // Cap: once a region has had ~enough ticks to consume all fuel it is
        // stable (burnt out). 1/burn_per_tick ticks fully consumes a cell at
        // full intensity; a generous multiple guarantees convergence without
        // unbounded work after a long absence.
        let max_useful = ((1.0 / p.burn_per_tick.max(1e-3)) * 4.0).ceil() as u32;
        self.step(n.min(max_useful.max(1)), p);
    }

    /// Run `steps` combustion steps. Two-buffer per step so neighbour reads are
    /// order-independent (deterministic).
    fn step(&mut self, steps: u32, p: &FireFieldParams) {
        let (w, h) = (self.w, self.h);
        for _ in 0..steps {
            // 1) heat exchange: each burning cell adds heat to neighbours.
            let mut add_heat = vec![0.0f32; w * h];
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let c = self.cells[i];
                    if c.is_burning() {
                        let radiate = p.heat_spread_per_tick * c.intensity;
                        for (nx, ny) in neigh4(x, y, w, h) {
                            add_heat[ny * w + nx] += radiate;
                        }
                    }
                }
            }
            // 2) per-cell update: apply heat, ignite, burn fuel, decay heat.
            for i in 0..(w * h) {
                let c = &mut self.cells[i];
                // accumulate + decay heat
                c.heat = clamp01(c.heat * (1.0 - p.heat_decay_per_tick) + add_heat[i]);
                // ignition: a fuelled cell that's hot enough lights up
                if c.fuel > 0.0 && c.heat >= p.ignite_heat && c.intensity < 1.0 {
                    c.intensity = clamp01(c.intensity + 0.5);
                }
                // burn: consume fuel proportional to intensity
                if c.is_burning() {
                    let burned = (p.burn_per_tick * c.intensity).min(c.fuel);
                    c.fuel = (c.fuel - burned).max(0.0);
                    // intensity tracks fuel — fades as fuel runs out
                    if c.fuel <= 0.0 {
                        c.intensity = 0.0; // burnt out
                    } else {
                        // a burning cell sustains its own heat
                        c.heat = clamp01(c.heat.max(c.intensity));
                    }
                }
            }
        }
    }
}

/// How to process a fire region this tick (mirrors `grass_field::GrassWork`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireWork {
    /// Front: high-res per-cell burn ticking.
    HighResFront,
    /// Back: fast-forward the field by `catch_up_ticks` in one propagation.
    FieldFastForward { catch_up_ticks: u32 },
    /// Out of the sim band — dormant (frozen until it re-enters a band).
    Dormant,
}

/// Decide how to process a fire region from its chunk band + the gap since it
/// last updated, reusing the engine's band classifier (same as grass).
pub fn band_plan(band: ChunkTickBand, ticks_since_update: u32, _p: &TickScheduleParams) -> FireWork {
    match band {
        ChunkTickBand::Render => FireWork::HighResFront,
        ChunkTickBand::Simulation => {
            FireWork::FieldFastForward { catch_up_ticks: ticks_since_update.max(1) }
        }
        ChunkTickBand::CatchUp => FireWork::Dormant,
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn neigh4(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    [
        if x > 0 { Some((x - 1, y)) } else { None },
        if x + 1 < w { Some((x + 1, y)) } else { None },
        if y > 0 { Some((x, y - 1)) } else { None },
        if y + 1 < h { Some((x, y + 1)) } else { None },
    ]
    .into_iter()
    .flatten()
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn fire_spreads_from_a_source_into_fuel() {
        // a lit cell in the middle of a fuelled field → fire spreads outward.
        let mut f = FireField::new(5, 5, FireCell::fuel(1.0));
        f.cells[12] = FireCell::lit(1.0); // centre
        let p = FireFieldParams::default();
        let burning0 = f.burning_count();
        for _ in 0..5 {
            f.tick_front(&p);
        }
        assert!(f.burning_count() > burning0, "fire spread to neighbours");
    }

    #[test]
    fn fuel_is_monotonically_consumed() {
        let mut f = FireField::new(4, 4, FireCell::lit(1.0));
        let p = FireFieldParams::default();
        let mut last = f.total_fuel();
        for _ in 0..10 {
            f.tick_front(&p);
            let now = f.total_fuel();
            assert!(now <= last + 1e-4, "fuel went up: {last} -> {now}");
            last = now;
        }
    }

    #[test]
    fn fast_forward_advances_the_burn_like_stepping() {
        // fast_forward(n) iterates the same step, so it must match n front
        // ticks exactly (fire's step is shared, not a separate closed form).
        let mut seed = FireField::new(5, 5, FireCell::fuel(1.0));
        seed.cells[12] = FireCell::lit(1.0);
        let mut batched = seed.clone();
        let mut stepped = seed.clone();
        let p = FireFieldParams::default();
        let n = 8u32;
        batched.fast_forward(n, &p);
        for _ in 0..n {
            stepped.tick_front(&p);
        }
        for (a, b) in batched.cells.iter().zip(stepped.cells.iter()) {
            assert!(approx(a.fuel, b.fuel) && approx(a.intensity, b.intensity), "cell mismatch");
        }
    }

    #[test]
    fn a_fire_burns_itself_out() {
        // small fuel everywhere, fully lit → eventually all fuel consumed,
        // nothing burning. fast_forward with a huge n must converge (the cap).
        let mut f = FireField::new(4, 4, FireCell::lit(0.5));
        let p = FireFieldParams::default();
        f.fast_forward(100_000, &p); // capped internally
        assert!(approx(f.total_fuel(), 0.0), "all fuel consumed, fuel={}", f.total_fuel());
        assert_eq!(f.burning_count(), 0, "fire is out");
    }

    #[test]
    fn no_fuel_means_no_fire() {
        let mut f = FireField::new(3, 3, FireCell::fuel(0.0));
        f.cells[4].heat = 1.0; // hot but no fuel
        let p = FireFieldParams::default();
        f.tick_front(&p);
        assert_eq!(f.burning_count(), 0, "no fuel → cannot ignite");
    }

    #[test]
    fn band_plan_splits_front_back_dormant() {
        let p = TickScheduleParams::default();
        assert_eq!(band_plan(ChunkTickBand::Render, 0, &p), FireWork::HighResFront);
        assert_eq!(
            band_plan(ChunkTickBand::Simulation, 9, &p),
            FireWork::FieldFastForward { catch_up_ticks: 9 }
        );
        assert_eq!(band_plan(ChunkTickBand::CatchUp, 5, &p), FireWork::Dormant);
    }
}
