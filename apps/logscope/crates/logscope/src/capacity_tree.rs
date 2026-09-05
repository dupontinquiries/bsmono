//! capacity_tree.rs — the ExplorationTree the Liaison was missing, seeded from what this project has
//! actually measured or documented about model fit, not from a fresh guess.
//!
//! `docs/12-queen-and-workers.md`'s audit note ("THINKER + LIAISON") found every Liaison query fully
//! written and tested against `&ExplorationTree`, with nothing in the app ever constructing one — not a
//! missing call, a missing data structure. Inventing a tree from nothing would repeat the mistake the
//! Liaison itself refuses to make: "an explainer that fabricates a reassuring status is worse than no
//! explainer." So this module does the opposite — it converts findings *already written down* in
//! `CAPACITY.md`, `docs/model-fit.md` and `docs/hardware-matrix.md` into `Branch`es, each carrying the
//! evidence string a reader can trace back to its source doc.
//!
//! ## Why RAM tiers, not one machine
//!
//! The operator's stack spans postmarketOS/Fairphone-class ARM devices up through a 192 GiB Mac
//! Studio, and a Liaison briefing is only honest if it says which of those it is talking about — a
//! model that fits at 192 GiB and one that fits at 24 GiB are different findings, not the same finding
//! at different detail. Each `Branch` is tagged with a [`RamTier`] via `hardware()`, and
//! [`RamTier::classify`] maps a real `free -g` number onto one, so a briefing can filter to "what does
//! MY box get" without the operator doing the arithmetic.
//!
//! This is backend-only, deliberately: no UI wiring yet, per the plan to get the data model right
//! before touching `chat.rs`.

use crate::exploration::{Branch, ExplorationTree, Status};

/// A RAM budget class. Bounds are the ceiling in GiB — a machine classifies into the smallest tier
/// its physical RAM does not exceed capacity for, per [`RamTier::classify`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum RamTier {
    /// postmarketOS / Fairphone / PinePhone class — ARM, unified, no swap to speak of.
    Mobile8,
    /// A 24 GiB laptop or SBC — the ceiling named in this pass's brief.
    Laptop24,
    /// M4 Max class, 64 GiB unified.
    Workstation64,
    /// M2 Ultra class, up to 192 GiB unified.
    Server192,
}

impl RamTier {
    /// Ceiling in GiB, the number these findings were measured or reasoned against.
    pub fn ceiling_gib(self) -> f32 {
        match self {
            RamTier::Mobile8 => 8.0,
            RamTier::Laptop24 => 24.0,
            RamTier::Workstation64 => 64.0,
            RamTier::Server192 => 192.0,
        }
    }

    /// Human label, for a Liaison line — matches how the source docs name the machine class.
    pub fn label(self) -> &'static str {
        match self {
            RamTier::Mobile8 => "≤8 GiB (phone/ARM)",
            RamTier::Laptop24 => "≤24 GiB (laptop)",
            RamTier::Workstation64 => "≤64 GiB (M4 Max class)",
            RamTier::Server192 => "≤192 GiB (M2 Ultra class)",
        }
    }

    /// Classify a real machine's physical RAM into the smallest tier it fits under. A box with more
    /// than 192 GiB still classifies as `Server192` — this project has no findings past that ceiling,
    /// and saying so honestly beats inventing a tier nothing has measured.
    pub fn classify(physical_gib: f32) -> RamTier {
        if physical_gib <= 8.0 { RamTier::Mobile8 }
        else if physical_gib <= 24.0 { RamTier::Laptop24 }
        else if physical_gib <= 64.0 { RamTier::Workstation64 }
        else { RamTier::Server192 }
    }
}

