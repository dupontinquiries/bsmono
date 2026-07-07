//! Noise-driven pressure sampling.
//!
//! The architectural pattern: instead of every plant rolling probability
//! dice every tick (cost = O(plants × ticks)), we draw a fixed number
//! of "samples" per chunk per tick. Each sample lands at a deterministic
//! random position within the chunk and carries a deterministic random
//! value. Handlers process only the sampled positions, not every cell.
//!
//! **Cost is O(samples × chunks × ticks), independent of plant population.**
//! A chunk with 200 plants and a chunk with 5 plants pay the same per-tick
//! cost. The 200-plant chunk just has more potential targets for the
//! samples to land on.
//!
//! **Determinism**: all sampling is hash-based on
//! `(chunk_coord, tick_id, dimension, sample_index)`. Same inputs always
//! produce the same samples. Two clients running the same simulation
//! see identical sample streams, which means identical sampled positions
//! and identical fire/no-fire outcomes. Catch-up of N ticks generates
//! N batches of samples in the right order.
//!
//! **Spatial clustering**: because the sample positions are derived from
//! a hash of (chunk, tick, sample_index), and the noise value at each
//! position is independent, samples don't form smooth spatial patterns
//! by themselves — they're random within the chunk. Spatial clustering
//! emerges from the COMBINATION of (a) the underlying field's gradient
//! (e.g. moisture varies smoothly across space) and (b) the threshold
//! function (e.g. a sigmoid that fires above a moisture threshold). A
//! chunk with one wet corner and one dry corner won't have salt-and-
//! pepper deaths — deaths cluster in the dry corner because that's
//! where samples cross the threshold.

use crate::world::chunk::ChunkCoord;
use crate::world::noise::hash_3d;
use crate::world::ticks::TickId;

/// Pressure field dimensions. Currently only `MoistureStress` is used —
/// other dimensions are reserved for future pressure-driven physics
/// (erosion, structural failure, thermal stress, biological growth).
/// Each dimension has its own seed offset so samples for different
/// dimensions don't collide even at the same (chunk, tick).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureDim {
    /// Plants drying out / dying due to insufficient moisture.
    /// Probability rises sharply as moisture falls below ~0.4.
    MoistureStress = 0,
    /// Plants growing / promoting stages due to favorable conditions.
    /// Probability rises as moisture exceeds ~0.4. Symmetric counter
    /// to MoistureStress.
    Growth = 1,
    /// Tree wood expanding via leaf-to-wood replacement. Same kind
    /// of probability response as `Growth` but slower (typical
    /// scaling: 0.3 × p_growth) so trees don't visibly explode in
    /// size. Eligibility: a leaf block adjacent to a tree's wood
    /// cluster, where the cluster's diameter is below the global
    /// tree-size cap.
    TreeGrowth = 2,
    /// Tree wood retracting via wood-to-leaf or wood-to-air
    /// replacement. Mirror of `TreeGrowth` — fires under POOR
    /// moisture conditions (dry trees lose leaves and twigs).
    /// Probability follows `0.3 × p_death(moisture)` so trees in
    /// healthy forests grow net-positive while drought-stressed
    /// trees shrink. Eligibility: a wood block at a "leaf edge"
    /// of the tree's cluster (i.e. it has at least one air or
    /// leaf neighbor — interior trunk wood is protected).
    TreeShrink = 3,
    // Reserved for future use:
    // Erosion = 4,
    // Structural = 5,
    // Thermal = 6,
}

impl PressureDim {
    /// Per-dimension seed offset. Hashed into the sample's hash so
    /// different dimensions sampling the same (chunk, tick) get
    /// independent positions and values.
    #[inline]
    fn seed_offset(self) -> u32 {
        // Arbitrary primes per dimension. Don't change without
        // accepting that all replays since this constant shipped will
        // diverge — the seed is the *meaning* of "what samples got
        // drawn at tick T."
        match self {
            PressureDim::MoistureStress => 0x9E3779B9,
            PressureDim::Growth         => 0x85EBCA6B,
            PressureDim::TreeGrowth     => 0xC2B2AE35,
            PressureDim::TreeShrink     => 0xC2B2AE36,
        }
    }
}

