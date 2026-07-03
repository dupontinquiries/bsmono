//! # Chunk — one piece of a video that encodes independently
//!
//! A chunk is the atomic unit of work in the scheduler. Each chunk:
//! - covers a frame range `[start_frame, end_frame)` of the source video
//! - encodes in its own subprocess (failure → retry, never takes down the job)
//! - serializes its state to disk so a crash + restart resumes correctly
//!
//! The per-chunk state file is the ground truth. The scheduler reads these 
//! at startup; the chunk is "done" iff its state says `Done` and its output 
//! file exists on disk.

use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};

/// Lifecycle of a single chunk.
///
/// Transitions:
/// ```text
/// Pending ──► Encoding ──► Done
///                 │           ▲
///                 ▼           │
///              Failed ── retry (if attempts < max_attempts)
///                 │
///                 ▼ (after retry exhaustion)
///              Skipped
/// ```
///
/// `Skipped` is terminal but doesn't take the job down — the scheduler 
/// logs a loud warning and the final concat omits skipped chunks. This is 
/// the right call for "5 minutes of corrupt source frames buried in a 
/// 4-hour encode": ship the rest, flag the gap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChunkStatus {
    Pending,
    Encoding,
    Done,
    Failed,
    Skipped,
}

/// Persistent state for one chunk. Lives at 
/// `<job_state_dir>/chunk_<index>.json`. The encoder output (the .ivf 
/// bytes) lives next to it as `chunk_<index>.ivf` once status is Done.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChunkState {
    pub index: u32,
    pub start_frame: u64,
    /// Exclusive — the chunk covers `[start_frame, end_frame)`.
    pub end_frame: u64,
    pub status: ChunkStatus,
    /// How many times we've tried to encode this chunk.
    pub attempts: u32,
    /// Most recent error message if status is Failed or Skipped.
    pub last_error: Option<String>,
    /// Wall-clock encode duration in milliseconds, recorded on Done.
    /// Useful for ETA estimation across remaining chunks.
    pub encode_ms: Option<u64>,
    /// Output file size in bytes, recorded on Done.
    pub output_bytes: Option<u64>,
}

impl ChunkState {
    pub fn new(index: u32, start_frame: u64, end_frame: u64) -> Self {
        Self {
            index,
            start_frame,
            end_frame,
            status: ChunkStatus::Pending,
            attempts: 0,
            last_error: None,
            encode_ms: None,
            output_bytes: None,
        }
    }

    /// Number of frames in this chunk.
    pub fn frame_count(&self) -> u64 {
        self.end_frame - self.start_frame
    }

    /// Path to the state JSON file inside a job's state dir.
    pub fn state_path(job_dir: &Path, index: u32) -> PathBuf {
        job_dir.join(format!("chunk_{:04}.json", index))
    }

    /// Path to the encoder output file inside a job's state dir.
    /// `ext` is the file extension without leading dot (e.g. "ivf", "266").
    pub fn output_path_with_ext(job_dir: &Path, index: u32, ext: &str) -> PathBuf {
        job_dir.join(format!("chunk_{:04}.{}", index, ext))
    }
    
    /// Legacy: defaults to .ivf. Kept for callers that haven't been 
    /// migrated. New code should use `output_path_with_ext`.
    pub fn output_path(job_dir: &Path, index: u32) -> PathBuf {
        Self::output_path_with_ext(job_dir, index, "ivf")
    }

    /// Load chunk state from disk if it exists. Returns None on missing 
    /// file (caller treats as Pending); returns Err only on parse failure.
    pub fn load(job_dir: &Path, index: u32) -> anyhow::Result<Option<Self>> {
        let path = Self::state_path(job_dir, index);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(&path)?;
        let state: ChunkState = serde_json::from_slice(&bytes)?;
        Ok(Some(state))
    }

    /// Persist to disk. Writes via a tmp+rename so a crash mid-write 
    /// doesn't leave a corrupt JSON in place.
    pub fn save(&self, job_dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(job_dir)?;
        let path = Self::state_path(job_dir, self.index);
        let tmp_path = path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp_path, &bytes)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// True if the chunk needs encoding work (Pending or Failed-with-retries-left).
    pub fn needs_work(&self, max_attempts: u32) -> bool {
        match self.status {
            ChunkStatus::Pending => true,
            ChunkStatus::Encoding => true,  // crashed mid-encode; retry
            ChunkStatus::Failed => self.attempts < max_attempts,
            ChunkStatus::Done | ChunkStatus::Skipped => false,
        }
    }

    /// True if the chunk contributes bytes to the final concat.
    pub fn contributes_to_output(&self) -> bool {
        matches!(self.status, ChunkStatus::Done)
    }
}

/// Convenience: a chunk plus its job-dir context for handing off to workers.
#[derive(Clone, Debug)]
pub struct Chunk {
    pub state: ChunkState,
    pub job_dir: PathBuf,
}

impl Chunk {
    pub fn new(state: ChunkState, job_dir: PathBuf) -> Self {
        Self { state, job_dir }
    }

    pub fn output_path(&self) -> PathBuf {
        ChunkState::output_path(&self.job_dir, self.state.index)
    }

    pub fn save_state(&self) -> anyhow::Result<()> {
        self.state.save(&self.job_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn save_load_roundtrip() {
        let dir = tempdir_or_skip();
        let mut s = ChunkState::new(7, 100, 250);
        s.attempts = 2;
        s.last_error = Some("encoder segfaulted".to_string());
        s.save(&dir).unwrap();

        let loaded = ChunkState::load(&dir, 7).unwrap().unwrap();
        assert_eq!(loaded.index, 7);
        assert_eq!(loaded.start_frame, 100);
        assert_eq!(loaded.end_frame, 250);
        assert_eq!(loaded.attempts, 2);
        assert_eq!(loaded.last_error.as_deref(), Some("encoder segfaulted"));
        assert_eq!(loaded.frame_count(), 150);
    }

    #[test]
    fn missing_state_returns_none() {
        let dir = tempdir_or_skip();
        let loaded = ChunkState::load(&dir, 99).unwrap();
        assert!(loaded.is_none());
    }

    #[test]
    fn needs_work_states() {
        let max = 3;
        let mut s = ChunkState::new(0, 0, 100);

        // Pending → needs work
        assert!(s.needs_work(max));

        // Encoding (likely crashed) → retry
        s.status = ChunkStatus::Encoding;
        assert!(s.needs_work(max));

        // Failed under retry limit → retry
        s.status = ChunkStatus::Failed;
        s.attempts = 1;
        assert!(s.needs_work(max));

        // Failed at retry limit → no more work
        s.attempts = 3;
        assert!(!s.needs_work(max));

        // Done → no work
        s.status = ChunkStatus::Done;
        assert!(!s.needs_work(max));

        // Skipped → no work
        s.status = ChunkStatus::Skipped;
        assert!(!s.needs_work(max));
    }

    fn tempdir_or_skip() -> PathBuf {
        let base = std::env::temp_dir().join(format!("av1tuner_test_{}", 
            std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }
}