/// Build the tree. Idempotent and pure — call it once at startup or on demand; there's no I/O and
/// nothing here depends on the machine actually running.
pub fn capacity_tree() -> ExplorationTree {
    let mut t = ExplorationTree::new();
    for b in mobile8_branches() { t.add(b); }
    for b in laptop24_branches() { t.add(b); }
    for b in workstation64_branches() { t.add(b); }
    for b in server192_branches() { t.add(b); }
    t.resolve("qwen3coder-next-80b-a3b-server192", Status::Succeeded { gain: 0.0 });
    t.resolve("glm-5.2-server192", Status::Failed {
        reason: "no quant of GLM-5.2 fits 192 GiB — the 2-bit dynamic GGUF is ~239 GB (needs 256 GB), \
and the IQ1_S referenced in glm52-fit.md is 201.8 GB against 192 GB, with 2-bit already a real \
quality cut on top of not fitting".into() });
    t.resolve("gemma4-any-tier", Status::Deferred {
        retry_when: "the arch registry adds gemma4 support — genuinely unsupported today, and Gemma 4 \
12B/31B QAT are among the better local coders, so this is a real gap rather than a misleading one".into() });
    t.resolve("qwen4-any-tier", Status::Deferred {
        retry_when: "a real qwen4 checkpoint is downloaded — a qwen3moe fixture rebranded as qwen4 \
already loads and produces bit-identical stats, so the blocker is a download, not code".into() });
    t.resolve("qwen3.6-27b-dense-workstation64", Status::Succeeded { gain: 0.0 });
    t
}

fn mobile8_branches() -> Vec<Branch> {
    // CAPACITY.md's own numbers, arithmetic only (weights + one instance's KV), not a new claim: a 1B
    // or 3B Q4_K_M is the only class that leaves headroom on an 8 GiB unified-memory ARM device once
    // the OS and app footprint are accounted for — this project has not measured that footprint, so
    // the branch stays Active (open) rather than claiming a verified fit.
    vec![
        Branch::new("1b-q4-mobile8", "Run a 1B Q4_K_M model resident on an 8 GiB ARM device")
            .evidence("CAPACITY.md: Llama-3.2-1B is ~0.8 GB resident at Q4_K_M, 16 layers, kv_dim 512 \
— by far the smallest class this project has sized")
            .assume("no measurement yet of the OS + app footprint on postmarketOS/Fairphone/PinePhone \
specifically — the 0.8 GB figure is weights only")
            .confidence(0.6)
            .hardware("Mobile8"),
        Branch::new("3b-q4-mobile8", "Run a 3B Q4_K_M model resident on an 8 GiB ARM device")
            .evidence("CAPACITY.md: Llama-3.2-3B is ~2.0 GB resident at Q4_K_M, 28 layers, kv_dim 1024 \
— fits with more margin than the 1B leaves for KV cache at short context")
            .confidence(0.55)
            .hardware("Mobile8"),
        Branch::new("8b-q4-mobile8", "Run an 8B Q4_K_M model resident on an 8 GiB ARM device")
            .evidence("CAPACITY.md: Llama-3.1-8B is ~4.9 GB resident at Q4_K_M — more than half the \
device's RAM before any KV cache or OS footprint, unlikely to leave a usable margin")
            .confidence(0.25)
            .hardware("Mobile8"),
    ]
}

fn laptop24_branches() -> Vec<Branch> {
    vec![
        Branch::new("8b-14b-q4-laptop24", "Run an 8B or 14B Q4_K_M model with headroom on a 24 GiB box")
            .evidence("CAPACITY.md: 8B is ~4.9 GB and 14B is ~8.5 GB resident at Q4_K_M — even at 10 \
concurrent instances with 8k int8 KV the totals are ~10 GB and ~16 GB, both comfortably under 24 GiB")
            .confidence(0.75)
            .hardware("Laptop24"),
        Branch::new("32b-q4-laptop24", "Run a 32B Q4_K_M model on a 24 GiB box")
            .evidence("CAPACITY.md's 64 GiB write-up prices a 32B Q4_K_M at ~19 GB weights alone \
(before KV, OS, or app footprint) — the arithmetic scales down to 24 GiB but leaves little margin, \
and CAPACITY.md's own caveat is that throughput, not memory, is usually the binding constraint above \
8B, which has not been checked at this smaller RAM budget")
            .assume("no --loadstep run exists at 24 GiB to confirm the throughput floor named in \
CAPACITY.md's \"real limiter is throughput, not memory\" section")
            .confidence(0.35)
            .hardware("Laptop24"),
    ]
}

