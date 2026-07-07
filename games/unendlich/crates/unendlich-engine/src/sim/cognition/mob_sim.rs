//! Mob perception simulator — find where the functional/stateful line pays off.
//!
//! Runs a flock of sentinels against a scripted player path (approach → linger →
//! hide behind a wall → leave) and measures two things per preset: a **cost**
//! (how many LOS/hearing ray-marches actually fire) and an **immersion** score
//! (does the creature notice you when it genuinely should, stay alert while you
//! linger, calm down when you leave, and not twitch). The perception runs as the
//! reject-cascade we discussed:
//!
//!   cadence (LOD stride) → relevance (alarmed creatures coast) →
//!   range gate → cone gate → LOS march, with a (eye,target,version)-keyed
//!   memo that reuses a ray result while nothing along it moved.
//!
//! The real `vision::line_of_sight` and `sound::heard_loudness_world` do the
//! marching, so the costs are faithful; the gates are the thing under study.
//! `sweep()` evaluates a grid of presets and ranks them by immersion-vs-cost, so
//! the tradeoff shows up as numbers. Pure + deterministic.

use crate::sim::ambient::{ambient_floor, signal_over_noise, NoiseEmitter};
use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::physics::{BlockPhysics, SceneWorld};
use crate::sim::sentinel::{Posture, Senses, Sentinel};
use crate::sim::sound::{heard_loudness_world, Loudness, SoundSource};
use crate::sim::vision::{can_see, line_of_sight};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Approach,
    Linger,
    Hide,
    Leave,
    Peek,
}

