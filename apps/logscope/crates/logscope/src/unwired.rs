//! unwired.rs — public items that exist and are never called.
//!
//! A module that compiles, passes its tests and is referenced by nothing is not half-done work. It
//! is **work that looks done from every angle a check can see**: green tests, clean build, a commit
//! message describing a feature. The only thing missing is that it never runs.
//!
//! This session produced several — `MOE_MATVEC_IQ1_S` with six shape tests and no dispatch,
//! `MoeModeChoice` deciding routed-versus-dense with nothing asking it, `add_landmark` with no scene
//! calling it — and each was recorded honestly in its commit and then not tracked anywhere a later
//! session would see.
//!
//! ## Why `dead_code` does not cover this
//!
//! Rust's lint fires on private items. A `pub` item in a library crate is assumed to be someone's
//! API and never warned about, which is correct for a library and wrong for an application's
//! internal modules — where `pub` mostly means "another module might want this" rather than "this is
//! published".
//!
//! ## What counts as wired
//!
//! Referenced from a module OTHER than the one defining it, outside `#[cfg(test)]`. **A test calling
//! it does not count**, and that is the whole point: every one of the items above has passing tests.

/// Where a name appears.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Usage { pub name: String, pub defined_in: String, pub used_from: Vec<String> }

impl Usage {
    /// Called from somewhere other than its own module, in non-test code?
    pub fn wired(&self) -> bool {
        self.used_from.iter().any(|m| m != &self.defined_in)
    }

    pub fn explain(&self) -> String {
        if self.wired() {
            format!("{} is called from {}", self.name, self.used_from.join(", "))
        } else if self.used_from.is_empty() {
            format!("{} is defined in {} and referenced NOWHERE — it compiles, its tests pass, and \
it never runs", self.name, self.defined_in)
        } else {
            format!("{} is defined in {} and used only there. That may be correct for a helper, and \
it is also what a feature nobody connected looks like", self.name, self.defined_in)
        }
    }
}

/// Names that are expected to be unwired and should not be reported.
///
/// **An allowlist is what keeps this usable.** Constants read by name from the shell, types
/// constructed only by serde, and genuinely public API all live here — and without it the report is
/// long enough to ignore, which is the failure mode of every scan that cries wolf.
pub const EXPECTED_UNWIRED: &[&str] = &[
    // Env-var names and gate strings are consumed by the shell, not by Rust.
    "LOGSCOPE_", "GATE_",
    // Test scaffolding that is deliberately available to other crates' tests.
    "fixture", "sample_",
];

fn is_expected(name: &str) -> bool {
    EXPECTED_UNWIRED.iter().any(|p| name.starts_with(p) || name.contains(p))
}

/// Report the unwired ones, worst first.
///
/// Sorted by definition site so a reader sees whole modules at a time — a module with eight unwired
/// items is a different problem from eight modules with one each, and the first is usually a feature
/// that was built and never landed.
pub fn report(usages: &[Usage]) -> String {
    let mut unwired: Vec<&Usage> = usages.iter()
        .filter(|u| !u.wired() && !is_expected(&u.name)).collect();
    if unwired.is_empty() {
        return "every public item is referenced from outside its own module".into();
    }
    unwired.sort_by(|a, b| a.defined_in.cmp(&b.defined_in).then(a.name.cmp(&b.name)));
    let mut by_module: std::collections::BTreeMap<&str, usize> = Default::default();
    for u in &unwired { *by_module.entry(&u.defined_in).or_insert(0) += 1; }
    let mut out = vec![format!("{} public item(s) are never called from another module:",
        unwired.len())];
    for u in unwired.iter().take(20) {
        out.push(format!("  {}::{}", u.defined_in, u.name));
    }
    if let Some((m, n)) = by_module.iter().max_by_key(|(_, n)| **n) {
        if *n >= 3 {
            out.push(format!("{m} has {n} of them — a module with several is usually a feature that \
was built and never connected, rather than a few stray helpers"));
        }
    }
    out.push("A test calling it does not count as wired, and that is the point: work that compiles, \
passes its tests and never runs looks finished from every angle a check can see".into());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(name: &str, def: &str, used: &[&str]) -> Usage {
        Usage { name: name.into(), defined_in: def.into(),
                used_from: used.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn a_TEST_calling_it_does_not_count_as_wired() {
        // Every item this was written for has passing tests. MOE_MATVEC_IQ1_S has six.
        let it = u("MOE_MATVEC_IQ1_S", "lib", &["lib"]);
        assert!(!it.wired());
        assert!(it.explain().contains("what a feature nobody connected looks like"));
    }

    #[test]
    fn something_referenced_NOWHERE_is_named_most_bluntly() {
        // "Compiles, its tests pass, and it never runs" is the sentence that makes the problem
        // visible, because each clause on its own reads as success.
        let it = u("add_landmark", "scene3d", &[]);
        assert!(it.explain().contains("it never runs"));
    }

    #[test]
    fn a_WIRED_item_is_not_reported() {
        assert!(u("dequant_q3_k", "kquant", &["gguf"]).wired());
        assert!(report(&[u("dequant_q3_k", "kquant", &["gguf"])]).contains("every public item"));
    }

    #[test]
    fn the_ALLOWLIST_keeps_the_report_short_enough_to_read() {
        // Without it the list is long enough to ignore, which is the failure mode of every scan that
        // cries wolf — and this project has already decided that a scan reporting noise gets
        // ignored.
        let items = vec![u("LOGSCOPE_MOE_STREAM", "lib", &[]), u("real_thing", "m", &[])];
        let r = report(&items);
        assert!(!r.contains("LOGSCOPE_MOE_STREAM"));
        assert!(r.contains("real_thing"));
    }

    #[test]
    fn a_module_with_SEVERAL_unwired_items_is_called_out_as_a_feature_not_strays() {
        // Eight in one module is a feature that was built and never landed. One each in eight
        // modules is housekeeping, and conflating them wastes the reader's judgement.
        let items: Vec<Usage> = (0..4).map(|i| u(&format!("thing{i}"), "machinery", &[])).collect();
        assert!(report(&items).contains("built and never connected"));
    }

    #[test]
    fn the_report_is_ORDERED_by_module_so_whole_features_read_together() {
        let items = vec![u("b", "zed", &[]), u("a", "alpha", &[])];
        let r = report(&items);
        assert!(r.find("alpha::a").unwrap() < r.find("zed::b").unwrap());
    }
}