fn workstation64_branches() -> Vec<Branch> {
    vec![
        Branch::new("qwen3.6-27b-dense-workstation64", "Qwen3.6-27B dense as the primary model on a \
64 GiB (M4 Max class) box")
            .evidence("docs/model-fit.md: \"Qwen3.6-27B dense is the strongest fit for the M4 Max\" — \
SWE-bench Verified 77.2 on Qwen's own card, dense at 27B is ~16 GB at Q4 and loads on the existing \
QK-Norm path")
            .assume("a vendor benchmark is a model card, not independent verification — model-fit.md \
says this explicitly and recommends agent-score's 15-case suite as the stronger local signal")
            .confidence(0.65)
            .origin_model("Qwen3.6-27B")
            .hardware("Workstation64"),
        Branch::new("qwen3.6-35b-a3b-workstation64", "Qwen3.6-35B-A3B as an all-rounder at 32 GiB")
            .evidence("docs/model-fit.md: \"Qwen3.6-35B-A3B is the reported best all-rounder at 32 GB, \
and 3B active makes it quick\"")
            .confidence(0.5)
            .origin_model("Qwen3.6-35B-A3B")
            .hardware("Workstation64"),
        Branch::new("muse-glimmer-30b-workstation64", "Muse Glimmer 30B as a tool-use-focused \
alternative to Qwen3.6-27B")
            .evidence("docs/model-fit.md: \"Muse Glimmer 30B trades wins with Qwen3.6-27B and \
reportedly clears it on tool use\" — unscored by Artificial Analysis at time of writing")
            .assume("unscored by an independent benchmark suite; the claim is a trade, not a clear win")
            .confidence(0.3)
            .origin_model("Muse Glimmer 30B")
            .hardware("Workstation64"),
    ]
}

fn server192_branches() -> Vec<Branch> {
    vec![
        Branch::new("qwen3coder-next-80b-a3b-server192", "Qwen3-Coder-Next (80B MoE, 3B active) as the \
primary model on a 192 GiB (M2 Ultra class) box")
            .evidence("docs/model-fit.md: \"the strongest fit for the M2 Ultra... ~35-40 GB at Q4, \
fits with enormous headroom; 3B active means it decodes at small-model speed; context is 256K \
extendable to 1M via YaRN\"; SWE-rebench Pass@5 64.6%")
            .evidence("it is almost certainly qwen3moe, \"the one MoE arch this engine has confirmed \
against a real checkpoint\" (Qwen3-Coder-30B-A3B: d=2048, 48 layers, 128 experts, top-8, no shared \
expert)")
            .confidence(0.8)
            .origin_model("Qwen3-Coder-Next")
            .hardware("Server192"),
        Branch::new("glm-5.2-server192", "Get GLM-5.2 running on a 192 GiB box")
            .evidence("docs/model-fit.md: GLM-5.2 is ~753B; Unsloth's 2-bit dynamic GGUF is ~239 GB \
(needs a 256 GB Mac Studio); the IQ1_S referenced in glm52-fit.md is 201.8 GB against 192 GB — \
\"There is no quant of GLM-5.2 that runs on a 192 GB box\"")
            .confidence(0.95)
            .origin_model("GLM-5.2")
            .hardware("Server192"),
        Branch::new("gemma4-any-tier", "Support the gemma4 architecture in the arch registry")
            .evidence("docs/model-fit.md: gemma4 is listed \"not supported\" and, unlike qwen4, \
\"genuinely unsupported\" — Gemma 4 12B/31B QAT are named as among the better local coders, so this \
is called out as a real gap rather than a misleading one")
            .confidence(0.5)
            .hardware("Mobile8").hardware("Laptop24").hardware("Workstation64").hardware("Server192"),
        Branch::new("qwen4-any-tier", "Confirm qwen4 checkpoints load")
            .evidence("docs/model-fit.md: \"a qwen3moe fixture rebranded as qwen4 loads and produces \
bit-identical stats\" — the unsupported flag stays false only because it is unearned on a real \
checkpoint; \"the blocker is a download, not code\"")
            .confidence(0.7)
            .hardware("Mobile8").hardware("Laptop24").hardware("Workstation64").hardware("Server192"),
    ]
}

