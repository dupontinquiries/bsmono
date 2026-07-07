//! Bioluminescent cave flora — *lumen pearls*.
//!
//! A lumen pearl is a gem grown by a cave plant that behaves like a slow
//! capacitor for light: by day it soaks up whatever sunlight reaches its cell
//! (its `exposure`, precomputed from a sky probe — pearls deep underground get
//! ~0 and stay dark), and by night it discharges that stored charge as light.
//! Crucially the pearls are *linked* (by the vines that string them together,
//! or by proximity), and a pearl that lights up ignites its neighbours one hop
//! later — so a wall charged during the day comes alive at night as travelling
//! waves of light rippling along the network, draining as they go until dawn
//! recharges them. Caves near a skylight pulse brightly; sealed ones never do.
//!
//! This is a server-side environment sim (like `water_field`/`weather`), not in
//! the rolled-back deterministic core: it's a pure function of placement +
//! exposure + time-of-day, so every host computes the same pattern. The
//! renderer reads each pearl's `emission` as a light source; binding a pearl to
//! an actual glowing `BlockId` and feeding emission into the froxel/mesher pass
//! is the render-side hookup. The link graph is supplied by the caller (the
//! vine sim is the natural source) — this field only walks it.

use crate::sim::gpu_bench::{bench, BenchResult};
use std::collections::HashSet;

/// Energy a fully sun-exposed pearl gains per tick in full daylight.
const CHARGE_RATE: f32 = 0.02;
/// Light lost per tick (emission decay after an ignition).
const DISCHARGE_RATE: f32 = 0.06;
/// Charge a pearl needs before it can ignite.
const IGNITE_THRESHOLD: f32 = 0.25;
/// Charge spent each time a pearl ignites.
const IGNITE_COST: f32 = 0.15;
/// Ticks between a pearl igniting and its neighbours catching — the wave speed.
const PROPAGATE_DELAY: i32 = 4;
/// Ticks a pearl must rest before it can ignite again (keeps the wave moving
/// forward instead of bouncing straight back).
const COOLDOWN: u16 = 20;
/// Daylight below this counts as night: the chain reaction runs.
const DAY_THRESHOLD: f32 = 0.15;
/// Emission at or below this counts as "off".
const EMIT_EPS: f32 = 0.01;

/// A single glowing pearl in the network.
#[derive(Clone, Debug, PartialEq)]
pub struct Pearl {
    /// Voxel cell the pearl occupies.
    pub cell: (i32, i32, i32),
    /// Fraction of daylight reaching this cell, 0..1 (precomputed at placement
    /// from a sky probe; deep pearls are ~0 and never charge).
    pub exposure: f32,
    /// Stored energy, 0..1.
    pub charge: f32,
    /// Current light output, 0..1 — what the renderer reads as emission.
    pub emission: f32,
    cooldown: u16,
    ignite_in: i32, // <0 idle · ==0 ignite this step · >0 ticks remaining
}

impl Pearl {
    pub fn new(cell: (i32, i32, i32), exposure: f32) -> Self {
        Self { cell, exposure: exposure.clamp(0.0, 1.0), charge: 0.0, emission: 0.0, cooldown: 0, ignite_in: -1 }
    }

    /// A pearl placed already holding charge (a wall lit at worldgen, or tests).
    pub fn charged(cell: (i32, i32, i32), exposure: f32, charge: f32) -> Self {
        Self { charge: charge.clamp(0.0, 1.0), ..Self::new(cell, exposure) }
    }
}

/// A network of pearls plus the links light travels along.
#[derive(Clone, Debug, Default)]
pub struct LumenField {
    pub pearls: Vec<Pearl>,
    adj: Vec<Vec<u32>>,
}

impl LumenField {
    /// Build from pearls and undirected links between pearl indices. Links to
    /// out-of-range or self indices are ignored.
    pub fn new(pearls: Vec<Pearl>, links: &[(u32, u32)]) -> Self {
        let mut adj = vec![Vec::new(); pearls.len()];
        for &(a, b) in links {
            let (ai, bi) = (a as usize, b as usize);
            if ai < adj.len() && bi < adj.len() && ai != bi {
                adj[ai].push(b);
                adj[bi].push(a);
            }
        }
        Self { pearls, adj }
    }

    fn eligible(&self, i: usize) -> bool {
        let p = &self.pearls[i];
        p.cooldown == 0 && p.charge >= IGNITE_THRESHOLD
    }

