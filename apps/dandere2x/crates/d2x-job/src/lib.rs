//! dandere2x as a job: settings in, events out, a file on disk.
//!
//! `Job` is the whole configuration; `run` / `run_segmented` execute it and
//! report `Event`s (pipeline events plus `Stage` messages for the slow parts
//! before the first frame: probe, keyframe scan, model load, GPU init, the
//! full-frame upscale of frame 0). Long inputs are not silent any more.

pub mod ffmpeg;
pub mod checkpoint;
pub mod job;

pub use d2x_core::pipeline::{Event as PipelineEvent, Summary};
pub use d2x_core::plan::FrameStats;
pub use ffmpeg::{EncodeOptions, Probe, Segment};
pub use job::{Backend, Event, Job};
