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

/// Suggested `power_mode()` for a `GL_RENDERER` string, when nothing has chosen one explicitly.
///
/// Every dial in this file defaults to full power regardless of hardware — correct for the sandbox
/// this was built in, wrong for the actual fleet: postmarketOS / Fairphone / PinePhone / Asahi
/// devices run tiled mobile GPUs or software rasterisers that were never going to sustain the same
/// render scale and mesh density as a desktop discrete GPU, and nothing here has ever looked at
/// `GL_RENDERER` to say so. The renderer string is already logged at context creation
/// (`glow_scene::Renderer::new`) and thrown away.
///
/// Conservative both directions: only renderers this can positively identify as weak get throttled;
/// anything unrecognised — including hardware not in this list yet — stays at full power rather than
/// guessing a desktop GPU is slow. An explicit `LOGSCOPE_POWER` / `SCENE_KIT_POWER` or an in-app
/// setting always overrides this; it only fills the gap where nothing has decided yet.
pub fn suggested_power_for_renderer(renderer: &str) -> f32 {
    let r = renderer.to_ascii_lowercase();
    // Software rasterisers: no GPU at all. llvmpipe is this sandbox's own renderer (see every
    // Xvfb capture this session) and would otherwise "verify" full power on hardware that has none.
    if r.contains("llvmpipe") || r.contains("softpipe") || r.contains("swiftshader") {
        return 0.25;
    }
    // Older/entry mobile tiles: Mali-4xx (Utgard, no compute, tiny fill rate) and PowerVR SGX
    // (iPhone 4S/5-era and the low-end Android sets sharing that IP) are the weakest GPUs this
    // fleet's phone targets plausibly ship. Half power, not quarter — these still have a real GPU.
    if r.contains("mali-4") || r.contains("powervr sgx") {
        return 0.5;
    }
    // Everything else — Mali-T/G (Fairphone/PinePhone's actual Midgard/Bifrost/Valhall tiers),
    // Adreno, Asahi's AGX, VideoCore, or any desktop GPU — is left at full power. This is
    // deliberately not a full mobile-GPU database: it grows the next time a specific device is
    // slow and its GL_RENDERER string is in hand, not from a guess about silicon not yet measured.
    1.0
}

thread_local! {
    static DETECTED_RENDERER: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Record the `GL_RENDERER` string once a GL context exists, for `suggested_power_for_renderer`
/// callers that run before or after context creation and have no `&glow::Context` of their own
/// (a UI settings panel, in particular). Thread-local because glow contexts are `!Send` and this
/// is only ever read back on the same thread that created the context.
pub fn record_detected_renderer(renderer: String) {
    DETECTED_RENDERER.with(|d| *d.borrow_mut() = Some(renderer));
}

/// The last-recorded `GL_RENDERER` string, if a context has been created on this thread yet.
pub fn detected_renderer() -> Option<String> {
    DETECTED_RENDERER.with(|d| d.borrow().clone())
}


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

    // ── suggested_power_for_renderer / detected_renderer ──────────────────────

    #[test]
    fn software_rasterizers_are_throttled_hardest() {
        assert_eq!(suggested_power_for_renderer("llvmpipe (LLVM 20.1.2, 256 bits)"), 0.25);
        assert_eq!(suggested_power_for_renderer("softpipe"), 0.25);
        assert_eq!(suggested_power_for_renderer("SwiftShader Device (LLVM 16.0.0)"), 0.25);
    }

    #[test]
    fn entry_mobile_gpus_get_half_power() {
        assert_eq!(suggested_power_for_renderer("Mali-400 MP"), 0.5);
        assert_eq!(suggested_power_for_renderer("PowerVR SGX 543MP2"), 0.5);
    }

    #[test]
    fn newer_mobile_and_desktop_gpus_stay_full() {
        // These are the fleet's actual phone targets (Fairphone/PinePhone-class Mali, and Asahi's
        // AGX) — the classifier must NOT lump them in with the Mali-4xx (Utgard) entry above.
        for r in ["Mali-G610 MC4", "Mali-T860", "Apple M1 (AGX)", "Adreno (TM) 640",
                  "NVIDIA GeForce RTX 3080", "AMD Radeon RX 6800", "unknown future GPU XYZ"] {
            assert_eq!(suggested_power_for_renderer(r), 1.0, "{r} should stay full power");
        }
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(suggested_power_for_renderer("LLVMPIPE"), 0.25);
        assert_eq!(suggested_power_for_renderer("MALI-450 MP"), 0.5);
    }

    #[test]
    fn detected_renderer_round_trips() {
        // Thread-local, so no lock needed against the numeric-override tests above — but this test
        // still only asserts what IT just recorded, since another test on the same thread may run
        // either side of it and there is no "unset" to restore afterward.
        record_detected_renderer("Mali-400 MP".into());
        assert_eq!(detected_renderer().as_deref(), Some("Mali-400 MP"));
        assert_eq!(
            detected_renderer().map(|r| suggested_power_for_renderer(&r)),
            Some(0.5)
        );
    }
}