    /// Advance one tick with the current `daylight` (0 = night .. 1 = noon).
    /// By day exposed pearls charge; by night the charged network pulses.
    pub fn tick(&mut self, daylight: f32) {
        let night = daylight < DAY_THRESHOLD;

        // 1. charge / decay / cooldown
        for p in &mut self.pearls {
            if daylight > 0.0 {
                p.charge = (p.charge + p.exposure * daylight * CHARGE_RATE).min(1.0);
            }
            p.emission = (p.emission - DISCHARGE_RATE).max(0.0);
            if p.cooldown > 0 {
                p.cooldown -= 1;
            }
        }

        // 2. advance ignition timers; collect pearls firing this tick
        let mut ready: Vec<usize> = Vec::new();
        for (i, p) in self.pearls.iter_mut().enumerate() {
            if p.ignite_in > 0 {
                p.ignite_in -= 1;
            }
            if p.ignite_in == 0 {
                ready.push(i);
                p.ignite_in = -1;
            }
        }

        // 3. ignite, then arm eligible neighbours one hop downstream
        for &i in &ready {
            {
                let p = &mut self.pearls[i];
                p.emission = 1.0;
                p.charge = (p.charge - IGNITE_COST).max(0.0);
                p.cooldown = COOLDOWN;
            }
            let neighbours = self.adj[i].clone();
            for j in neighbours {
                let j = j as usize;
                if self.pearls[j].ignite_in < 0 && self.eligible(j) {
                    self.pearls[j].ignite_in = PROPAGATE_DELAY;
                }
            }
        }

        // 4. at night, when the cave is dark and quiescent, seed a fresh wave
        //    from the most-charged eligible pearl (ties: lowest index).
        if night {
            let emitting = self.pearls.iter().any(|p| p.emission > EMIT_EPS);
            let pending = self.pearls.iter().any(|p| p.ignite_in >= 0);
            if !emitting && !pending {
                let mut best: Option<usize> = None;
                let mut best_c = IGNITE_THRESHOLD - f32::EPSILON;
                for i in 0..self.pearls.len() {
                    let p = &self.pearls[i];
                    if p.cooldown == 0 && p.charge >= IGNITE_THRESHOLD && p.charge > best_c {
                        best = Some(i);
                        best_c = p.charge;
                    }
                }
                if let Some(i) = best {
                    self.pearls[i].ignite_in = 0;
                }
            }
        }
    }

    /// GPU-ready parallel form of [`tick`](Self::tick). Same charge/decay/chain
    /// behaviour, but expressed so it maps onto a compute shader and is
    /// order-independent: per-pearl passes, then neighbour arming computed from a
    /// post-ignition snapshot, idempotent, skipping any neighbour also igniting
    /// this tick. (This last rule is the only divergence from `tick`, which lets
    /// processing order decide that rare both-adjacent-ready case.)
    ///
    /// GPU DISPATCH CONTRACT
    /// - Pass A (one thread per pearl): charge/decay/cooldown, advance `ignite_in`;
    ///   append index to a `ready` list (atomic counter) when it hits 0.
    /// - Pass B1 (one thread per ready pearl): ignite self (emission=1, drain
    ///   charge, set cooldown).
    /// - Pass B2 (one thread per ready pearl): for each neighbour not in `ready`,
    ///   idle and eligible, `atomicMax`/store `ignite_in = PROPAGATE_DELAY`
    ///   (idempotent ⇒ order-independent).
    /// - Pass C (single reduction): if night and quiescent, seed the most-charged
    ///   eligible pearl. Output must match `tick_parallel` bit-for-bit.
    pub fn tick_parallel(&mut self, daylight: f32) {
        let night = daylight < DAY_THRESHOLD;

        // Pass A — per pearl: charge/decay/cooldown + advance timer; collect ready.
        let mut ready: Vec<usize> = Vec::new();
        for (i, p) in self.pearls.iter_mut().enumerate() {
            if daylight > 0.0 {
                p.charge = (p.charge + p.exposure * daylight * CHARGE_RATE).min(1.0);
            }
            p.emission = (p.emission - DISCHARGE_RATE).max(0.0);
            if p.cooldown > 0 {
                p.cooldown -= 1;
            }
            if p.ignite_in > 0 {
                p.ignite_in -= 1;
            }
            if p.ignite_in == 0 {
                ready.push(i);
                p.ignite_in = -1;
            }
        }

        // Pass B1 — ignite every ready pearl.
        for &i in &ready {
            let p = &mut self.pearls[i];
            p.emission = 1.0;
            p.charge = (p.charge - IGNITE_COST).max(0.0);
            p.cooldown = COOLDOWN;
        }

        // Pass B2 — arm neighbours from the post-ignition snapshot (idempotent).
        let ready_set: HashSet<usize> = ready.iter().copied().collect();
        let mut to_arm: Vec<usize> = Vec::new();
        for &i in &ready {
            let neighbours = self.adj[i].clone();
            for j in neighbours {
                let j = j as usize;
                if !ready_set.contains(&j) && self.pearls[j].ignite_in < 0 && self.eligible(j) {
                    to_arm.push(j);
                }
            }
        }
        for j in to_arm {
            self.pearls[j].ignite_in = PROPAGATE_DELAY;
        }

        // Pass C — seed a fresh wave when dark and quiescent at night.
        if night {
            let emitting = self.pearls.iter().any(|p| p.emission > EMIT_EPS);
            let pending = self.pearls.iter().any(|p| p.ignite_in >= 0);
            if !emitting && !pending {
                let mut best: Option<usize> = None;
                let mut best_c = IGNITE_THRESHOLD - f32::EPSILON;
                for i in 0..self.pearls.len() {
                    let p = &self.pearls[i];
                    if p.cooldown == 0 && p.charge >= IGNITE_THRESHOLD && p.charge > best_c {
                        best = Some(i);
                        best_c = p.charge;
                    }
                }
                if let Some(i) = best {
                    self.pearls[i].ignite_in = 0;
                }
            }
        }
    }

