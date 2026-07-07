//! `sim::mob_spawn_director` — decide which mobs to place, where, and how many.
//!
//! The missing piece that actually populates the overworld: per tick the bin
//! gathers context (biome, time of day, current per-kind population, a seed, the
//! player position) and a spawn budget, and this returns a list of
//! [`SpawnRequest`]s. Placement + persistence already exist
//! (`spawn.rs`, `world::mob_persistence`); this only *decides*.
//!
//! Composition (grounded):
//! - kinds are `crate::item::CreatureKind` (`Dragon, Lauscher, Spaeher, Fauna,
//!   Dinosaur`; `CreatureKind::ALL`); per-kind arrays are indexed by
//!   `kind as usize` (= `ALL` order, `Dragon = 0 .. Dinosaur = 4`).
//! - biome is `crate::world::biomes::Biome` (its `is_forest`/`is_cave_biome`/
//!   `is_canyon` helpers drive the weights).
//! - the per-kind cap clamp reuses
//!   `crate::sim::batch_spawn_budget::allowed_one` (= `min(pending, budget,
//!   cap - active)`), so this director is bit-consistent with the existing
//!   spawn-budget rule.
//!
//! Pure and deterministic: data in, data out. No render/Vulkan/winit, no IO, no
//! wall-clock, RNG is a seeded LCG only; no `.unwrap()`/`.expect()`.

use crate::item::CreatureKind;
use crate::sim::batch_spawn_budget::allowed_one;
use crate::world::biomes::Biome;

/// Number of creature kinds (mirrors `CreatureKind::ALL.len()`).
pub const KINDS: usize = 5;

/// Per-tick context for the spawn director. Per-kind arrays are indexed by
/// `CreatureKind as usize` (Dragon = 0 .. Dinosaur = 4).
#[derive(Clone, Copy, Debug)]
pub struct SpawnCtx {
    /// Deterministic seed (e.g. `world_seed ^ tick`). Same ctx ⇒ same plan.
    pub seed: u64,
    /// Biome at the spawn center.
    pub biome: Biome,
    /// Time of day in `[0,1]`: `0`/`1` = midnight, `0.5` = noon.
    pub time_of_day01: f32,
    /// Center to spawn around (e.g. the loaded-region / player anchor).
    pub center: [f32; 3],
    /// Player position — nothing spawns within `no_spawn_radius` of it.
    pub player_pos: [f32; 3],
    /// No-spawn bubble radius around `player_pos` (m).
    pub no_spawn_radius: f32,
    /// Max distance from `center` to place a spawn (m).
    pub spawn_radius: f32,
    /// Current active population per kind.
    pub active_counts: [u32; KINDS],
    /// Density cap (max active) per kind.
    pub caps: [u32; KINDS],
}

/// One spawn group: `count` of `kind` near `position` (already outside the
/// no-spawn radius and inside the spawn radius). `count >= 1`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpawnRequest {
    pub kind: CreatureKind,
    pub position: [f32; 3],
    pub count: u32,
}

// ── seeded LCG (deterministic; no real RNG) ─────────────────────────────────
struct Lcg(u64);
impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed ^ 0x9E37_79B9_7F4A_7C15)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    /// Uniform f32 in `[0,1)`.
    fn unit(&mut self) -> f32 {
        (self.next_u32() as f32) / 4_294_967_296.0
    }
}

