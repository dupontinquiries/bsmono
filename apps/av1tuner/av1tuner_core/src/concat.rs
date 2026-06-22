//! # Concat — assemble final output from chunk .ivf files + audio
//!
//! Two-step:
//! 1. Build an ffmpeg concat list file pointing at each Done chunk's 
//!    .ivf in order.
//! 2. Invoke ffmpeg with the concat list as the video input, the original 
//!    source as an audio/subtitle input (if needed), and the audio 
//!    settings-derived stream selection + codec args.
//!
//! Skipped chunks are omitted. The output is a valid AV1 stream with a 
//! gap where the skip was. We emit a warning so users know it happened.

use std::path::Path;
use std::process::Command;
use std::io::Write;
use thiserror::Error;

use crate::audio::AudioSettings;
use crate::chunk::{ChunkState, ChunkStatus};

#[derive(Debug, Error)]
pub enum ConcatError {
    #[error("ffmpeg not found")]
    FfmpegNotFound,
    #[error("ffmpeg failed: {0}")]
    Failed(String),
    #[error("no completed chunks to concat")]
    NoChunks,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub struct ConcatResult {
    pub output_bytes: u64,
    pub chunks_concatenated: usize,
    pub chunks_skipped: usize,
}

/// Build the final output by concatenating chunks + remuxing audio.
///
/// - `state_dir`: where the chunk .ivf files live + where the concat 
///   list will be written.
/// - `chunks`: all chunks for this job; only Done ones contribute to 
///   the output (others are skipped with a logged warning).
/// - `source`: original source path. Needed for passthrough and 
///   re-encode modes (we read audio + subtitles from here). Pass any 
///   path when audio mode is Drop — it won't be touched.
/// - `output`: final output file (typically .mkv).
/// - `audio`: how to handle audio + subtitles in the final mux.
pub fn concat(
    state_dir: &Path,
    chunks: &[ChunkState],
    source: &Path,
    output: &Path,
    audio: &AudioSettings,
    encoder_kind: crate::encoder::EncoderKind,
) -> Result<ConcatResult, ConcatError> {
    let ffmpeg = which::which("ffmpeg").map_err(|_| ConcatError::FfmpegNotFound)?;
    
    let done_chunks: Vec<&ChunkState> = chunks.iter()
        .filter(|c| c.status == ChunkStatus::Done)
        .collect();
    
    if done_chunks.is_empty() {
        return Err(ConcatError::NoChunks);
    }
    
    let skipped = chunks.iter().filter(|c| {
        matches!(c.status, ChunkStatus::Failed | ChunkStatus::Skipped)
    }).count();
    
    // Codec-specific concat strategy.
    //
    // SVT-AV1 → .ivf chunks. IVF carries timestamps, so ffmpeg's concat 
    //   DEMUXER reads them with correct timing. Standard path.
    //
    // VVenC → raw H.266 Annex B (.266) chunks. These have NO container 
    //   timestamps; the concat demuxer + Matroska fails with "Can't 
    //   write packet with unknown timestamp". BUT Annex B streams use 
    //   start-code-delimited NAL units, so they can be joined by plain 
    //   BYTE concatenation into one valid Annex B stream. Each chunk 
    //   begins with an IDR (we force a leading keyframe), so decoding 
    //   continues cleanly across joins. We then feed the combined raw 
    //   stream to ffmpeg with -r to assign a frame rate (and thus 
    //   timestamps) before muxing to Matroska.
    let is_vvenc = matches!(encoder_kind, crate::encoder::EncoderKind::Vvenc);
    
    let mut cmd = Command::new(&ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    
    // This holds the combined raw stream path for VVenC so it stays 
    // alive until the command runs.
    let combined_path = state_dir.join("combined.266");
    let list_path = state_dir.join("concat_list.txt");
    
    if is_vvenc {
        // Byte-concatenate all done chunks into one Annex B stream.
        let mut combined = std::fs::File::create(&combined_path)?;
        for c in &done_chunks {
            let p = ChunkState::output_path_with_ext(
                state_dir, c.index, encoder_kind.chunk_extension());
            let mut chunk_file = std::fs::File::open(&p)?;
            std::io::copy(&mut chunk_file, &mut combined)?;
        }
        combined.flush()?;
        drop(combined);
        
        // Frame rate for timestamp assignment. Fast probe of the source; 
        // fall back to 30 fps (only affects playback speed, not validity).
        let fps = crate::probe::probe_fast(source)
            .ok()
            .map(|info| info.fps)
            .filter(|f| *f > 0.0)
            .unwrap_or(30.0);
        
        // Read the combined raw H.266 with an explicit frame rate so 
        // ffmpeg builds timestamps. -fflags +genpts as a belt-and-braces.
        cmd.args(["-fflags", "+genpts"]);
        cmd.arg("-r").arg(format!("{}", fps));
        cmd.arg("-i").arg(&combined_path);
    } else {
        // AV1 path: concat demuxer over the .ivf list.
        let mut f = std::fs::File::create(&list_path)?;
        for c in &done_chunks {
            let p = ChunkState::output_path_with_ext(
                state_dir, c.index, encoder_kind.chunk_extension());
            let abs = p.canonicalize()?;
            let s = abs.to_string_lossy().replace('\'', r"'\''");
            writeln!(f, "file '{}'", s)?;
        }
        drop(f);
        cmd.args(["-f", "concat", "-safe", "0", "-i"]);
        cmd.arg(&list_path);
    }
    
    if audio.needs_source_at_concat() {
        cmd.args(["-i"]);
        cmd.arg(source);
    } else {
        // Drop mode: don't add source. The map/codec args from 
        // build_ffmpeg_args reference only input 0.
    }
    
    // Audio mapping + codec args. Note: in Drop mode the audio module's
    // build_ffmpeg_args still references `-map 1:s?` for subtitles, 
    // which would error because there's no input 1. Strip that branch
    // here by special-casing.
    let audio_args = if audio.needs_source_at_concat() {
        audio.build_ffmpeg_args()
    } else {
        // Drop without a source for subtitles. Video-only mux.
        vec![
            "-map".into(), "0:v".into(),
            "-c:v".into(), "copy".into(),
            "-an".into(),
        ]
    };
    for a in &audio_args {
        cmd.arg(a);
    }
    
    cmd.arg(output);
    
    let result = cmd.output()?;
    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();
        return Err(ConcatError::Failed(stderr));
    }
    
    let meta = std::fs::metadata(output)?;
    Ok(ConcatResult {
        output_bytes: meta.len(),
        chunks_concatenated: done_chunks.len(),
        chunks_skipped: skipped,
    })
}