/// Default sample budget per chunk per tick. 8 samples means each
/// chunk has 8 chances per tick for some kind of pressure-driven
/// event. At default 0.5s tick rate, that's 16 samples per chunk per
/// real second. For a 16³ chunk with maybe 50 plants, this means a
/// plant has about a 50% chance of being sampled per second under
/// the binomial approximation — plenty of opportunity for transitions
/// to fire while keeping the cost bounded.
///
/// Tunable later. Higher = more events per second but more compute.
/// Lower = rarer events, smoother behavior, less compute. We picked
/// 8 because it matches the simulation_divisor (8) — chunks in
/// Simulation band that fire 1/8 ticks each get 8 samples per fire,
/// matching the per-tick rate of Render-band chunks.
pub const SAMPLES_PER_CHUNK_PER_TICK: usize = 8;

/// One sample drawn from the pressure field for a chunk/tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PressureSample {
    /// Local coordinates within the chunk (0..CHUNK_SIZE on each axis).
    pub local_x: i32,
    pub local_y: i32,
    pub local_z: i32,
    /// Hash-derived value, normalized to [0, 1] via integer division.
    /// Compared against a probability threshold (e.g. sigmoid of
    /// moisture) to decide whether the sample "fires."
    pub noise_value: f32,
}

const CHUNK_SIZE: i32 = crate::constants::CHUNK_SIZE;

/// Draw `n` samples for the given (chunk, tick, dimension). Returns a
/// vector of `n` `PressureSample`s. Pure function: same inputs always
/// produce the same output sequence.
///
/// Each sample's position is derived from
/// `hash_3d(chunk_x, chunk_y, chunk_z + sample_idx, dim_seed ^ tick)`
/// (with sample_idx mixed into one of the position dimensions to
/// keep the calls within `hash_3d`'s 3-arg signature). The hash output
/// is split into separate fields for x, y, z, and the noise value.
///
/// **Why this hash split**: a 32-bit hash provides 32 bits of entropy.
/// We need:
///   - 4 bits each for x/y/z within a 16-cube (so 12 bits)
///   - 16 bits for the noise value
///   - The remaining bits are scratch for hash mixing.
/// 32 bits is comfortable for this. If chunk size grows past 16 we
/// need wider hashing.
pub fn pressure_samples(
    chunk: ChunkCoord,
    tick: TickId,
    dim: PressureDim,
    n: usize,
) -> Vec<PressureSample> {
    // v0.24.69-no-alloc: delegate to the iterator version and
    // collect. The iterator version is the production path
    // (used in the lifecycle handler hot loops); this Vec wrapper
    // exists for tests that compare full sample sequences with
    // `assert_eq!(a, b)`.
    pressure_samples_iter(chunk, tick, dim, n).collect()
}

/// Iterator version of `pressure_samples`. Same outputs in the same
/// order, but no Vec allocation. Used in the plant_lifecycle
/// handler's hot loops to drop ~3700 small heap allocations per
/// second at typical render distances (~925 chunks × 4 calls per
/// chunk per tick × 2 ticks/sec).
///
/// Identical math to `pressure_samples` — the bodies share the
/// exact same sample-generation formula. If you change one, change
/// the other (or refactor both to share a helper).
pub fn pressure_samples_iter(
    chunk: ChunkCoord,
    tick: TickId,
    dim: PressureDim,
    n: usize,
) -> impl Iterator<Item = PressureSample> {
    let dim_seed = dim.seed_offset();
    let tick_lo = (tick.raw() & 0xFFFF_FFFF) as u32;
    let tick_hi = ((tick.raw() >> 32) & 0xFFFF_FFFF) as u32;
    let cx = chunk.x;
    let cy = chunk.y;
    let cz = chunk.z;

    (0..n).map(move |i| {
        let seed = dim_seed
            ^ tick_lo
            ^ (tick_hi.wrapping_mul(2654435761))
            ^ ((i as u32).wrapping_mul(2246822519));
        let h = hash_3d(cx, cy, cz, seed);

        let local_x = ((h >> 0) & 0xF) as i32;
        let local_y = ((h >> 4) & 0xF) as i32;
        let local_z = ((h >> 8) & 0xF) as i32;

        let noise_bits = (h >> 16) as u16;
        let noise_value = (noise_bits as f32) / 65536.0;

        let local_x = local_x.min(CHUNK_SIZE - 1);
        let local_y = local_y.min(CHUNK_SIZE - 1);
        let local_z = local_z.min(CHUNK_SIZE - 1);

        PressureSample { local_x, local_y, local_z, noise_value }
    })
}

