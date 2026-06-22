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
    /// Render resolution scale for the neural upscaler (0.5..1.0; <1 renders
    /// internally smaller and upscales).
    pub render_scale: f32,
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
        RuntimeConfig {
            render_distance: u32v("UNENDLICH_RENDER_DISTANCE", 6),
            lod_bands: arr3("UNENDLICH_LOD_BANDS", [8, 16, 32]),
            lod_factors: arr4("UNENDLICH_LOD_FACTORS", [1, 2, 4, 8]),
            max_uploads_per_frame: u32v("UNENDLICH_MAX_UPLOADS_PER_FRAME", 64),
            upload_budget_ms: f32v("UNENDLICH_UPLOAD_BUDGET_MS", 4.0),
            frame_target_ms: f32v("UNENDLICH_FRAME_TARGET_MS", 16.67),
            mesh_cache_size: u32v("UNENDLICH_MESH_CACHE_SIZE", 0) as usize,
            gpu_gen_batch: u32v("UNENDLICH_GPU_GEN_BATCH", 16),
            render_scale: f32v("UNENDLICH_RENDER_SCALE", 1.0),
            shadow_map_size: u32v("UNENDLICH_SHADOW_MAP_SIZE", 2048),
            vsync: u32v("UNENDLICH_VSYNC", 1),
            gpu_mesher: u32v("UNENDLICH_GPU_MESHER", 2),
        }
    }

    /// A named quality preset. `Medium` mirrors the shipping defaults; the
    /// others scale the render distance / LOD reach / upload throughput / cache
    /// / shadow resolution around it. vsync and gpu_mesher are held constant
    /// (vsync is a user preference, gpu_mesher=2 is the recommended backend).
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
            render_scale: 1.0,
            shadow_map_size: 2048,
            vsync: 1,
            gpu_mesher: 2,
        };
        match p {
            PerfPreset::BatterySaver => RuntimeConfig {
                render_distance: 3,
                lod_bands: [4, 8, 16],
                max_uploads_per_frame: 4,
                upload_budget_ms: 2.0,
                frame_target_ms: 33.3, // 30 fps cap to save power
                mesh_cache_size: 0,
                gpu_gen_batch: 4,
                render_scale: 0.6,
                shadow_map_size: 1024,
                ..base
            },
            PerfPreset::Low => RuntimeConfig {
                render_distance: 4,
                lod_bands: [6, 12, 24],
                max_uploads_per_frame: 8,
                upload_budget_ms: 3.0,
                gpu_gen_batch: 8,
                render_scale: 0.8,
                shadow_map_size: 1024,
                ..base
            },
            PerfPreset::Medium => base,
            PerfPreset::High => RuntimeConfig {
                render_distance: 10,
                lod_bands: [12, 24, 48],
                max_uploads_per_frame: 96,
                upload_budget_ms: 6.0,
                mesh_cache_size: 256,
                gpu_gen_batch: 24,
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
                shadow_map_size: 8192,
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
        assert_eq!(m.gpu_mesher, 2);
        assert_eq!(m.mesh_cache_size, 0);
    }

    #[test]
    fn presets_scale_monotonically() {
        let bs = RuntimeConfig::preset(PerfPreset::BatterySaver);
        let ul = RuntimeConfig::preset(PerfPreset::Ultra);
        assert!(bs.render_distance < ul.render_distance);
        assert!(bs.max_uploads_per_frame < ul.max_uploads_per_frame);
        assert!(bs.shadow_map_size <= ul.shadow_map_size);
        assert!(bs.mesh_cache_size <= ul.mesh_cache_size);
    }

    #[test]
    fn matching_preset_roundtrips() {
        for p in PerfPreset::SELECTABLE {
            assert_eq!(RuntimeConfig::preset(p).matching_preset(), p);
        }
    }
}