#[inline]
fn fin(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Diurnal activity factor in `[0.25, 1.0]`, peaking at noon (`tod = 0.5`).
fn diurnal(tod01: f32) -> f32 {
    let t = fin(tod01).rem_euclid(1.0);
    let day = (core::f32::consts::PI * t).sin().max(0.0); // 0 at midnight, 1 at noon
    0.25 + 0.75 * day
}

fn is_highland(b: Biome) -> bool {
    matches!(
        b,
        Biome::TemperateHighlands
            | Biome::AlpineHighlands
            | Biome::MistyMountains
            | Biome::Skylands
    )
}
fn is_open(b: Biome) -> bool {
    matches!(
        b,
        Biome::Plains
            | Biome::Desert
            | Biome::RedSandstoneCanyon
            | Biome::BloomingCanyon
            | Biome::Beach
    ) || is_highland(b)
}
fn is_warm_open(b: Biome) -> bool {
    matches!(
        b,
        Biome::Desert | Biome::RedSandstoneCanyon | Biome::BloomingCanyon | Biome::Plains
    )
}

/// Spawn weight for `(biome, kind)` — a TUNABLE policy. Cave biomes and open
/// ocean get no surface mobs. Grounded in the `CreatureKind` doc comments
/// (Dinosaur "warm, open lands"; Fauna "everyday surface animals"; Späher
/// "winged sentinel"; Lauscher "air-attuned"; Dragon rare). `0` ⇒ never here.
fn base_weight(b: Biome, k: CreatureKind) -> f32 {
    if b.is_cave_biome() || b == Biome::Ocean {
        return 0.0; // surface director only
    }
    match k {
        CreatureKind::Fauna => match b {
            Biome::Desert | Biome::Tundra | Biome::RedSandstoneCanyon => 0.5,
            Biome::Beach => 0.7,
            _ => 1.0, // common everywhere on land
        },
        CreatureKind::Spaeher => {
            if is_open(b) {
                0.5
            } else if b.is_forest() {
                0.3
            } else {
                0.2
            }
        }
        CreatureKind::Lauscher => {
            if b.is_forest() || is_highland(b) {
                0.4
            } else {
                0.15
            }
        }
        CreatureKind::Dragon => {
            if b.is_forest() || is_highland(b) {
                0.08 // rare
            } else {
                0.03
            }
        }
        CreatureKind::Dinosaur => {
            if is_warm_open(b) {
                0.4
            } else {
                0.0
            }
        }
    }
}

/// Time-of-day multiplier per kind. Birds/megafauna are diurnal; the
/// air-listener and the dragon are roughly flat.
fn tod_factor(k: CreatureKind, tod01: f32) -> f32 {
    match k {
        CreatureKind::Fauna | CreatureKind::Spaeher | CreatureKind::Dinosaur => diurnal(tod01),
        CreatureKind::Lauscher | CreatureKind::Dragon => 1.0,
    }
}

/// Apportion `budget` across kinds proportional to `weight`, never exceeding
/// `headroom[k]` for any kind nor `budget` in total. Capped largest-remainder
/// (deterministic; ties break toward the lower kind index).
fn apportion(weights: &[f32; KINDS], headroom: &[u32; KINDS], budget: u32) -> [u32; KINDS] {
    let mut out = [0u32; KINDS];
    let total_w: f32 = (0..KINDS)
        .filter(|&k| headroom[k] > 0)
        .map(|k| weights[k].max(0.0))
        .sum();
    if budget == 0 || total_w <= 0.0 {
        return out;
    }

    // floor allocation by ideal share, capped at headroom.
    let mut frac = [0f32; KINDS];
    let mut used = 0u32;
    for k in 0..KINDS {
        if headroom[k] == 0 || weights[k] <= 0.0 {
            continue;
        }
        let ideal = budget as f32 * (weights[k].max(0.0) / total_w);
        let mut base = ideal.floor() as u32;
        if base > headroom[k] {
            base = headroom[k];
        }
        out[k] = base;
        frac[k] = ideal - ideal.floor();
        used += base;
    }

    // distribute the remaining units to kinds with leftover headroom, by
    // largest fractional part then lowest index. Loop until exhausted.
    let mut remaining = budget.saturating_sub(used);
    while remaining > 0 {
        let mut best = KINDS; // sentinel
        let mut best_frac = -1.0f32;
        for k in 0..KINDS {
            if weights[k] <= 0.0 || out[k] >= headroom[k] {
                continue;
            }
            if frac[k] > best_frac + 1e-9 {
                best_frac = frac[k];
                best = k;
            }
        }
        if best == KINDS {
            break; // no kind can take more
        }
        out[best] += 1;
        frac[best] = -1.0; // give each kind at most one "remainder" unit per pass priority
        remaining -= 1;
        // if everyone has been given a remainder unit but budget remains and
        // headroom remains, reset fracs to 0 so we keep filling round-robin.
        if remaining > 0 && (0..KINDS).all(|k| frac[k] < 0.0 || out[k] >= headroom[k] || weights[k] <= 0.0) {
            for k in 0..KINDS {
                if out[k] < headroom[k] && weights[k] > 0.0 {
                    frac[k] = 0.0;
                }
            }
        }
    }
    out
}

/// Plan this tick's spawns. Guarantees (see tests): total `count` ≤ `budget`;
/// per-kind `count` ≤ `caps[k] - active_counts[k]`; every `position` is outside
/// `no_spawn_radius` of `player_pos` and within `spawn_radius` of `center`;
/// deterministic in `seed`.
pub fn plan_spawns(ctx: SpawnCtx, budget: u32) -> Vec<SpawnRequest> {
    // weights and headroom per kind.
    let mut weights = [0f32; KINDS];
    let mut headroom = [0u32; KINDS];
    for &k in CreatureKind::ALL.iter() {
        let i = k as usize;
        weights[i] = base_weight(ctx.biome, k) * tod_factor(k, ctx.time_of_day01);
        headroom[i] = ctx.caps[i].saturating_sub(ctx.active_counts[i]);
    }

    let alloc = apportion(&weights, &headroom, budget);

    let mut out: Vec<SpawnRequest> = Vec::new();
    let mut rng = Lcg::new(ctx.seed);
    let spawn_r = fin(ctx.spawn_radius).max(0.0);
    let no_spawn_r = fin(ctx.no_spawn_radius).max(0.0);

    for &k in CreatureKind::ALL.iter() {
        let i = k as usize;
        // Final safety clamp through the shared spawn-budget rule.
        let count = allowed_one(alloc[i], budget, ctx.active_counts[i], ctx.caps[i]);
        if count == 0 {
            continue;
        }

        // Find a position within spawn_radius of center but outside the
        // player's no-spawn bubble. Up to 8 seeded candidates; skip the kind
        // if none qualifies (never place inside the bubble).
        let mut placed: Option<[f32; 3]> = None;
        for _ in 0..8 {
            let ang = rng.unit() * core::f32::consts::TAU;
            // sqrt for area-uniform radius within the disc
            let r = spawn_r * rng.unit().sqrt();
            let pos = [
                fin(ctx.center[0]) + ang.cos() * r,
                fin(ctx.center[1]),
                fin(ctx.center[2]) + ang.sin() * r,
            ];
            let dx = pos[0] - fin(ctx.player_pos[0]);
            let dz = pos[2] - fin(ctx.player_pos[2]);
            let dy = pos[1] - fin(ctx.player_pos[1]);
            let d2 = dx * dx + dy * dy + dz * dz;
            if d2 >= no_spawn_r * no_spawn_r {
                placed = Some(pos);
                break;
            }
        }
        if let Some(pos) = placed {
            out.push(SpawnRequest { kind: k, position: pos, count });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(biome: Biome, tod: f32, active: [u32; KINDS], caps: [u32; KINDS]) -> SpawnCtx {
        SpawnCtx {
            seed: 0xABCD_1234,
            biome,
            time_of_day01: tod,
            center: [100.0, 64.0, 100.0],
            player_pos: [100.0, 64.0, 100.0],
            no_spawn_radius: 12.0,
            spawn_radius: 80.0,
            active_counts: active,
            caps,
        }
    }

    fn total(reqs: &[SpawnRequest]) -> u32 {
        reqs.iter().map(|r| r.count).sum()
    }
    fn per_kind(reqs: &[SpawnRequest], k: CreatureKind) -> u32 {
        reqs.iter().filter(|r| r.kind == k).map(|r| r.count).sum()
    }

    #[test]
    fn budget_never_exceeded() {
        for budget in [0u32, 1, 3, 10, 50, 1000] {
            let c = ctx(Biome::Plains, 0.5, [0; KINDS], [100; KINDS]);
            let reqs = plan_spawns(c, budget);
            assert!(total(&reqs) <= budget, "budget {} exceeded: {}", budget, total(&reqs));
        }
    }

    #[test]
    fn density_caps_respected() {
        let caps = [2u32, 1, 3, 5, 0];
        let active = [1u32, 0, 3, 2, 0];
        // headroom = [1,1,0,3,0]
        let c = ctx(Biome::Plains, 0.5, active, caps);
        let reqs = plan_spawns(c, 1000);
        for &k in CreatureKind::ALL.iter() {
            let i = k as usize;
            let headroom = caps[i] - active[i];
            assert!(per_kind(&reqs, k) <= headroom, "{:?} exceeded headroom {}", k, headroom);
        }
        // capped/no-headroom kinds never appear
        assert_eq!(per_kind(&reqs, CreatureKind::Spaeher), 0); // headroom 0
        assert_eq!(per_kind(&reqs, CreatureKind::Dinosaur), 0); // cap 0
    }

    #[test]
    fn nothing_spawns_inside_no_spawn_radius() {
        // center == player, so candidates near center must be rejected.
        let c = ctx(Biome::Plains, 0.5, [0; KINDS], [50; KINDS]);
        let reqs = plan_spawns(c, 200);
        assert!(!reqs.is_empty());
        for r in &reqs {
            let dx = r.position[0] - c.player_pos[0];
            let dz = r.position[2] - c.player_pos[2];
            let dy = r.position[1] - c.player_pos[1];
            let d = (dx * dx + dy * dy + dz * dz).sqrt();
            assert!(d >= c.no_spawn_radius - 1e-3, "spawn at d={} inside no-spawn {}", d, c.no_spawn_radius);
            // and within spawn radius of center
            let cx = r.position[0] - c.center[0];
            let cz = r.position[2] - c.center[2];
            assert!((cx * cx + cz * cz).sqrt() <= c.spawn_radius + 1e-3);
        }
    }

    #[test]
    fn deterministic() {
        let c = ctx(Biome::TemperateForest, 0.3, [0; KINDS], [40; KINDS]);
        let a = plan_spawns(c, 37);
        let b = plan_spawns(c, 37);
        assert_eq!(a, b);
    }

    #[test]
    fn cave_and_ocean_get_no_surface_mobs() {
        for biome in [Biome::FungalCave, Biome::IceCave, Biome::ThermalCave, Biome::Ocean] {
            let c = ctx(biome, 0.5, [0; KINDS], [100; KINDS]);
            assert!(plan_spawns(c, 100).is_empty(), "{:?} produced surface mobs", biome);
        }
    }

    #[test]
    fn dinosaur_only_in_warm_open() {
        let warm = ctx(Biome::Desert, 0.5, [0; KINDS], [50; KINDS]);
        assert!(per_kind(&plan_spawns(warm, 200), CreatureKind::Dinosaur) > 0);
        let cold = ctx(Biome::Tundra, 0.5, [0; KINDS], [50; KINDS]);
        assert_eq!(per_kind(&plan_spawns(cold, 200), CreatureKind::Dinosaur), 0);
    }

    #[test]
    fn zero_budget_or_full_population_yields_nothing() {
        let c0 = ctx(Biome::Plains, 0.5, [0; KINDS], [50; KINDS]);
        assert!(plan_spawns(c0, 0).is_empty());
        let full = ctx(Biome::Plains, 0.5, [50; KINDS], [50; KINDS]); // headroom 0 everywhere
        assert!(plan_spawns(full, 100).is_empty());
    }

    #[test]
    fn apportion_caps_and_budget_hold_over_sweep() {
        // deterministic seeded sweep (R20)
        let mut x: u64 = 0x5A11_0076;
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        let biomes = [
            Biome::Plains, Biome::TemperateForest, Biome::TemperateHighlands,
            Biome::BirchForest, Biome::MapleForest, Biome::CloudForest,
            Biome::MistyMountains, Biome::AlpineHighlands, Biome::Desert,
            Biome::RedSandstoneCanyon, Biome::BloomingCanyon, Biome::Ocean,
            Biome::Beach, Biome::Swamp, Biome::Tundra, Biome::Skylands,
            Biome::IceCave, Biome::FungalCave, Biome::ThermalCave,
        ];
        for _ in 0..3000 {
            let biome = biomes[(next() as usize) % biomes.len()];
            let tod = (next() % 1000) as f32 / 1000.0;
            let mut active = [0u32; KINDS];
            let mut caps = [0u32; KINDS];
            for k in 0..KINDS {
                caps[k] = next() % 12;
                active[k] = next() % 12;
            }
            let budget = next() % 30;
            let c = SpawnCtx {
                seed: (next() as u64) << 16 | next() as u64,
                biome, time_of_day01: tod,
                center: [0.0, 64.0, 0.0], player_pos: [0.0, 64.0, 0.0],
                no_spawn_radius: 10.0, spawn_radius: 64.0,
                active_counts: active, caps,
            };
            let reqs = plan_spawns(c, budget);
            assert!(total(&reqs) <= budget);
            for k in 0..KINDS {
                let headroom = caps[k].saturating_sub(active[k]);
                assert!(per_kind(&reqs, CreatureKind::ALL[k]) <= headroom);
            }
            for r in &reqs {
                let d = (r.position[0] * r.position[0] + r.position[2] * r.position[2]).sqrt();
                assert!(d >= 10.0 - 1e-3 && d <= 64.0 + 1e-3);
                assert!(r.count >= 1);
            }
        }
    }
}