/// Probability of moisture-stress death given current moisture.
/// Sigmoid centered at 0.4 (the old deterministic threshold). At
/// moisture=0.4, p_death = 0.5 (50/50). At moisture=0.6, p_death
/// drops to ~0.12 (rare). At moisture=0.2, p_death rises to ~0.88
/// (very likely if sample lands).
///
/// **Steepness**: tuned so that the curve crosses [0.1, 0.9] over a
/// moisture range of about 0.4 (i.e. moisture 0.2 → 0.88 prob,
/// moisture 0.6 → 0.12 prob). Higher steepness = more cliff-like,
/// approaching the old binary threshold. Lower = smoother, blurrier.
///
/// This is per-sample probability. The plant's actual per-tick death
/// rate is approximately:
///   p_death_per_tick = (samples_per_chunk × p_death(moisture)) / chunk_volume × plant_density_at_position
/// For a chunk with 50 plants, 8 samples, plant density ~50/16³ ≈ 0.012,
/// p_death(0.4) = 0.5 → expected fires per tick = 8 × 0.5 × 0.012 ≈ 0.05.
/// At 2 Hz that's 0.1 deaths per second per chunk = ~6 per minute. About
/// right for a "drying forest" feel.
#[inline]
pub fn p_death(moisture: f32) -> f32 {
    // Sigmoid: 1 / (1 + exp(STEEPNESS * (moisture - MIDPOINT)))
    const MIDPOINT: f32 = 0.4;
    const STEEPNESS: f32 = 10.0;
    1.0 / (1.0 + (STEEPNESS * (moisture - MIDPOINT)).exp())
}

/// Probability of growth-promotion given current moisture. Symmetric
/// counter to `p_death` — high in wet conditions, low in dry. Same
/// midpoint and steepness so the two curves cross at moisture=0.4
/// where p_death = p_growth = 0.5.
///
/// At moisture=0.6, p_growth ≈ 0.88. At moisture=0.2, p_growth ≈ 0.12.
///
/// In a healthy forest (avg moisture > 0.6), few samples fire death
/// but many fire growth → net positive, forest sustains. In a desert
/// (moisture < 0.2), many fire death and few fire growth → forest
/// recedes.
#[inline]
pub fn p_growth(moisture: f32) -> f32 {
    // Just 1 - p_death (sigmoid mirror around the midpoint).
    1.0 - p_death(moisture)
}

/// Probability of a tree-growth event (leaf → wood replacement) given
/// current moisture. Scaled to 0.3× of `p_growth` because trees
/// shouldn't visibly explode in size — slow growth feels real, fast
/// growth would have a forest become a wall in seconds.
///
/// At moisture=0.6: p_growth ≈ 0.88, p_tree_growth ≈ 0.26 — about a
/// quarter of leaves with wood adjacency get a chance per sample.
/// At moisture=0.2: p_growth ≈ 0.12, p_tree_growth ≈ 0.04 — trees
/// barely grow in dry conditions.
///
/// Combined with the sample budget (8 samples per chunk per tick),
/// the per-tree-per-second growth rate ends up at roughly one event
/// per minute of play time per typical tree. Slow enough that trees
/// feel real, fast enough that you can watch a forest change over
/// 5-10 minutes of play.
#[inline]
pub fn p_tree_growth(moisture: f32) -> f32 {
    0.3 * p_growth(moisture)
}

