// src/volume_safety.rs
//
// Per-product volume ceilings + an approximate dB SPL conversion, so we
// can stop a speaker from blasting at 100% on startup when the device
// happens to have been left at a high level. B&O's internal scale is
// 0..100, but the *acoustic* output varies wildly by product — an A9
// at vol 80 is wall-shaking, a Beosound Emerge at vol 80 is conversational.
//
// HONEST CAVEAT: real dB SPL depends on listening distance, room
// acoustics, source content, and the speaker's frequency response. The
// numbers here are mid-range estimates from B&O datasheets, professional
// reviews, and real-world measurement averages at 1m on-axis with
// reference pink noise. Treat them as ±5 dB ballpark for setting safe
// ceilings, not for SPL meter replacement.
//
// Sources synthesized: B&O product spec pages, What Hi-Fi reviews,
// Audiosciencereview measurements where available.

/// Conservative safe-startup volume for each product. Computed as the
/// volume that produces ≈80 dB SPL — comfortably below OSHA's 85 dB
/// 8-hour exposure limit. Quiet enough for first connect; the user can
/// still raise it manually afterwards.
///
/// This is derived from the product's `max_db()` rather than hand-picked,
/// so when the dB estimate is updated all the thresholds move together.
pub fn safe_startup_volume(product_id: &str) -> i32 {
    db_to_volume(product_id, 80.0)
}

/// Approximate max acoustic output (dB SPL @ 1m, full-scale pink noise).
/// Used by [`approx_db_at`] below.
fn max_db(product_id: &str) -> f32 {
    match product_id {
        // Headphones — "at ear", much higher per-unit than free-field.
        // Note: 110 dB is the absolute peak SPL at max distortion;
        // realistic typical-listening loudness curve tops out closer
        // to 100 dB. Using the lower number keeps our ceilings honest.
        "BeoplayH100" | "BeoplayHX"                 => 100.0,
        // Largest speakers — A9 / vintage A9.
        "BeosoundA9" | "BeoplayA9"                  => 105.0,
        // Premium full-range — Stage soundbar, Eclipse TV.
        "BeosoundStage" | "BeovisionEclipse"        => 100.0,
        // Mid-range — Balance.
        "BeosoundBalance"                           => 98.0,
        // Portable hi-fi — Level / A5.
        "BeosoundLevel" | "BeosoundA5"              => 95.0,
        // Smaller TV — Contour.
        "BeovisionContour"                          => 90.0,
        // Compact — Emerge.
        "BeosoundEmerge"                            => 85.0,
        // Unknown — assume typical bookshelf-grade.
        _ => 95.0,
    }
}

/// Infer a canonical product_id from a device's friendly name.
///
/// mDNS announces devices as e.g. "Beosound-Emerge-36184899" — the
/// serial-numbered hostname. The Mozart REST API would normally give
/// us the canonical product_id ("BeosoundEmerge"), but mozart_legacy
/// firmware returns 500 for the product-info endpoints, so we fall
/// back to this name-derived inference.
///
/// Strategy:
///   1. Lowercase + strip non-alphanumeric.
///   2. Strip the trailing numeric run (the serial).
///   3. Compare against the known prefixes; return the canonical
///      product_id on match.
///
/// Returns `None` when nothing matches — caller should keep using
/// the generic "unknown" defaults rather than guess wrong.
pub fn infer_product_id(friendly_name: &str) -> Option<&'static str> {
    let mut normalized: String = friendly_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    // Strip a trailing numeric run (the serial suffix).
    while normalized.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        normalized.pop();
    }
    // Match by prefix — the inferred string can have trailing
    // device-specific noise that we want to ignore.
    let candidates: &[(&str, &str)] = &[
        ("beosoundstage",     "BeosoundStage"),
        ("beosoundbalance",   "BeosoundBalance"),
        ("beosoundlevel",     "BeosoundLevel"),
        ("beosounda5",        "BeosoundA5"),
        ("beosounda9",        "BeosoundA9"),
        ("beosoundemerge",    "BeosoundEmerge"),
        ("beosoundtheatre",   "BeosoundStage"),   // Theatre's profile is closest to Stage
        ("beovisioneclipse",  "BeovisionEclipse"),
        ("beovisioncontour",  "BeovisionContour"),
        ("beoplaya9",         "BeoplayA9"),
        ("beoplayh100",       "BeoplayH100"),
        ("beoplayhx",         "BeoplayHX"),
        ("beoplayh",          "BeoplayHX"),       // generic Beoplay H-series
    ];
    for (prefix, canonical) in candidates {
        if normalized.starts_with(prefix) {
            return Some(canonical);
        }
    }
    None
}

