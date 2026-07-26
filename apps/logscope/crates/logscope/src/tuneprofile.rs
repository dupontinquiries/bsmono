//! tuneprofile.rs — tuning defaults and measured verdicts, with the provenance to audit them.
//!
//! Two autotuner bugs this session motivate the shape of this:
//!
//! 1. **A verdict was keyed on the adapter alone.** The fast-decode levers were measured on a dense model,
//!    recorded as "off", and then applied to MoE models — where the two MoE fusions are the whole point.
//!    Same GPU, different question. A tuning key has to include what was actually varied.
//! 2. **A default ignored the model.** The prefill chunk was a constant 256 tokens, but what a GPU
//!    watchdog bounds is the DURATION of a dispatch, which scales with `chunk x d x ff`. Safe on a 3B,
//!    it hung a 109B.
//!
//! So: verdicts carry the measurement that produced them, defaults are expressed as the invariant rather
//! than the number, and a verdict that was not decisive is not cached at all — locking in noise is worse
//! than re-measuring, because it persists and nobody knows why.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What a tuning result is allowed to be reused across.
///
/// Anything that changes the answer belongs here. `decode_class` exists because MoE and dense decode have
/// different dispatch profiles; omitting it was the bug.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TuneKey {
    pub adapter: String,
    /// "moe" or "dense".
    pub decode_class: String,
    /// Resident weight format, e.g. "q4" / "int8" — a lever can pay at one precision and not another.
    pub residency: String,
}

impl TuneKey {
    pub fn new(adapter: &str, decode_class: &str, residency: &str) -> Self {
        TuneKey { adapter: adapter.into(), decode_class: decode_class.into(), residency: residency.into() }
    }
    /// Stable string form for a flat cache file.
    pub fn id(&self) -> String { format!("{} [{}/{}]", self.adapter, self.decode_class, self.residency) }
}

/// A measured A/B, kept with its numbers so a suspicious verdict can be argued with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub option: String,
    pub chosen: bool,
    /// Milliseconds per token with the option ON and OFF.
    pub on_ms: f32,
    pub off_ms: f32,
    /// Free-form provenance: when, and on what.
    pub note: String,
}

impl Verdict {
    /// Relative improvement of the chosen side, as a fraction. Negative means the choice was worse.
    pub fn margin(&self) -> f32 {
        if self.off_ms <= 0.0 { return 0.0; }
        let gain = (self.off_ms - self.on_ms) / self.off_ms;
        if self.chosen { gain } else { -gain }
    }

    /// Was this measurement decisive enough to keep?
    ///
    /// Run-to-run variance on a busy GPU is easily a few percent, so a 1% win is not a result — it is a
    /// coin flip that will be cached forever and quietly mislead every later run. Below the threshold the
    /// caller should decline to record anything and let the next run measure again.
    pub fn is_decisive(&self, min_margin: f32) -> bool { self.margin().abs() >= min_margin }
}

/// Defaults expressed as INVARIANTS where a bare number would be wrong.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Defaults {
    /// Prefill work budget: `chunk x d x ff` held roughly constant, so a bigger model gets a shorter
    /// chunk and the dispatch duration stays inside the watchdog. Expressed as the reference product.
    pub prefill_work_budget: u64,
    /// Upper bound on the chunk regardless of the budget.
    pub prefill_chunk_max: u32,
    /// Compute passes per queue submission.
    pub submit_batch: u32,
    /// Minimum relative margin for an A/B verdict to be cached at all.
    pub min_verdict_margin: f32,
    /// Subgroup matvec: known to hang large matmuls on some drivers, so it is opt-in there.
    pub subgroup_matvec: bool,
    /// Conversion chunk, in Q4_K super-blocks, per dispatch — bounds GPU work per submit.
    pub convert_chunk_blocks: u32,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            // 3072 x 8192 is the shape the 256-token chunk was originally tuned on.
            prefill_work_budget: 3072 * 8192 * 256,
            prefill_chunk_max: 256,
            submit_batch: 16,
            min_verdict_margin: 0.05,
            subgroup_matvec: true,
            convert_chunk_blocks: 16384,
        }
    }
}