/// Probability of a tree-shrink event (wood → leaf/air replacement
/// at a tree's "leaf edge") given current moisture. Mirror of
/// `p_tree_growth` — high in dry conditions, low in wet.
///
/// Scaled to 0.3× of `p_death` for the same reason growth is scaled:
/// trees shouldn't visibly evaporate. At neutral moisture (0.4)
/// growth and shrink fire at equal rates so a mature tree's
/// boundary breathes; in wet conditions growth dominates and trees
/// expand; in dry conditions shrink dominates and trees retract.
///
/// At moisture=0.2: p_tree_shrink ≈ 0.26 — drought visibly retracts
/// trees over a minute or two of play.
/// At moisture=0.6: p_tree_shrink ≈ 0.04 — wet forest barely loses
/// any branches.
#[inline]
pub fn p_tree_shrink(moisture: f32) -> f32 {
    0.3 * p_death(moisture)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Determinism: two calls with the same inputs return byte-identical samples.
    #[test]
    fn samples_are_deterministic() {
        let chunk = ChunkCoord::new(5, 0, -3);
        let tick = TickId(1234);
        let a = pressure_samples(chunk, tick, PressureDim::MoistureStress, 8);
        let b = pressure_samples(chunk, tick, PressureDim::MoistureStress, 8);
        assert_eq!(a, b, "same inputs must produce same samples");
    }

    /// Different ticks produce different samples (the noise field rotates
    /// over time so we don't keep picking the same positions).
    #[test]
    fn samples_change_across_ticks() {
        let chunk = ChunkCoord::new(0, 0, 0);
        let a = pressure_samples(chunk, TickId(1), PressureDim::MoistureStress, 8);
        let b = pressure_samples(chunk, TickId(2), PressureDim::MoistureStress, 8);
        assert_ne!(a, b, "different ticks should produce different samples");
    }

    /// Different chunks produce different samples (samples are local to chunk).
    #[test]
    fn samples_differ_between_chunks() {
        let a = pressure_samples(ChunkCoord::new(0, 0, 0), TickId(0),
                                  PressureDim::MoistureStress, 8);
        let b = pressure_samples(ChunkCoord::new(1, 0, 0), TickId(0),
                                  PressureDim::MoistureStress, 8);
        assert_ne!(a, b, "different chunks should produce different samples");
    }

    /// Different dimensions produce different samples for the same (chunk, tick).
    /// Without this, MoistureStress and Growth would always sample the same
    /// positions, breaking the "few death + many growth" balance idea.
    #[test]
    fn dimensions_are_independent() {
        let chunk = ChunkCoord::new(2, 0, 5);
        let tick = TickId(100);
        let stress = pressure_samples(chunk, tick, PressureDim::MoistureStress, 8);
        let growth = pressure_samples(chunk, tick, PressureDim::Growth, 8);
        assert_ne!(stress, growth, "different dimensions must produce different samples");
    }

    /// All sample positions are in chunk-local bounds [0, CHUNK_SIZE).
    #[test]
    fn samples_stay_in_chunk_bounds() {
        for tick_raw in 0..100u64 {
            let samples = pressure_samples(
                ChunkCoord::new(7, -2, 3),
                TickId(tick_raw),
                PressureDim::MoistureStress,
                16,
            );
            for s in &samples {
                assert!(s.local_x >= 0 && s.local_x < CHUNK_SIZE,
                    "x out of bounds: {}", s.local_x);
                assert!(s.local_y >= 0 && s.local_y < CHUNK_SIZE,
                    "y out of bounds: {}", s.local_y);
                assert!(s.local_z >= 0 && s.local_z < CHUNK_SIZE,
                    "z out of bounds: {}", s.local_z);
                assert!(s.noise_value >= 0.0 && s.noise_value < 1.0,
                    "noise out of bounds: {}", s.noise_value);
            }
        }
    }

    /// Samples spread across the chunk volume — not all at the same
    /// position, not all clustered at corners. This is a basic
    /// distribution sanity check, not a rigorous statistical test.
    #[test]
    fn samples_spread_across_volume() {
        // Draw a lot of samples and check we hit at least 4 different
        // positions on each axis. Across many ticks this is easy.
        let mut x_seen = std::collections::HashSet::new();
        let mut y_seen = std::collections::HashSet::new();
        let mut z_seen = std::collections::HashSet::new();
        for tick_raw in 0..200u64 {
            for s in pressure_samples(
                ChunkCoord::new(0, 0, 0),
                TickId(tick_raw),
                PressureDim::MoistureStress,
                8,
            ) {
                x_seen.insert(s.local_x);
                y_seen.insert(s.local_y);
                z_seen.insert(s.local_z);
            }
        }
        assert!(x_seen.len() >= 4, "x diversity too low: {} unique values", x_seen.len());
        assert!(y_seen.len() >= 4, "y diversity too low: {} unique values", y_seen.len());
        assert!(z_seen.len() >= 4, "z diversity too low: {} unique values", z_seen.len());
    }

    /// Noise values are roughly uniform in [0, 1). A weak distribution
    /// check — across many samples, both tails (< 0.1 and > 0.9) and
    /// the middle (0.4-0.6) should all be represented.
    #[test]
    fn noise_distribution_is_roughly_uniform() {
        let mut low = 0;
        let mut mid = 0;
        let mut high = 0;
        let total_ticks = 1000u64;
        let samples_per_tick = 8;
        for tick_raw in 0..total_ticks {
            for s in pressure_samples(
                ChunkCoord::new(0, 0, 0),
                TickId(tick_raw),
                PressureDim::MoistureStress,
                samples_per_tick,
            ) {
                if s.noise_value < 0.1 { low += 1; }
                else if s.noise_value > 0.9 { high += 1; }
                else if s.noise_value > 0.4 && s.noise_value < 0.6 { mid += 1; }
            }
        }
        let total = (total_ticks as usize * samples_per_tick) as f32;
        // Each band covers 10% (low, high) or 20% (mid) of the [0,1) range.
        // Expected counts: 10% × 8000 = 800 low, 800 high, 1600 mid.
        // Tolerance: factor of 2 in either direction (rough check).
        assert!((low as f32) > total * 0.05 && (low as f32) < total * 0.20,
            "low bin out of range: {}", low);
        assert!((high as f32) > total * 0.05 && (high as f32) < total * 0.20,
            "high bin out of range: {}", high);
        assert!((mid as f32) > total * 0.10 && (mid as f32) < total * 0.30,
            "mid bin out of range: {}", mid);
    }

    /// Sigmoid endpoints check.
    #[test]
    fn p_death_endpoints() {
        // At the midpoint, p_death = 0.5.
        assert!((p_death(0.4) - 0.5).abs() < 0.001);
        // Wet: very low.
        assert!(p_death(0.8) < 0.05, "p_death(0.8) = {}, expected < 0.05", p_death(0.8));
        // Dry: very high.
        assert!(p_death(0.0) > 0.95, "p_death(0.0) = {}, expected > 0.95", p_death(0.0));
        // Symmetric.
        assert!((p_death(0.4) - 0.5).abs() < 0.001);
    }

    /// Growth probability is the mirror of death.
    #[test]
    fn p_growth_mirrors_p_death() {
        for moisture_centi in 0..=100 {
            let m = moisture_centi as f32 * 0.01;
            let sum = p_death(m) + p_growth(m);
            assert!((sum - 1.0).abs() < 0.001,
                "p_death({}) + p_growth({}) = {}, expected 1.0",
                m, m, sum);
        }
    }

    /// Tree-growth is a scaled-down version of growth: same shape,
    /// 0.3× magnitude.
    #[test]
    fn p_tree_growth_is_scaled_growth() {
        for moisture_centi in 0..=100 {
            let m = moisture_centi as f32 * 0.01;
            let scaled = p_growth(m) * 0.3;
            assert!((p_tree_growth(m) - scaled).abs() < 0.001,
                "p_tree_growth({}) = {}, expected {} (= 0.3 * p_growth)",
                m, p_tree_growth(m), scaled);
        }
    }

    /// Tree-shrink mirrors tree-growth: same 0.3× magnitude scaling,
    /// but applied to p_death instead of p_growth. At neutral moisture
    /// (0.4) growth and shrink are equal; at wet, growth dominates;
    /// at dry, shrink dominates.
    #[test]
    fn p_tree_shrink_is_scaled_death() {
        for moisture_centi in 0..=100 {
            let m = moisture_centi as f32 * 0.01;
            let scaled = p_death(m) * 0.3;
            assert!((p_tree_shrink(m) - scaled).abs() < 0.001,
                "p_tree_shrink({}) = {}, expected {} (= 0.3 * p_death)",
                m, p_tree_shrink(m), scaled);
        }
    }

    /// At moisture=0.4 (neutral / midpoint), tree-growth and
    /// tree-shrink should fire at equal rates so the tree's edge
    /// breathes around its current size.
    #[test]
    fn p_tree_growth_equals_p_tree_shrink_at_midpoint() {
        let m = 0.4;
        let g = p_tree_growth(m);
        let s = p_tree_shrink(m);
        assert!((g - s).abs() < 0.001,
            "expected p_tree_growth(0.4) == p_tree_shrink(0.4), got {} vs {}",
            g, s);
    }

    /// TreeShrink dimension is independent of TreeGrowth — separate
    /// seeds so the same (chunk, tick) doesn't draw correlated
    /// samples for both directions of tree change.
    #[test]
    fn tree_shrink_dim_independent_of_growth() {
        let chunk = ChunkCoord::new(2, -1, 5);
        let tick = TickId(1729);
        let grow   = pressure_samples(chunk, tick, PressureDim::TreeGrowth, 8);
        let shrink = pressure_samples(chunk, tick, PressureDim::TreeShrink, 8);
        assert_ne!(grow, shrink,
            "TreeGrowth and TreeShrink share a (chunk, tick) but produced identical samples");
    }

    /// TreeGrowth dimension is independent of MoistureStress and
    /// Growth — each draws its own samples.
    #[test]
    fn tree_growth_dim_independent_of_others() {
        let chunk = ChunkCoord::new(0, 0, 0);
        let tick = TickId(42);
        let stress = pressure_samples(chunk, tick, PressureDim::MoistureStress, 8);
        let growth = pressure_samples(chunk, tick, PressureDim::Growth, 8);
        let tree   = pressure_samples(chunk, tick, PressureDim::TreeGrowth, 8);
        assert_ne!(tree, stress);
        assert_ne!(tree, growth);
    }

    /// Time-homogeneity: samples drawn ONE tick at a time over N ticks
    /// equals samples drawn for those N ticks via repeated calls.
    /// (Trivially true given the function is pure and deterministic,
    /// but worth a test to lock in the expectation.)
    #[test]
    fn samples_time_homogeneous() {
        let chunk = ChunkCoord::new(3, 1, -7);
        let mut individual = Vec::new();
        for t in 100..150 {
            individual.push(pressure_samples(
                chunk, TickId(t), PressureDim::MoistureStress, 4));
        }
        // Same ticks, same outputs.
        for (i, t) in (100..150).enumerate() {
            let again = pressure_samples(
                chunk, TickId(t), PressureDim::MoistureStress, 4);
            assert_eq!(individual[i], again,
                "tick {} produced different samples on repeat", t);
        }
    }
}