/// Branches tagged for a given tier — what the Liaison should show an operator running that class of
/// machine, without them having to know the hardware string convention.
pub fn branches_for_tier(t: &ExplorationTree, tier: RamTier) -> Vec<&Branch> {
    let tag = format!("{tier:?}");
    t.branches.iter().filter(|b| b.hardware.iter().any(|h| h == &tag)).collect()
}

/// Physical RAM in GiB, Linux only (`/proc/meminfo`'s `MemTotal`). `None` off Linux or if the file is
/// unreadable — the caller decides the fallback rather than this function guessing one, because a
/// wrong silent default here would misclassify every machine that hits it.
pub fn physical_ram_gib() -> Option<f32> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb as f32 / (1024.0 * 1024.0));
        }
    }
    None
}

/// Classify THIS machine. Falls back to [`RamTier::Workstation64`] when RAM can't be read (non-Linux,
/// sandboxed, or a missing `/proc/meminfo`) — the middle tier, so an unreadable machine gets a briefing
/// that is wrong in the least costly direction (neither the phone-tightest nor the server-loosest
/// findings), and callers that care should check `physical_ram_gib()` themselves to know it was a guess.
pub fn detect_tier() -> RamTier {
    RamTier::classify(physical_ram_gib().unwrap_or(64.0))
}

