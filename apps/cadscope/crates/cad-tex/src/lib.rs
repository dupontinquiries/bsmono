//! cad-tex — material graphs, triplanar + auto-unwrap, bake to the <name>.png/norm_/spec_ convention, audit/dedupe (texture_export_v13 + texture_audit_v2 lineage).
//!
//! Scaffolded at M0 so the workspace shape (and dependency direction) is fixed from day one;
//! see apps/cadscope/PARITY.md and the milestone plan in CADSCOPE_PROMPT.md.

/// Milestone at which this crate gains its real implementation.
pub const MILESTONE: &str = "M4";

#[cfg(test)]
mod tests {
    #[test]
    fn scaffolded() { assert!(!super::MILESTONE.is_empty()); }
}
