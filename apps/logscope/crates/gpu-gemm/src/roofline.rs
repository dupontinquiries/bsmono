//! roofline.rs — what single-stream decode COULD run at, and what fraction of it we get.
//!
//! The perf verdict has been saying "the bandwidth ceiling is UNMEASURED — run
//! `kernel-validate --gate-perf` first, because without it there is nothing to be a percentage of".
//! That is honest but it leaves the most useful number unavailable on every ordinary run, and it
//! turns out not to need a measurement at all.
//!
//! Single-token decode reads each ACTIVE weight exactly once. So the achieved effective bandwidth
//! falls straight out of the throughput — `active_bytes x tok/s` — with no calibration, no probe,
//! and no constant to go stale. Comparing that against the device's spec figure gives the fraction
//! of roofline, which is the number that decides whether the next move is a better model or a
//! faster engine.
//!
//! It also settles claims from elsewhere. "Gemma 4 26B at 500+ tok/s on a 5090" sounds implausible
//! for a 26B model and is not: it activates 3.8B of those parameters, so at Q4 the 5090's 1.79 TB/s
//! allows about 943 tok/s and 500 is 53% of roofline — a good but unremarkable result. The same
//! model on 800 GB/s allows about 421. What decides whether an M2 Ultra sees 400 or 40 is not the
//! model; it is the fraction, and this engine's measured fraction on a dense 8B is about 9%.
//!
//! ACTIVE parameters, not total, is the whole subtlety of an MoE. The router touches one expert's
//! weights per active expert per layer and never reads the rest, so a 26B-A4B decodes at the
//! bandwidth cost of a ~4B dense model while holding 26B in memory. Using total here would
//! under-predict by 7x and make every MoE look broken.

/// Bytes per parameter for a resident quantisation. int8 residency is one byte plus a per-32 f32
/// scale; q4 residency is a nibble plus the same scale.
pub fn bytes_per_param(bits: f32) -> f32 { bits / 8.0 + 4.0 / 32.0 }

/// Tokens per second this device could reach if decode were purely bandwidth-bound.
pub fn ceiling_tok_s(bandwidth_gb_s: f64, active_params: f64, bits: f32) -> f64 {
    let bytes = active_params * bytes_per_param(bits) as f64;
    if bytes <= 0.0 { return 0.0; }
    bandwidth_gb_s * 1e9 / bytes
}

/// The effective bandwidth a run actually achieved. No probe needed: decode reads each active
/// weight once per token, so this is just the arithmetic of what was moved.
pub fn achieved_gb_s(tok_s: f64, active_params: f64, bits: f32) -> f64 {
    tok_s * active_params * bytes_per_param(bits) as f64 / 1e9
}

/// Fraction of roofline, 0..1. `None` when the device's bandwidth is unknown — reporting a
/// percentage of a number nobody measured is how the 892957502x phase balance and the 265600%
/// cache rate happened.
pub fn fraction(tok_s: f64, active_params: f64, bits: f32, bandwidth_gb_s: Option<f64>) -> Option<f64> {
    let bw = bandwidth_gb_s?;
    if bw <= 0.0 { return None; }
    Some(achieved_gb_s(tok_s, active_params, bits) / bw)
}

/// What the fraction implies about where to spend effort. The thresholds are deliberately coarse:
/// this is a signpost, not a score.
pub fn verdict(frac: f64) -> &'static str {
    if frac >= 0.60 {
        "bandwidth-bound — the engine is close to the hardware and only a smaller model or a faster \
device moves this"
    } else if frac >= 0.25 {
        "partly bandwidth-bound — there is headroom, but a better model choice may pay as well as \
engine work"
    } else {
        "NOT bandwidth-bound — the hardware is mostly idle and the limit is dispatch, submit or sync \
overhead. A faster model will not help; the engine is the lever"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cross-checked against a THIRD-PARTY published benchmark: a DGX Spark write-up computed
    /// 143.7 tok/s for gemma4 26B-A4B at 4-bit on 273 GB/s, arriving at it independently. A model of
    /// the hardware that only agrees with itself is not evidence.
    #[test]
    fn it_reproduces_a_published_ceiling() {
        // That write-up used a flat 0.5 bytes/param; the per-32 scale is ours, so compare on theirs.
        let theirs: f64 = 273.0 * 1e9 / (3.8e9 * 0.5);
        assert!((theirs - 143.7).abs() < 1.0, "sanity: {theirs:.1} should be ~143.7");
        // Ours, with the scale overhead included, must land slightly below — never above, which
        // would mean claiming a ceiling the memory system cannot deliver.
        let ours = ceiling_tok_s(273.0, 3.8e9, 4.0);
        assert!(ours < theirs && ours > theirs * 0.7,
            "ours {ours:.0} should sit just under {theirs:.0} — scales are bytes too");
    }

    /// ACTIVE parameters, not total. Using 26B here would predict 61 tok/s where the truth is ~420,
    /// and every MoE would look broken.
    #[test]
    fn an_moe_is_priced_on_its_active_parameters() {
        let active = ceiling_tok_s(800.0, 3.8e9, 4.0);
        let total = ceiling_tok_s(800.0, 26.0e9, 4.0);
        // 337, not the 421 a flat 0.5 bytes/param gives. The per-32 f32 scale is 20% more traffic
        // and it is real traffic — every published ceiling quoting 0.5 is optimistic by that much,
        // including the DGX Spark figure this module checks itself against. Worth stating rather
        // than rounding away: it is the difference between "we are at 45% of roofline" and 56%.
        assert!(active > 320.0 && active < 350.0,
            "M2 Ultra ceiling for 26B-A4B at 4-bit WITH scales: {active:.0}");
        assert!(active / total > 6.0, "pricing on total under-predicts by {:.1}x", active / total);
    }

    /// The inverse needs no probe, and must round-trip with the ceiling.
    #[test]
    fn achieved_bandwidth_inverts_the_ceiling() {
        for (bw, p, bits) in [(800.0, 3.8e9, 4.0f32), (1792.0, 8.0e9, 8.0), (273.0, 1.0e9, 4.0)] {
            let at_ceiling = ceiling_tok_s(bw, p, bits);
            let back = achieved_gb_s(at_ceiling, p, bits);
            assert!((back - bw).abs() < 1e-6 * bw, "{back} != {bw}");
            assert!((fraction(at_ceiling, p, bits, Some(bw)).unwrap() - 1.0).abs() < 1e-9);
        }
    }

    /// The operator's own measurement: a dense 8B at 56.2 ms/tok on a device speced at 800 GB/s.
    /// This is the number that answers "would 500 tok/s translate" — it would not, and the model is
    /// not why.
    #[test]
    fn the_measured_dense_run_is_far_from_roofline() {
        let tok_s = 1000.0 / 56.2;
        let f = fraction(tok_s, 8.0e9, 4.0, Some(800.0)).unwrap();
        assert!(f < 0.15, "expected well under roofline, got {:.0}%", 100.0 * f);
        assert!(verdict(f).contains("NOT bandwidth-bound"));
        assert!(verdict(f).contains("engine is the lever"));
    }

    /// Unknown bandwidth yields no fraction rather than a fabricated one.
    #[test]
    fn an_unmeasured_device_gets_no_percentage() {
        assert!(fraction(50.0, 4e9, 4.0, None).is_none());
        assert!(fraction(50.0, 4e9, 4.0, Some(0.0)).is_none());
    }

    #[test]
    fn the_verdict_bands_are_ordered() {
        assert!(verdict(0.9).contains("bandwidth-bound") && !verdict(0.9).contains("NOT"));
        assert!(verdict(0.4).contains("partly"));
        assert!(verdict(0.05).contains("NOT"));
    }
}

