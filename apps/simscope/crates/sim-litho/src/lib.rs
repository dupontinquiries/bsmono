//! sim-litho — Level 3: EUV imaging by Fourier optics. f32-safe, GPU-shaped.
//!
//! Scaffolded so the layer boundaries are fixed before anything grows across them. See
//! apps/simscope/PLAN.md for what this crate owes and what it may depend on.

/// Which layer of the stack this crate implements.
pub const LAYER: &str = "L3";

#[cfg(test)]
mod tests {
    #[test]
    fn scaffolded() { assert!(!super::LAYER.is_empty()); }
}
