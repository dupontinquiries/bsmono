//! verifyplan.rs — what a change obliges you to check, derived from the paths it touched.
//!
//! This repository has 47 gates, two mutually-exclusive feature flags, and a workspace where a shader
//! edit in `gpu-gemm` can break a build in `logscope-egui`. Choosing the verification set from memory
//! fails in a specific, repeatable way: **you run the checks you were thinking about.**
//!
//! Two failures from one session:
//!
//! * added `--gate-quantref`, ran the gate, shipped — the advertised-gate-list test was red and I did
//!   not run the suite that contains it
//! * edited `tabs/chat.rs` behind `wgpu-engine`, checked that build, shipped — the `gui`-only build
//!   did not compile because `gpu_kernels` is not linked there
//!
//! Both were caught, one by a guard I had written and one by a later run. Neither was caught by me
//! deciding what to check.
//!
//! ## The plan is obligations, not suggestions
//!
//! Every entry names the failure it prevents. A checklist whose items do not say why they exist gets
//! trimmed the first time someone is in a hurry.

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Obligation {
    /// The command to run.
    pub check: String,
    /// What it catches. **Required** — an unexplained check is the first one skipped.
    pub because: &'static str,
}

/// What must be verified, given the paths a change touched.
pub fn plan(paths: &[&str]) -> Vec<Obligation> {
    let mut v: Vec<Obligation> = Vec::new();
    let touched = |frag: &str| paths.iter().any(|p| p.contains(frag));
    let mut push = |check: String, because: &'static str| {
        if !v.iter().any(|o| o.check == check) { v.push(Obligation { check, because }); }
    };

    // A shader is a string literal in lib.rs, so "did gpu-gemm change" cannot distinguish it — but a
    // WGSL edit is invisible to `cargo check` and only fails at pipeline creation, on hardware.
    if touched("gpu-gemm") {
        push("cargo test -p gpu-gemm --lib".into(),
             "unit tests, including the advertised-gate-list guard that catches a new --gate-* \
dispatched without being listed");
        push("kernel-validate --gate-arch".into(),
             "shader compilation — a WGSL edit passes cargo check and fails at pipeline creation");
    }
    // Quantizer changes need the widths that actually exercise ties.
    if touched("quantref") || touched("gguf.rs") || touched("q2demote") {
        push("kernel-validate --gate-quantref".into(),
             "the reciprocal and tie rules, before they propagate to a format that does not exist yet");
        push("kernel-validate --gate-q4k".into(),
             "the large-tensor case — 112 blocks hit no ties and 70000 hit 1104, so a small test \
reports the tie rule as correct");
        push("kernel-validate --gate-q6k".into(), "the other scalar width, which shares the fix sites");
    }
    if touched("memplan") || touched("store.rs") {
        push("kernel-validate --gate-memplan".into(),
             "budget and file-view decisions, where a wrong answer refuses a model that fits");
        push("kernel-validate --gate-store".into(),
             "cache epochs — a stale tsv serves verdicts from a build that no longer exists");
    }
    // BOTH feature builds. This is the one that gets skipped.
    if touched("logscope-egui") {
        push("cargo check -p logscope-egui --features wgpu-engine".into(),
             "the engine build — where a shader constant, a LinKind arm or a dims layout change first surfaces as a type error rather than as wrong output");
        push("cargo check -p logscope-egui --features gui".into(),
             "the GUI-ONLY build, where gpu_kernels is not linked — an engine-gated block added \
without a cfg compiles in one and not the other");
    }
    if touched("logscope/src") {
        push("cargo test -p logscope --lib".into(), "the policy crate's own tests");
    }
    // Anything touching the MoE path can produce plausible wrong text rather than a crash, which no
    // build check finds.
    if touched("moe") || touched("expert") || touched("prescale") {
        push("LOGSCOPE_MOE_V2_CHECK=1 <a real generation>".into(),
             "v1 against the routed path on the same activations — the MoE failure mode is silently \
wrong text, and nothing in CI can see it");
    }
    v
}

/// Obligations that no static plan can discharge, because they need the hardware.
///
/// **Named separately so they are not quietly counted as done.** lavapipe divides correctly, rounds
/// like Rust, does not contract FMAs and handles lane-varying shifts — it has passed at every step of
/// bugs that were live on an M2.
pub const HARDWARE_ONLY: [&str; 3] = [
    "quantization exactness — lavapipe's arithmetic differs from Honeykrisp's in four measured ways",
    "GPU timeouts — they appear in journalctl and never in the application log",
    "concurrent-buffer ceiling — the probe allocates and releases; the arena has to hold",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn checks(paths: &[&str]) -> Vec<String> { plan(paths).into_iter().map(|o| o.check).collect() }

    #[test]
    fn an_egui_change_demands_BOTH_feature_builds() {
        // The one that gets skipped. I edited tabs/chat.rs behind wgpu-engine, checked that build,
        // and shipped a tree where the gui-only build did not compile.
        let c = checks(&["crates/logscope-egui/src/tabs/chat.rs"]);
        assert!(c.iter().any(|x| x.contains("--features gui")), "{c:?}");
        assert!(c.iter().any(|x| x.contains("--features wgpu-engine")));
    }

    #[test]
    fn a_gpu_gemm_change_demands_the_SUITE_not_just_the_gate() {
        // Adding --gate-quantref, running it, and shipping left the advertised-gate-list test red.
        // Running the gate proves the gate; running the suite proves the repo.
        let c = checks(&["crates/gpu-gemm/src/lib.rs"]);
        assert!(c.iter().any(|x| x.contains("cargo test -p gpu-gemm")));
    }

    #[test]
    fn a_quantizer_change_demands_the_LARGE_tensor_case() {
        // 112 blocks hit no ties and 70000 hit 1104, so a small check reports the tie rule as
        // correct — which it did, for weeks.
        let o = plan(&["crates/gpu-gemm/src/quantref.rs"]);
        let q4k = o.iter().find(|x| x.check.contains("q4k")).expect("q4k required");
        assert!(q4k.because.contains("70000"));
    }

    #[test]
    fn every_obligation_says_WHY() {
        // A checklist whose items do not explain themselves gets trimmed the first time someone is in
        // a hurry, and the trimmed one is always the one that would have caught it.
        for o in plan(&["crates/gpu-gemm/src/lib.rs", "crates/logscope-egui/src/x.rs",
                        "crates/logscope/src/y.rs", "crates/gpu-gemm/src/gguf.rs"]) {
            assert!(!o.because.is_empty(), "{}", o.check);
            assert!(o.because.len() > 20, "too thin to survive a hurry: {}", o.because);
        }
    }

    #[test]
    fn a_MOE_change_demands_a_real_generation() {
        // Its failure mode is plausible wrong text, which no build or unit test can see.
        let o = plan(&["crates/gpu-gemm/src/moe.rs"]);
        let m = o.iter().find(|x| x.check.contains("MOE_V2_CHECK")).expect("required");
        assert!(m.because.contains("silently"));
    }

    #[test]
    fn HARDWARE_ONLY_items_are_kept_OUT_of_the_plan() {
        // So they cannot be quietly counted as discharged. lavapipe has passed at every step of bugs
        // that were live on an M2.
        let c = checks(&["crates/gpu-gemm/src/lib.rs"]);
        assert!(!c.iter().any(|x| x.contains("journalctl")));
        assert!(HARDWARE_ONLY.iter().any(|x| x.contains("journalctl")));
    }

    #[test]
    fn an_unrelated_path_demands_nothing() {
        assert!(plan(&["README.md"]).is_empty());
    }
}