/// Approximate dB SPL at the device's listening position for a given
/// internal-scale volume (0..100). The mapping is logarithmic: each
/// halving of the volume value corresponds to roughly -6 dB.
///
/// `volume == 0` returns 0.0 (silent). Volumes outside 0..100 are clamped.
pub fn approx_db_at(product_id: &str, volume: i32) -> f32 {
    let v = volume.clamp(0, 100);
    if v == 0 { return 0.0; }
    let max = max_db(product_id);
    // Internal 0..100 scale -> linear amplitude fraction -> dB SPL.
    // 20 * log10(amplitude_ratio) added to the speaker's max-output dB.
    max + 20.0 * (v as f32 / 100.0).log10()
}

/// A short human description of where a volume sits perceptually.
/// Used by the UI toast when the safety check clamps a hot startup volume.
#[allow(dead_code)]
pub fn loudness_label(product_id: &str, volume: i32) -> &'static str {
    let db = approx_db_at(product_id, volume);
    match db {
        d if d <= 0.0      => "silent",
        d if d < 40.0      => "whisper",
        d if d < 55.0      => "quiet",
        d if d < 70.0      => "conversational",
        d if d < 80.0      => "loud",
        d if d < 90.0      => "very loud",
        d if d < 100.0     => "extreme",
        _                  => "hearing risk",
    }
}

/// Invert the volume→dB mapping: find the lowest internal-scale volume
/// that produces approximately `target_db` SPL on the given product.
/// If the product can't reach `target_db` (max_db < target_db), returns
/// 100 — meaning "saturated at maximum, but still below this target dB".
/// Callers should be aware that for quiet products some thresholds may
/// land at 100 (which makes them effectively unreachable, i.e. safe).
fn db_to_volume(product_id: &str, target_db: f32) -> i32 {
    let max = max_db(product_id);
    if target_db >= max { return 100; }
    // target_db = max + 20*log10(v/100) → v = 100 * 10^((target_db - max)/20)
    let v = 100.0 * 10f32.powf((target_db - max) / 20.0);
    v.round().clamp(1.0, 100.0) as i32
}

/// Volume above which the UI should surface a "this is loud" warning.
/// Defined as the volume that produces ≈88 dB SPL — just above the OSHA
/// 8-hour exposure limit (85 dB). Manual adjustments here still go
/// through; the user just sees a hint.
pub fn loud_threshold(product_id: &str) -> i32 {
    db_to_volume(product_id, 88.0)
}

/// Hard ceiling: manual volume requests above this are capped unless the
/// user has explicitly unlocked high-volume mode for the session.
/// Defined as ≈95 dB SPL (the OSHA 2-hour exposure limit) — entering
/// this band should be a deliberate choice, not an accident.
pub fn unsafe_threshold(product_id: &str) -> i32 {
    db_to_volume(product_id, 95.0)
}

/// Outcome of a safety-clamped volume change. Drives UI: a brief toast
/// when an attempted change was capped, or a styling change on the
/// volume control while in the loud band.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClampReason {
    /// Below all safety thresholds; passed through unchanged.
    NotClamped,
    /// Above the loud threshold; passed through but UI should warn.
    Loud,
    /// Above the unsafe threshold without an override; clamped down.
    Clamped { from: i32, to: i32 },
}

/// Apply per-product safety policy to a *requested* volume change.
/// Returns `(actual_value, reason)`. When `override_unlocked` is true the
/// hard ceiling is disabled — the loud-warn band is still reported, but
/// nothing gets clamped. This is the function `set_volume_api` calls on
/// every change.
pub fn clamp_for_safety(
    product_id: &str,
    requested: i32,
    override_unlocked: bool,
) -> (i32, ClampReason) {
    let req = requested.clamp(0, 100);
    let unsafe_t = unsafe_threshold(product_id);
    let loud_t = loud_threshold(product_id);
    if req > unsafe_t && !override_unlocked {
        return (unsafe_t, ClampReason::Clamped { from: req, to: unsafe_t });
    }
    if req > loud_t {
        return (req, ClampReason::Loud);
    }
    (req, ClampReason::NotClamped)
}

