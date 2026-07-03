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

use std::path::{Path, PathBuf};
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
    
    if is_vvenc {
        // VVenC chunks are raw H.266 Annex B. Two real problems with
        // muxing them to MKV/MP4 via ffmpeg:
        //   1. ffmpeg's raw H.266 demuxer doesn't synthesize per-packet
        //      PTS, so direct copy to Matroska fails with "Can't write
        //      packet with unknown timestamp."
        //   2. Even if you route through MP4 to get timestamps, ffmpeg's
        //      VVC mp4 muxer (very new in 7.x) doesn't build a correct
        //      vvcC extradata box — the resulting file fails to decode
        //      ("vps_layer_id[1] should > vps_layer_id[0]", "Failed to
        //      parse extradata").
        //
        // The Fraunhofer-recommended path is MP4Box (from GPAC) which
        // correctly parses VVC parameter sets and writes a valid
        // VVCDecoderConfigurationRecord. mkvmerge from mkvtoolnix also
        // does this correctly. We try MP4Box first (the VVenC project's
        // recommendation, GPAC co-developed the spec impl), then
        // mkvmerge, then a clear install hint.
        return concat_vvenc_via_external_muxer(
            state_dir, &done_chunks, source, output, audio,
            encoder_kind, skipped,
        );
    }
    
    // AV1 path: ffmpeg concat demuxer over .ivf list. .ivf carries 
    // timestamps, so this path doesn't need mkvmerge.
    let ffmpeg = which::which("ffmpeg").map_err(|_| ConcatError::FfmpegNotFound)?;
    let mut cmd = Command::new(&ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
    
    let list_path = state_dir.join("concat_list.txt");
    {
        let mut f = std::fs::File::create(&list_path)?;
        for c in &done_chunks {
            let p = ChunkState::output_path_with_ext(
                state_dir, c.index, encoder_kind.chunk_extension());
            let abs = p.canonicalize()?;
            let s = abs.to_string_lossy().replace('\'', r"'\''");
            writeln!(f, "file '{}'", s)?;
        }
    }
    cmd.args(["-f", "concat", "-safe", "0", "-i"]);
    cmd.arg(&list_path);
    
    if audio.needs_source_at_concat() {
        cmd.args(["-i"]);
        cmd.arg(source);
    }
    
    let audio_args = if audio.needs_source_at_concat() {
        audio.build_ffmpeg_args()
    } else {
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

/// VVenC concat path: byte-concatenate the .266 chunks into one Annex B
/// stream, then mux to MKV via mkvmerge (which can synthesize timestamps
/// from --default-duration; ffmpeg cannot for raw H.266).
///
/// VVenC external-muxer dispatcher.
///
/// ffmpeg cannot mux raw VVC correctly in 7.x — the MKV path lacks
/// timestamps, the MP4 path produces a broken vvcC extradata box.
/// So we shell out to a tool that handles VVC correctly:
///   1. MP4Box (from GPAC) — Fraunhofer's recommended path; correct
///      vvcC construction. Available in most distros' `gpac` package.
///   2. mkvmerge (from mkvtoolnix) — also correct; in `mkvtoolnix-cli`.
///   3. If neither is present, return a clear error with install hints
///      for both options.
///
/// Audio handling identical across muxers:
///   - Passthrough: pull audio from source
///   - EncodeOpus: ffmpeg-encode to temp .opus, then mux
///   - Drop: video-only mux
fn concat_vvenc_via_external_muxer(
    state_dir: &Path,
    done_chunks: &[&ChunkState],
    source: &Path,
    output: &Path,
    audio: &AudioSettings,
    encoder_kind: crate::encoder::EncoderKind,
    skipped: usize,
) -> Result<ConcatResult, ConcatError> {
    // Step 1: byte-concat the .266 chunks. Annex B uses start-code-
    // delimited NAL units; concatenation is valid because each chunk
    // begins with an IDR (forced via -ip in the encoder).
    let combined_path = state_dir.join("combined.266");
    {
        let mut combined = std::fs::File::create(&combined_path)?;
        for c in done_chunks {
            let p = ChunkState::output_path_with_ext(
                state_dir, c.index, encoder_kind.chunk_extension());
            let mut chunk_file = std::fs::File::open(&p)?;
            std::io::copy(&mut chunk_file, &mut combined)?;
        }
        combined.flush()?;
    }

    // Step 2: optional audio prep (EncodeOpus needs a temp .opus file).
    let opus_audio_path: Option<PathBuf> = match &audio.mode {
        crate::audio::AudioMode::EncodeOpus(opts) => {
            // ffmpeg is fine for audio-only transcode — the bug is only
            // in VVC video muxing.
            let ffmpeg = which::which("ffmpeg").map_err(|_|
                ConcatError::Failed("ffmpeg not on PATH (needed for opus audio transcode)".to_string()))?;
            let opus_path = state_dir.join("audio.opus");
            let mut acmd = Command::new(&ffmpeg);
            acmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"]);
            acmd.arg(source);
            acmd.args([
                "-map", "0:a:0", "-vn", "-sn",
                "-c:a", "libopus",
                "-b:a", &format!("{}k", opts.bitrate_kbps),
            ]);
            acmd.arg(&opus_path);
            let r = acmd.output()?;
            // Non-fatal: if no audio in source, the transcode fails and
            // we proceed without audio.
            if r.status.success() { Some(opus_path) } else { None }
        }
        _ => None,
    };

    // Step 3: pick a muxer. MP4Box first (Fraunhofer-recommended), then
    // mkvmerge, then error.
    if let Ok(mp4box) = which::which("MP4Box") {
        return mux_with_mp4box(
            &mp4box, &combined_path, source, output, audio,
            opus_audio_path.as_deref(), done_chunks.len(), skipped,
        );
    }
    if let Ok(mkvmerge) = which::which("mkvmerge") {
        let fps = crate::probe::probe_fast(source).ok()
            .map(|i| i.fps).filter(|f| *f > 0.0).unwrap_or(30.0);
        return mux_with_mkvmerge(
            &mkvmerge, &combined_path, source, output, audio,
            opus_audio_path.as_deref(), fps, done_chunks.len(), skipped,
        );
    }

    Err(ConcatError::Failed(
        "VVenC output requires a VVC-aware muxer; neither MP4Box nor mkvmerge          is on PATH. Install one:\n\
         \n\
           Fedora/Asahi: sudo dnf install gpac          (preferred)\n\
                    or:  sudo dnf install mkvtoolnix-cli\n\
           Debian/Ubuntu: sudo apt install gpac\n\
                    or:  sudo apt install mkvtoolnix\n\
           macOS:        brew install gpac\n\
                    or:  brew install mkvtoolnix\n\
         \n\
         ffmpeg cannot mux raw VVC correctly in its current versions \
         (timestamps for MKV, vvcC extradata for MP4 are both buggy)."
            .to_string()
    ))
}

/// Mux via MP4Box (GPAC). MP4Box handles VVC parameter-set parsing and
/// vvcC construction correctly. Output is .mkv per `output` extension;
/// MP4Box auto-detects from extension.
fn mux_with_mp4box(
    mp4box: &Path,
    combined_h266: &Path,
    source: &Path,
    output: &Path,
    audio: &AudioSettings,
    opus_audio: Option<&Path>,
    chunks_count: usize,
    skipped: usize,
) -> Result<ConcatResult, ConcatError> {
    // MP4Box command shape:
    //   MP4Box -add raw.h266 [-add audio_input] -new output.mkv
    // -new replaces any existing output (we already removed it via -y
    // semantics by not opening it here, but MP4Box will overwrite -new).
    let mut cmd = Command::new(mp4box);
    cmd.arg("-add").arg(combined_h266);

    match &audio.mode {
        crate::audio::AudioMode::Drop => {
            // Video-only.
        }
        crate::audio::AudioMode::Passthrough => {
            // Add the source as a second -add; MP4Box will pull its
            // audio. (It also picks up the video, but a single video
            // track per output means the source video is treated as a
            // second video track and warned about — to avoid that, we
            // could -add source#audio. Use that explicit syntax.)
            let mut arg = std::ffi::OsString::from(source);
            arg.push("#audio");
            cmd.arg("-add").arg(arg);
        }
        crate::audio::AudioMode::EncodeOpus(_) => {
            if let Some(opus) = opus_audio {
                cmd.arg("-add").arg(opus);
            } else {
                // Transcode failed — fall through to passthrough.
                let mut arg = std::ffi::OsString::from(source);
                arg.push("#audio");
                cmd.arg("-add").arg(arg);
            }
        }
    }

    // -new <output> creates a fresh file (overwrites existing).
    // Remove first to be safe (MP4Box prints a confirm prompt otherwise
    // in some versions).
    let _ = std::fs::remove_file(output);
    cmd.arg("-new").arg(output);

    let r = cmd.output()?;
    if !r.status.success() {
        let stderr = String::from_utf8_lossy(&r.stderr).to_string();
        let stdout = String::from_utf8_lossy(&r.stdout).to_string();
        let msg = if !stderr.trim().is_empty() { stderr } else { stdout };
        return Err(ConcatError::Failed(format!("MP4Box failed: {}", msg.trim())));
    }
    let meta = std::fs::metadata(output)?;
    Ok(ConcatResult {
        output_bytes: meta.len(),
        chunks_concatenated: chunks_count,
        chunks_skipped: skipped,
    })
}

/// Mux via mkvmerge (mkvtoolnix). Also handles VVC correctly. Used as
/// the second-choice muxer when MP4Box isn't available.
fn mux_with_mkvmerge(
    mkvmerge: &Path,
    combined_h266: &Path,
    source: &Path,
    output: &Path,
    audio: &AudioSettings,
    opus_audio: Option<&Path>,
    fps: f64,
    chunks_count: usize,
    skipped: usize,
) -> Result<ConcatResult, ConcatError> {
    let default_duration = fps_to_mkvmerge_duration(fps);

    let mut cmd = Command::new(mkvmerge);
    cmd.arg("-o").arg(output);
    cmd.arg("--default-duration").arg(format!("0:{}", default_duration));
    cmd.arg(combined_h266);

    match &audio.mode {
        crate::audio::AudioMode::Drop => {}
        crate::audio::AudioMode::Passthrough => {
            cmd.arg("--no-video").arg(source);
        }
        crate::audio::AudioMode::EncodeOpus(_) => {
            if let Some(opus) = opus_audio {
                cmd.arg(opus);
            } else {
                cmd.arg("--no-video").arg(source);
            }
        }
    }

    let r = cmd.output()?;
    if !r.status.success() {
        let stderr = String::from_utf8_lossy(&r.stderr).to_string();
        let stdout = String::from_utf8_lossy(&r.stdout).to_string();
        let msg = if !stderr.trim().is_empty() { stderr } else { stdout };
        return Err(ConcatError::Failed(format!("mkvmerge failed: {}", msg.trim())));
    }
    let meta = std::fs::metadata(output)?;
    Ok(ConcatResult {
        output_bytes: meta.len(),
        chunks_concatenated: chunks_count,
        chunks_skipped: skipped,
    })
}

/// Convert an fps value (float) into mkvmerge's --default-duration 
/// format. Returns something like "25p", "30000/1001p", "60p".
/// Recognizes common NTSC rationals (23.976, 29.97, 59.94) and emits 
/// the exact fraction; falls back to a 4-decimal float for unusual rates.
fn fps_to_mkvmerge_duration(fps: f64) -> String {
    // NTSC rates within 0.01 → emit exact rational.
    let common: &[(f64, &str)] = &[
        (24.0,    "24p"),
        (23.976,  "24000/1001p"),
        (25.0,    "25p"),
        (29.97,   "30000/1001p"),
        (30.0,    "30p"),
        (50.0,    "50p"),
        (59.94,   "60000/1001p"),
        (60.0,    "60p"),
        (120.0,   "120p"),
    ];
    for (target, repr) in common {
        if (fps - target).abs() < 0.01 {
            return (*repr).to_string();
        }
    }
    // Fallback: integer if very close to one, else a float with 'p'.
    if (fps - fps.round()).abs() < 0.001 {
        format!("{}p", fps.round() as u32)
    } else {
        format!("{:.4}p", fps)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn fps_to_mkvmerge_common_rates() {
        assert_eq!(fps_to_mkvmerge_duration(25.0), "25p");
        assert_eq!(fps_to_mkvmerge_duration(30.0), "30p");
        assert_eq!(fps_to_mkvmerge_duration(60.0), "60p");
        // NTSC rationals.
        assert_eq!(fps_to_mkvmerge_duration(29.97), "30000/1001p");
        assert_eq!(fps_to_mkvmerge_duration(23.976), "24000/1001p");
        assert_eq!(fps_to_mkvmerge_duration(59.94), "60000/1001p");
        // Float fallback.
        assert!(fps_to_mkvmerge_duration(48.5).ends_with('p'));
    }
}
