//! cad-pool — pool.toml manifests + harvest/diagnose/audit/shrink/install verbs; game adapters per games/EDITORS.md.
//!
//! Scaffolded at M0 so the workspace shape (and dependency direction) is fixed from day one;
//! see apps/cadscope/PARITY.md and the milestone plan in CADSCOPE_PROMPT.md.

/// Milestone at which this crate gains its real implementation.
pub const MILESTONE: &str = "M5";

#[cfg(test)]
mod tests {
    #[test]
    fn scaffolded() { assert!(!super::MILESTONE.is_empty()); }
}
