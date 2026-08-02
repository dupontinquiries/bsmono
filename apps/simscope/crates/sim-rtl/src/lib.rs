//! sim-rtl — Level 4: RTL/logic evaluation. Bit-exact, no floating point, CPU-bound.
//!
//! Scaffolded so the layer boundaries are fixed before anything grows across them. See
//! apps/simscope/PLAN.md for what this crate owes and what it may depend on.

/// Which layer of the stack this crate implements.
pub const LAYER: &str = "L4";

#[cfg(test)]
mod tests {
    #[test]
    fn scaffolded() { assert!(!super::LAYER.is_empty()); }
}
