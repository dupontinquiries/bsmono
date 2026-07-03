//! # av1tuner_core
//!
//! Core types and orchestration for the AV1 batch tuner. UI-agnostic.
//!
//! Module layout:
//! - `chunk` — chunk state, what each segment of a job knows about itself
//! - `encoder` — the trait every encoder backend implements + the SVT-AV1 impl
//! - `job` — a single encode job (one input video → one output video)
//! - `scheduler` — runs chunks in parallel with retry + isolation
//! - `preflight` — pre-encode validation (binary present, settings sane, etc.)
//! - `probe` — ffprobe wrapper for source video metadata
//! - `scene` — scene detection for chunk boundaries
//! - `concat` — final ffmpeg concat from chunk outputs

pub mod audio;
pub mod basic_upscaler;
pub mod blockproc;
pub mod cache;
pub mod chunk;
pub mod config;
pub mod cleanup;
pub mod concat;
pub mod eta;
pub mod discovery;
pub mod encoder;
pub mod group;
pub mod group_runner;
pub mod job;
pub mod preflight;
pub mod probe;
pub mod sample;
pub mod scene;
pub mod merge;
pub mod prediction_stats;
pub mod scheduler;
pub mod vvenc_install;

pub use audio::{AudioSettings, AudioMode, OpusSettings, OpusApplication};
pub use blockproc::{Block, BlockId, BlockFormat, BlockProcessor, DedupBatch, BlockError};
pub use basic_upscaler::BasicUpscaler;
pub use cache::{Signature, SignatureMode, CacheManifest, CacheEntry, compute_signature};
pub use cleanup::{clean_state_dir, CleanupReport};
pub use chunk::{Chunk, ChunkState, ChunkStatus};
pub use discovery::{DiscoveredVideo, DiscoveryError, ZipInspection};
pub use encoder::{Encoder, EncoderKind, EncoderSettings, VvencPreset};
pub use group::{MergeGroup, GroupClip, ResolutionPolicy, UpscaleConfig};
pub use group_runner::{run_group, GroupEvent, GroupRun, GroupRunConfig};
pub use job::{Job, JobState, JobStatus};
pub use preflight::{PreflightCheck, PreflightReport};
pub use probe::{AudioStreamInfo, ProbeError, VideoInfo};
pub use sample::{CodecSample, ComparisonResult, SampleError};
pub use merge::{MergeClip, MergeError, MergeResult};
pub use prediction_stats::{PredictionError, PredictionStats, Verdict};
pub use scheduler::{Scheduler, SchedulerEvent};
