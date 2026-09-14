//! Central runtime configuration for the performance / graphics knobs that are
//! today scattered across `UNENDLICH_*` env vars (see [`crate::runtime_flags`]).
//!
//! This is the foundation for the unified Graphics + Performance settings UI:
//! the plan is for systems to read this struct instead of calling `env::var`
//! at startup, so a slider or preset can change them. To keep that migration
//! safe and incremental, [`RuntimeConfig::from_env`] seeds every field from the
//! same env var + default the system uses today, so wiring a system to read the
//! config is behavior-preserving by construction.
//!
//! Fields split into two classes:
//!   * **live** — can be applied while running (re-stream / re-mesh / adjust a
//!     cap): render distance, LOD bands/factors, upload caps, frame target,
//!     mesh cache, gen batch.
//!   * **needs restart** — require recreating GPU resources (swapchain / shadow
//!     map / mesher backend): vsync, shadow map size, gpu_mesher mode. The UI
//!     tags these and persists them for next launch.

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeConfig {
    // ---- live-applicable ----
    pub render_distance: u32,
    pub lod_bands: [u32; 3],
    pub lod_factors: [u32; 4],
    pub max_uploads_per_frame: u32,
    pub upload_budget_ms: f32,
    pub frame_target_ms: f32,
    pub mesh_cache_size: usize,
    pub gpu_gen_batch: u32,
    /// r708: per-frame GPU-mesh time budget (µs). When the octree GPU mesher is
    /// active (gpu_mesher=2), the dispatcher yields the GPU back to the render
    /// once it's spent this much meshing per frame — bounding the load-burst
    /// contention. 0 = unthrottled. No effect under the default CPU mesher.
    pub gpu_mesh_budget_us: u32,
    /// Render resolution scale for the neural upscaler (0.5..1.0; <1 renders
    /// internally smaller and upscales).
    pub render_scale: f32,
    /// Temporal amortization of shadow-caster re-rendering: render only
    /// `1/N` of shadow chunks per frame, round-robin, reusing the rest of
    /// last frame's depth (see `sim::shadow_refresh_plan` and
    /// `world_renderer::set_shadow_chunk_stride`). 1 = off (render every
    /// caster every frame). Live: `set_shadow_chunk_stride` applies it
    /// without recreating GPU resources.
    pub shadow_chunk_stride: u32,
    /// Skip the whole shadow pass on frames where the cascade view-proj is
    /// unchanged from last frame (sun and camera both static), reusing the
    /// persistent shadow image. Live: `set_shadow_cache`.
    pub shadow_cache: bool,
    // ---- needs restart ----
    pub shadow_map_size: u32,
    pub vsync: u32,
    pub gpu_mesher: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerfPreset {
    BatterySaver,
    Low,
    Medium,
    High,
    Ultra,
    /// Not a fixed table — the user has hand-tuned individual knobs.
    Custom,
}

impl PerfPreset {
    pub const SELECTABLE: [PerfPreset; 5] = [
        PerfPreset::BatterySaver,
        PerfPreset::Low,
        PerfPreset::Medium,
        PerfPreset::High,
        PerfPreset::Ultra,
    ];
    pub fn label(self) -> &'static str {
        match self {
            PerfPreset::BatterySaver => "Battery Saver",
            PerfPreset::Low => "Low",
            PerfPreset::Medium => "Medium",
            PerfPreset::High => "High",
            PerfPreset::Ultra => "Ultra",
            PerfPreset::Custom => "Custom",
        }
    }
}

/// Fields that only take effect after a restart (recreate GPU resources).
pub const NEEDS_RESTART: &[&str] = &["shadow_map_size", "vsync", "gpu_mesher"];

