//! # Job — one input video → one output video
//!
//! A Job's lifecycle is roughly:
//! 1. Created (input path + output path + settings)
//! 2. Preflighted (validation results stored)
//! 3. Probed (source video info read via ffprobe)
//! 4. Planned (chunks created from probe info + scene strategy)
//! 5. Encoding (scheduler running chunks)
//! 6. Concatenating (combining chunk outputs into final output)
//! 7. Done OR Failed
//!
//! Persistent state lives at `<output>.av1tuner/job.json`. Chunk states 
//! live next to it as `chunk_NNNN.json`. The whole directory is the 
//! resume artifact — delete it to start over.

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

use crate::chunk::ChunkState;
use crate::audio::AudioSettings;
use crate::encoder::EncoderSettings;
use crate::probe::VideoInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Created,
    Probed,
    Encoding,
    Concatenating,
    Done,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobState {
    pub input: PathBuf,
    pub output: PathBuf,
    pub state_dir: PathBuf,
    pub settings: EncoderSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    pub status: JobStatus,
    pub info: Option<VideoInfo>,
    /// Frame boundaries `[0, b1, b2, ..., total]`. Length = chunk_count + 1.
    pub chunk_boundaries: Vec<u64>,
    /// Max attempts per chunk before marking Skipped. Default 2 (one retry).
    pub max_chunk_attempts: u32,
    /// Number of chunks to encode in parallel. Default = num_cpus / 2 
    /// to leave room for the OS + UI. Saved with the job so a restart 
    /// uses the same concurrency level.
    pub concurrency: u32,
    /// Optional human-readable label for the job.
    pub label: Option<String>,
    /// Timestamp (seconds since epoch) of last status update.
    pub updated_at: u64,
}

impl JobState {
    pub fn new(input: PathBuf, output: PathBuf, settings: EncoderSettings) -> Self {
        let state_dir = derive_state_dir(&output);
        let concurrency = std::thread::available_parallelism()
            .map(|n| (n.get() as u32 / 2).max(1))
            .unwrap_or(2);
        Self {
            input,
            output,
            state_dir,
            settings,
            audio: AudioSettings::default(),
            status: JobStatus::Created,
            info: None,
            chunk_boundaries: Vec::new(),
            max_chunk_attempts: 2,
            concurrency,
            label: None,
            updated_at: now_secs(),
        }
    }
    

    
    pub fn state_path(&self) -> PathBuf {
        self.state_dir.join("job.json")
    }
    
    pub fn save(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.state_dir)?;
        let path = self.state_path();
        let tmp = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
    
    pub fn load(state_dir: &Path) -> anyhow::Result<Self> {
        let path = state_dir.join("job.json");
        let bytes = std::fs::read(&path)?;
        let mut state: JobState = serde_json::from_slice(&bytes)?;
        // state_dir might have moved (user reorganized files); use what 
        // we were called with rather than what's in the JSON.
        state.state_dir = state_dir.to_path_buf();
        Ok(state)
    }
    
    /// Load all chunk states for this job. Missing chunks (i.e. 
    /// `chunk_NNNN.json` not on disk yet) are returned as fresh Pending 
    /// states. So the result is always exactly chunk_count long.
    pub fn load_chunks(&self) -> anyhow::Result<Vec<ChunkState>> {
        let ranges = crate::scene::ranges(&self.chunk_boundaries);
        let mut out = Vec::with_capacity(ranges.len());
        for (idx, (start, end)) in ranges.into_iter().enumerate() {
            let i = idx as u32;
            match ChunkState::load(&self.state_dir, i)? {
                Some(s) => out.push(s),
                None => out.push(ChunkState::new(i, start, end)),
            }
        }
        Ok(out)
    }
    
    pub fn chunk_count(&self) -> usize {
        self.chunk_boundaries.len().saturating_sub(1)
    }
    
    pub fn touch(&mut self) {
        self.updated_at = now_secs();
    }
}

/// `<output>.av1tuner/` next to the output file. So 
/// `/tmp/foo.mkv` → `/tmp/foo.mkv.av1tuner/`. Living next to the output 
/// makes the state dir easy to find + easy to clean up when the job is 
/// done (or to delete to force a restart).
pub fn derive_state_dir(output: &Path) -> PathBuf {
    let mut name = output.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    name.push(".av1tuner");
    output.parent().unwrap_or(Path::new(".")).join(name)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Wrapping convenience type combining a JobState with its on-disk chunks.
pub struct Job {
    pub state: JobState,
    pub chunks: Vec<ChunkState>,
}

impl Job {
    pub fn new(state: JobState) -> Self {
        Self { state, chunks: Vec::new() }
    }
    
    pub fn refresh_chunks(&mut self) -> anyhow::Result<()> {
        self.chunks = self.state.load_chunks()?;
        Ok(())
    }
    
    pub fn progress(&self) -> JobProgress {
        let total = self.chunks.len();
        let done = self.chunks.iter().filter(|c| {
            matches!(c.status, crate::chunk::ChunkStatus::Done)
        }).count();
        let failed = self.chunks.iter().filter(|c| {
            matches!(c.status, crate::chunk::ChunkStatus::Failed | crate::chunk::ChunkStatus::Skipped)
        }).count();
        let encoding = self.chunks.iter().filter(|c| {
            matches!(c.status, crate::chunk::ChunkStatus::Encoding)
        }).count();
        JobProgress { total, done, failed, encoding }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct JobProgress {
    pub total: usize,
    pub done: usize,
    pub failed: usize,
    pub encoding: usize,
}

impl JobProgress {
    pub fn pending(&self) -> usize {
        self.total.saturating_sub(self.done + self.failed + self.encoding)
    }
    pub fn complete(&self) -> bool {
        self.done + self.failed == self.total && self.total > 0
    }
    pub fn fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { self.done as f32 / self.total as f32 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_state_dir_adjacent() {
        let d = derive_state_dir(Path::new("/tmp/movie.mkv"));
        assert_eq!(d, PathBuf::from("/tmp/movie.mkv.av1tuner"));
    }

    #[test]
    fn derive_state_dir_no_parent() {
        let d = derive_state_dir(Path::new("video.mp4"));
        assert_eq!(d, PathBuf::from("./video.mp4.av1tuner"));
    }

    #[test]
    fn job_progress_arithmetic() {
        let p = JobProgress { total: 10, done: 3, failed: 1, encoding: 2 };
        assert_eq!(p.pending(), 4);
        assert!(!p.complete());
        assert!((p.fraction() - 0.3).abs() < 0.01);

        let p2 = JobProgress { total: 10, done: 9, failed: 1, encoding: 0 };
        assert!(p2.complete());
    }
}