    /// Brightest emission anywhere in the field (debug HUD / tests).
    pub fn peak_emission(&self) -> f32 {
        self.pearls.iter().fold(0.0, |m, p| m.max(p.emission))
    }
}

/// Benchmark `ticks` of [`LumenField::tick_parallel`] from `base` at a fixed
/// `daylight`, over `iters` runs, through the shared `gpu_bench` harness.
pub fn bench_field(base: &LumenField, ticks: u32, daylight: f32, iters: usize) -> BenchResult {
    bench(iters, || {
        let mut f = base.clone();
        for _ in 0..ticks {
            f.tick_parallel(daylight);
        }
        std::hint::black_box(&f);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposure_gates_charge() {
        let mut f = LumenField::new(
            vec![Pearl::new((0, 0, 0), 1.0), Pearl::new((1, 0, 0), 0.6), Pearl::new((2, 0, 0), 0.0)],
            &[],
        );
        for _ in 0..10 {
            f.tick(1.0);
        }
        assert_eq!(f.pearls[2].charge, 0.0, "an unexposed pearl never charges");
        assert!(
            f.pearls[0].charge > f.pearls[1].charge && f.pearls[1].charge > f.pearls[2].charge,
            "charge tracks exposure: {:?}",
            f.pearls.iter().map(|p| p.charge).collect::<Vec<_>>()
        );
    }

    #[test]
    fn night_chain_travels_in_order() {
        let mut f = LumenField::new(
            (0..4).map(|i| Pearl::charged((i, 0, 0), 0.0, 1.0)).collect(),
            &[(0, 1), (1, 2), (2, 3)],
        );
        let mut first = [0i32; 4];
        for t in 1..40 {
            f.tick(0.0);
            for i in 0..4 {
                if first[i] == 0 && f.pearls[i].emission >= 1.0 {
                    first[i] = t;
                }
            }
        }
        assert!(first.iter().all(|&t| t > 0), "every pearl should fire: {:?}", first);
        for k in 0..3 {
            assert_eq!(first[k + 1] - first[k], PROPAGATE_DELAY, "wave hops evenly: {:?}", first);
        }
    }

    #[test]
    fn under_charged_stays_dark() {
        let mut f = LumenField::new(vec![Pearl::charged((0, 0, 0), 0.0, 0.1)], &[]);
        for _ in 0..60 {
            f.tick(0.0);
            assert_eq!(f.pearls[0].emission, 0.0, "below the ignite threshold it stays dark");
        }
    }

    #[test]
    fn parallel_exposure_gates_charge() {
        let mut f = LumenField::new(
            vec![Pearl::new((0, 0, 0), 1.0), Pearl::new((1, 0, 0), 0.6), Pearl::new((2, 0, 0), 0.0)],
            &[],
        );
        for _ in 0..10 {
            f.tick_parallel(1.0);
        }
        assert_eq!(f.pearls[2].charge, 0.0);
        assert!(f.pearls[0].charge > f.pearls[1].charge && f.pearls[1].charge > f.pearls[2].charge);
    }

    #[test]
    fn parallel_night_chain_travels_in_order() {
        let mut f = LumenField::new(
            (0..4).map(|i| Pearl::charged((i, 0, 0), 0.0, 1.0)).collect(),
            &[(0, 1), (1, 2), (2, 3)],
        );
        let mut first = [0i32; 4];
        for t in 1..40 {
            f.tick_parallel(0.0);
            for i in 0..4 {
                if first[i] == 0 && f.pearls[i].emission >= 1.0 {
                    first[i] = t;
                }
            }
        }
        assert!(first.iter().all(|&t| t > 0), "every pearl fires: {first:?}");
        for k in 0..3 {
            assert_eq!(first[k + 1] - first[k], PROPAGATE_DELAY, "even wave: {first:?}");
        }
    }

    #[test]
    fn parallel_matches_serial_on_a_single_front() {
        // With one travelling front (no two adjacent pearls ready at once), the
        // parallel and serial steps must agree exactly.
        let links = [(0, 1), (1, 2), (2, 3)];
        let mut a = LumenField::new((0..4).map(|i| Pearl::charged((i, 0, 0), 0.5, 1.0)).collect(), &links);
        let mut b = LumenField::new((0..4).map(|i| Pearl::charged((i, 0, 0), 0.5, 1.0)).collect(), &links);
        for _ in 0..30 {
            a.tick(0.0);
            b.tick_parallel(0.0);
        }
        for (pa, pb) in a.pearls.iter().zip(&b.pearls) {
            assert!((pa.charge - pb.charge).abs() < 1e-6 && (pa.emission - pb.emission).abs() < 1e-6);
        }
    }
}