impl Defaults {
    /// The prefill chunk for a model of these dimensions. Never exceeds `prefill_chunk_max`, never drops
    /// below 16 — past that the dispatch overhead dominates and a watchdog hang is not the binding
    /// problem any more.
    pub fn prefill_chunk(&self, d: usize, ff: usize) -> u32 {
        let work = (d.max(1) as u64).saturating_mul(ff.max(1) as u64);
        let scaled = (self.prefill_work_budget / work.max(1)) as u32;
        scaled.clamp(16, self.prefill_chunk_max)
    }
}

/// Device-specific defaults, from measurements on this project rather than from guesses.
///
/// Only deviations from `Defaults::default()` are set, and each one has a reason recorded next to it.
pub fn device_defaults(adapter: &str) -> Defaults {
    let a = adapter.to_ascii_lowercase();
    let mut d = Defaults::default();
    // Asahi/Honeykrisp on Apple silicon: subgroup matvec can hang large matmuls, so it is opt-in; the
    // firmware watchdog is strict, so conversion dispatches are kept small.
    if a.contains("apple") && a.contains("vulkan") {
        d.subgroup_matvec = false;
        d.convert_chunk_blocks = 16384;
        // Submission is the dominant cost here, so a larger batch is worth more than on other drivers.
        d.submit_batch = 16;
    }
    d
}

/// A profile: defaults plus whatever has been measured, keyed properly.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Profile {
    pub defaults: Defaults,
    /// Keyed by `TuneKey::id()` then option name.
    pub verdicts: BTreeMap<String, BTreeMap<String, Verdict>>,
}

impl Profile {
    pub fn for_device(adapter: &str) -> Self {
        Profile { defaults: device_defaults(adapter), verdicts: BTreeMap::new() }
    }

    /// Record a verdict, but only if it was decisive. Returns whether it was kept.
    pub fn record(&mut self, key: &TuneKey, v: Verdict) -> bool {
        if !v.is_decisive(self.defaults.min_verdict_margin) { return false; }
        self.verdicts.entry(key.id()).or_default().insert(v.option.clone(), v);
        true
    }

    /// Look up a verdict for exactly this key. Never falls back to a different key — that fallback is the
    /// bug this type exists to prevent.
    pub fn verdict(&self, key: &TuneKey, option: &str) -> Option<&Verdict> {
        self.verdicts.get(&key.id())?.get(option)
    }

    /// Operator-facing account of what has been measured and how convincingly.
    pub fn report(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (k, opts) in &self.verdicts {
            for (name, v) in opts {
                out.push(format!("{k}: {name} = {} ({:+.0}%, {:.1} vs {:.1} ms/tok) — {}",
                    if v.chosen { "ON" } else { "off" }, v.margin() * 100.0, v.on_ms, v.off_ms, v.note));
            }
        }
        out
    }

