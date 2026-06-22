//! `sim::bending_information` — the Information Bending discipline.
//!
//! Bends *resonance/information* rather than matter: sensing hidden state,
//! disrupting others' bending, and masking your own resonance signature. Pure
//! logic over [`Signature`]s; the bin supplies what is sensible to sense.
//!
//! Spec: `resonance_bending_upgrade.docx` (Information Bending).
//!
//! # Concepts
//!
//! - **Signature** — a bender's resonance fingerprint: raw `strength`, a
//!   `discipline` tag, and `coherence01` (how cleanly the resonance reads).
//! - **sense** — detect other signatures, returning a confidence that rises
//!   with their strength and coherence and falls with distance proxy
//!   `range_falloff`. Weak/incoherent/distant signatures drop below a floor
//!   and are omitted.
//! - **disrupt** — lower a target's coherence (a disrupted bender's techniques
//!   weaken). Monotone in power, clamped ≥ 0.
//! - **mask** — lower your own apparent `strength` so you are harder to sense.
//! - **coherence_to_power** — how much coherence scales realized power, feeding
//!   the other disciplines.
//!
//! # Guarantees
//!
//! All outputs are finite. Confidences are in `[0, 1]`. `coherence01` and
//! `strength` never go negative. Deterministic; no panics on any finite input.

// ── Tunables ──────────────────────────────────────────────────────────────────

/// Confidence below this is treated as "undetectable" and dropped from `sense`.
const SENSE_FLOOR: f32 = 0.05;

/// Floor of `coherence_to_power` — even a fully-disrupted bender retains this
/// fraction of realized power (techniques get sloppy, not impossible).
const POWER_FLOOR: f32 = 0.1;

// ── Signature ─────────────────────────────────────────────────────────────────

/// A bender's resonance fingerprint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Signature {
    /// Raw resonance strength (non-negative; unbounded above).
    pub strength: f32,
    /// Discipline tag (caller-defined enumeration of bending disciplines).
    pub discipline: u8,
    /// Coherence in `[0, 1]`: how cleanly the resonance reads.
    pub coherence01: f32,
}