impl RuntimeConfig {
    /// Seed every field from its env var + the system's documented default, so
    /// reading the config matches today's behavior exactly.
    pub fn from_env() -> Self {
        fn u32v(k: &str, d: u32) -> u32 {
            std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
        }
        fn f32v(k: &str, d: f32) -> f32 {
            std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
        }
        fn arr3(k: &str, d: [u32; 3]) -> [u32; 3] {
            parse_csv::<3>(k).unwrap_or(d)
        }
        fn arr4(k: &str, d: [u32; 4]) -> [u32; 4] {
            parse_csv::<4>(k).unwrap_or(d)
        }
        // Start from the device-recommended preset instead of hardcoded Medium.
        // device_budget::recommend_preset() probes RAM, GPU class, thread count,
        // and mobile flag to pick a tier; the env overrides below let the user
        // override any individual field on top of that recommendation.
        // UNENDLICH_PERF_PRESET=<name> forces a specific preset as the base.
        let base = match std::env::var("UNENDLICH_PERF_PRESET").ok().as_deref() {
            Some("battery_saver") | Some("BatterySaver") =>
                Self::preset(PerfPreset::BatterySaver),
            Some("low") | Some("Low") =>
                Self::preset(PerfPreset::Low),
            Some("medium") | Some("Medium") =>
                Self::preset(PerfPreset::Medium),
            Some("high") | Some("High") =>
                Self::preset(PerfPreset::High),
            Some("ultra") | Some("Ultra") =>
                Self::preset(PerfPreset::Ultra),
            _ => {
                // Auto-detect from hardware.
                let caps = crate::device_budget::DeviceCaps::detect();
                let preset = crate::device_budget::recommend_preset(&caps);
                log::info!(
                    "device_budget: detected {:?} → preset {:?}",
                    caps, preset
                );
                Self::preset(preset)
            }
        };
        RuntimeConfig {
            render_distance: u32v("UNENDLICH_RENDER_DISTANCE", base.render_distance),
            lod_bands: arr3("UNENDLICH_LOD_BANDS", base.lod_bands),
            lod_factors: arr4("UNENDLICH_LOD_FACTORS", base.lod_factors),
            max_uploads_per_frame: u32v("UNENDLICH_MAX_UPLOADS_PER_FRAME", base.max_uploads_per_frame),
            upload_budget_ms: f32v("UNENDLICH_UPLOAD_BUDGET_MS", base.upload_budget_ms),
            frame_target_ms: f32v("UNENDLICH_FRAME_TARGET_MS", base.frame_target_ms),
            mesh_cache_size: u32v("UNENDLICH_MESH_CACHE_SIZE", base.mesh_cache_size as u32) as usize,
            gpu_gen_batch: u32v("UNENDLICH_GPU_GEN_BATCH", base.gpu_gen_batch),
            gpu_mesh_budget_us: u32v("UNENDLICH_GPU_MESH_BUDGET_US", base.gpu_mesh_budget_us),
            render_scale: f32v("UNENDLICH_RENDER_SCALE", base.render_scale),
            shadow_chunk_stride: u32v("UNENDLICH_SHADOW_CHUNK_STRIDE", base.shadow_chunk_stride)
                .clamp(1, 16),
            shadow_cache: std::env::var("UNENDLICH_SHADOW_CACHE")
                .ok()
                .map(|v| !(v == "0" || v.eq_ignore_ascii_case("false")))
                .unwrap_or(base.shadow_cache),
            shadow_map_size: u32v("UNENDLICH_SHADOW_MAP_SIZE", base.shadow_map_size),
            vsync: u32v("UNENDLICH_VSYNC", base.vsync),
            gpu_mesher: u32v("UNENDLICH_GPU_MESHER", base.gpu_mesher),
        }
    }