/// Active weight ELEMENTS per token, from what the loader actually made resident.
///
/// Derived rather than reconstructed from config on purpose. The loader already counts every weight
/// element it touched (LOAD_FAST_WEIGHTS + LOAD_SLOW_WEIGHTS), which is ground truth for what is in
/// memory; recomputing it from hidden size, layer count and expert geometry means re-deriving the
/// model's shape a second way and being quietly wrong when a family does something unusual — which
/// is the mistake c4-804 made about qwen35moe and c4-807 had to undo.
///
/// The one thing the totals cannot know is that an MoE reads only `n_used` of `n_routed` experts per
/// token. That ratio is applied to the EXPERT elements alone; attention, norms, embeddings and any
/// shared expert are read every token regardless.
pub fn active_elements(total_elems: u64, expert_elems: u64, n_routed: u32, n_used: u32) -> u64 {
    let dense = total_elems.saturating_sub(expert_elems);
    if n_routed == 0 || n_used == 0 || expert_elems == 0 { return total_elems; }
    let used = (expert_elems as f64) * (n_used.min(n_routed) as f64) / (n_routed as f64);
    dense + used as u64
}

/// Spec bandwidth for adapters we recognise, in GB/s. `None` for anything else — a fraction of a
/// guessed denominator is the class of number this project keeps deleting.
///
/// These are VENDOR figures, not measurements, and they are only ever a denominator: the achieved
/// bandwidth beside them is computed and needs no table.
pub fn spec_bandwidth(adapter: &str) -> Option<f64> {
    let a = adapter.to_ascii_lowercase();
    if a.contains("m2 ultra") { return Some(800.0); }
    if a.contains("m1 ultra") { return Some(800.0); }
    if a.contains("m4 max") { return Some(546.0); }
    if a.contains("m3 max") { return Some(400.0); }
    if a.contains("m2 max") { return Some(400.0); }
    if a.contains("m4 pro") { return Some(273.0); }
    None
}

#[cfg(test)]
mod active {
    use super::*;

    /// An MoE reads a fraction of its experts and all of everything else. Getting this wrong in
    /// either direction misprices the ceiling by several times.
    #[test]
    fn only_the_used_experts_count() {
        // 26B total, 22B of it experts, 8 of 128 active -> 4B dense + 1.375B experts.
        let a = active_elements(26_000_000_000, 22_000_000_000, 128, 8);
        assert!(a > 5_000_000_000 && a < 6_000_000_000, "got {a}");
        // A dense model is unchanged.
        assert_eq!(active_elements(8_000_000_000, 0, 0, 0), 8_000_000_000);
        // Every expert active is the same as dense.
        assert_eq!(active_elements(1000, 800, 4, 4), 1000);
    }

    /// Degenerate metadata must not silently zero the model out — an under-reported active count
    /// would make the engine look like it was beating its own roofline.
    #[test]
    fn nonsense_expert_counts_fall_back_to_the_total() {
        assert_eq!(active_elements(1000, 800, 0, 8), 1000);
        assert_eq!(active_elements(1000, 800, 8, 0), 1000);
        assert_eq!(active_elements(1000, 0, 8, 2), 1000);
    }

    #[test]
    fn unknown_adapters_get_no_denominator() {
        assert_eq!(spec_bandwidth("Apple M2 Ultra (G14D B1) [Vulkan]"), Some(800.0));
        assert_eq!(spec_bandwidth("Apple M4 Max [Metal]"), Some(546.0));
        assert!(spec_bandwidth("NVIDIA GeForce RTX 5090").is_none(),
            "not in the table — report achieved bandwidth and no fraction");
    }
}