    pub fn to_toml_like(&self) -> String { serde_json::to_string_pretty(self).unwrap_or_default() }
    pub fn from_str(s: &str) -> Option<Self> { serde_json::from_str(s).ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_measured_on_one_decode_class_does_not_apply_to_another() {
        // THE BUG: levers measured on a dense model were reused for MoE, where the MoE fusions are the
        // entire point. Different key, no reuse.
        let mut p = Profile::for_device("Apple M2 Ultra [Vulkan]");
        let dense = TuneKey::new("Apple M2 Ultra [Vulkan]", "dense", "q4");
        let moe = TuneKey::new("Apple M2 Ultra [Vulkan]", "moe", "q4");
        assert!(p.record(&dense, Verdict {
            option: "fast_decode".into(), chosen: false, on_ms: 100.0, off_ms: 80.0,
            note: "dense model".into() }));
        assert!(p.verdict(&dense, "fast_decode").is_some());
        assert!(p.verdict(&moe, "fast_decode").is_none(), "must NOT leak across decode class");
        // residency is part of the key too
        let moe_int8 = TuneKey::new("Apple M2 Ultra [Vulkan]", "moe", "int8");
        assert_ne!(moe.id(), moe_int8.id());
    }

    #[test]
    fn an_indecisive_measurement_is_not_cached() {
        // A 1% difference is run-to-run noise. Caching it locks in a coin flip that then persists and
        // silently misleads every later run.
        let mut p = Profile::for_device("gpu");
        let k = TuneKey::new("gpu", "moe", "q4");
        let noise = Verdict { option: "fast_decode".into(), chosen: true,
                              on_ms: 99.0, off_ms: 100.0, note: "noise".into() };
        assert!((noise.margin() - 0.01).abs() < 1e-6);
        assert!(!p.record(&k, noise), "1% must not be recorded");
        assert!(p.verdict(&k, "fast_decode").is_none());

        // the real measurement from this session: 236.7 -> 170.8 ms/tok is decisive
        let real = Verdict { option: "fast_decode".into(), chosen: true,
                             on_ms: 170.8, off_ms: 236.7, note: "M2 Ultra, MoE".into() };
        assert!(real.is_decisive(0.05));
        assert!(p.record(&k, real));
        assert!(p.verdict(&k, "fast_decode").is_some());
    }

    #[test]
    fn margin_is_signed_by_which_side_was_chosen() {
        let chose_on = Verdict { option: "x".into(), chosen: true, on_ms: 80.0, off_ms: 100.0, note: String::new() };
        assert!((chose_on.margin() - 0.2).abs() < 1e-6, "choosing the faster side is a positive margin");
        let chose_off = Verdict { option: "x".into(), chosen: false, on_ms: 80.0, off_ms: 100.0, note: String::new() };
        assert!(chose_off.margin() < 0.0, "choosing the slower side is negative — visible, not hidden");
    }

    #[test]
    fn prefill_chunk_scales_with_the_model_not_the_token_count() {
        // What the watchdog bounds is dispatch DURATION, which scales with chunk x d x ff.
        let d = Defaults::default();
        assert_eq!(d.prefill_chunk(3072, 8192), 256, "the reference shape keeps the full chunk");
        assert_eq!(d.prefill_chunk(2048, 5472), 256, "smaller models are capped, not enlarged");
        let scout = d.prefill_chunk(5120, 16384);
        assert!(scout < 256 && scout > 16, "Llama-4 Scout gets a shorter chunk: {scout}");
        let seventy_b = d.prefill_chunk(8192, 28672);
        assert!(seventy_b < scout, "a bigger model gets a shorter chunk still: {seventy_b}");
        // and it never collapses to something pathological
        assert_eq!(d.prefill_chunk(1_000_000, 1_000_000), 16);
    }

    #[test]
    fn device_defaults_only_deviate_where_there_is_a_reason() {
        let generic = device_defaults("Some GPU [Vulkan]");
        assert_eq!(generic, Defaults::default(), "unknown devices get the plain defaults");
        let asahi = device_defaults("Apple M2 Ultra (G14D B1) [Vulkan]");
        assert!(!asahi.subgroup_matvec, "subgroup matvec can hang large matmuls on Honeykrisp");
        assert!(asahi.convert_chunk_blocks > 0);
    }

    #[test]
    fn a_profile_round_trips_with_its_provenance() {
        let mut p = Profile::for_device("Apple M2 Ultra [Vulkan]");
        let k = TuneKey::new("Apple M2 Ultra [Vulkan]", "moe", "q4");
        p.record(&k, Verdict { option: "fast_decode".into(), chosen: true,
                               on_ms: 170.8, off_ms: 236.7, note: "measured 2026-07".into() });
        let back = Profile::from_str(&p.to_toml_like()).expect("round trip");
        let v = back.verdict(&k, "fast_decode").expect("kept");
        assert!((v.on_ms - 170.8).abs() < 1e-3);
        assert!(v.note.contains("2026-07"), "the provenance survives, or the number cannot be argued with");
        assert!(back.report()[0].contains("+28%"), "{}", back.report()[0]);
    }
}
