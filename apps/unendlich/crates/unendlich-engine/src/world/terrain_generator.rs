//! Unified `TerrainGenerator` that abstracts over CPU and GPU backends.
//!
//! The GPU path is gated behind the `gpu` cargo feature. With
//! `--no-default-features` the generator is CPU-only and `Backend::Gpu`
//! returns an error.
//!
//! Both paths produce byte-identical chunks for the same `(coord, seed)`,
//! which is what makes pristine-chunk regeneration safe — load a chunk by
//! computing it from seed, edit it, store the deltas only.

use crate::world::chunk::{Chunk, ChunkCoord};
use crate::world::cpu_generator::WorldGenerator;
use std::sync::Arc;

#[cfg(feature = "gpu")]
use crate::world::gpu_generator::{GpuGenError, VulkanChunkGenerator};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// Use the GPU if available, fall back to CPU on any failure.
    Auto,
    /// Force CPU.
    Cpu,
    /// Force GPU. Errors if Vulkan init fails or the `gpu` feature is disabled.
    Gpu,
}

#[derive(Debug)]
pub enum TerrainGenError {
    GpuUnavailable(String),
    GpuRequestedButFeatureDisabled,
}

impl std::fmt::Display for TerrainGenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerrainGenError::GpuUnavailable(s) => write!(f, "GPU unavailable: {}", s),
            TerrainGenError::GpuRequestedButFeatureDisabled =>
                write!(f, "Backend::Gpu requested but the `gpu` cargo feature is disabled"),
        }
    }
}
impl std::error::Error for TerrainGenError {}

#[cfg(feature = "gpu")]
impl From<GpuGenError> for TerrainGenError {
    fn from(e: GpuGenError) -> Self { TerrainGenError::GpuUnavailable(e.to_string()) }
}

pub struct TerrainGenerator {
    cpu: Arc<WorldGenerator>,
    #[cfg(feature = "gpu")]
    gpu: Option<Arc<VulkanChunkGenerator>>,
    seed: u32,
}

impl TerrainGenerator {
    pub fn new(seed: u64, backend: Backend, batch_size: u32) -> Result<Self, TerrainGenError> {
        let seed32 = seed as u32 ^ (seed >> 32) as u32;

        // Try to load the biome registry from disk. If it fails (assets/biomes
        // missing or empty), we proceed with no registry — biome lookups fall
        // back to the legacy decision tree, which is fully functional.
        let registry = match crate::world::biome_loader::BiomeRegistry::load_standard() {
            Ok(r) => Some(Arc::new(r)),
            Err(e) => {
                log::warn!("Biome registry unavailable: {}. Using legacy hardcoded biomes.", e);
                None
            }
        };

        let cpu = match registry.clone() {
            Some(r) => Arc::new(WorldGenerator::with_biome_registry(seed32, r)),
            None    => Arc::new(WorldGenerator::new(seed32)),
        };

        #[cfg(feature = "gpu")]
        let gpu = match backend {
            Backend::Cpu => None,
            Backend::Auto => match VulkanChunkGenerator::new(batch_size) {
                Ok(g) => {
                    log::info!("GPU world generation enabled (batch size = {}).", batch_size);
                    Some(Arc::new(g))
                }
                Err(e) => {
                    log::warn!("GPU unavailable: {}. Using CPU.", e);
                    None
                }
            },
            Backend::Gpu => Some(Arc::new(VulkanChunkGenerator::new(batch_size)?)),
        };

        #[cfg(not(feature = "gpu"))]
        match backend {
            Backend::Gpu => return Err(TerrainGenError::GpuRequestedButFeatureDisabled),
            _ => {}
        }

        let _ = batch_size; // unused in non-gpu builds

        Ok(Self {
            cpu,
            #[cfg(feature = "gpu")]
            gpu,
            seed: seed32,
        })
    }

    pub fn seed(&self) -> u32 { self.seed }

    pub fn has_gpu(&self) -> bool {
        #[cfg(feature = "gpu")] { self.gpu.is_some() }
        #[cfg(not(feature = "gpu"))] { false }
    }

    pub fn cpu(&self) -> &Arc<WorldGenerator> { &self.cpu }

    /// Get an `Arc` clone of the world's field model. Used to share
    /// the model with the mesher so it can sample temperature/humidity
    /// per face for material-response tinting. The model uses
    /// `DashMap` internally for caching, so multiple threads sharing
    /// one Arc all benefit from the same region-grid cache.
    pub fn model_arc(&self) -> Arc<crate::world::world_model::WorldModel> {
        self.cpu.model_arc()
    }

    /// Generate chunks. Uses GPU batching if available, falls back to CPU on
    /// Per-dispatch GPU batch size (chunks). The chunk loader uses this as its
    /// take-size so loader and generator stay matched (the GPU generator rejects
    /// batches larger than its own `batch_size`). Falls back to 16 with no GPU.
    pub fn batch_size(&self) -> usize {
        #[cfg(feature = "gpu")]
        {
            if let Some(gpu) = &self.gpu {
                return gpu.batch_size() as usize;
            }
        }
        16
    }

    /// any error from the GPU path. The CPU and GPU paths produce
    /// byte-identical output for the same inputs.
    pub fn generate(&self, coords: &[ChunkCoord]) -> Vec<Chunk> {
        if coords.is_empty() { return Vec::new(); }

        #[cfg(feature = "gpu")]
        {
            if let Some(gpu) = &self.gpu {
                let cap = gpu.batch_size() as usize;
                let mut all = Vec::with_capacity(coords.len());

                for batch in coords.chunks(cap) {
                    match gpu.generate_chunks(batch, self.seed) {
                        Ok(mut chunks) => all.append(&mut chunks),
                        Err(e) => {
                            log::warn!("GPU batch failed ({}). Falling back to CPU.", e);
                            for &c in batch { all.push(self.cpu.generate_chunk(c)); }
                        }
                    }
                }
                return all;
            }
        }

        coords.iter().map(|&c| self.cpu.generate_chunk(c)).collect()
    }
}
