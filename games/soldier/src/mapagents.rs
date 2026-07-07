//! mapagents.rs — **agent-based territorial map generation** (design abstraction #4). Instead of authoring
//! terrain top-down, drop autonomous agents on a blank grid and let them shape it by pursuing their instincts;
//! the map is whatever their tug-of-war leaves behind:
//!   • **Sniper** climbs toward the highest ground and raises it further → vantage hills grow where sightlines
//!     are longest.
//!   • **Rusher** runs the straightest line between the two spawns and flattens whatever it crosses → fast,
//!     level lanes get cut spawn-to-spawn.
//!   • **Camper** settles in quiet, untrafficked corners and piles cover around itself → defensible pockets
//!     form away from the through-routes.
//!
//! The agents compete for the same ground (a Rusher will flatten a hill a Sniper is trying to raise), so the
//! result is emergent rather than dictated. It's pure, deterministic and cheap — thousands of territories can
//! be simulated and the good ones curated, exactly like the graph factory. The output is an elevation field +
//! a cover field that a carve step can skin into a real `Level`.

use crate::level::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentKind {
    Sniper,
    Rusher,
    Camper,
}

#[derive(Clone, Copy)]
enum StampOp {
    Raise,
    Flatten,
    Traffic,
    Cover,
}

#[derive(Clone, Copy, Debug)]
pub struct Agent {
    pub kind: AgentKind,
    pub x: f32,
    pub z: f32,
    /// Rusher target: which spawn it's currently running toward (0 or 1).
    target: usize,
}

/// The emergent battleground: elevation, cover density and crossing-traffic fields over a grid, plus the two
/// spawn points the Rushers shuttle between.
#[derive(Clone, Debug)]
pub struct Territory {
    pub w: usize,
    pub h: usize,
    pub height: Vec<f32>,
    pub cover: Vec<f32>,
    pub traffic: Vec<f32>,
    pub spawns: [(f32, f32); 2],
    agents: Vec<Agent>,
}

impl Territory {
    #[inline]
    fn idx(&self, x: i32, z: i32) -> Option<usize> {
        if x < 0 || z < 0 || x >= self.w as i32 || z >= self.h as i32 {
            None
        } else {
            Some(z as usize * self.w + x as usize)
        }
    }

    #[inline]
    fn at(&self, field: &[f32], x: f32, z: f32) -> f32 {
        self.idx(x as i32, z as i32).map(|i| field[i]).unwrap_or(0.0)
    }

    /// Run a territorial simulation to completion and return the emergent fields. `n_agents` is split across
    /// the three instincts; `steps` is how long they fight over the ground.
    pub fn simulate(w: usize, h: usize, seed: u64, n_agents: usize, steps: usize) -> Territory {
        let mut rng = Rng::new(seed ^ 0xA5A5_1234);
        let spawns = [(w as f32 * 0.12, h as f32 * 0.12), (w as f32 * 0.88, h as f32 * 0.88)];
        let mut t = Territory {
            w,
            h,
            height: vec![0.0; w * h],
            cover: vec![0.0; w * h],
            traffic: vec![0.0; w * h],
            spawns,
            agents: Vec::with_capacity(n_agents),
        };
        // a faint initial height noise gives the snipers something to climb toward
        for i in 0..w * h {
            t.height[i] = (rng.f32() - 0.5) * 0.3;
        }
        // populate agents: a balanced mix of the three instincts
        for k in 0..n_agents {
            let kind = match k % 3 {
                0 => AgentKind::Sniper,
                1 => AgentKind::Rusher,
                _ => AgentKind::Camper,
            };
            t.agents.push(Agent {
                kind,
                x: rng.f32() * w as f32,
                z: rng.f32() * h as f32,
                target: k % 2,
            });
        }
        for _ in 0..steps {
            t.step(&mut rng);
        }
        // settle: smooth the height field a touch so cliffs are traversable, then a final clamp
        t.relax_height(2);
        t
    }

