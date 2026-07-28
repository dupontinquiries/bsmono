//! enginelevers.rs — the registry of engine optimizations: default, why, platform, and what validates it.
//!
//! The engine has accumulated levers with OPPOSITE default polarities and nothing recording which is
//! which. Some read `!= Ok("0")` (on unless disabled), some read `== Ok("1")` (off unless enabled), and
//! the reason for each choice lived only in the commit that added it. That is fine until someone asks
//! "which optimizations am I actually running, and which are unproven?" — at which point the answer
//! requires reading the source of every call site.
//!
//! ## The rule this encodes
//!
//! **An optimization may only be ON BY DEFAULT if something validates it.** A fusion that changes results
//! is a correctness bug shipped to everyone; a fusion nobody checked is a correctness bug waiting. So
//! every `Default::On` entry names the gate that proves it, and a test enforces that — a new lever cannot
//! be defaulted on without either a gate or an explicit, argued exemption.
//!
//! Off-by-default is the safe state and needs no gate, only a reason.

/// What the engine does when the variable is unset.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Default_ {
    /// Active unless explicitly disabled with `=0`.
    On,
    /// Inactive unless explicitly enabled with `=1`.
    Off,
    /// Chosen at runtime by measurement or hardware detection, not by a fixed default.
    Auto,
}

/// Why a lever is restricted to some hardware.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    /// Works anywhere the engine runs.
    Any,
    /// Disabled on Asahi/Honeykrisp Vulkan, where it hangs or misbehaves.
    NotAsahi,
    /// Requires a device feature that may be absent; detected at load.
    NeedsFeature(&'static str),
}

#[derive(Clone, Copy, Debug)]
pub struct Lever {
    pub var: &'static str,
    pub default: Default_,
    pub platform: Platform,
    /// The gate that proves it does not change results. `None` is only permitted for `Off` levers.
    pub validated_by: Option<&'static str>,
    /// Why this default. Present for every entry: a default without a reason is one nobody can revisit.
    pub why: &'static str,
}