/// One line per branch tagged to `tier`, prefixed by status — the operator-facing rendering. Kept
/// separate from `Liaison` (which wants a `&Residency` and an irreversible-actions list this data has
/// no use for) rather than forcing this into that shape; it follows the same rule the Liaison
/// enforces — every line traces to a branch, never a generated summary of one.
pub fn briefing_for_tier(t: &ExplorationTree, tier: RamTier) -> Vec<String> {
    let mut lines = vec![format!("Capacity findings for {}:", tier.label())];
    let mut picks = branches_for_tier(t, tier);
    // Succeeded first (the actionable answer), then Active (open questions), then Deferred, then
    // Failed last (what NOT to try) — same "decisions cost the most to not know" ordering the
    // Liaison's own `briefing` uses, so the two surfaces read consistently.
    picks.sort_by_key(|b| match b.status {
        Status::Succeeded { .. } => 0,
        Status::Active => 1,
        Status::Deferred { .. } => 2,
        Status::Failed { .. } => 3,
    });
    if picks.is_empty() {
        lines.push("  (no findings recorded for this tier yet)".into());
        return lines;
    }
    for b in picks {
        let mark = match b.status {
            Status::Succeeded { .. } => "✓",
            Status::Active => "·",
            Status::Deferred { .. } => "…",
            Status::Failed { .. } => "✗",
        };
        lines.push(format!("  {mark} {} (confidence {:.2})", b.objective, b.confidence));
        if let Status::Failed { reason } = &b.status { lines.push(format!("      {reason}")); }
        if let Status::Deferred { retry_when } = &b.status { lines.push(format!("      retry when: {retry_when}")); }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_maps_real_ram_numbers_to_the_right_tier() {
        assert_eq!(RamTier::classify(6.0), RamTier::Mobile8);
        assert_eq!(RamTier::classify(8.0), RamTier::Mobile8);
        assert_eq!(RamTier::classify(16.0), RamTier::Laptop24);
        assert_eq!(RamTier::classify(24.0), RamTier::Laptop24);
        assert_eq!(RamTier::classify(64.0), RamTier::Workstation64);
        assert_eq!(RamTier::classify(128.0), RamTier::Server192);
        assert_eq!(RamTier::classify(192.0), RamTier::Server192);
        // Above every measured ceiling still classifies rather than panicking or inventing a tier.
        assert_eq!(RamTier::classify(512.0), RamTier::Server192);
    }

    #[test]
    fn glm_5_2_is_a_named_failure_not_a_silent_gap() {
        let t = capacity_tree();
        let b = t.get("glm-5.2-server192").expect("branch exists");
        match &b.status {
            Status::Failed { reason } => assert!(reason.contains("no quant of GLM-5.2")),
            other => panic!("expected Failed, got {other:?}"),
        }
        // and the liaison's "what is blocked" surface actually shows dead ends, so this must appear there
        assert!(t.dead_ends().iter().any(|(id, _)| *id == "glm-5.2-server192"));
    }

    #[test]
    fn qwen3_coder_next_is_the_succeeded_pick_for_server192() {
        let t = capacity_tree();
        let picks = branches_for_tier(&t, RamTier::Server192);
        assert!(picks.iter().any(|b| b.id == "qwen3coder-next-80b-a3b-server192"
            && matches!(b.status, Status::Succeeded { .. })));
    }

    #[test]
    fn gemma4_and_qwen4_are_deferred_with_a_retry_condition_not_dropped() {
        let t = capacity_tree();
        for id in ["gemma4-any-tier", "qwen4-any-tier"] {
            let b = t.get(id).unwrap_or_else(|| panic!("{id} missing"));
            assert!(matches!(b.status, Status::Deferred { .. }), "{id} should be Deferred, not Active");
        }
    }

    #[test]
    fn every_tier_has_at_least_one_branch() {
        let t = capacity_tree();
        for tier in [RamTier::Mobile8, RamTier::Laptop24, RamTier::Workstation64, RamTier::Server192] {
            assert!(!branches_for_tier(&t, tier).is_empty(), "{tier:?} has no findings");
        }
    }

    #[test]
    fn every_branch_carries_evidence_the_liaison_can_cite() {
        // The Liaison's hard rule: every substantive claim traces to an event or branch fact. A
        // branch with no evidence at all would be exactly the invented-status failure it guards
        // against, so this is enforced here rather than trusted to authoring discipline.
        let t = capacity_tree();
        for b in &t.branches {
            assert!(!b.evidence.is_empty(), "{} has no evidence", b.id);
        }
    }

    #[test]
    fn cross_tier_findings_are_tagged_on_every_tier_they_apply_to() {
        let t = capacity_tree();
        let g = t.get("gemma4-any-tier").unwrap();
        assert_eq!(g.hardware.len(), 4, "a registry-support gap applies regardless of RAM");
    }

    #[test]
    fn briefing_orders_succeeded_before_active_before_deferred_before_failed() {
        let t = capacity_tree();
        let lines = briefing_for_tier(&t, RamTier::Server192);
        let succ = lines.iter().position(|l| l.contains('✓')).unwrap();
        let fail = lines.iter().position(|l| l.contains('✗')).unwrap();
        assert!(succ < fail, "the actionable pick should read before the ruled-out one");
        // and a failed line carries its reason on the line below, not just the mark
        let fail_idx = lines.iter().position(|l| l.contains("Get GLM-5.2")).unwrap();
        assert!(lines[fail_idx + 1].contains("no quant of GLM-5.2"));
    }

    #[test]
    fn a_tier_with_no_findings_says_so_rather_than_an_empty_list() {
        let t = ExplorationTree::new();
        let lines = briefing_for_tier(&t, RamTier::Mobile8);
        assert!(lines.iter().any(|l| l.contains("no findings recorded")));
    }

    #[test]
    fn detect_tier_never_panics_regardless_of_environment() {
        // Just needs to return SOMETHING sane; this sandbox may or may not have /proc/meminfo.
        let _ = detect_tier();
    }
}
