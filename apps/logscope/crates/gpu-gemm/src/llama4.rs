//! llama4.rs — Llama 4 iRoPE attention temperature tuning. Pure, no wgpu, testable.
//!
//! Llama 4 interleaves RoPE layers with NoPE (no positional encoding) layers. On the NoPE layers, as the
//! sequence grows the softmax denominator (sum of attention scores over all positions) grows while the
//! numerator stays fixed, so attention probabilities fade toward uniform — the model loses its grip on
//! position at long context. Llama 4's fix ("inference-time temperature scaling of attention") multiplies
//! the query by a per-position scale before the QK dot-product, sharpening the softmax back up. This is a
//! key part of Scout's long-context (10M token) generalization.
//!
//! Formula, straight from Meta's reference (`llama-models/models/llama4/model.py`):
//!
//!     attn_scale(pos) = log( floor((pos + 1) / floor_scale) + 1 ) * attn_scale + 1
//!
//! with defaults `floor_scale = 8192`, `attn_scale = 0.1`. It's the identity (1.0) until `pos+1` reaches
//! `floor_scale`, then grows logarithmically — so short contexts are unaffected and only long ones get the
//! correction. Applied on NoPE layers ONLY (RoPE layers attend to shorter sub-sequences via their rotation
//! and don't need it). The scale multiplies Q (equivalently, scales the pre-softmax logits).

/// Llama 4 attention-temperature parameters (from the checkpoint's config; these are the Meta defaults).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttnTemp {
    /// Base position scale (tokens) — the correction is the identity until pos+1 hits this. Default 8192.
    pub floor_scale: f32,
    /// Strength of the temperature tuning. Default 0.1.
    pub attn_scale: f32,
    /// Whether tuning is enabled (Meta default true; a checkpoint may disable it).
    pub enabled: bool,
}

impl Default for AttnTemp {
    fn default() -> Self { AttnTemp { floor_scale: 8192.0, attn_scale: 0.1, enabled: true } }
}

impl AttnTemp {
    /// The per-position query scale. `pos` is the 0-based absolute position of the query token. Returns
    /// exactly 1.0 (a no-op) when disabled or while `pos+1 < floor_scale`, so short contexts and RoPE
    /// layers (which pass enabled=false) are bit-unaffected.
    #[inline]
    pub fn scale(&self, pos: usize) -> f32 {
        if !self.enabled { return 1.0; }
        let p = pos as f32 + 1.0;
        (( p / self.floor_scale).floor() + 1.0).ln() * self.attn_scale + 1.0
    }

    /// Is the scale non-trivial at this position? (Lets the decode path skip the query multiply entirely
    /// below the floor — the common case — so there's zero overhead until long context.)
    #[inline]
    pub fn active_at(&self, pos: usize) -> bool {
        self.enabled && (pos as f32 + 1.0) >= self.floor_scale
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_temperature_parameters_are_DEFAULTS_not_constants() {
        // floor_scale 8192 and attn_scale 0.1 are Llama-4's documented values, and the loader now
        // reads {arch}.attention.floor_scale and .scale over them. Same class as nope_step being
        // hardcoded to 4: a value the file carries, taken from a constant instead — and a wrong scale
        // produces text that is merely WORSE, with every tensor and shape correct.
        let d = AttnTemp::default();
        assert_eq!(d.floor_scale, 8192.0);
        assert_eq!(d.attn_scale, 0.1);
        assert!(d.enabled);

        // An overridden pair must actually change the scale, or reading metadata is decoration.
        let mut o = AttnTemp::default();
        o.floor_scale = 4096.0;
        assert_ne!(o.scale(8192), d.scale(8192),
            "a different floor must change the scale at the same position");
    }

    #[test]
    fn the_scale_is_a_NO_OP_below_the_floor() {
        // Short contexts must be bit-unaffected, or every existing short-prompt result would shift
        // when this became metadata-driven.
        let d = AttnTemp::default();
        assert_eq!(d.scale(0), 1.0);
        assert_eq!(d.scale(100), 1.0);
    }

    use super::*;

    fn approx(a: f32, b: f32) -> bool { (a - b).abs() < 1e-6 }

    #[test]
    fn identity_below_the_floor() {
        let t = AttnTemp::default();
        // below floor_scale the floor() term is 0 → log(1) = 0 → scale is exactly 1.0
        for pos in [0usize, 100, 4095, 8190] { assert!(approx(t.scale(pos), 1.0), "pos {pos} must be 1.0"); }
        assert!(!t.active_at(0) && !t.active_at(8190));
    }

    #[test]
    fn grows_logarithmically_past_the_floor() {
        let t = AttnTemp::default();
        // at pos+1 == floor_scale, floor(1.0)=1 → log(2)*0.1 + 1
        let at_floor = t.scale(8191); // pos+1 = 8192
        assert!(approx(at_floor, 2f32.ln() * 0.1 + 1.0), "got {at_floor}");
        assert!(t.active_at(8191));
        // monotonic non-decreasing and strictly grows across floor multiples
        let a = t.scale(8192);    // ~1.069
        let b = t.scale(16384);   // pos+1=16385 → floor=2 → log(3)*.1+1
        let c = t.scale(100_000);
        assert!(b > a && c > b, "scale grows with position: {a} {b} {c}");
        assert!(approx(b, 3f32.ln() * 0.1 + 1.0));
    }

    #[test]
    fn disabled_is_always_identity() {
        let t = AttnTemp { enabled: false, ..Default::default() };
        for pos in [0usize, 8192, 1_000_000] { assert!(approx(t.scale(pos), 1.0)); }
        assert!(!t.active_at(1_000_000));
    }

    #[test]
    fn custom_params_respected() {
        let t = AttnTemp { floor_scale: 4096.0, attn_scale: 0.2, enabled: true };
        assert!(approx(t.scale(4095), 2f32.ln() * 0.2 + 1.0)); // pos+1=4096
        assert!(t.active_at(4095) && !t.active_at(4094));
    }
}