/// A tunable perception policy. The senses are fixed (bird); the cascade gates
/// are what we sweep.
#[derive(Debug, Clone)]
pub struct Preset {
    pub name: &'static str,
    /// Perceive cadence by distance band [Near, Mid, Far, VFar] (1 = every tick).
    pub strides: [u32; 4],
    /// Squared distance band edges separating Near/Mid/Far/VFar.
    pub bands2: [i64; 3],
    /// An Alarmed creature re-rays only every this-many ticks (coasts between).
    pub recheck_alarmed: u32,
    /// Reuse a ray result while the (eye, target) cells are unchanged.
    pub memo: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Counters {
    pub rays_fired: u64,
    pub elided_range: u64,
    pub elided_cone: u64,
    pub skipped_cadence: u64,
    pub skipped_relevance: u64,
    pub memo_hits: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Immersion {
    /// Reacted (≠ Calm) while you were genuinely detectable.
    pub recall: f32,
    /// How quickly it reacted after you became detectable.
    pub latency: f32,
    /// Stayed Calm once you were long gone.
    pub specificity: f32,
    /// Didn't flicker between postures while you lingered.
    pub steadiness: f32,
    /// Caught the brief far-range peeks (over-culling misses these).
    pub peek: f32,
    /// Weighted total in [0,1].
    pub total: f32,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub immersion: Immersion,
    pub counters: Counters,
    pub naive_rays: u64,
    pub cost_frac: f32,
    pub balanced: f32,
    /// Total ticks (summed over creatures) the flock was reacting (≠ Calm).
    pub detected_ticks: u32,
}

pub struct Scenario {
    pub world: SceneWorld,
    /// (position, horizontal eye facing) per creature.
    pub creatures: Vec<(Vec3, (f32, f32))>,
    pub path: Vec<(Vec3, Phase)>,
    pub loudness: Loudness,
    /// Global ambient wind level (raises every listener's noise floor).
    pub wind: Loudness,
    /// Standing environmental noise sources (rustling trees, streams) that mask
    /// the player acoustically near them.
    pub emitters: Vec<NoiseEmitter>,
}

fn fx(x: i64) -> Fixed {
    Fixed::from_int(x)
}
fn at(x: i64, y: i64, z: i64) -> Vec3 {
    Vec3::new(fx(x), fx(y), fx(z))
}
fn cell(p: Vec3) -> (i32, i32, i32) {
    (p.x.to_f32().floor() as i32, p.y.to_f32().floor() as i32, p.z.to_f32().floor() as i32)
}
fn band(dist2: i64, edges: [i64; 3]) -> usize {
    if dist2 < edges[0] {
        0
    } else if dist2 < edges[1] {
        1
    } else if dist2 < edges[2] {
        2
    } else {
        3
    }
}

/// The standard test scenario: three birds watching +X, a wall to hide behind,
/// and a player that walks in, lingers, ducks behind the wall, then leaves.
pub fn default_scenario() -> Scenario {
    let mut world = SceneWorld::new();
    // A wall at z=6 spanning x∈[3,9], y∈[0,5): blocks line of sight to anything
    // the player does on the far side of it (the Hide phase).
    for x in 3..9 {
        for y in 0..5 {
            world.set(x, y, 6, BlockPhysics::solid(Fixed::ZERO));
        }
    }
    let creatures = vec![
        (at(0, 1, -2), (1.0f32, 0.0f32)),
        (at(0, 1, 0), (1.0, 0.0)),
        (at(0, 1, 2), (1.0, 0.0)),
    ];

    let mut path = Vec::new();
    // Approach: walk in along z=0 from x=34 to x=6 (enters sight range at x=24).
    let mut x = 34;
    while x >= 6 {
        path.push((at(x, 1, 0), Phase::Approach));
        x -= 1;
    }
    // Linger: sit in view, close.
    for _ in 0..30 {
        path.push((at(6, 1, 0), Phase::Linger));
    }
    // Hide: step around to the far side of the wall (z grows), then crouch there
    // long enough that an unfed alarm fully decays to Calm.
    for z in [4, 8, 10, 12] {
        path.push((at(6, 1, z), Phase::Hide));
    }
    for _ in 0..40 {
        path.push((at(6, 1, 12), Phase::Hide));
    }
    // Leave: well out of sight and earshot, long enough to settle to Calm.
    for _ in 0..80 {
        path.push((at(60, 1, 60), Phase::Leave));
    }
    // Peeks: three brief 1-tick appearances at far-but-in-range (≈dist 20, Far
    // band), each preceded by a long out-of-range gap so the creature has gone
    // Calm. A coarse perceive cadence will be mid-skip and miss these; a fine one
    // catches them. This is what makes over-culling cost immersion.
    for _ in 0..3 {
        for _ in 0..10 {
            path.push((at(60, 1, 60), Phase::Leave)); // out of range gap
        }
        path.push((at(20, 1, 0), Phase::Peek)); // visible for one tick
    }
    for _ in 0..6 {
        path.push((at(60, 1, 60), Phase::Leave));
    }

    Scenario { world, creatures, path, loudness: 80, wind: 4, emitters: Vec::new() }
}

/// Run one preset over a scenario and report cost + immersion.
pub fn run(preset: &Preset, scn: &Scenario) -> Report {
    let senses = Senses::bird();
    let range_f = senses.sight_range.to_f32();
    let range2_f = range_f * range_f;
    let fov_cos2_f = senses.fov_cos2.to_f32();
    let n = scn.creatures.len();
    let ticks = scn.path.len();

    let mut counters = Counters::default();
    let mut naive_rays: u64 = 0;

    let mut sentinels = vec![Sentinel::new(); n];
    let mut since_perceive = vec![u32::MAX; n];
    let mut phase_ctr = vec![0u32; n];
    let mut memo_last: Vec<Option<((i32, i32, i32), bool, Loudness)>> = vec![None; n];

    // Per-creature timelines for scoring.
    let mut truedetect = vec![vec![false; ticks]; n];
    let mut reacting = vec![vec![false; ticks]; n];

    for (c, &(eye, (fx_, fz_))) in scn.creatures.iter().enumerate() {
        // Static environmental noise floor at this creature (wind + nearby
        // rustling emitters, occlusion-aware). The player must rise above it.
        let floor = ambient_floor(eye, scn.wind, &scn.emitters, &scn.world);
        for (t, &(target, phase)) in scn.path.iter().enumerate() {
            // The player is loud only while openly walking in; stalking, hiding,
            // and the brief peeks are near-silent, so those rely on sight.
            let loud = if phase == Phase::Approach { scn.loudness } else { 12 };
            let dx = target.x.to_f32() - eye.x.to_f32();
            let dz = target.z.to_f32() - eye.z.to_f32();
            let dist2f = dx * dx + dz * dz;
            let dist2i = dist2f as i64;
            let in_range = dist2f <= range2_f;
            let dot = fx_ * dx + fz_ * dz;
            let in_cone = if dist2f < 1e-4 { true } else { dot > 0.0 && (dot * dot) / dist2f >= fov_cos2_f };

            // Ground truth (full perception every tick — also the naive cost).
            let full_sight = in_range && in_cone && line_of_sight(eye, target, &scn.world);
            let heard_val_full = signal_over_noise(
                heard_loudness_world(SoundSource { pos: target, loudness: loud }, eye, &scn.world),
                floor,
            );
            let full_heard = heard_val_full >= senses.hear_threshold;
            truedetect[c][t] = full_sight || full_heard;
            naive_rays += 1 + if in_range && in_cone { 1 } else { 0 };

            // Cascade gates.
            let stride = preset.strides[band(dist2i, preset.bands2)].max(1);
            let cadence_gated = phase_ctr[c] % stride != 0;
            let relevance_gated = sentinels[c].posture == Posture::Alarmed
                && since_perceive[c] < preset.recheck_alarmed;

            if cadence_gated {
                counters.skipped_cadence += 1;
                since_perceive[c] = since_perceive[c].saturating_add(1);
            } else if relevance_gated {
                counters.skipped_relevance += 1;
                since_perceive[c] = since_perceive[c].saturating_add(1);
            } else {
                let target_cell = cell(target);
                let (sight_bool, heard_val);
                let memo_hit =
                    preset.memo && memo_last[c].map_or(false, |(tc, _, _)| tc == target_cell);
                if memo_hit {
                    let (_, sb, hv) = memo_last[c].unwrap();
                    sight_bool = sb;
                    heard_val = hv;
                    counters.memo_hits += 1;
                } else {
                    sight_bool = if !in_range {
                        counters.elided_range += 1;
                        false
                    } else if !in_cone {
                        counters.elided_cone += 1;
                        false
                    } else {
                        counters.rays_fired += 1;
                        line_of_sight(eye, target, &scn.world)
                    };
                    counters.rays_fired += 1; // hearing march
                    heard_val = heard_val_full;
                    memo_last[c] = Some((target_cell, sight_bool, heard_val));
                }
                let sighted = if sight_bool { Some(target) } else { None };
                let heard = if heard_val >= senses.hear_threshold { Some((target, heard_val)) } else { None };
                sentinels[c].perceive(sighted, heard, &senses);
                since_perceive[c] = 0;
            }

            sentinels[c].tick();
            reacting[c][t] = sentinels[c].posture != Posture::Calm;
            phase_ctr[c] += 1;
        }
    }

    let immersion = score(scn, &truedetect, &reacting);
    let detected_ticks = reacting.iter().flatten().filter(|&&b| b).count() as u32;
    let cost_frac = if naive_rays == 0 { 0.0 } else { counters.rays_fired as f32 / naive_rays as f32 };
    let balanced = immersion.total - 0.5 * cost_frac;
    Report { immersion, counters, naive_rays, cost_frac, balanced, detected_ticks }
}

fn score(scn: &Scenario, truedetect: &[Vec<bool>], reacting: &[Vec<bool>]) -> Immersion {
    let n = scn.creatures.len();
    let (mut recall, mut latency, mut specificity, mut steadiness, mut peek) =
        (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for c in 0..n {
        // Recall: reacted while genuinely detectable.
        let det: Vec<usize> = (0..scn.path.len()).filter(|&t| truedetect[c][t]).collect();
        let r = if det.is_empty() {
            1.0
        } else {
            det.iter().filter(|&&t| reacting[c][t]).count() as f32 / det.len() as f32
        };
        recall += r;

        // Latency: ticks from first detectable to first reaction.
        let lat = match det.first() {
            None => 0.0,
            Some(&t0) => match (t0..scn.path.len()).find(|&t| reacting[c][t]) {
                Some(tr) => 1.0 / (1.0 + (tr - t0) as f32 / 5.0),
                None => 0.0,
            },
        };
        latency += lat;

        // Specificity: Calm during Leave (player long gone).
        let leave: Vec<usize> = (0..scn.path.len()).filter(|&t| scn.path[t].1 == Phase::Leave).collect();
        let spec = if leave.is_empty() {
            1.0
        } else {
            // allow a decay grace at the start of Leave
            let grace = 8;
            let tail: Vec<usize> = leave.iter().skip(grace).copied().collect();
            if tail.is_empty() {
                1.0
            } else {
                tail.iter().filter(|&&t| !reacting[c][t]).count() as f32 / tail.len() as f32
            }
        };
        specificity += spec;

        // Steadiness: few posture flips while lingering.
        let linger: Vec<usize> = (0..scn.path.len()).filter(|&t| scn.path[t].1 == Phase::Linger).collect();
        let mut flips = 0u32;
        for w in linger.windows(2) {
            if reacting[c][w[0]] != reacting[c][w[1]] {
                flips += 1;
            }
        }
        steadiness += 1.0 / (1.0 + flips as f32);

        // Peek response: for each brief peek, did it react within 3 ticks?
        let peeks: Vec<usize> = (0..scn.path.len()).filter(|&t| scn.path[t].1 == Phase::Peek).collect();
        let pk = if peeks.is_empty() {
            1.0
        } else {
            let caught = peeks
                .iter()
                .filter(|&&t| (t..(t + 4).min(scn.path.len())).any(|u| reacting[c][u]))
                .count();
            caught as f32 / peeks.len() as f32
        };
        peek += pk;
    }
    let inv = 1.0 / n as f32;
    let (recall, latency, specificity, steadiness, peek) =
        (recall * inv, latency * inv, specificity * inv, steadiness * inv, peek * inv);
    let total =
        0.30 * recall + 0.15 * latency + 0.15 * specificity + 0.15 * steadiness + 0.25 * peek;
    Immersion { recall, latency, specificity, steadiness, peek, total }
}

/// A grid of presets spanning the cadence / relevance / memo axes.
pub fn preset_grid() -> Vec<Preset> {
    let bands2 = [64, 256, 1024]; // dist 8 / 16 / 32
    let stride_sets: [(&str, [u32; 4]); 3] =
        [("flat", [1, 1, 1, 1]), ("mild", [1, 1, 2, 4]), ("steep", [1, 2, 4, 8])];
    let rechecks: [(&str, u32); 3] = [("r1", 1), ("r8", 8), ("r30", 30)];
    let mut out = Vec::new();
    for (sn, strides) in stride_sets {
        for (rn, recheck_alarmed) in rechecks {
            for memo in [false, true] {
                let name: &'static str = Box::leak(
                    format!("{sn}/{rn}/{}", if memo { "memo" } else { "raw" }).into_boxed_str(),
                );
                out.push(Preset { name, strides, bands2, recheck_alarmed, memo });
            }
        }
    }
    out
}

/// Run the whole grid and rank by the balanced objective (immersion − ½·cost).
pub fn sweep() -> Vec<(String, Report)> {
    let scn = default_scenario();
    let mut results: Vec<(String, Report)> =
        preset_grid().iter().map(|p| (p.name.to_string(), run(p, &scn))).collect();
    results.sort_by(|a, b| b.1.balanced.partial_cmp(&a.1.balanced).unwrap());
    results
}

// ── path variation: randomize the scenario so scores are robust, not lucky ──

fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
fn jit(state: &mut u64, lo: i64, hi: i64) -> i64 {
    lo + (splitmix(state) % (hi - lo + 1) as u64) as i64
}

/// A jittered variant of the default scenario: the approach distance, phase
/// durations, gap lengths, and peek positions/timings all wiggle, so a preset
/// can't win on a single lucky cadence phase-alignment.
pub fn scenario_seeded(seed: u64) -> Scenario {
    let mut s = seed.wrapping_mul(0x2545_F491_4F6C_DD1D).wrapping_add(1);
    let mut world = SceneWorld::new();
    for x in 3..9 {
        for y in 0..5 {
            world.set(x, y, 6, BlockPhysics::solid(Fixed::ZERO));
        }
    }
    let creatures = vec![
        (at(0, 1, -2), (1.0f32, 0.0f32)),
        (at(0, 1, 0), (1.0, 0.0)),
        (at(0, 1, 2), (1.0, 0.0)),
    ];
    let mut path = Vec::new();
    let mut x = jit(&mut s, 30, 38);
    while x >= 6 {
        path.push((at(x, 1, 0), Phase::Approach));
        x -= 1;
    }
    for _ in 0..jit(&mut s, 20, 40) {
        path.push((at(6, 1, 0), Phase::Linger));
    }
    for z in [4, 8, 10, 12] {
        path.push((at(6, 1, z), Phase::Hide));
    }
    for _ in 0..jit(&mut s, 30, 50) {
        path.push((at(6, 1, 12), Phase::Hide));
    }
    for _ in 0..jit(&mut s, 70, 90) {
        path.push((at(60, 1, 60), Phase::Leave));
    }
    for _ in 0..3 {
        for _ in 0..jit(&mut s, 8, 14) {
            path.push((at(60, 1, 60), Phase::Leave));
        }
        // brief peek at a jittered in-range, in-cone spot (dist ≈ 17..23, |z| small)
        let px = jit(&mut s, 17, 23);
        let pz = jit(&mut s, -4, 4);
        path.push((at(px, 1, pz), Phase::Peek));
    }
    for _ in 0..6 {
        path.push((at(60, 1, 60), Phase::Leave));
    }
    Scenario { world, creatures, path, loudness: 80, wind: 4, emitters: Vec::new() }
}

/// Mean ± stddev of a preset's behavior across many randomized paths.
#[derive(Debug, Clone, Copy)]
pub struct Aggregate {
    pub imm_mean: f32,
    pub imm_std: f32,
    pub peek_mean: f32,
    pub peek_std: f32,
    pub cost_mean: f32,
    pub cost_std: f32,
    pub balanced_mean: f32,
}

fn mean_std(xs: &[f32]) -> (f32, f32) {
    let n = xs.len() as f32;
    let m = xs.iter().sum::<f32>() / n;
    let var = xs.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / n;
    (m, var.sqrt())
}

/// Evaluate a preset over `n` randomized paths and aggregate.
pub fn run_varied(preset: &Preset, n: u64) -> Aggregate {
    let (mut imm, mut peek, mut cost, mut bal) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for seed in 0..n {
        let r = run(preset, &scenario_seeded(seed));
        imm.push(r.immersion.total);
        peek.push(r.immersion.peek);
        cost.push(r.cost_frac);
        bal.push(r.balanced);
    }
    let (imm_mean, imm_std) = mean_std(&imm);
    let (peek_mean, peek_std) = mean_std(&peek);
    let (cost_mean, cost_std) = mean_std(&cost);
    let (balanced_mean, _) = mean_std(&bal);
    Aggregate { imm_mean, imm_std, peek_mean, peek_std, cost_mean, cost_std, balanced_mean }
}

/// Robust sweep: rank the grid by mean balanced score across `n` randomized paths.
pub fn sweep_varied(n: u64) -> Vec<(String, Aggregate)> {
    let mut results: Vec<(String, Aggregate)> =
        preset_grid().iter().map(|p| (p.name.to_string(), run_varied(p, n))).collect();
    results.sort_by(|a, b| b.1.balanced_mean.partial_cmp(&a.1.balanced_mean).unwrap());
    results
}

// ── scare test: vision vs sound reaction time when a threat drops in ──

/// A threat drops from a ledge in front-or-behind a watcher and lands a few
/// blocks away. Eyes catch the *descent* the instant it falls (if it's in the
/// cone); ears catch only the *impact* thud. This returns the reaction latency —
/// ticks from the threat appearing to the watcher leaving Calm — or `None` if it
/// never reacts. Run it head-toward vs head-away to measure the sight advantage,
/// and with a quiet impact to model a silent predator dropping behind you.
pub fn scare_latency(
    facing: (f32, f32),
    landing: Vec3,
    fall_ticks: u32,
    descent_loud: Loudness,
    impact_loud: Loudness,
    wind: Loudness,
) -> Option<u32> {
    let senses = crate::sim::sentinel::Senses::bird();
    let world = SceneWorld::new();
    let eye = at(0, 1, 0);
    let floor = ambient_floor(eye, wind, &[], &world);
    let mut s = crate::sim::sentinel::Sentinel::new();
    let horizon = fall_ticks + 40;
    let fticks = fall_ticks.max(1);
    for t in 0..horizon {
        // Descend from y≈12 to the landing over fall_ticks, then rest.
        let y = if t < fticks {
            12 - (11 * t as i64 / fticks as i64)
        } else {
            landing.y.to_f32() as i64
        };
        let target = Vec3::new(landing.x, Fixed::from_int(y), landing.z);
        let loud = if t == fticks { impact_loud } else { descent_loud };

        let seen = can_see(
            eye,
            Fixed::from_f64(facing.0 as f64),
            Fixed::from_f64(facing.1 as f64),
            target,
            senses.sight_range,
            senses.fov_cos2,
            &world,
        );
        let raw = heard_loudness_world(SoundSource { pos: target, loudness: loud }, eye, &world);
        let masked = signal_over_noise(raw, floor);
        let sighted = if seen { Some(target) } else { None };
        let heard = if masked > 0 { Some((target, masked)) } else { None };
        s.perceive(sighted, heard, &senses);
        if s.tick() != Posture::Calm {
            return Some(t);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive() -> Preset {
        Preset { name: "naive", strides: [1, 1, 1, 1], bands2: [64, 256, 1024], recheck_alarmed: 1, memo: false }
    }

    #[test]
    fn ground_truth_detects_during_approach_and_linger() {
        // The naive (every-tick, no-memo) preset should react strongly while the
        // player is genuinely in view.
        let r = run(&naive(), &default_scenario());
        assert!(r.immersion.recall > 0.7, "should notice a visible player, got {}", r.immersion.recall);
        assert!(r.immersion.total > 0.6, "naive immersion should be high, got {}", r.immersion.total);
    }

    #[test]
    fn memo_cuts_rays_without_wrecking_immersion() {
        let scn = default_scenario();
        let raw = Preset { name: "raw", strides: [1, 1, 1, 1], bands2: [64, 256, 1024], recheck_alarmed: 1, memo: false };
        let memo = Preset { name: "memo", memo: true, ..raw.clone() };
        let a = run(&raw, &scn);
        let b = run(&memo, &scn);
        assert!(b.counters.rays_fired < a.counters.rays_fired, "memo should fire fewer rays");
        assert!(b.counters.memo_hits > 0, "memo should hit while the player lingers");
        // Immersion essentially preserved (memo only reuses unchanged results).
        assert!((b.immersion.total - a.immersion.total).abs() < 0.05, "memo shouldn't change behavior much");
    }

    #[test]
    fn relevance_gate_cuts_rays_while_alarmed() {
        let scn = default_scenario();
        let r1 = Preset { name: "r1", strides: [1, 1, 1, 1], bands2: [64, 256, 1024], recheck_alarmed: 1, memo: false };
        let r30 = Preset { name: "r30", recheck_alarmed: 30, ..r1.clone() };
        let a = run(&r1, &scn);
        let b = run(&r30, &scn);
        assert!(b.counters.skipped_relevance > 0, "an alarmed creature should coast");
        assert!(b.counters.rays_fired < a.counters.rays_fired, "coasting fires fewer rays");
    }

    #[test]
    fn sweep_finds_a_balanced_preset_beating_naive() {
        let ranked = sweep();
        let best = &ranked[0];
        let naive_rep = run(&naive(), &default_scenario());
        // The top balanced preset should cost less than naive while keeping
        // immersion respectable.
        assert!(best.1.cost_frac < 1.0, "best preset should fire fewer rays than naive");
        assert!(best.1.immersion.total > 0.55, "best preset should still feel alive: {}", best.1.immersion.total);
        assert!(best.1.balanced >= naive_rep.immersion.total - 0.5, "balanced beats raw naive tradeoff");
    }

    #[test]
    fn acoustic_cover_masks_a_noisy_player_out_of_sight() {
        use crate::sim::ambient::NoiseEmitter;
        // One creature facing +X; the player is behind it (−X, out of the cone),
        // so detection is hearing-only. The player is loud (Approach phase).
        let creatures = vec![(at(0, 1, 0), (1.0f32, 0.0f32))];
        let path: Vec<(Vec3, Phase)> = (0..30).map(|_| (at(-5, 1, 0), Phase::Approach)).collect();
        let mk = |wind, emitters| Scenario {
            world: SceneWorld::new(),
            creatures: creatures.clone(),
            path: path.clone(),
            loudness: 80,
            wind,
            emitters,
        };
        let open = mk(4, Vec::new());
        let grove = mk(
            28,
            vec![
                NoiseEmitter { pos: at(2, 1, 1), loudness: 75 },
                NoiseEmitter { pos: at(1, 1, -1), loudness: 75 },
                NoiseEmitter { pos: at(2, 1, -2), loudness: 70 },
            ],
        );
        let ro = run(&naive(), &open);
        let rg = run(&naive(), &grove);
        assert!(ro.detected_ticks > 0, "in the open the noisy player is heard from behind");
        assert!(
            rg.detected_ticks < ro.detected_ticks,
            "the grove masks the player acoustically: grove {} < open {}",
            rg.detected_ticks,
            ro.detected_ticks
        );
    }

    #[test]
    fn sight_beats_sound_and_a_silent_drop_from_behind_is_missed() {
        let landing = at(8, 1, 0); // a few blocks in front of an eye facing +X
        let toward = (1.0f32, 0.0f32);
        let away = (-1.0f32, 0.0f32);
        // Eyes catch the descent immediately; ears wait for the impact thud.
        let seen = scare_latency(toward, landing, 10, 6, 90, 4).expect("sees the fall");
        let heard = scare_latency(away, landing, 10, 6, 90, 4).expect("hears the thud");
        assert!(seen < heard, "vision reacts sooner: sight {seen} < sound {heard}");
        assert_eq!(seen, 0, "the descent is seen the instant it starts");
        assert_eq!(heard, 10, "behind, you only react on impact (after the fall)");
        // A silent predator dropping behind you is never noticed.
        let silent = scare_latency(away, landing, 10, 4, 18, 4);
        assert_eq!(silent, None, "a quiet landing behind goes unnoticed");
    }

    #[test]
    #[ignore = "diagnostic: run with --ignored --nocapture for the scare numbers"]
    fn print_scare() {
        let landing = at(8, 1, 0);
        let toward = (1.0f32, 0.0f32);
        let away = (-1.0f32, 0.0f32);
        println!("\n  drop-in reaction latency (ticks from appearance):");
        for fall in [4u32, 10, 20] {
            let s = scare_latency(toward, landing, fall, 6, 90, 4);
            let h = scare_latency(away, landing, fall, 6, 90, 4);
            let q = scare_latency(away, landing, fall, 4, 18, 4);
            println!(
                "    fall={fall:>2}: head-toward(sight)={:?}  head-away(sound)={:?}  silent-behind={:?}",
                s, h, q
            );
        }
    }

    #[test]
    fn deterministic() {
        let a = run(&naive(), &default_scenario());
        let b = run(&naive(), &default_scenario());
        assert_eq!(a.counters.rays_fired, b.counters.rays_fired);
        assert_eq!(a.immersion.total.to_bits(), b.immersion.total.to_bits());
    }

    #[test]
    #[ignore = "diagnostic: run with --ignored --nocapture to see the ranked sweep"]
    fn print_sweep() {
        let naive_rep = run(&naive(), &default_scenario());
        println!(
            "\n  naive ref: immersion={:.3} rays={} (cost_frac=1.000)\n",
            naive_rep.immersion.total, naive_rep.counters.rays_fired
        );
        println!("  {:<14} {:>13} {:>13} {:>9} {:>7}", "preset", "imm(mean±sd)", "peek(mean±sd)", "cost", "bal");
        for (name, a) in sweep_varied(32) {
            println!(
                "  {:<14} {:>7.3}±{:<5.3} {:>7.2}±{:<5.2} {:>7.1}% {:>7.3}",
                name, a.imm_mean, a.imm_std, a.peek_mean, a.peek_std, a.cost_mean * 100.0, a.balanced_mean
            );
        }
    }

    #[test]
    fn variance_exposes_unreliable_cadence() {
        // A fine cadence catches the peeks on every path (low variance); a steep
        // cadence's catch rate depends on phase alignment, so over many paths it
        // both misses more and is less reliable.
        let bands2 = [64, 256, 1024];
        let flat = Preset { name: "flat", strides: [1, 1, 1, 1], bands2, recheck_alarmed: 30, memo: true };
        let steep = Preset { name: "steep", strides: [1, 2, 4, 8], bands2, recheck_alarmed: 30, memo: true };
        let f = run_varied(&flat, 32);
        let s = run_varied(&steep, 32);
        assert!(f.peek_mean > s.peek_mean, "fine cadence catches more peeks on average");
        assert!(s.peek_std > f.peek_std, "steep cadence is less reliable (higher variance)");
    }

    #[test]
    fn varied_sweep_is_deterministic_and_favors_memo_coast() {
        let a = sweep_varied(16);
        let b = sweep_varied(16);
        assert_eq!(a[0].0, b[0].0, "same ranking across runs");
        assert!(a[0].1.balanced_mean.is_finite());
        // The robust winner should still keep a fine cadence and use memo.
        assert!(a[0].0.contains("flat") && a[0].0.contains("memo"), "winner = {}", a[0].0);
    }
}