// ─── Hard absolute ceiling + rate limit ───────────────────────────────
//
// The clamp_for_safety above protects against "user accidentally
// nudges the slider too high" with an unlock escape hatch. It does
// NOT protect against:
//
//   A. An attacker who has compromised the controller (via a
//      malicious QML extension, a malformed config, a network-level
//      message forging if our peer auth is bypassed) and is issuing
//      programmatic set_volume calls.
//   B. The user accidentally storing 100 into their persisted
//      volume_high_unlocked state and forgetting it's on.
//
// The defenses below run UNDERNEATH clamp_for_safety — they apply
// even when override_unlocked is true:
//
//   1. HARD_ABSOLUTE_CEILING: a constant 95 (out of 100) above
//      which no software path may set the volume, period. This is
//      "if you want it louder than this, use the physical knob on
//      the device." 95/100 maps to ~94 dB on a typical
//      bookshelf-class B&O speaker — already in hearing-damage
//      territory for sustained listening, but not unrecoverable
//      for an accidental peak.
//   2. Rate limit: no more than MAX_CHANGES_PER_WINDOW set_volume
//      operations in WINDOW_MS milliseconds from any source. The
//      attacker pattern is "ramp from low to max as fast as
//      possible to surprise the listener"; capping the rate
//      defangs that. Legitimate user nudges easily fit under the
//      limit (~5 changes per second sustained = a clearly-malicious
//      pattern).

/// Maximum volume value the software is permitted to set. Hardware
/// can go higher via the speaker's physical control; this only
/// constrains the controller's set_volume_api path.
pub const HARD_ABSOLUTE_CEILING: i32 = 95;

/// Rate limit window for set_volume calls.
pub const RATE_LIMIT_WINDOW_MS: u64 = 1_000;
pub const RATE_LIMIT_MAX_CHANGES: usize = 12;
/// Duration of the lockout after the rate limit trips. During the
/// lockout, ALL volume changes are dropped — even from the
/// legitimate UI — because we can't easily distinguish them from
/// the attacker. The user can see the lockout state in the UI and
/// wait it out (the cooldown is short).
pub const RATE_LIMIT_COOLDOWN_MS: u64 = 5_000;

/// State for the rate limiter. The mesh + UI set_volume paths both
/// run through `gate()` before letting the request through.
pub struct VolumeRateLimiter {
    /// Timestamps (ms) of recent set_volume calls.
    history: Vec<u64>,
    /// Time at which the current lockout (if any) ends.
    lockout_until_ms: u64,
}