/// Every performance-affecting lever the engine reads.
///
/// Diagnostic and UI variables are deliberately excluded — this is the list that changes what the model
/// computes or how fast, which is the list someone auditing a run needs.
pub const LEVERS: &[Lever] = &[
    // NOTE: --gate-qkv's per-projection checks all pass bit-exactly, but its END-TO-END check
    // ("fused decode == separate-projection decode") is currently RED in the lavapipe container. The
    // fusion itself is proven; what is unproven is the whole decode path with it enabled. Recorded here
    // rather than quietly left on, because "a gate exists" and "the gate passes" are different claims and
    // this registry is worthless if it conflates them.
    Lever { var: "LOGSCOPE_FUSED_QKV", default: Default_::On, platform: Platform::Any,
        validated_by: Some("--gate-qkv (per-projection green; end-to-end check RED, unresolved)"),
        why: "one dispatch instead of three for Q/K/V; bit-exact, so there is no reason to withhold it" },
    Lever { var: "LOGSCOPE_FUSE_NORM", default: Default_::On, platform: Platform::Any,
        validated_by: Some("--gate-matvec"),
        why: "folds the norm into the following matvec in the m=1 decode case; bit-exact" },
    Lever { var: "LOGSCOPE_FLASH_ATTN", default: Default_::On, platform: Platform::Any,
        validated_by: Some("--gate-prefill"),
        why: "tiled attention with online softmax; argmax-identical and much less memory traffic" },
    Lever { var: "LOGSCOPE_BG_CACHE", default: Default_::Off, platform: Platform::Any,
        validated_by: Some("--gate-moe-fusion"),
        why: "OFF because measurement said so, not because it is unsafe: the A/B on Apple M2 Ultra came \
back +0.5% (92.7 vs 92.2 ms/tok), which is run-to-run noise. The autotuner may still turn it on where it \
helps" },
    Lever { var: "LOGSCOPE_FUSE_GLU", default: Default_::Off, platform: Platform::Any,
        validated_by: Some("--gate-moe-fusion"),
        why: "same A/B verdict as the bind-group cache; bit-exact but not measurably faster on the \
hardware tested" },
    Lever { var: "LOGSCOPE_FUSE_DOWN", default: Default_::Off, platform: Platform::Any,
        validated_by: Some("--gate-moe-fusion"),
        why: "fuses the expert down-projection with its weighted accumulate. Bit-exact by the same gate, and off for the same measured reason as FUSE_GLU — but listed separately because they can be enabled independently and a future device may want one and not the other" },
    Lever { var: "LOGSCOPE_MOE_PREFETCH", default: Default_::Off, platform: Platform::Any,
        validated_by: Some("--gate-moe-prefetch"),
        why: "cross-layer expert prefetch (Colibri couple_prefetch). Off by default because it only pays \
when experts stream AND the coupling table has learned something; a cold table issues useless fills" },
    // --gate-matvec's subgroup rows fail on lavapipe at rel ~2.5. That gate exercises the kernel
    // DIRECTLY, bypassing the self-check the engine uses to decide whether to select it — so a red row
    // here means "this driver's subgroups are broken", which is information, not a defect.
    Lever { var: "LOGSCOPE_MATVEC_SG", default: Default_::Off, platform: Platform::NotAsahi,
        validated_by: Some("--gate-matvec (subgroup rows RED on lavapipe — driver, not engine)"),
        why: "subgroup matvec. Force-on only: the loader DISABLES it on Asahi/Honeykrisp, where subgroup \
support can hang large matmuls. This variable exists to re-enable it for bisection, which is why a config \
file must not be able to override it" },
    Lever { var: "LOGSCOPE_MOE_STREAM", default: Default_::Auto, platform: Platform::Any,
        validated_by: Some("--gate-moe-stream"),
        why: "expert streaming engages automatically when the model will not fit resident — a decision \
that depends on the model and the device, so a fixed default would be wrong for one of them" },
    Lever { var: "LOGSCOPE_ARENA", default: Default_::Auto, platform: Platform::Any,
        validated_by: Some("--gate-store"),
        why: "slab-packing resident weights. Auto-on for BO-VMA-constrained drivers, where buffer-object \
count is the binding limit; unnecessary elsewhere" },
    Lever { var: "LOGSCOPE_STAGE_FENCE", default: Default_::On, platform: Platform::Any,
        validated_by: Some("--gate-moe-stream (bit-exact both ways; the hazard needs deep queues)"),
        why: "the expert staging ring fences on COMPLETION by default. Setting it to `submit` restores \
the weaker submission-only fence for A/B — the wait costs real time, and the claim that it stops the \
Asahi GPU timeouts deserves a measurement rather than an argument. Default is the safe side because a \
slot overwritten under a running dispatch corrupts weights silently" },
    Lever { var: "LOGSCOPE_NO_Q4K_GPU", default: Default_::Off, platform: Platform::Any,
        validated_by: Some("--gate-q4k"),
        why: "an ESCAPE HATCH, not an optimization: setting it restores the CPU conversion path. Named \
negatively so the fast path is what you get by not thinking about it" },
    Lever { var: "LOGSCOPE_ZEROCOPY", default: Default_::Off,
        platform: Platform::NeedsFeature("VK_EXT_external_memory_host"),
        validated_by: Some("--gate-zerocopy"),
        why: "importing host memory directly. Measured ABSENT on Honeykrisp, so it stays off there by \
detection rather than by default — llama.cpp cannot import on that device either" },
];

/// Levers that are on by default without anything validating them. Must be empty.
pub fn unvalidated_defaults() -> Vec<&'static str> {
    LEVERS.iter().filter(|l| l.default == Default_::On && l.validated_by.is_none())
        .map(|l| l.var).collect()
}

/// Levers whose availability depends on the device, for a loader that must probe before trusting them.
pub fn needs_platform_detection() -> Vec<&'static Lever> {
    LEVERS.iter().filter(|l| !matches!(l.platform, Platform::Any)).collect()
}

