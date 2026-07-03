//! # Sample — encode a short slice of source with multiple codecs to 
//! compare file size at the user's current settings.
//!
//! This is the comparison primitive that the M2 side-by-side view will 
//! eventually use across full encodes. For now it's a quick "should I 
//! bother re-encoding this in VVC?" answer.
//!
//! The function is sync — call it from a background thread. UI polls 
//! a Receiver to get the result without blocking.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use thiserror::Error;

use crate::encoder::{
    make_encoder, EncoderError, EncoderKind, EncoderSettings,
};
use crate::probe::VideoInfo;

#[derive(Debug, Error)]
pub enum SampleError {
    #[error("encoder error: {0}")]
    Encoder(#[from] EncoderError),
    #[error("probe info had zero frames")]
    EmptyVideo,
    #[error("cancelled")]
    Cancelled,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Per-codec result for one sample slice.
#[derive(Clone, Debug)]
pub struct CodecSample {
    pub kind: EncoderKind,
    /// Encoded output size in bytes for the sample slice.
    pub bytes: u64,
    /// Wall-clock time to encode the slice.
    pub elapsed_ms: u64,
    /// Path to the encoded sample file (kept for follow-up inspection 
    /// — e.g. visual diff, decoding for quality metric). Caller is 
    /// responsible for deleting when done.
    pub output_path: PathBuf,
}

/// Outcome of a full comparison run.
#[derive(Clone, Debug)]
pub struct ComparisonResult {
    /// Number of frames encoded in the sample slice.
    pub sample_frames: u64,
    /// Source duration in seconds covered by the sample.
    pub sample_seconds: f64,
    /// Per-codec results, in the order they were requested.
    pub samples: Vec<CodecSample>,
}

impl ComparisonResult {
    /// Project full-source output bytes from sample bytes.
    ///
    /// Naive linear scaling: full ≈ sample * (total_frames / sample_frames).
    /// Real encodes won't track this exactly because:
    /// - Rate control averages over the full duration, not the sample
    /// - High-motion scenes spend more bits than the sample average suggests
    /// - Encoder warmup costs are paid once, not per chunk
    ///
    /// So the projection is a useful first approximation, not a 
    /// promise. Caller should label it as estimated.
    pub fn project_total_bytes(&self, sample: &CodecSample, total_frames: u64) -> u64 {
        if self.sample_frames == 0 {
            return 0;
        }
        let ratio = total_frames as f64 / self.sample_frames as f64;
        (sample.bytes as f64 * ratio) as u64
    }
}

/// Choose a sample slice within the source. Defaults to the middle 
/// `sample_seconds` of the video — intros/outros are often less 
/// representative than the middle.
pub fn pick_sample_range(info: &VideoInfo, sample_seconds: f64) -> (u64, u64) {
    if info.frame_count == 0 || info.fps <= 0.0 {
        return (0, 0);
    }
    let sample_frames = ((sample_seconds * info.fps).round() as u64)
        .min(info.frame_count)
        .max(1);
    let mid = info.frame_count / 2;
    let half = sample_frames / 2;
    let start = mid.saturating_sub(half);
    let end = (start + sample_frames).min(info.frame_count);
    (start, end)
}

/// Run a sample encode with every requested codec setting and return 
/// the comparison. Each setting is encoded sequentially (not in 
/// parallel) so resource usage is bounded — we're trying to give the 
/// user a fast preview, not maximize throughput.
///
/// `cancel` lets the UI abort mid-comparison.
/// `temp_dir` is where sample outputs land. Caller should clean up.
pub fn run_comparison(
    source: &Path,
    info: &VideoInfo,
    settings_list: &[EncoderSettings],
    sample_seconds: f64,
    temp_dir: &Path,
    cancel: Arc<AtomicBool>,
) -> Result<ComparisonResult, SampleError> {
    if info.frame_count == 0 {
        return Err(SampleError::EmptyVideo);
    }
    
    std::fs::create_dir_all(temp_dir)?;
    
    let (start_frame, end_frame) = pick_sample_range(info, sample_seconds);
    let sample_frames = end_frame.saturating_sub(start_frame);
    let sample_seconds_actual = sample_frames as f64 / info.fps.max(0.001);
    
    let mut samples = Vec::with_capacity(settings_list.len());
    for (idx, settings) in settings_list.iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SampleError::Cancelled);
        }
        
        let encoder = make_encoder(settings)?;
        let ext = settings.kind.chunk_extension();
        let output_path = temp_dir.join(
            format!("sample_{}_{:?}.{}", idx, settings.kind, ext)
        );
        
        let cancel_cb = {
            let cancel = cancel.clone();
            move || cancel.load(std::sync::atomic::Ordering::SeqCst)
        };
        
        let result = encoder.encode_chunk(
            source, start_frame, end_frame, &output_path, settings, &cancel_cb,
        )?;
        
        samples.push(CodecSample {
            kind: settings.kind,
            bytes: result.output_bytes,
            elapsed_ms: result.elapsed_ms,
            output_path,
        });
    }
    
    Ok(ComparisonResult {
        sample_frames,
        sample_seconds: sample_seconds_actual,
        samples,
    })
}

/// Best-effort cleanup of sample output files. Called when the user 
/// dismisses the comparison panel or starts a new comparison.
pub fn cleanup_samples(result: &ComparisonResult) {
    for s in &result.samples {
        let _ = std::fs::remove_file(&s.output_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn fake_info(frames: u64, fps: f64) -> VideoInfo {
        VideoInfo {
            width: 1920, height: 1080,
            frame_count: frames, fps,
            duration_s: frames as f64 / fps,
            pixel_format: "yuv420p".into(),
            codec: "h264".into(),
            audio_streams: Vec::new(),
        }
    }
    
    #[test]
    fn sample_range_is_centered() {
        let info = fake_info(3600, 30.0);  // 120 seconds
        let (start, end) = pick_sample_range(&info, 10.0);
        // 10 seconds at 30fps = 300 frames
        assert_eq!(end - start, 300);
        // Centered around 1800
        assert!((start as i64 - 1650).abs() < 5);
    }
    
    #[test]
    fn sample_capped_at_video_length() {
        let info = fake_info(60, 30.0);  // 2-second video
        let (start, end) = pick_sample_range(&info, 10.0);  // ask for 10s
        // Should be capped at the full video.
        assert_eq!(start, 0);
        assert_eq!(end, 60);
    }
    
    #[test]
    fn projection_scales_linearly() {
        let result = ComparisonResult {
            sample_frames: 300,
            sample_seconds: 10.0,
            samples: vec![],
        };
        let sample = CodecSample {
            kind: EncoderKind::SvtAv1,
            bytes: 1_000_000,  // 1 MB for 300 frames
            elapsed_ms: 5_000,
            output_path: PathBuf::new(),
        };
        // For 30,000 total frames, projection = 1 MB * (30000/300) = 100 MB
        assert_eq!(result.project_total_bytes(&sample, 30_000), 100_000_000);
    }
}
