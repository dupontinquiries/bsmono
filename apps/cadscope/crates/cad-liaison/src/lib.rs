//! cad-liaison — the sanitization boundary (PartSpec/OpScript), logscope-fan dispatch adapters, liaison facts, genviz-style progress.
//!
//! Scaffolded at M0 so the workspace shape (and dependency direction) is fixed from day one;
//! see apps/cadscope/PARITY.md and the milestone plan in CADSCOPE_PROMPT.md.

/// Milestone at which this crate gains its real implementation.
pub const MILESTONE: &str = "M3";

#[cfg(test)]
mod tests {
    #[test]
    fn scaffolded() { assert!(!super::MILESTONE.is_empty()); }
}