    /// A named quality preset. `Medium` mirrors the shipping defaults; the
    /// others scale the render distance / LOD reach / upload throughput / cache
    /// / shadow resolution around it. vsync and gpu_mesher are held constant
    /// (vsync is a user preference, gpu_mesher=0 (CPU) is the default; octree=2 is opt-in).
    pub fn preset(p: PerfPreset) -> Self {
        let base = RuntimeConfig {
            render_distance: 6,
            lod_bands: [8, 16, 32],
            lod_factors: [1, 2, 4, 8],
            max_uploads_per_frame: 64,
            upload_budget_ms: 4.0,
            frame_target_ms: 16.67,
            mesh_cache_size: 0,
            gpu_gen_batch: 16,
            gpu_mesh_budget_us: 4000,
            render_scale: 1.0,
            shadow_chunk_stride: 2,
            shadow_cache: true,
            shadow_map_size: 2048,
            vsync: 1,
            gpu_mesher: 0,
        };
        match p {
            PerfPreset::BatterySaver => RuntimeConfig {
                render_distance: 3,
                lod_bands: [4, 8, 16],
                max_uploads_per_frame: 4,
                upload_budget_ms: 2.0,
                frame_target_ms: 33.3, // 30 fps cap to save power
                mesh_cache_size: 16,
                gpu_gen_batch: 4,
                gpu_mesh_budget_us: 2000,
                render_scale: 0.6,
                // Standing/looking-around dominates on battery; a wide
                // stride hides almost all shadow-caster cost as long as the
                // camera isn't sprinting (draw_frame still forces a full
                // render on fast motion, so this doesn't smear in practice).
                shadow_chunk_stride: 4,
                shadow_map_size: 1024,
                ..base
            },
            PerfPreset::Low => RuntimeConfig {
                render_distance: 4,
                lod_bands: [6, 12, 24],
                max_uploads_per_frame: 8,
                upload_budget_ms: 3.0,
                gpu_gen_batch: 8,
                gpu_mesh_budget_us: 3000,
                render_scale: 0.8,
                shadow_chunk_stride: 3,
                shadow_map_size: 1024,
                mesh_cache_size: 32,
                ..base
            },
            PerfPreset::Medium => RuntimeConfig {
                // r-perf: a modest mesh cache avoids re-meshing on camera
                // turnarounds. At render distance 6 the player's viewable
                // shell is ~300 chunks; 64 cached slots cover ~20% of
                // behind-the-camera evictions without meaningful memory cost
                // (each cached entry is a MeshSlot handle, not the mesh data).
                mesh_cache_size: 64,
                ..base
            },
            PerfPreset::High => RuntimeConfig {
                render_distance: 10,
                lod_bands: [12, 24, 48],
                max_uploads_per_frame: 96,
                upload_budget_ms: 6.0,
                mesh_cache_size: 256,
                gpu_gen_batch: 24,
                gpu_mesh_budget_us: 6000,
                // More casters in view at RD10; amortize lightly rather
                // than not at all.
                shadow_chunk_stride: 2,
                shadow_map_size: 4096,
                ..base
            },
            PerfPreset::Ultra => RuntimeConfig {
                render_distance: 16,
                lod_bands: [16, 32, 64],
                max_uploads_per_frame: 128,
                upload_budget_ms: 8.0,
                mesh_cache_size: 512,
                gpu_gen_batch: 32,
                gpu_mesh_budget_us: 8000,
                // No amortization: at this tier we're assuming headroom to
                // spend, and stride>1 costs a frame of shadow staleness on
                // every camera move.
                shadow_chunk_stride: 1,
                shadow_map_size: 4096, // 8192 = 256MB D32, risks OOM
                ..base
            },
            PerfPreset::Custom => base,
        }
    }

    /// Best-matching preset for the current values, or `Custom` if none match
    /// (used to highlight the active preset button in the UI).
    pub fn matching_preset(&self) -> PerfPreset {
        PerfPreset::SELECTABLE
            .into_iter()
            .find(|&p| &RuntimeConfig::preset(p) == self)
            .unwrap_or(PerfPreset::Custom)
    }
}