impl Signature {
    pub fn new(strength: f32, discipline: u8, coherence01: f32) -> Signature {
        Signature {
            strength: sanitize_nonneg(strength),
            discipline,
            coherence01: sanitize01(coherence01),
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

#[inline]
fn sanitize_nonneg(v: f32) -> f32 {
    if v.is_finite() && v > 0.0 { v } else { 0.0 }
}

#[inline]
fn sanitize01(v: f32) -> f32 {
    if !v.is_finite() { return 0.0; }
    if v < 0.0 { 0.0 } else if v > 1.0 { 1.0 } else { v }
}

#[inline]
fn clamp01(v: f32) -> f32 {
    sanitize01(v)
}

// ── sense ─────────────────────────────────────────────────────────────────────

/// Detect which of `others` `local` can sense, with a confidence in `[0, 1]`.
///
/// Confidence rises with a signature's `strength` and `coherence01`, and falls
/// with `range_falloff` (a distance proxy ≥ 0: 0 = adjacent, larger = farther).
/// `local`'s own coherence acts as a sensing-acuity multiplier. Signatures
/// whose confidence falls below [`SENSE_FLOOR`] are omitted.
///
/// Returns `(index_into_others, confidence)` pairs, in ascending index order.
pub fn sense(local: &Signature, others: &[Signature], range_falloff: f32) -> Vec<(usize, f32)> {
    let falloff = sanitize_nonneg(range_falloff);
    // Distance attenuation in (0, 1]: 1 / (1 + falloff).
    let distance_atten = 1.0 / (1.0 + falloff);
    // Sensing acuity: a more coherent sensor reads more (0.5..1.0 range so even
    // an incoherent sensor picks up strong nearby signatures).
    let acuity = 0.5 + 0.5 * sanitize01(local.coherence01);

    let mut out = Vec::new();
    for (i, s) in others.iter().enumerate() {
        let strength = sanitize_nonneg(s.strength);
        let coherence = sanitize01(s.coherence01);
        // Strength maps to (0,1) via a saturating curve so confidence stays bounded.
        let strength_term = strength / (1.0 + strength);
        let raw = strength_term * coherence * distance_atten * acuity;
        let confidence = clamp01(raw);
        if confidence >= SENSE_FLOOR {
            out.push((i, confidence));
        }
    }
    out
}

// ── disrupt ───────────────────────────────────────────────────────────────────

/// Lower `target`'s coherence by `power01` (clamped to `[0, 1]`).
///
/// New coherence = `old * (1 - power01)`, clamped ≥ 0. Monotone: higher power
/// removes more coherence. `power01 == 0` is a no-op; `power01 == 1` fully
/// decoheres the target.
pub fn disrupt(target: &mut Signature, power01: f32) {
    let p = sanitize01(power01);
    let new_coherence = target.coherence01 * (1.0 - p);
    target.coherence01 = clamp01(new_coherence);
}

// ── mask ──────────────────────────────────────────────────────────────────────

/// Lower `local`'s apparent `strength` by `power01` (clamped to `[0, 1]`).
///
/// New strength = `old * (1 - power01)`, clamped ≥ 0. A fully-masked bender
/// (`power01 == 1`) reads as zero strength and so drops out of `sense`.
pub fn mask(local: &mut Signature, power01: f32) {
    let p = sanitize01(power01);
    let new_strength = local.strength * (1.0 - p);
    local.strength = sanitize_nonneg(new_strength);
}

// ── coherence_to_power ─────────────────────────────────────────────────────────

/// How much a bender's coherence scales their realized power.
///
/// `coherence_to_power(1.0) == 1.0` (full coherence = full power).
/// `coherence_to_power(0.0) == POWER_FLOOR` (decohered = sloppy but non-zero).
/// Linear in between; result in `[POWER_FLOOR, 1.0]`.
pub fn coherence_to_power(c01: f32) -> f32 {
    let c = sanitize01(c01);
    POWER_FLOOR + (1.0 - POWER_FLOOR) * c
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sig(strength: f32, coherence: f32) -> Signature {
        Signature::new(strength, 0, coherence)
    }

    // ── sense: confidence ordering ──

    #[test]
    fn stronger_signature_higher_confidence() {
        let local = sig(1.0, 1.0);
        let others = [sig(0.5, 1.0), sig(5.0, 1.0)];
        let detected = sense(&local, &others, 0.0);
        // Both detected; index 1 (stronger) has higher confidence.
        let c0 = detected.iter().find(|(i, _)| *i == 0).map(|(_, c)| *c).unwrap_or(0.0);
        let c1 = detected.iter().find(|(i, _)| *i == 1).map(|(_, c)| *c).unwrap_or(0.0);
        assert!(c1 > c0, "stronger should have higher confidence: {} vs {}", c1, c0);
    }

    #[test]
    fn more_coherent_signature_higher_confidence() {
        let local = sig(1.0, 1.0);
        let others = [sig(3.0, 0.2), sig(3.0, 1.0)];
        let detected = sense(&local, &others, 0.0);
        let c0 = detected.iter().find(|(i, _)| *i == 0).map(|(_, c)| *c).unwrap_or(0.0);
        let c1 = detected.iter().find(|(i, _)| *i == 1).map(|(_, c)| *c).unwrap_or(0.0);
        assert!(c1 > c0, "more coherent should rank higher: {} vs {}", c1, c0);
    }

    #[test]
    fn closer_signature_higher_confidence() {
        let local = sig(1.0, 1.0);
        let others = [sig(3.0, 1.0)];
        let near = sense(&local, &others, 0.0);
        let far  = sense(&local, &others, 10.0);
        let cn = near.iter().find(|(i, _)| *i == 0).map(|(_, c)| *c).unwrap_or(0.0);
        let cf = far.iter().find(|(i, _)| *i == 0).map(|(_, c)| *c).unwrap_or(0.0);
        assert!(cn > cf, "closer should be higher: {} vs {}", cn, cf);
    }

    // ── sense: weak ones drop out ──

    #[test]
    fn very_weak_signature_dropped() {
        let local = sig(1.0, 1.0);
        let others = [sig(0.001, 0.1)];
        let detected = sense(&local, &others, 5.0);
        assert!(detected.is_empty(), "weak distant signature should drop out");
    }

    #[test]
    fn zero_strength_not_detected() {
        let local = sig(1.0, 1.0);
        let others = [sig(0.0, 1.0)];
        let detected = sense(&local, &others, 0.0);
        assert!(detected.is_empty());
    }

    #[test]
    fn zero_coherence_not_detected() {
        let local = sig(1.0, 1.0);
        let others = [sig(10.0, 0.0)];
        let detected = sense(&local, &others, 0.0);
        assert!(detected.is_empty());
    }

    // ── sense: confidence in [0,1] ──

    #[test]
    fn confidence_in_range() {
        let local = sig(1.0, 1.0);
        let others = [sig(1000.0, 1.0), sig(0.5, 0.7), sig(50.0, 0.9)];
        let detected = sense(&local, &others, 0.0);
        for (_, c) in &detected {
            assert!(*c >= 0.0 && *c <= 1.0, "confidence {} out of [0,1]", c);
        }
    }

    #[test]
    fn empty_others_empty_result() {
        let local = sig(1.0, 1.0);
        assert!(sense(&local, &[], 0.0).is_empty());
    }

    // ── sense: indices preserved ──

    #[test]
    fn indices_are_original_positions() {
        let local = sig(1.0, 1.0);
        // others[1] is undetectable, so result should contain 0 and 2.
        let others = [sig(5.0, 1.0), sig(0.0, 0.0), sig(5.0, 1.0)];
        let detected = sense(&local, &others, 0.0);
        let indices: Vec<usize> = detected.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, vec![0, 2]);
    }

    // ── disrupt ──

    #[test]
    fn disrupt_lowers_coherence() {
        let mut t = sig(5.0, 1.0);
        disrupt(&mut t, 0.5);
        assert!((t.coherence01 - 0.5).abs() < 1e-5);
    }

    #[test]
    fn disrupt_monotone_in_power() {
        let mut low = sig(5.0, 1.0);
        let mut high = sig(5.0, 1.0);
        disrupt(&mut low, 0.3);
        disrupt(&mut high, 0.7);
        assert!(high.coherence01 < low.coherence01,
            "more power should lower coherence more: {} vs {}", high.coherence01, low.coherence01);
    }

    #[test]
    fn disrupt_full_power_decoheres() {
        let mut t = sig(5.0, 1.0);
        disrupt(&mut t, 1.0);
        assert!((t.coherence01).abs() < 1e-6);
    }

    #[test]
    fn disrupt_zero_power_noop() {
        let mut t = sig(5.0, 0.8);
        disrupt(&mut t, 0.0);
        assert!((t.coherence01 - 0.8).abs() < 1e-6);
    }

    #[test]
    fn disrupt_coherence_never_negative() {
        let mut t = sig(5.0, 0.1);
        disrupt(&mut t, 2.0); // power clamped to 1.0
        assert!(t.coherence01 >= 0.0);
    }

    #[test]
    fn disrupt_clamps_negative_power() {
        let mut t = sig(5.0, 0.8);
        disrupt(&mut t, -1.0); // clamped to 0 → no-op
        assert!((t.coherence01 - 0.8).abs() < 1e-6);
    }

    // ── mask ──

    #[test]
    fn mask_lowers_strength() {
        let mut s = sig(10.0, 1.0);
        mask(&mut s, 0.5);
        assert!((s.strength - 5.0).abs() < 1e-4);
    }

    #[test]
    fn mask_full_power_zeroes_strength() {
        let mut s = sig(10.0, 1.0);
        mask(&mut s, 1.0);
        assert!(s.strength < 1e-6);
    }

    #[test]
    fn masked_bender_drops_out_of_sense() {
        let local = sig(1.0, 1.0);
        let mut hidden = sig(8.0, 1.0);
        // Before masking: detected.
        assert!(!sense(&local, &[hidden], 0.0).is_empty());
        // After full mask: undetectable.
        mask(&mut hidden, 1.0);
        assert!(sense(&local, &[hidden], 0.0).is_empty());
    }

    #[test]
    fn mask_strength_never_negative() {
        let mut s = sig(10.0, 1.0);
        mask(&mut s, 2.0); // clamped
        assert!(s.strength >= 0.0);
    }

    #[test]
    fn mask_zero_power_noop() {
        let mut s = sig(7.0, 1.0);
        mask(&mut s, 0.0);
        assert!((s.strength - 7.0).abs() < 1e-6);
    }

    // ── coherence_to_power ──

    #[test]
    fn power_full_at_full_coherence() {
        assert!((coherence_to_power(1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn power_floor_at_zero_coherence() {
        assert!((coherence_to_power(0.0) - POWER_FLOOR).abs() < 1e-6);
    }

    #[test]
    fn power_monotone_in_coherence() {
        let mut prev = coherence_to_power(0.0);
        let n = 50;
        for i in 1..=n {
            let c = i as f32 / n as f32;
            let p = coherence_to_power(c);
            assert!(p >= prev, "not monotone at c={}: {} < {}", c, p, prev);
            prev = p;
        }
    }

    #[test]
    fn power_in_range() {
        for i in 0..=100 {
            let c = i as f32 / 100.0;
            let p = coherence_to_power(c);
            assert!(p >= POWER_FLOOR - 1e-6 && p <= 1.0 + 1e-6,
                "power {} out of [{}, 1.0]", p, POWER_FLOOR);
        }
    }

    #[test]
    fn power_clamps_out_of_range_coherence() {
        assert!((coherence_to_power(2.0) - 1.0).abs() < 1e-6);
        assert!((coherence_to_power(-1.0) - POWER_FLOOR).abs() < 1e-6);
    }

    // ── Sanitization / no panic ──

    #[test]
    fn signature_new_sanitizes() {
        let s = Signature::new(f32::NAN, 3, 2.0);
        assert_eq!(s.strength, 0.0);
        assert_eq!(s.coherence01, 1.0); // 2.0 clamped
        assert_eq!(s.discipline, 3);
    }

    #[test]
    fn no_panic_on_nan_inf() {
        let local = Signature::new(f32::INFINITY, 0, f32::NAN);
        let others = [Signature::new(f32::NAN, 0, f32::INFINITY)];
        let _ = sense(&local, &others, f32::NAN);
        let mut t = sig(1.0, 1.0);
        disrupt(&mut t, f32::NAN);
        mask(&mut t, f32::INFINITY);
        let _ = coherence_to_power(f32::NAN);
    }

    // ── Determinism ──

    #[test]
    fn deterministic() {
        let local = sig(1.0, 0.8);
        let others = [sig(2.0, 0.9), sig(0.3, 0.5), sig(10.0, 1.0)];
        let a = sense(&local, &others, 1.5);
        let b = sense(&local, &others, 1.5);
        assert_eq!(a, b);
        assert!((coherence_to_power(0.42) - coherence_to_power(0.42)).abs() < 1e-9);
    }
}
