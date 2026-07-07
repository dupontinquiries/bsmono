//! `grass_field` — batch-as-field LOD for dense grass (EXPERIMENTAL, r238).
//!
//! The usual distance LOD is *cull or simplify the far stuff*: stop drawing /
//! stop ticking blades you can barely see. This module does something
//! different — it **batches a dense patch into a field** and splits the work
//! by distance band:
//!
//! - **Front band** (near the camera): high-resolution, per-blade ticking —
//!   the sway and growth you actually see up close, one tick of detail per
//!   game tick.
//! - **Back band** (far): the patch is treated as an aggregate **field**
//!   (density / growth-phase / sway-phase per field cell, not per blade) and
//!   **fast-forwarded** with a single *field-propagation* step that
//!   compensates for however many ticks were skipped — the same trick
//!   `world::leaf_decay::advance_n_ticks` uses for leaves, but over a field
//!   instead of per-instance. Growth advances by the gap, density diffuses to
//!   neighbours (grass spreading), and the sway phase is advanced modulo its
//!   period so when the player turns to look, the far field is already in a
//!   plausible *present* state rather than frozen or visibly catching up.
//!
//! Why a field for the back: ticking N blades is O(N); advancing a coarse
//! W×H field is O(W·H) « N for a dense patch, and at distance the per-blade
//! detail isn't resolvable anyway — the eye reads the aggregate (how green,
//! how tall, how it ripples). So we pay for what's visible.
//!
//! This is the pure model + its propagation math; it's always compiled and
//! unit-tested. The live tick-path hookup is behind the `grass-field-batch`
//! cargo feature (off by default) — see `band_plan` for the split and the
//! integration notes in `docs/design/grass_field_batch.md`.
//!
//! Pure & deterministic: no clock, no RNG, no I/O. Same determinism boundary
//! as the rest of the sim, so it's reproducible and headless-testable.

#![allow(dead_code)]

use crate::world::scheduler::{ChunkTickBand, TickScheduleParams};

/// Aggregate state of one **field cell** — a small tile of a grass patch
/// summarised, not its individual blades.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldCell {
    /// Blade density in `[0,1]` (coverage fraction of the tile).
    pub density: f32,
    /// Mean growth phase in `[0,1]` (0 = just sprouted, 1 = mature). Wraps via
    /// reseed in the model (mature tufts drop seed → new growth), but here we
    /// just saturate at 1 for simplicity.
    pub growth: f32,
    /// Wind-sway phase in radians `[0, 2π)`. Advances with wind each tick;
    /// fast-forwarded modulo 2π for the back band so it stays plausible.
    pub sway: f32,
}

impl FieldCell {
    pub fn new(density: f32, growth: f32) -> FieldCell {
        FieldCell { density: clamp01(density), growth: clamp01(growth), sway: 0.0 }
    }
}

/// Per-cell rates for the field propagation. Tunable per biome/species.
#[derive(Clone, Copy, Debug)]
pub struct GrassFieldParams {
    /// Growth gained per tick (added to `growth`, saturating at 1).
    pub growth_per_tick: f32,
    /// Fraction of a cell's density that diffuses to its 4 neighbours per tick
    /// (grass spreading into bare tiles). `[0, ~0.25]` keeps it stable.
    pub spread_per_tick: f32,
    /// Sway phase advance per tick at unit wind (radians).
    pub sway_per_tick: f32,
}

impl Default for GrassFieldParams {
    fn default() -> Self {
        GrassFieldParams { growth_per_tick: 0.02, spread_per_tick: 0.04, sway_per_tick: 0.20 }
    }
}

/// A coarse density/growth/sway field over a `w × h` tile grid covering a
/// grass patch. Row-major.
#[derive(Clone, Debug)]
pub struct GrassField {
    pub w: usize,
    pub h: usize,
    pub cells: Vec<FieldCell>,
}

impl GrassField {
    pub fn new(w: usize, h: usize, fill: FieldCell) -> GrassField {
        GrassField { w, h, cells: vec![fill; w * h] }
    }

    /// Total blade-equivalent mass (Σ density) — a cheap aggregate metric.
    pub fn mass(&self) -> f32 {
        self.cells.iter().map(|c| c.density).sum()
    }

    /// Mean growth across covered cells (density-weighted). 0 if empty.
    pub fn mean_growth(&self) -> f32 {
        let (mut num, mut den) = (0.0f32, 0.0f32);
        for c in &self.cells {
            num += c.growth * c.density;
            den += c.density;
        }
        if den > 0.0 { num / den } else { 0.0 }
    }