    fn step(&mut self, rng: &mut Rng) {
        let agents = self.agents.clone();
        for (ai, a) in agents.iter().enumerate() {
            let mut a = *a;
            match a.kind {
                AgentKind::Sniper => {
                    // climb toward the highest neighbour, then raise where we stand → hills grow at high points
                    let (mut bx, mut bz, mut bh) = (a.x, a.z, self.at(&self.height, a.x, a.z));
                    for (dx, dz) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0), (1.0, 1.0), (-1.0, -1.0)] {
                        let (nx, nz) = (a.x + dx * 2.0, a.z + dz * 2.0);
                        let hh = self.at(&self.height, nx, nz);
                        if hh > bh {
                            bh = hh;
                            bx = nx;
                            bz = nz;
                        }
                    }
                    a.x = (a.x + (bx - a.x) * 0.5 + (rng.f32() - 0.5)).clamp(0.0, self.w as f32 - 1.0);
                    a.z = (a.z + (bz - a.z) * 0.5 + (rng.f32() - 0.5)).clamp(0.0, self.h as f32 - 1.0);
                    // broad raise → vantage plateaus you can actually stand and fight on, not needle spikes
                    self.stamp(StampOp::Raise, a.x, a.z, 4.0, 0.32);
                }
                AgentKind::Rusher => {
                    // run toward the target spawn, flattening and wearing a path as we go
                    let (tx, tz) = self.spawns[a.target];
                    let (dx, dz) = (tx - a.x, tz - a.z);
                    let d = (dx * dx + dz * dz).sqrt().max(0.1);
                    a.x = (a.x + dx / d * 1.6 + (rng.f32() - 0.5) * 0.6).clamp(0.0, self.w as f32 - 1.0);
                    a.z = (a.z + dz / d * 1.6 + (rng.f32() - 0.5) * 0.6).clamp(0.0, self.h as f32 - 1.0);
                    self.stamp(StampOp::Flatten, a.x, a.z, 1.6, 0.45);
                    self.stamp(StampOp::Traffic, a.x, a.z, 1.6, 1.0);
                    if d < 4.0 {
                        a.target ^= 1; // reached it; head back the other way
                    }
                }
                AgentKind::Camper => {
                    // drift toward quiet, low-cover ground away from the lanes, then pile cover around us
                    let (mut bx, mut bz, mut bscore) = (a.x, a.z, f32::MAX);
                    for _ in 0..5 {
                        let nx = (a.x + (rng.f32() - 0.5) * 8.0).clamp(0.0, self.w as f32 - 1.0);
                        let nz = (a.z + (rng.f32() - 0.5) * 8.0).clamp(0.0, self.h as f32 - 1.0);
                        let s = self.at(&self.traffic, nx, nz) + self.at(&self.cover, nx, nz) * 0.5;
                        if s < bscore {
                            bscore = s;
                            bx = nx;
                            bz = nz;
                        }
                    }
                    a.x = a.x + (bx - a.x) * 0.4;
                    a.z = a.z + (bz - a.z) * 0.4;
                    self.stamp(StampOp::Cover, a.x, a.z, 2.5, 0.4);
                }
            }
            self.agents[ai] = a;
        }
        // gentle global decay so features stay bounded as the fight continues
        for i in 0..self.w * self.h {
            self.traffic[i] *= 0.985;
            self.height[i] = self.height[i].clamp(-3.0, 3.2);
        }
    }

    /// Apply a stamp operation with a falloff disc around (cx, cz) to the appropriate field.
    fn stamp(&mut self, op: StampOp, cx: f32, cz: f32, radius: f32, amt: f32) {
        let r = radius.ceil() as i32;
        let (ix, iz) = (cx as i32, cz as i32);
        for dz in -r..=r {
            for dx in -r..=r {
                if let Some(i) = self.idx(ix + dx, iz + dz) {
                    let d = ((dx * dx + dz * dz) as f32).sqrt() / radius;
                    if d > 1.0 {
                        continue;
                    }
                    let a = amt * (1.0 - d);
                    match op {
                        StampOp::Raise => self.height[i] = (self.height[i] + a).min(3.2),
                        StampOp::Flatten => self.height[i] *= 1.0 - a,
                        StampOp::Traffic => self.traffic[i] += a,
                        StampOp::Cover => self.cover[i] = (self.cover[i] + a).min(4.0),
                    }
                }
            }
        }
    }

    fn relax_height(&mut self, iters: usize) {
        for _ in 0..iters {
            let mut next = self.height.clone();
            for z in 1..self.h - 1 {
                for x in 1..self.w - 1 {
                    let i = z * self.w + x;
                    let s = self.height[i - 1] + self.height[i + 1] + self.height[i - self.w] + self.height[i + self.w];
                    next[i] = self.height[i] * 0.5 + (s / 4.0) * 0.5;
                }
            }
            self.height = next;
        }
    }
}

