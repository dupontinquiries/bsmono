//! perf — the two dials glow_scene/gpuscene used to read from logscope's
//! `theme` module, decoupled. Env-tunable, faithful defaults (logscope tier 6).

/// 1.0 full · 0.5 half · 0.25 quarter. Scales render resolution and widens
/// the frustum-cull margin (`SCENE_KIT_POWER`).
pub fn power_mode() -> f32 {
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
    std::env::var("SCENE_KIT_TEX").ok().and_then(|v| v.parse().ok()).unwrap_or(256).clamp(64, 1024)
}

/// Normal-map relief baked into the texture arrays (`SCENE_KIT_BUMP`).
pub fn bump_scale() -> f32 {
    std::env::var("SCENE_KIT_BUMP").ok().and_then(|v| v.parse().ok()).unwrap_or(1.05f32).clamp(0.2, 3.0)
}
