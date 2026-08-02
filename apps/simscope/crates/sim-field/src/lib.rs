//! sim-field — Level 1: 2D field solve (Laplace/Poisson) on triangles -> RLGC extraction. The FEA on-ramp.
//!
//! Scaffolded so the layer boundaries are fixed before anything grows across them. See
//! apps/simscope/PLAN.md for what this crate owes and what it may depend on.

/// Which layer of the stack this crate implements.
pub const LAYER: &str = "L1";

#[cfg(test)]
mod tests {
    #[test]
    fn scaffolded() { assert!(!super::LAYER.is_empty()); }
}
