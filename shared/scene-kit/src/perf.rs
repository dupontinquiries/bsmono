//! perf — the two dials glow_scene/gpuscene used to read from logscope's
//! `theme` module, decoupled. Env-tunable, faithful defaults (logscope tier 6).

/// 1.0 full · 0.5 half · 0.25 quarter. Scales render resolution and widens
/// the frustum-cull margin (`SCENE_KIT_POWER`).
pub fn power_mode() -> f32 {
    if let Some(v) = ov_f32(&OV_POWER) { return v; }
    match std::env::var("SCENE_KIT_POWER").ok().map(|v| v.trim().to_lowercase()) {
        Some(ref v) if v == "half" => 0.5,
        Some(ref v) if v == "quarter" => 0.25,
        Some(ref v) if v == "full" || v.is_empty() => 1.0,
        Some(ref v) => v.parse::<f32>().unwrap_or(1.0).clamp(0.25, 1.0),
        None => 1.0,
    }
}

/// Baked material texture resolution per layer (`SCENE_KIT_TEX`, px).
pub fn texture_res() -> usize {
    let t = OV_TEX.load(Ordering::Relaxed);
    if t != u32::MAX { return t as usize; }
    std::env::var("SCENE_KIT_TEX").ok().and_then(|v| v.parse().ok()).unwrap_or(256).clamp(64, 1024)
}

/// Normal-map relief baked into the texture arrays (`SCENE_KIT_BUMP`).
pub fn bump_scale() -> f32 {
    if let Some(v) = ov_f32(&OV_BUMP) { return v; }
    std::env::var("SCENE_KIT_BUMP").ok().and_then(|v| v.parse().ok()).unwrap_or(1.05f32).clamp(0.2, 3.0)
}

// ── host override ────────────────────────────────────────────────────────────
//
// The env vars above are the DEFAULT, not the only source. logscope reads
// these two dials from its `theme` module, where they are user-settable in the
// UI, and that is why `glow_scene`/`gpuscene` have not migrated here: adopting
// an env-only `perf` would silently turn a working in-app power control into
// something only settable by restarting with a variable set.
//
// That is the same trap `texgen` presents from the other direction (the shared
// copy is the stale one there) — the extracted version looks cleaner and
// quietly drops a capability the app has. So the seam takes an override: a
// host pushes its own values, and anything that has not been pushed falls back
// to the env/default behaviour unchanged.

use std::sync::atomic::{AtomicU32, Ordering};

/// Bit-cast f32 overrides. `u32::MAX` is the "unset" sentinel rather than a
/// separate `Option` + lock: these are read per frame from the render path,
/// and NaN is not usable as the sentinel because a caller could legitimately
/// push one and we want that to be visible as a bug rather than swallowed.
static OV_POWER: AtomicU32 = AtomicU32::new(u32::MAX);
static OV_TEX: AtomicU32 = AtomicU32::new(u32::MAX);
static OV_BUMP: AtomicU32 = AtomicU32::new(u32::MAX);

/// Push the host's dials. Call once per frame or whenever settings change;
/// it is three relaxed stores.
pub fn set_overrides(power: f32, texture_res: usize, bump: f32) {
    OV_POWER.store(power.clamp(0.25, 1.0).to_bits(), Ordering::Relaxed);
    OV_TEX.store(texture_res.clamp(64, 1024) as u32, Ordering::Relaxed);
    OV_BUMP.store(bump.clamp(0.2, 3.0).to_bits(), Ordering::Relaxed);
}

/// Drop back to env/defaults — for tests, and for a host that stops driving.
pub fn clear_overrides() {
    for a in [&OV_POWER, &OV_TEX, &OV_BUMP] { a.store(u32::MAX, Ordering::Relaxed); }
}

fn ov_f32(a: &AtomicU32) -> Option<f32> {
    let b = a.load(Ordering::Relaxed);
    (b != u32::MAX).then(|| f32::from_bits(b))
}

#[cfg(test)]
mod override_tests {
    use super::*;

    // Serialised: these are process-wide, and the counter races fixed
    // elsewhere this session are the same shape.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn an_unset_override_leaves_the_env_default_alone() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_overrides();
        assert_eq!(power_mode(), 1.0);
        assert_eq!(texture_res(), 256);
    }

    #[test]
    fn a_host_value_wins_over_the_default() {
        // The property the migration needs: logscope's in-app power control
        // must still work after glow_scene moves here.
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_overrides(0.5, 512, 2.0);
        assert_eq!(power_mode(), 0.5);
        assert_eq!(texture_res(), 512);
        assert!((bump_scale() - 2.0).abs() < 1e-6);
        clear_overrides();
    }

    #[test]
    fn out_of_range_host_values_are_clamped_not_trusted() {
        // A host bug must not hand the renderer a zero resolution.
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_overrides(99.0, 1, -5.0);
        assert_eq!(power_mode(), 1.0);
        assert_eq!(texture_res(), 64);
        assert!(bump_scale() >= 0.2);
        clear_overrides();
    }

    #[test]
    fn clearing_restores_the_default() {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_overrides(0.25, 128, 0.5);
        clear_overrides();
        assert_eq!(power_mode(), 1.0);
        assert_eq!(texture_res(), 256);
    }
}
