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
    /// Upper bound for ROUTED MoE models, where the FFN is split across per-expert dispatches so the
    /// chunk drives dispatch COUNT more than dispatch duration. Higher than the dense cap, but still a
    /// cap: attention does scale with the chunk, and that is what a GPU watchdog kills.
    pub prefill_chunk_max_moe: u32,
    /// Largest total GPU allocation this device was MEASURED to honour, in GiB. 0 = never probed.
    ///
    /// The alternative is a constant, and the constant was wrong by 2.6x: `memplan`'s Asahi cap of
    /// 24 GiB was an empirical figure from the STAGING upload path, taken before the map-at-creation
    /// arena existed, and `--gate-vram` measured the same device honouring 63 GiB. Nothing in the code
    /// distinguished "the driver refuses" from "we assumed it would", and a model that fitted was being
    /// streamed for no reason.
    ///
    /// Cached because probing costs ~4.5 s and allocates everything on the device — acceptable once per
    /// adapter, not per launch.
    pub measured_vram_gib: f32,
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
            // 0 = never probed. NOT a conservative default value, because a plausible-looking number
            // here would be indistinguishable from a measurement and would inherit exactly the
            // authority the 24 GiB constant wrongly had.
            measured_vram_gib: 0.0,
            // 3072 x 8192 is the shape the 256-token chunk was originally tuned on.
            prefill_work_budget: 3072 * 8192 * 256,
            prefill_chunk_max: 256,
            prefill_chunk_max_moe: 1024,
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
        self.prefill_chunk_for_model(d, ff, false)
    }

    /// The prefill chunk, accounting for whether the model STREAMS its experts.
    ///
    /// The work budget models the duration of one attention dispatch, which is the right thing to bound
    /// when the weights are resident. It is the wrong thing to optimise when experts stream, because then
    /// the dominant cost is not dispatch length but REFILL TRAFFIC, and that scales with 1/chunk: each
    /// chunk re-pulls the cold experts for every layer.
    ///
    /// Measured on Llama-4 Scout (48 layers, 16 experts, ring 2, ~68 MiB per expert-layer, 1839 tokens):
    ///     chunk 256 ->  8 chunks -> ~4600 fills -> ~304 GiB moved
    ///     chunk  76 -> 25 chunks -> ~14400 fills -> ~949 GiB moved
    /// and the measured prefill went 196s -> 314s when the chunk was lowered. Shrinking it was a
    /// three-fold pessimisation for exactly the models that can least afford one.
    pub fn prefill_chunk_for_model(&self, d: usize, ff: usize, streams_experts: bool) -> u32 {
        self.prefill_chunk_moe(d, ff, streams_experts, None)
    }

    /// As above, accounting for MoE: `(n_expert, n_used)` when the model routes.
    ///
    /// The work budget models a DENSE FFN, where one dispatch processes the whole chunk. A routed MoE is
    /// different: the FFN is split into per-expert dispatches, and each sees only
    /// `chunk x n_used / n_expert` tokens. So the cap that protects against long FFN dispatches does not
    /// bind, and holding the chunk down instead multiplies the DISPATCH COUNT — which is what actually
    /// costs.
    ///
    /// Measured on Qwen3-Coder-30B (48 layers, 128 experts, 8 used, 1890-token prompt):
    ///     chunk  256 -> 8 chunks, 16 tokens per expert group, 49152 expert dispatches (~71 s observed)
    ///     chunk 1024 -> 2 chunks, 64 tokens per expert group, 12288 expert dispatches
    /// Same arithmetic, a quarter of the dispatches, and each one four times better occupied.
    ///
    /// Attention still scales with the chunk, so this is bounded by `prefill_chunk_max_moe` rather than
    /// left to grow — a long-context attention dispatch is exactly what the watchdog kills.
    pub fn prefill_chunk_moe(&self, d: usize, ff: usize, streams_experts: bool,
                             experts: Option<(usize, usize)>) -> u32 {
        if streams_experts { return self.prefill_chunk_max; }
        let work = (d.max(1) as u64).saturating_mul(ff.max(1) as u64);
        let scaled = (self.prefill_work_budget / work.max(1)) as u32;
        match experts {
            // sparsity = how much of the FFN each dispatch actually touches
            Some((n_expert, n_used)) if n_expert > n_used && n_used > 0 => {
                let sparsity = (n_expert / n_used.max(1)) as u32;
                scaled.saturating_mul(sparsity).clamp(16, self.prefill_chunk_max_moe)
            }
            _ => scaled.clamp(16, self.prefill_chunk_max),
        }
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

    /// Load a profile from disk, falling back to this device's defaults.
    ///
    /// A missing or unreadable file is NOT an error: the defaults are usable on their own, and refusing
    /// to start because a tuning cache is corrupt would be a bad trade. A malformed file is reported to
    /// the caller so it can say so once rather than silently discarding measurements every run.
    pub fn load_or_default(path: &std::path::Path, adapter: &str) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Err(_) => (Profile::for_device(adapter), None),
            Ok(text) => match Profile::from_str(&text) {
                Some(mut p) => {
                    // Defaults come from the DEVICE, not the file: a profile copied between machines
                    // must not carry another GPU's hardware workarounds. Only measurements are portable
                    // within their key, and the key names the adapter.
                    p.defaults = device_defaults(adapter);
                    (p, None)
                }
                None => (Profile::for_device(adapter),
                         Some(format!("tuning profile at {} is unreadable and was ignored; \
measurements will be taken again", path.display()))),
            },
        }
    }

    /// Write the profile, creating the parent directory. Returns the error text rather than panicking —
    /// failing to save a tuning cache must never take down a load.
    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        }
        std::fs::write(path, self.to_toml_like()).map_err(|e| format!("write {}: {e}", path.display()))
    }

    /// How many measurements are held, for a one-line startup report.
    pub fn measurement_count(&self) -> usize { self.verdicts.values().map(|m| m.len()).sum() }
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
    fn a_streaming_model_keeps_the_full_chunk() {
        // Scout's shape would otherwise be shrunk to 76, which TRIPLES expert refill traffic — the
        // opposite of what helps a model whose weights do not fit.
        let d = Defaults::default();
        assert_eq!(d.prefill_chunk_for_model(5120, 16384, false), 76, "resident: bounded by dispatch work");
        assert_eq!(d.prefill_chunk_for_model(5120, 16384, true), 256, "streaming: bounded by refill traffic");
        // a small resident model is unaffected either way
        assert_eq!(d.prefill_chunk_for_model(3072, 8192, false), 256);
        assert_eq!(d.prefill_chunk_for_model(3072, 8192, true), 256);
    }

    #[test]
    fn a_routed_moe_gets_a_larger_chunk_because_its_ffn_dispatches_are_small() {
        let d = Defaults::default();
        // Qwen3-Coder-30B: d=2048, ff=5472, 128 experts, 8 used. The dense rule caps it at 256, which on
        // a 1890-token prompt is 8 chunks x 48 layers x 128 experts = 49152 expert dispatches — measured
        // at ~71 s. Each of those dispatches sees 16 tokens, so the cap is limiting nothing except
        // throughput.
        let dense = d.prefill_chunk_moe(2048, 5472, false, None);
        let moe = d.prefill_chunk_moe(2048, 5472, false, Some((128, 8)));
        assert_eq!(dense, 256);
        assert!(moe > dense, "a routed MoE should get a bigger chunk: {moe} vs {dense}");
        assert!(moe <= d.prefill_chunk_max_moe, "still capped — attention DOES scale with the chunk");

        // A model with few experts, or one active per token, gains little and must not be inflated
        // recklessly: sparsity is the whole justification.
        let dense_like = d.prefill_chunk_moe(2048, 5472, false, Some((8, 8)));
        assert_eq!(dense_like, dense, "n_expert == n_used is not sparse; treat it as dense");

        // Streaming still overrides everything: refill traffic dominates there, not dispatch count.
        assert_eq!(d.prefill_chunk_moe(5120, 16384, true, Some((16, 1))), d.prefill_chunk_max);
    }

    #[test]
    fn a_big_dense_model_is_still_shrunk() {
        // The MoE rule must not leak into the dense path and undo the watchdog fix.
        let d = Defaults::default();
        assert_eq!(d.prefill_chunk_moe(5120, 16384, false, None), 76);
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
    fn a_missing_profile_is_not_an_error_and_a_corrupt_one_is_reported() {
        let dir = std::env::temp_dir().join(format!("tp-{}", std::process::id()));
        let missing = dir.join("nope.json");
        let (p, err) = Profile::load_or_default(&missing, "Apple M2 Ultra [Vulkan]");
        assert!(err.is_none(), "a missing file is normal, not a failure");
        assert!(!p.defaults.subgroup_matvec, "device defaults still applied");

        let bad = dir.join("bad.json");
        std::fs::create_dir_all(&dir).ok();
        std::fs::write(&bad, "{ not json").ok();
        let (p2, err2) = Profile::load_or_default(&bad, "Apple M2 Ultra [Vulkan]");
        let msg = err2.expect("a corrupt file must be reported, not silently discarded");
        assert!(msg.contains("unreadable"), "{msg}");
        assert_eq!(p2.measurement_count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn defaults_come_from_the_device_even_when_the_file_came_from_elsewhere() {
        // A profile copied between machines must not carry another GPU's hardware workarounds. The
        // measurements are keyed by adapter and stay; the defaults are recomputed.
        let dir = std::env::temp_dir().join(format!("tp2-{}", std::process::id()));
        let path = dir.join("t.json");
        let mut written = Profile::for_device("Apple M2 Ultra [Vulkan]");   // subgroup_matvec = false
        let k = TuneKey::new("Apple M2 Ultra [Vulkan]", "moe", "q4");
        written.record(&k, Verdict { option: "fast_decode".into(), chosen: true,
                                     on_ms: 170.8, off_ms: 236.7, note: "m2".into() });
        written.save(&path).expect("save");

        let (loaded, err) = Profile::load_or_default(&path, "NVIDIA RTX 4090 [Vulkan]");
        assert!(err.is_none());
        assert!(loaded.defaults.subgroup_matvec, "the 4090 must NOT inherit Honeykrisp's workaround");
        assert_eq!(loaded.measurement_count(), 1, "measurements are kept — they are keyed by adapter");
        assert!(loaded.verdict(&k, "fast_decode").is_some(), "and still reachable under their own key");
        std::fs::remove_dir_all(&dir).ok();
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