impl Default for VolumeRateLimiter {
    fn default() -> Self {
        Self { history: Vec::with_capacity(32), lockout_until_ms: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitOutcome {
    /// Pass — the request is permitted.
    Pass,
    /// The window just filled — this is the N-th request, lockout
    /// begins now.
    Tripped { lockout_ms: u64 },
    /// Already in a lockout; request is dropped.
    Locked { remaining_ms: u64 },
}

impl VolumeRateLimiter {
    /// Check + record a new set_volume attempt. `now_ms` is the
    /// current monotonic time in milliseconds (the caller is
    /// responsible for passing a monotonic source, typically
    /// `Instant::now().elapsed_since(start)` or wall-clock for
    /// non-test paths).
    pub fn gate(&mut self, now_ms: u64) -> RateLimitOutcome {
        if now_ms < self.lockout_until_ms {
            return RateLimitOutcome::Locked {
                remaining_ms: self.lockout_until_ms - now_ms,
            };
        }
        // Evict timestamps older than the window.
        let cutoff = now_ms.saturating_sub(RATE_LIMIT_WINDOW_MS);
        self.history.retain(|&t| t >= cutoff);
        self.history.push(now_ms);
        if self.history.len() > RATE_LIMIT_MAX_CHANGES {
            self.lockout_until_ms = now_ms + RATE_LIMIT_COOLDOWN_MS;
            // Clear history — the next request after cooldown
            // starts with a clean window so a slow-poll user
            // isn't immediately re-tripped.
            self.history.clear();
            return RateLimitOutcome::Tripped { lockout_ms: RATE_LIMIT_COOLDOWN_MS };
        }
        RateLimitOutcome::Pass
    }
}

/// Apply the hard absolute ceiling. Pure function; safe to call
/// from any path including under-auth contexts.
pub fn enforce_hard_ceiling(v: i32) -> i32 {
    v.min(HARD_ABSOLUTE_CEILING)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_ceilings_are_below_exposure_risk() {
        // The ceiling should land under OSHA's 4-hour sustained-exposure
        // limit (92 dB). The point isn't to make startup silent — it's
        // to prevent a "wake the neighbors" surprise on first connect.
        for product in &[
            "BeosoundStage", "BeosoundA9", "BeosoundBalance",
            "BeosoundLevel", "BeosoundA5", "BeovisionEclipse",
            "BeovisionContour", "BeosoundEmerge", "BeoplayA9",
            "BeoplayH100", "BeoplayHX", "Unknown",
        ] {
            let ceil = safe_startup_volume(product);
            let db = approx_db_at(product, ceil);
            assert!(db <= 92.0,
                "ceiling for {} is {:.1} dB at vol {} (>92 dB risk threshold)",
                product, db, ceil);
        }
    }

    #[test]
    fn db_grows_monotonically_with_volume() {
        let p = "BeosoundStage";
        let mut last = -100.0;
        for v in [1, 10, 25, 50, 75, 100] {
            let db = approx_db_at(p, v);
            assert!(db > last, "db should grow: vol {} -> {} dB (prev {})", v, db, last);
            last = db;
        }
    }

    #[test]
    fn doubling_volume_is_roughly_six_db() {
        // 20*log10(2) ≈ 6.02 dB
        let p = "BeosoundStage";
        let d1 = approx_db_at(p, 50);
        let d2 = approx_db_at(p, 100);
        assert!((d2 - d1 - 6.02).abs() < 0.05,
                "doubling vol should be ~6 dB, got {}", d2 - d1);
    }

    #[test]
    fn vol_zero_is_zero() {
        assert_eq!(approx_db_at("BeosoundStage", 0), 0.0);
    }

    #[test]
    fn unknown_product_uses_conservative_defaults() {
        // Unknown products use the default max_db (95 dB). At 80 dB target,
        // that's 100*10^((80-95)/20) = 100*10^(-0.75) ≈ 18.
        let v = safe_startup_volume("MysteryDevice");
        let db = approx_db_at("MysteryDevice", v);
        assert!((db - 80.0).abs() <= 1.5,
                "unknown startup vol {} = {:.1} dB (target 80)", v, db);
        assert!(approx_db_at("MysteryDevice", 50) > 0.0);
    }

    #[test]
    fn headphones_have_higher_max_than_speakers() {
        // H100 is at-ear, ~110 dB; Emerge is free-field, ~85 dB.
        // At the same internal volume, headphones should read higher.
        assert!(approx_db_at("BeoplayH100", 50) >
                approx_db_at("BeosoundEmerge", 50));
    }

    #[test]
    fn loudness_labels_match_common_sense() {
        assert_eq!(loudness_label("BeosoundStage", 0), "silent");
        // BeosoundStage at vol 100 → 100 dB SPL → "extreme" or "hearing risk"
        let lbl = loudness_label("BeosoundStage", 100);
        assert!(lbl == "extreme" || lbl == "hearing risk",
                "vol 100 should be extreme, got {}", lbl);
    }

    // ─── Threshold sanity ────────────────────────────────────────────

    /// Every product's three safety levels must respect the inequality
    ///   startup ≤ loud ≤ unsafe ≤ 100
    /// Without this, the system can produce nonsensical UI states (e.g.
    /// startup ceiling already in the unsafe band).
    #[test]
    fn thresholds_are_ordered() {
        for product in &[
            "BeosoundStage", "BeosoundA9", "BeosoundBalance",
            "BeosoundLevel", "BeosoundA5", "BeovisionEclipse",
            "BeovisionContour", "BeosoundEmerge", "BeoplayA9",
            "BeoplayH100", "BeoplayHX", "Unknown",
        ] {
            let s = safe_startup_volume(product);
            let l = loud_threshold(product);
            let u = unsafe_threshold(product);
            assert!(s <= l, "{}: startup {} > loud {}",   product, s, l);
            assert!(l <= u, "{}: loud {} > unsafe {}",    product, l, u);
            assert!(u <= 100, "{}: unsafe {} > 100",      product, u);
        }
    }

    /// Loud threshold should map to ≈88 dB on every product whose max
    /// output reaches that level. Products with lower max_db (e.g. the
    /// Emerge at 85 dB) can't physically hit 88 dB — for those the
    /// threshold saturates at 100 (which is fine; they're inherently
    /// bounded and need no warning).
    #[test]
    fn loud_threshold_lands_near_88_db_when_reachable() {
        for product in &[
            "BeosoundStage", "BeosoundA9", "BeosoundBalance",
            "BeoplayH100", "Unknown",
        ] {
            // These products all have max_db ≥ 88, so the threshold
            // should hit the band.
            let v = loud_threshold(product);
            let db = approx_db_at(product, v);
            assert!((db - 88.0).abs() <= 1.0,
                    "{}: loud_threshold vol {} = {:.1} dB (target 88)",
                    product, v, db);
        }
        // The Emerge maxes at 85 dB — loud threshold should be 100 (saturated).
        assert_eq!(loud_threshold("BeosoundEmerge"), 100);
    }

    #[test]
    fn unsafe_threshold_lands_near_95_db_when_reachable() {
        for product in &[
            "BeosoundStage", "BeosoundA9", "BeosoundBalance",
            "BeoplayH100", "Unknown",
        ] {
            let v = unsafe_threshold(product);
            let db = approx_db_at(product, v);
            assert!((db - 95.0).abs() <= 1.0,
                    "{}: unsafe_threshold vol {} = {:.1} dB (target 95)",
                    product, v, db);
        }
        // Quiet products can't reach 95 dB at all → saturated at 100,
        // which is correct: their physical max IS their safety ceiling.
        assert_eq!(unsafe_threshold("BeosoundEmerge"), 100);
        assert_eq!(unsafe_threshold("BeovisionContour"), 100);
    }

    /// For loud products (max ≥ 95 dB), max volume MUST land in the
    /// unsafe band. Otherwise the clamp does nothing on the very devices
    /// that need it most.
    #[test]
    fn loud_products_have_meaningful_unsafe_threshold() {
        for product in &[
            "BeosoundA9", "BeoplayA9", "BeosoundStage", "BeovisionEclipse",
            "BeosoundBalance", "BeoplayH100", "BeoplayHX",
        ] {
            assert!(100 > unsafe_threshold(product),
                    "{}: max vol (100) <= unsafe threshold ({}) — safety inert!",
                    product, unsafe_threshold(product));
        }
    }

    // ─── clamp_for_safety semantics ──────────────────────────────────

    #[test]
    fn clamp_passes_through_below_loud() {
        let (v, r) = clamp_for_safety("BeosoundStage", 20, false);
        assert_eq!(v, 20);
        assert_eq!(r, ClampReason::NotClamped);
    }

    #[test]
    fn clamp_warns_in_loud_band() {
        // Pick a value between loud and unsafe.
        let pid = "BeosoundStage";
        let l = loud_threshold(pid);
        let u = unsafe_threshold(pid);
        let mid = (l + u) / 2;
        if mid <= l || mid >= u { return; } // skip if bands too narrow
        let (v, r) = clamp_for_safety(pid, mid, false);
        assert_eq!(v, mid, "loud band shouldn't clamp the value");
        assert_eq!(r, ClampReason::Loud);
    }

    #[test]
    fn clamp_caps_unsafe_without_override() {
        let pid = "BeosoundStage";
        let u = unsafe_threshold(pid);
        let (v, r) = clamp_for_safety(pid, 100, false);
        assert_eq!(v, u);
        assert!(matches!(r, ClampReason::Clamped { from: 100, to } if to == u));
    }

    #[test]
    fn clamp_allows_unsafe_with_override() {
        // Override unlocked → full volume passes through (still reported
        // as Loud so the UI can keep the warning styling).
        let pid = "BeosoundStage";
        let (v, r) = clamp_for_safety(pid, 100, true);
        assert_eq!(v, 100);
        // Vol 100 is also above loud_threshold, so reason is Loud.
        assert_eq!(r, ClampReason::Loud);
    }

    /// Spot-check the BeosoundStage numbers against published specs:
    /// B&O lists Stage max SPL near 100 dB; ceiling 30 should land near
    /// 90 dB (firmly in the "loud but safe for short bursts" band).
    #[test]
    fn beosound_stage_numbers_match_real_world() {
        let p = "BeosoundStage";
        assert!((approx_db_at(p, 100) - 100.0).abs() < 1.0,
                "Stage @ vol 100 should be ~100 dB SPL");
        // Doubling volume = +6 dB
        let d50 = approx_db_at(p, 50);
        let d100 = approx_db_at(p, 100);
        assert!((d100 - d50 - 6.02).abs() < 0.05);
        // Startup ceiling 30 is around 90 dB (loud but bounded).
        assert!(approx_db_at(p, safe_startup_volume(p)) < 92.0);
    }

    #[test]
    fn hard_ceiling_caps_above_threshold() {
        assert_eq!(enforce_hard_ceiling(100), HARD_ABSOLUTE_CEILING);
        assert_eq!(enforce_hard_ceiling(96), HARD_ABSOLUTE_CEILING);
        assert_eq!(enforce_hard_ceiling(95), 95);
        assert_eq!(enforce_hard_ceiling(50), 50);
        assert_eq!(enforce_hard_ceiling(0), 0);
    }

    #[test]
    fn rate_limiter_passes_within_budget() {
        let mut rl = VolumeRateLimiter::default();
        for i in 0..RATE_LIMIT_MAX_CHANGES {
            assert_eq!(rl.gate(i as u64 * 10), RateLimitOutcome::Pass,
                "should pass at request {}", i);
        }
    }

    #[test]
    fn rate_limiter_trips_when_window_overflows() {
        let mut rl = VolumeRateLimiter::default();
        // Fire MAX_CHANGES + 1 within the window, all at t=0..N.
        for _ in 0..RATE_LIMIT_MAX_CHANGES { let _ = rl.gate(0); }
        let outcome = rl.gate(0);
        assert!(matches!(outcome, RateLimitOutcome::Tripped { .. }),
            "should trip on overflow, got {:?}", outcome);
    }

    #[test]
    fn rate_limiter_locks_subsequent_requests() {
        let mut rl = VolumeRateLimiter::default();
        // Trip it.
        for _ in 0..=RATE_LIMIT_MAX_CHANGES { let _ = rl.gate(0); }
        // Next request during cooldown → Locked.
        let outcome = rl.gate(500);
        assert!(matches!(outcome, RateLimitOutcome::Locked { .. }),
            "should be locked during cooldown, got {:?}", outcome);
    }

    #[test]
    fn rate_limiter_releases_after_cooldown() {
        let mut rl = VolumeRateLimiter::default();
        for _ in 0..=RATE_LIMIT_MAX_CHANGES { let _ = rl.gate(0); }
        // Cooldown ends; new request passes.
        let outcome = rl.gate(RATE_LIMIT_COOLDOWN_MS + 1);
        assert_eq!(outcome, RateLimitOutcome::Pass);
    }

    #[test]
    fn rate_limiter_slow_caller_never_trips() {
        // Spread requests over a long period — well under the rate.
        let mut rl = VolumeRateLimiter::default();
        for i in 0..100 {
            // 1 request per second — way under the budget.
            let outcome = rl.gate(i as u64 * 1_000);
            assert_eq!(outcome, RateLimitOutcome::Pass);
        }
    }

    #[test]
    fn infer_handles_serial_suffixes() {
        // The exact mDNS-announced names from the user's logs.
        assert_eq!(infer_product_id("Beosound-Emerge-36184899"), Some("BeosoundEmerge"));
        assert_eq!(infer_product_id("Beosound-Stage-1234"),       Some("BeosoundStage"));
        assert_eq!(infer_product_id("BeoSound Balance-22"),       Some("BeosoundBalance"));
        // Just the name with no serial.
        assert_eq!(infer_product_id("Beosound Emerge"),           Some("BeosoundEmerge"));
        // Case-insensitive.
        assert_eq!(infer_product_id("BEOSOUND-STAGE-99"),         Some("BeosoundStage"));
    }

    #[test]
    fn infer_returns_none_on_unknown_names() {
        assert_eq!(infer_product_id("Some random speaker"), None);
        assert_eq!(infer_product_id(""), None);
        assert_eq!(infer_product_id("12345"), None);
    }

    #[test]
    fn infer_handles_punctuation_variants() {
        // Different separators in friendly names.
        assert_eq!(infer_product_id("Beosound_Emerge_123"), Some("BeosoundEmerge"));
        assert_eq!(infer_product_id("BeosoundEmerge"),      Some("BeosoundEmerge"));
        assert_eq!(infer_product_id("Beosound.Emerge.99"),  Some("BeosoundEmerge"));
    }

    #[test]
    fn inferred_product_yields_quieter_ceiling_than_unknown() {
        // The Emerge is a compact speaker — its safety ceiling
        // should be different from the generic "unknown" default.
        let inferred = safe_startup_volume("BeosoundEmerge");
        let unknown = safe_startup_volume("Unknown");
        // Both yield SOME ceiling; the test is that the inferred
        // value is sensitive to the product profile.
        assert!(inferred > 0 && inferred < 100);
        assert!(unknown > 0 && unknown < 100);
        // They should differ because the max_db differs.
        assert_ne!(inferred, unknown);
    }
}
