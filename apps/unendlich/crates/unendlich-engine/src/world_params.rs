//! world_params.rs — pure world-creation wizard parameters.
//!
//! Seed, world size, a biome weight mix, cave density and weather toggles, with
//! defaults, normalization (biome weights sum to 1) and validation. Pure,
//! deterministic, panic-free (R1/R11/R21). The world-creation GUI (coordinator)
//! edits these; generation consumes the validated struct.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WorldSize {
    Small,
    Medium,
    Large,
}

impl WorldSize {
    /// Edge length in chunks for this size.
    pub fn chunks(self) -> u32 {
        match self {
            WorldSize::Small => 256,
            WorldSize::Medium => 1024,
            WorldSize::Large => 4096,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct WorldParams {
    pub seed: u64,
    pub size: WorldSize,
    /// Relative weights per biome; normalized to sum to 1 by `normalized()`.
    pub biome_weights: Vec<f32>,
    /// Cave carving intensity, 0..=1.
    pub cave_density: f32,
    pub weather_enabled: bool,
    pub storms_enabled: bool,
}

impl Default for WorldParams {
    fn default() -> Self {
        WorldParams {
            seed: 0,
            size: WorldSize::Medium,
            // four default biomes, equal weight
            biome_weights: vec![1.0, 1.0, 1.0, 1.0],
            cave_density: 0.5,
            weather_enabled: true,
            storms_enabled: true,
        }
    }
}

impl WorldParams {
    /// Return a copy with biome weights clamped non-negative and normalized to
    /// sum to 1, and `cave_density` clamped to `[0,1]`. If all weights are zero
    /// (or empty), falls back to a uniform mix.
    pub fn normalized(&self) -> WorldParams {
        let n = self.biome_weights.len();
        let cleaned: Vec<f32> = self.biome_weights.iter().map(|&w| w.max(0.0)).collect();
        let sum: f32 = cleaned.iter().sum();
        let weights = if n == 0 {
            Vec::new()
        } else if sum > 0.0 {
            cleaned.iter().map(|&w| w / sum).collect()
        } else {
            vec![1.0 / n as f32; n] // all-zero -> uniform
        };
        WorldParams {
            seed: self.seed,
            size: self.size,
            biome_weights: weights,
            cave_density: if self.cave_density < 0.0 {
                0.0
            } else if self.cave_density > 1.0 {
                1.0
            } else {
                self.cave_density
            },
            weather_enabled: self.weather_enabled,
            storms_enabled: self.storms_enabled,
        }
    }

    /// True iff weights are non-negative and sum to 1 (±eps), cave density is in
    /// range, and there is at least one biome. (A freshly `normalized()` value
    /// always validates.)
    pub fn validate(&self) -> bool {
        if self.biome_weights.is_empty() {
            return false;
        }
        if !(0.0..=1.0).contains(&self.cave_density) {
            return false;
        }
        let mut sum = 0.0f32;
        for &w in &self.biome_weights {
            if !w.is_finite() || w < 0.0 {
                return false;
            }
            sum += w;
        }
        (sum - 1.0).abs() < 1e-4
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn defaults_normalize_and_validate() {
        let d = WorldParams::default().normalized();
        assert!(d.validate(), "default (normalized) must validate");
        let sum: f32 = d.biome_weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4);
    }

    #[test]
    fn prop_normalize_sums_to_one_and_clamps() {
        let mut seed = 0x1357_2468u32;
        for _ in 0..300 {
            let n = 1 + (lcg(&mut seed) * 8.0) as usize;
            let weights: Vec<f32> = (0..n).map(|_| lcg(&mut seed) * 6.0 - 2.0).collect(); // some negative
            let p = WorldParams {
                seed: 7,
                size: WorldSize::Large,
                biome_weights: weights,
                cave_density: lcg(&mut seed) * 3.0 - 1.0, // out of range
                weather_enabled: false,
                storms_enabled: true,
            };
            let norm = p.normalized();
            assert!(norm.validate(), "normalized params must validate");
            assert!(norm.biome_weights.iter().all(|&w| w >= 0.0));
            assert!((0.0..=1.0).contains(&norm.cave_density));
            let sum: f32 = norm.biome_weights.iter().sum();
            assert!((sum - 1.0).abs() < 1e-4, "weights sum {} != 1", sum);
        }
    }

    #[test]
    fn all_zero_weights_become_uniform() {
        let p = WorldParams { biome_weights: vec![0.0, 0.0, 0.0], ..WorldParams::default() };
        let n = p.normalized();
        for &w in &n.biome_weights {
            assert!((w - 1.0 / 3.0).abs() < 1e-5);
        }
        assert!(n.validate());
    }

    #[test]
    fn empty_mix_is_invalid() {
        let p = WorldParams { biome_weights: vec![], ..WorldParams::default() };
        assert!(!p.normalized().validate(), "no biomes -> invalid");
    }

    #[test]
    fn same_seed_and_params_identical_struct() {
        let a = WorldParams { seed: 12345, ..WorldParams::default() }.normalized();
        let b = WorldParams { seed: 12345, ..WorldParams::default() }.normalized();
        assert_eq!(a, b, "same inputs must build identical params");
        let c = WorldParams { seed: 12346, ..WorldParams::default() }.normalized();
        assert!(a != c, "different seed should differ");
    }
}