/// A scorecard for an emergent territory — does the agents' tug-of-war actually produce good terrain? Mirrors
/// the spirit of the graph/map harnesses: reward vantage relief, a clear lane, and distinct cover pockets.
#[derive(Clone, Copy, Debug)]
pub struct TerritoryScore {
    pub relief: f32,     // height variation (vantages vs lowlands)
    pub lane: f32,       // a strong, level through-route exists
    pub pockets: f32,    // cover clusters formed off the lane
    pub total: f32,      // 0..100
}

pub fn score(t: &Territory) -> TerritoryScore {
    let n = (t.w * t.h) as f32;
    // relief: spread of the height field
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for &v in &t.height {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let relief = ((hi - lo) / 5.0).clamp(0.0, 1.0);

    // lane: peak traffic along the diagonal between spawns relative to the field — a worn through-route
    let max_traffic = t.traffic.iter().cloned().fold(0.0f32, f32::max);
    let lane = (max_traffic / 12.0).clamp(0.0, 1.0);

    // pockets: how many cells crossed the "this is a cover pocket" threshold, away from heavy traffic
    let mut pocket_cells = 0;
    for i in 0..t.w * t.h {
        if t.cover[i] > 1.5 && t.traffic[i] < 2.0 {
            pocket_cells += 1;
        }
    }
    let pockets = (pocket_cells as f32 / (n * 0.04)).clamp(0.0, 1.0);

    let total = (relief * 0.4 + lane * 0.35 + pockets * 0.25) * 100.0;
    TerritoryScore { relief, lane, pockets, total }
}

/// Mass-simulate territories and rank them best-first — the agent factory (parallel to the graph factory).
pub fn mass_rank(n: usize, w: usize, h: usize, base_seed: u64) -> Vec<(u64, TerritoryScore)> {
    let mut out: Vec<(u64, TerritoryScore)> = (0..n)
        .map(|i| {
            let seed = base_seed.wrapping_add(i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            let t = Territory::simulate(w, h, seed, 18, 120);
            (seed, score(&t))
        })
        .collect();
    out.sort_by(|a, b| b.1.total.partial_cmp(&a.1.total).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_carve_lanes_raise_hills_and_build_pockets() {
        let t = Territory::simulate(48, 48, 7, 18, 140);
        // snipers grew vantage relief
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for &v in &t.height {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(hi - lo > 1.5, "snipers should raise real vantage relief, got {:.2}", hi - lo);
        // rushers wore a through-lane (some cells carry heavy traffic)
        let max_traffic = t.traffic.iter().cloned().fold(0.0f32, f32::max);
        assert!(max_traffic > 4.0, "rushers should wear a clear lane, peak traffic {:.1}", max_traffic);
        // campers built at least one cover pocket
        assert!(t.cover.iter().any(|&c| c > 1.5), "campers should pile cover into pockets");
    }

    #[test]
    fn simulation_is_deterministic() {
        let a = Territory::simulate(40, 40, 99, 15, 80);
        let b = Territory::simulate(40, 40, 99, 15, 80);
        assert_eq!(a.height.len(), b.height.len());
        assert!(a.height.iter().zip(b.height.iter()).all(|(x, y)| (x - y).abs() < 1e-6), "same seed → same territory");
    }

    #[test]
    fn the_agent_factory_ranks_territories() {
        let ranked = mass_rank(120, 44, 44, 3);
        assert_eq!(ranked.len(), 120);
        for w in ranked.windows(2) {
            assert!(w[0].1.total >= w[1].1.total, "factory ranking must be best-first");
        }
        assert!(ranked[0].1.total > 30.0, "the best emergent territory should be decent");
    }
}
