//! rope.rs — synthesize long-context RoPE scaling factors from GGUF metadata.
//!
//! The engine already *applies* per-dimension RoPE factors when a model bakes them into a
//! `rope_freqs.weight` tensor (llama-3 style). Models that instead declare scaling through metadata —
//! `rope_scaling.type` plus `factor` / `original_context_length` / `beta_fast` / `beta_slow` (Qwen2-YaRN,
//! DeepSeek, Yi-200K) — ship no such tensor, so there's nothing for that path to apply. This module
//! computes the equivalent per-dimension factor vector (linear or YaRN) from the metadata, matching
//! llama.cpp's `rope_yarn` math, so the existing apply path covers those models too. It also returns
//! YaRN's attention temperature (`mscale`). Pure; the rotary kernel itself is the seam.
//!
//! Note: `arch::Feature::RopeScaling` is exactly what this fills in — detection there, synthesis here.
#![allow(dead_code)]

use std::f32::consts::PI;

/// Base RoPE inverse frequencies: `base^(-2i/head_dim)` for pair `i` in `0..head_dim/2`.
pub fn inv_freq(head_dim: u32, base: f32) -> Vec<f32> {
    let half = (head_dim / 2).max(1);
    (0..half).map(|i| base.powf(-2.0 * i as f32 / head_dim as f32)).collect()
}

/// How a model declares its context scaling.
#[derive(Clone, Copy, Debug)]
pub enum Scaling {
    None,
    Linear { factor: f32 },
    Yarn { factor: f32, orig_ctx: u32, beta_fast: f32, beta_slow: f32 },
}

/// The dimension at which `n_rot` full rotations fit in `orig_ctx` (llama.cpp `rope_yarn_corr_dim`).
fn corr_dim(head_dim: u32, orig_ctx: u32, n_rot: f32, base: f32) -> f32 {
    head_dim as f32 * (orig_ctx as f32 / (n_rot * 2.0 * PI)).ln() / (2.0 * base.ln())
}

fn corr_range(head_dim: u32, orig_ctx: u32, base: f32, beta_fast: f32, beta_slow: f32) -> (f32, f32) {
    let lo = corr_dim(head_dim, orig_ctx, beta_fast, base).floor().max(0.0);
    let max_pair = (head_dim / 2).saturating_sub(1) as f32;
    let hi = corr_dim(head_dim, orig_ctx, beta_slow, base).ceil().min(max_pair);
    (lo, hi)
}

/// Interpolation ↔ extrapolation ramp: 1 at high-frequency dims (keep, i.e. extrapolate), 0 at
/// low-frequency dims (interpolate / scale), linear between the correction dims.
fn ramp(low: f32, high: f32, i: u32) -> f32 {
    let y = (i as f32 - low) / (high - low).max(1e-3);
    1.0 - y.clamp(0.0, 1.0)
}

/// Per-dimension multiplicative factors for `inv_freq` — exactly what a `rope_freqs.weight` tensor would
/// hold. `1.0` means unscaled (extrapolate); `1/factor` means fully interpolated.
pub fn scaling_factors(head_dim: u32, base: f32, scaling: Scaling) -> Vec<f32> {
    let half = (head_dim / 2).max(1) as usize;
    match scaling {
        Scaling::None => vec![1.0; half],
        Scaling::Linear { factor } => vec![1.0 / factor.max(1e-6); half],
        Scaling::Yarn { factor, orig_ctx, beta_fast, beta_slow } => {
            let inv_scale = 1.0 / factor.max(1e-6);
            let (lo, hi) = corr_range(head_dim, orig_ctx, base, beta_fast, beta_slow);
            (0..half as u32).map(|i| {
                let r = ramp(lo, hi, i);
                inv_scale * (1.0 - r) + r // blend interp factor (1/factor) with extrap factor (1.0)
            }).collect()
        }
    }
}

/// YaRN attention temperature, multiplying cos/sin (`1 + 0.1·ln(factor)`). `1.0` for non-YaRN.
pub fn mscale(scaling: Scaling) -> f32 {
    match scaling {
        Scaling::Yarn { factor, .. } if factor > 1.0 => 1.0 + 0.1 * factor.ln(),
        _ => 1.0,
    }
}

/// Effective scaled inverse frequencies = `inv_freq · scaling_factors`.
pub fn scaled_inv_freq(head_dim: u32, base: f32, scaling: Scaling) -> Vec<f32> {
    inv_freq(head_dim, base).iter().zip(scaling_factors(head_dim, base, scaling)).map(|(a, b)| a * b).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool { (a - b).abs() < eps }

    #[test]
    fn inv_freq_matches_the_definition() {
        let f = inv_freq(4, 10000.0); // pairs i=0,1 → 10000^0, 10000^-0.5
        assert!(approx(f[0], 1.0, 1e-6));
        assert!(approx(f[1], 0.01, 1e-6));
    }

    #[test]
    fn none_and_linear_are_uniform() {
        assert_eq!(scaling_factors(8, 10000.0, Scaling::None), vec![1.0; 4]);
        let lin = scaling_factors(8, 10000.0, Scaling::Linear { factor: 4.0 });
        assert!(lin.iter().all(|&x| approx(x, 0.25, 1e-6)));
        // scaled_inv_freq under linear == inv_freq / factor
        let base = inv_freq(8, 10000.0);
        let sif = scaled_inv_freq(8, 10000.0, Scaling::Linear { factor: 4.0 });
        for (b, s) in base.iter().zip(&sif) { assert!(approx(*s, b / 4.0, 1e-6)); }
    }

    #[test]
    fn yarn_interpolates_low_freqs_and_keeps_high_freqs() {
        let (hd, base, factor) = (128u32, 10000.0f32, 4.0f32);
        let f = scaling_factors(hd, base, Scaling::Yarn { factor, orig_ctx: 4096, beta_fast: 32.0, beta_slow: 1.0 });
        assert_eq!(f.len(), 64);
        // every factor lies between full interpolation (1/factor) and no scaling (1.0)
        assert!(f.iter().all(|&x| x >= 0.25 - 1e-4 && x <= 1.0 + 1e-4));
        // high-frequency dims (small i) are kept ≈ 1.0; low-frequency dims (large i) are scaled ≈ 1/factor
        assert!(approx(f[0], 1.0, 1e-3));
        assert!(approx(f[63], 0.25, 1e-3));
        // and it's monotonically non-increasing across dimensions
        assert!(f.windows(2).all(|w| w[0] >= w[1] - 1e-6));
    }

    #[test]
    fn yarn_attention_temperature() {
        assert!(approx(mscale(Scaling::Yarn { factor: 4.0, orig_ctx: 4096, beta_fast: 32.0, beta_slow: 1.0 }),
                       1.0 + 0.1 * 4.0f32.ln(), 1e-6));
        assert_eq!(mscale(Scaling::Linear { factor: 4.0 }), 1.0);
        assert_eq!(mscale(Scaling::None), 1.0);
    }
}