fn parse_csv<const N: usize>(key: &str) -> Option<[u32; N]> {
    let s = std::env::var(key).ok()?;
    let parts: Vec<u32> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
    if parts.len() == N {
        let mut out = [0u32; N];
        out.copy_from_slice(&parts);
        Some(out)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn medium_matches_shipping_defaults() {
        // With no env overrides, from_env should equal the Medium preset.
        // (Run in a child-process-free way: only assert the fields that have
        // fixed defaults regardless of ambient env.)
        let m = RuntimeConfig::preset(PerfPreset::Medium);
        assert_eq!(m.render_distance, 6);
        assert_eq!(m.lod_bands, [8, 16, 32]);
        assert_eq!(m.gpu_mesher, 0); // r477: CPU mesher is the default
        assert_eq!(m.mesh_cache_size, 64);
        assert_eq!(m.gpu_mesh_budget_us, 4000); // r708: budget on by default
        assert_eq!(m.shadow_chunk_stride, 2);
        assert!(m.shadow_cache);
    }

    #[test]
    fn presets_scale_monotonically() {
        let bs = RuntimeConfig::preset(PerfPreset::BatterySaver);
        let ul = RuntimeConfig::preset(PerfPreset::Ultra);
        assert!(bs.render_distance < ul.render_distance);
        assert!(bs.max_uploads_per_frame < ul.max_uploads_per_frame);
        assert!(bs.shadow_map_size <= ul.shadow_map_size);
        assert!(bs.mesh_cache_size <= ul.mesh_cache_size);
        assert!(bs.gpu_mesh_budget_us <= ul.gpu_mesh_budget_us);
        // Shadow amortization runs the other way: cheaper tiers amortize
        // MORE (higher stride), Ultra spends full-rate every frame.
        assert!(bs.shadow_chunk_stride >= ul.shadow_chunk_stride);
        assert_eq!(ul.shadow_chunk_stride, 1);
    }

    #[test]
    fn shadow_stride_always_in_valid_range() {
        // world_renderer clamps stride to 1..=8 at the live setter, but the
        // shipped presets themselves must already fall in the wider 1..=16
        // range from_env accepts, and none should be 0 (that would mean
        // "never refresh").
        for p in PerfPreset::SELECTABLE {
            let cfg = RuntimeConfig::preset(p);
            assert!((1..=16).contains(&cfg.shadow_chunk_stride), "{:?}: {}", p, cfg.shadow_chunk_stride);
        }
    }

    // `from_env` reads process-wide env vars, which races with any other
    // test doing the same in this module. Serialize with a poisoning-
    // tolerant mutex (mirrors runtime_flags::tests::env_lock).
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn shadow_chunk_stride_env_override_is_clamped() {
        // A bogus/out-of-range env value should fall back to being clamped,
        // not panic or silently produce 0.
        let _g = env_lock();
        std::env::set_var("UNENDLICH_PERF_PRESET", "medium");
        std::env::set_var("UNENDLICH_SHADOW_CHUNK_STRIDE", "999");
        let cfg = RuntimeConfig::from_env();
        assert_eq!(cfg.shadow_chunk_stride, 16);
        std::env::remove_var("UNENDLICH_SHADOW_CHUNK_STRIDE");
        std::env::remove_var("UNENDLICH_PERF_PRESET");
    }

    #[test]
    fn shadow_cache_env_override_parses_false_variants() {
        let _g = env_lock();
        std::env::set_var("UNENDLICH_PERF_PRESET", "medium");
        for v in ["0", "false", "False", "FALSE"] {
            std::env::set_var("UNENDLICH_SHADOW_CACHE", v);
            assert!(!RuntimeConfig::from_env().shadow_cache, "{v} should disable shadow_cache");
        }
        std::env::set_var("UNENDLICH_SHADOW_CACHE", "1");
        assert!(RuntimeConfig::from_env().shadow_cache);
        std::env::remove_var("UNENDLICH_SHADOW_CACHE");
        std::env::remove_var("UNENDLICH_PERF_PRESET");
    }

    #[test]
    fn matching_preset_roundtrips() {
        for p in PerfPreset::SELECTABLE {
            assert_eq!(RuntimeConfig::preset(p).matching_preset(), p);
        }
    }
}