    /// **Field propagation — fast-forward the back band by `n` ticks in one
    /// pass.** Equivalent (to within the field abstraction) to running `n`
    /// per-tick updates, but computed as an aggregate:
    /// - growth advances by `n · growth_per_tick` (saturating);
    /// - density diffuses to 4-neighbours by `n · spread_per_tick`, conserving
    ///   total mass except where it saturates a neighbour at 1 (grass can't
    ///   over-pack a tile) — so mass is non-decreasing toward the cap, like
    ///   real spreading filling bare ground;
    /// - sway advances by `n · sway_per_tick · wind`, wrapped to `[0, 2π)`.
    ///
    /// `wind` scales the sway advance. `n == 0` is a no-op.
    pub fn fast_forward(&mut self, n: u32, wind: f32, p: &GrassFieldParams) {
        if n == 0 {
            return;
        }
        let nf = n as f32;
        // growth + sway are per-cell, independent → close-form for n ticks.
        let dgrowth = nf * p.growth_per_tick;
        let dsway = nf * p.sway_per_tick * wind.max(0.0);
        for c in self.cells.iter_mut() {
            c.growth = clamp01(c.growth + dgrowth);
            c.sway = wrap_tau(c.sway + dsway);
        }
        // spreading diffuses across cells → do it as one diffusion of strength
        // `min(n·spread, 0.5)` (cap keeps a single aggregate step stable even
        // for large n; the cap is the field analogue of leaf_decay's saturating
        // catch-up — a long-skipped patch spreads to "mostly full", not beyond).
        let s = (nf * p.spread_per_tick).min(0.5);
        if s > 0.0 {
            self.diffuse_density(s);
        }
    }

    /// One high-resolution tick of the **front band**: full per-tick update
    /// (growth + sway + a single-strength spread). This is what the player
    /// sees up close; identical to `fast_forward(1, …)` but named for intent
    /// and kept exact (no cap) so front detail is precise.
    pub fn tick_front(&mut self, wind: f32, p: &GrassFieldParams) {
        for c in self.cells.iter_mut() {
            c.growth = clamp01(c.growth + p.growth_per_tick);
            c.sway = wrap_tau(c.sway + p.sway_per_tick * wind.max(0.0));
        }
        if p.spread_per_tick > 0.0 {
            self.diffuse_density(p.spread_per_tick);
        }
    }

    /// Diffuse density to 4-neighbours by strength `s` (each cell gives `s/4`
    /// of its density to each neighbour; neighbours saturate at 1). Mass is
    /// conserved except for the saturation clip (spreading into full tiles is
    /// lost, as in reality). Two-buffer so it's order-independent.
    fn diffuse_density(&mut self, s: f32) {
        let (w, h) = (self.w, self.h);
        let mut out: Vec<f32> = self.cells.iter().map(|c| c.density).collect();
        let give = s * 0.25;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let d = self.cells[i].density;
                if d <= 0.0 {
                    continue;
                }
                for (nx, ny) in neigh4(x, y, w, h) {
                    let j = ny * w + nx;
                    let moved = give * d;
                    let room = (1.0 - out[j]).max(0.0);
                    let actually = moved.min(room);
                    out[j] += actually;
                    out[i] -= actually;
                }
            }
        }
        for (c, &nd) in self.cells.iter_mut().zip(out.iter()) {
            c.density = clamp01(nd);
        }
    }
}

/// The split decision: given a chunk's band, how to treat its grass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrassWork {
    /// Front: high-res per-blade ticking (the band the player sees up close).
    HighResFront,
    /// Back: fast-forward the field by `catch_up_ticks` in one propagation
    /// pass instead of per-blade ticking.
    FieldFastForward { catch_up_ticks: u32 },
    /// Out of range entirely — do nothing (true culling still applies past the
    /// sim band).
    Dormant,
}

/// Decide how to process a grass patch this tick from its chunk band and the
/// gap since it was last updated. Reuses the engine's existing band classifier
/// so grass LOD lines up with the rest of the streaming bands:
/// - `Render` band → high-res front;
/// - `Simulation` band → field fast-forward over the skipped gap;
/// - `CatchUp`/beyond → dormant.
///
/// `ticks_since_update` is the gap the field must fast-forward (0 → treat as 1
/// so a freshly-entered sim chunk still advances once).
pub fn band_plan(band: ChunkTickBand, ticks_since_update: u32, _p: &TickScheduleParams) -> GrassWork {
    match band {
        ChunkTickBand::Render => GrassWork::HighResFront,
        ChunkTickBand::Simulation => {
            GrassWork::FieldFastForward { catch_up_ticks: ticks_since_update.max(1) }
        }
        ChunkTickBand::CatchUp => GrassWork::Dormant,
    }
}