/// A human-readable audit of what a run is actually using.
pub fn audit() -> Vec<String> {
    LEVERS.iter().map(|l| {
        let d = match l.default { Default_::On => "ON", Default_::Off => "off", Default_::Auto => "auto" };
        let p = match l.platform {
            Platform::Any => String::new(),
            Platform::NotAsahi => " [disabled on Asahi]".into(),
            Platform::NeedsFeature(f) => format!(" [needs {f}]"),
        };
        let v = l.validated_by.map(|g| format!(" · {g}")).unwrap_or_else(|| " · UNVALIDATED".into());
        format!("{:<26} {d}{p}{v}", l.var)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_on_by_default_without_a_gate_proving_it() {
        // The rule the module exists for. A fusion that changes results is a correctness bug shipped to
        // everyone; one nobody checked is the same bug waiting to be found by a user.
        let bad = unvalidated_defaults();
        assert!(bad.is_empty(), "on by default with nothing validating them: {bad:?}");
    }

    #[test]
    fn every_lever_states_a_reason() {
        // A default without a reason cannot be revisited: the next person cannot tell whether it was
        // measured, assumed, or a workaround for a bug that has since been fixed.
        for l in LEVERS {
            assert!(l.why.len() > 40, "{} needs a real reason, got {:?}", l.var, l.why);
            assert!(l.var.starts_with("LOGSCOPE_"));
        }
    }

    #[test]
    fn off_by_default_is_distinguished_from_unsafe() {
        // BG_CACHE and the MoE fusions are OFF because an A/B measured them as noise, NOT because they
        // are unsafe — all three are gate-proven bit-exact. Conflating "not worth it here" with "not
        // trusted" would stop someone enabling them on hardware where they do pay.
        for v in ["LOGSCOPE_BG_CACHE", "LOGSCOPE_FUSE_GLU", "LOGSCOPE_FUSE_DOWN"] {
            let l = LEVERS.iter().find(|l| l.var == v).unwrap();
            assert_eq!(l.default, Default_::Off);
            assert!(l.validated_by.is_some(), "{v} is proven correct even though it is off");
        }
        let bg = LEVERS.iter().find(|l| l.var == "LOGSCOPE_BG_CACHE").unwrap();
        assert!(bg.why.contains("not because it is unsafe"), "the distinction must be explicit");
    }

    #[test]
    fn platform_gated_levers_name_their_condition() {
        let p = needs_platform_detection();
        assert!(!p.is_empty());
        for l in &p {
            match l.platform {
                Platform::NotAsahi => assert!(l.why.contains("Asahi"), "{} must say why", l.var),
                Platform::NeedsFeature(f) => assert!(l.why.contains(f) || l.why.contains("Honeykrisp"),
                    "{} must name the feature it needs", l.var),
                Platform::Any => unreachable!(),
            }
        }
        // The subgroup lever is force-on-only BY DESIGN: the driver check must win over a config file,
        // because it exists to be turned during a bisection.
        let sg = LEVERS.iter().find(|l| l.var == "LOGSCOPE_MATVEC_SG").unwrap();
        assert!(sg.why.contains("bisection"));
    }

    #[test]
    fn auto_levers_explain_what_they_decide_from() {
        // "Auto" is the easiest place to hide an unexamined heuristic, so each must say what it measures.
        for l in LEVERS.iter().filter(|l| l.default == Default_::Auto) {
            assert!(l.why.contains("depends") || l.why.contains("automatically") || l.why.contains("Auto-on"),
                "{} is auto but does not say what it decides from: {}", l.var, l.why);
        }
    }

    #[test]
    fn the_audit_marks_the_escape_hatch_as_such() {
        // LOGSCOPE_NO_Q4K_GPU is negatively named so the fast path is the default. Reading it as an
        // optimization to enable would be exactly backwards.
        let l = LEVERS.iter().find(|l| l.var == "LOGSCOPE_NO_Q4K_GPU").unwrap();
        assert!(l.why.contains("ESCAPE HATCH"));
        let a = audit();
        assert!(a.iter().any(|s| s.contains("LOGSCOPE_NO_Q4K_GPU")));
        assert!(!a.iter().any(|s| s.contains("UNVALIDATED")), "nothing should be unvalidated: {a:?}");
    }
}
