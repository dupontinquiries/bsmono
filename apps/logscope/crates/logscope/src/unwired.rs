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

/// Strip `#[cfg(test)]` regions, so a test reference does not count as wiring.
fn without_tests(src: &str) -> String {
    let (mut out, mut in_test, mut depth, mut depth_at) = (String::new(), false, 0i32, 0i32);
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("#[cfg(test)]") { in_test = true; depth_at = depth; }
        depth += line.matches('{').count() as i32 - line.matches('}').count() as i32;
        if in_test && depth <= depth_at && t.contains('}') { in_test = false; continue }
        if !in_test { out.push_str(line); out.push('\n'); }
    }
    out
}

/// Build the usage map across several crate source roots.
///
/// **Several roots, not one.** The first version of this analysis scanned `logscope/src` alone and
/// reported 742 unwired items; 208 of those were used from `logscope-egui` and `gpu-gemm`. A scan
/// that reports a third false positives is the noise this module warns about, delivered by the
/// module itself.
pub fn scan_roots(roots: &[&std::path::Path], define_in: &std::path::Path) -> Vec<Usage> {
    let mut bodies: Vec<(String, String)> = Vec::new();
    for r in roots {
        let Ok(rd) = std::fs::read_dir(r) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("rs") { continue }
            let (Some(stem), Ok(src)) = (p.file_stem().and_then(|x| x.to_str()),
                                         std::fs::read_to_string(&p)) else { continue };
            bodies.push((format!("{}/{stem}", r.display()), without_tests(&src)));
        }
    }
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(define_in) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("rs") { continue }
        let (Some(stem), Ok(src)) = (p.file_stem().and_then(|x| x.to_str()),
                                     std::fs::read_to_string(&p)) else { continue };
        if stem == "lib" || stem == "main" { continue }
        let body = without_tests(&src);
        let key = format!("{}/{stem}", define_in.display());
        for line in body.lines() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("pub ") else { continue };
            let name: Option<&str> = ["fn ", "struct ", "enum ", "const ", "static ", "trait "]
                .iter().find_map(|kw| rest.strip_prefix(*kw))
                .map(|a| a.split(|c: char| !(c.is_alphanumeric() || c == '_')).next().unwrap_or(""));
            let Some(name) = name.filter(|n| n.len() >= 4) else { continue };
            let used_from: Vec<String> = bodies.iter()
                .filter(|(m, b)| m != &key && b.contains(name))
                .map(|(m, _)| m.clone()).collect();
            out.push(Usage { name: name.to_string(), defined_in: key.clone(), used_from });
        }
    }
    out
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Runs on the real tree every `cargo test`, and asserts a CEILING.
    ///
    /// Zero is not the target and never will be: some `pub` items genuinely are API, and demanding
    /// zero produces a test people delete. **The ceiling catches the next feature that is built and
    /// never connected** without relitigating the 534 that already exist.
    #[test]
    fn unwired_items_do_not_grow() {
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let me = base.join("src");
        let egui = base.parent().map(|p| p.join("logscope-egui/src"));
        let gpu = base.parent().map(|p| p.join("gpu-gemm/src"));
        let mut roots: Vec<&std::path::Path> = vec![&me];
        if let Some(e) = egui.as_deref() { if e.exists() { roots.push(e) } }
        if let Some(g) = gpu.as_deref() { if g.exists() { roots.push(g) } }
        let u = scan_roots(&roots, &me);
        if u.is_empty() { return }   // not a source checkout
        let n = u.iter().filter(|x| !x.wired() && !super::is_expected(&x.name)).count();
        // 594 measured by THIS implementation. My python prototype said 534 — it matched
        // identifiers differently and I nearly set the ceiling from the estimate rather than from
        // the code that runs. A ceiling taken from a different implementation's number fails on the
        // first honest run, which is how a useful test gets deleted as flaky.
        // 602. The two added by peerrecord are a TRUE POSITIVE and are recorded rather than hidden:
        // that module defines a signed DHT record and nothing calls it, because the DHT it belongs
        // to does not exist yet — snapclone-dht's announce.rs is a seven-line TODO.
        //
        // Raising a ceiling for genuinely unwired code is the right move ONLY with the reason
        // attached, which is the difference between a ratchet and a rubber stamp. When
        // reachability starts consuming validate_record, this should come back down.
        const CEILING: usize = 602;
        assert!(n <= CEILING,
            "{n} unwired public items, ceiling {CEILING}. Something was built and not connected — \
which looks finished from every angle a check can see, and is why this ceiling exists rather than a \
note in a commit message");
    }

    #[test]
    fn the_scan_sees_MORE_THAN_ONE_crate() {
        // The first version scanned one crate and reported 742, of which 208 were used from
        // logscope-egui and gpu-gemm. A scan that is a third false positives is the noise this
        // module warns about, delivered by the module itself.
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let me = base.join("src");
        let egui = base.parent().map(|p| p.join("logscope-egui/src"));
        if !me.exists() { return }
        let solo = scan_roots(&[&me], &me);
        let Some(e) = egui.as_deref().filter(|p| p.exists()) else { return };
        let both = scan_roots(&[&me, e], &me);
        let count = |v: &[Usage]| v.iter().filter(|x| !x.wired()).count();
        assert!(count(&both) < count(&solo),
            "scanning logscope-egui too must find consumers the single-crate scan missed");
    }
}