// ── helpers ─────────────────────────────────────────────────────────────────

const TAU: f32 = std::f32::consts::TAU;

fn wrap_tau(x: f32) -> f32 {
    let m = x % TAU;
    if m < 0.0 {
        m + TAU
    } else {
        m
    }
}

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
    fn fast_forward_grows_and_saturates() {
        let mut f = GrassField::new(3, 3, FieldCell::new(0.5, 0.0));
        let p = GrassFieldParams::default();
        // 10 ticks → growth += 10*0.02 = 0.2
        f.fast_forward(10, 0.0, &p);
        assert!(approx(f.mean_growth(), 0.2), "growth after 10 ticks = {}", f.mean_growth());
        // a very long skip saturates growth at 1 (doesn't overflow)
        f.fast_forward(10_000, 0.0, &p);
        assert!(approx(f.mean_growth(), 1.0));
    }

    #[test]
    fn fast_forward_matches_repeated_front_ticks_for_growth_and_sway() {
        // For the independent per-cell channels (growth, sway), one
        // fast_forward(n) must equal n single front ticks — the catch-up is
        // exact there (spreading is the only capped/aggregated channel).
        let p = GrassFieldParams { spread_per_tick: 0.0, ..GrassFieldParams::default() };
        let mut batched = GrassField::new(2, 2, FieldCell::new(0.7, 0.1));
        let mut stepped = batched.clone();
        let n = 25u32;
        batched.fast_forward(n, 0.6, &p);
        for _ in 0..n {
            stepped.tick_front(0.6, &p);
        }
        for (a, b) in batched.cells.iter().zip(stepped.cells.iter()) {
            assert!(approx(a.growth, b.growth), "growth {} vs {}", a.growth, b.growth);
            assert!(approx(a.sway, b.sway), "sway {} vs {}", a.sway, b.sway);
        }
    }

    #[test]
    fn spreading_fills_bare_neighbours_conserving_into_cap() {
        // one full cell surrounded by bare ground → density spreads out, total
        // mass is conserved (nothing saturates yet at low strength).
        let mut f = GrassField::new(3, 3, FieldCell::new(0.0, 0.0));
        f.cells[4] = FieldCell::new(1.0, 0.5); // centre full
        let before = f.mass();
        let p = GrassFieldParams { growth_per_tick: 0.0, sway_per_tick: 0.0, spread_per_tick: 0.1 };
        f.tick_front(0.0, &p);
        // neighbours of centre gained density
        assert!(f.cells[1].density > 0.0 && f.cells[3].density > 0.0);
        // mass conserved (no neighbour hit the cap at this strength)
        assert!(approx(f.mass(), before), "mass {} vs {}", f.mass(), before);
    }

    #[test]
    fn sway_wraps_into_range() {
        let mut f = GrassField::new(1, 1, FieldCell::new(1.0, 0.0));
        let p = GrassFieldParams { sway_per_tick: 1.0, growth_per_tick: 0.0, spread_per_tick: 0.0 };
        f.fast_forward(1000, 1.0, &p); // huge phase advance
        let s = f.cells[0].sway;
        assert!((0.0..TAU).contains(&s), "sway {s} out of [0,2π)");
    }

    #[test]
    fn band_plan_splits_front_back_dormant() {
        let p = TickScheduleParams::default();
        assert_eq!(band_plan(ChunkTickBand::Render, 0, &p), GrassWork::HighResFront);
        assert_eq!(
            band_plan(ChunkTickBand::Simulation, 7, &p),
            GrassWork::FieldFastForward { catch_up_ticks: 7 }
        );
        // a sim chunk with no recorded gap still advances once
        assert_eq!(
            band_plan(ChunkTickBand::Simulation, 0, &p),
            GrassWork::FieldFastForward { catch_up_ticks: 1 }
        );
        assert_eq!(band_plan(ChunkTickBand::CatchUp, 99, &p), GrassWork::Dormant);
    }

    #[test]
    fn empty_field_is_stable() {
        let mut f = GrassField::new(4, 4, FieldCell::new(0.0, 0.0));
        let p = GrassFieldParams::default();
        f.fast_forward(50, 1.0, &p);
        assert!(approx(f.mass(), 0.0), "bare field stays bare");
    }
}
