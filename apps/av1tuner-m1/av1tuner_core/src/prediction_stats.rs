//! # Prediction analysis — Phase 0 measurement
//!
//! Measures how much structure AV1's existing inter-frame prediction 
//! captures on a sample of source content. Surfaces a verdict (Green / 
//! Yellow / Red) about whether building a learned predictor on top of 
//! AV1 is likely to pay off.
//!
//! ## What this is NOT
//!
//! This is a coarse proxy. It does NOT actually train a neural 
//! predictor or measure learned-residual size. It uses AV1's own 
//! per-frame statistics to infer "how much inter-prediction headroom 
//! is left."
//!
//! The actual Phase 1 experiment — train a small neural predictor, 
//! compare residuals to AV1's — is much more work and lives outside 
//! av1tuner. This Phase 0 sidecar exists to answer "should you bother 
//! with Phase 1 on this content?"
//!
//! ## How it works
//!
//! 1. Encode a short sample of source with `--enable-stat-report 1`
//! 2. Parse SVT-AV1's per-frame stat output from stderr
//! 3. Classify frames as I-type (keyframes) vs P-type (inter-predicted)
//!    using either explicit slice-type tags or position-based heuristic
//! 4. Compute P/I size ratio + variance
//! 5. Apply thresholds → verdict

use std::path::Path;
use std::process::{Command, Stdio};
use thiserror::Error;

use crate::encoder::{EncoderKind, EncoderSettings};
use crate::probe::VideoInfo;

#[derive(Debug, Error)]
pub enum PredictionError {
    #[error("only SVT-AV1 backend supports prediction analysis (got {0:?})")]
    UnsupportedEncoder(EncoderKind),
    #[error("encode failed: {0}")]
    EncodeFailed(String),
    #[error("no frames detected in stat output")]
    NoStats,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled")]
    Cancelled,
}

/// Raw per-frame entry pulled from SVT-AV1's stat output.
#[derive(Clone, Debug)]
pub struct FrameStat {
    pub poc: u32,        // picture order count = display order index
    pub bytes: u64,      // size of the encoded frame
    pub is_keyframe: bool, // I-slice / keyframe
}

/// Aggregated stats for the verdict.
#[derive(Clone, Debug)]
pub struct PredictionStats {
    pub total_frames: u32,
    pub keyframe_count: u32,
    pub inter_count: u32,
    /// Average bytes per keyframe (I-slice).
    pub avg_keyframe_bytes: f64,
    /// Average bytes per inter-predicted frame (P/B-slice).
    pub avg_inter_bytes: f64,
    /// avg_inter_bytes / avg_keyframe_bytes — the headline number.
    /// Lower = AV1's inter-prediction is finding lots of structure.
    /// Higher = AV1's prediction isn't helping much; learned predictor 
    /// has more theoretical headroom.
    pub p_to_i_ratio: f64,
    /// Coefficient of variation of inter-frame sizes (stddev / mean).
    /// High variance = heterogeneous content; some frames predict 
    /// well, others don't.
    pub inter_size_cv: f64,
    /// Whether stat-report parsing succeeded; if false, the numbers 
    /// are estimated from output bytes alone and less reliable.
    pub stat_report_parsed: bool,
}

/// Verdict on whether learned predictive coding is likely to pay off 
/// for this content.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// AV1's prediction isn't capturing much structure on this content. 
    /// A learned predictor that exploits long-range or content-specific 
    /// patterns has real headroom. Worth pursuing Phase 1.
    Green,
    /// Moderate amount of inter-prediction headroom. Phase 1 might pay 
    /// off but isn't guaranteed. Worth a small experiment.
    Yellow,
    /// AV1's prediction is already extracting most of the structure. 
    /// A learned predictor's theoretical ceiling is small; the 
    /// engineering effort probably isn't worth it.
    Red,
}

impl Verdict {
    pub fn label(&self) -> &'static str {
        match self {
            Verdict::Green => "Green — worth pursuing",
            Verdict::Yellow => "Yellow — moderate headroom",
            Verdict::Red => "Red — AV1 already captures most structure",
        }
    }
    
    pub fn explanation(&self, stats: &PredictionStats) -> String {
        match self {
            Verdict::Green => format!(
                "Inter-predicted frames are {:.1}% the size of keyframes — \
                AV1's 7-frame inter-prediction window isn't capturing much \
                of this content's structure. A learned predictor with longer \
                context, or one trained on your content domain, has real room \
                to do better. Worth running Phase 1 (train a small predictor, \
                measure actual residual reduction) before committing to a \
                full architecture.",
                stats.p_to_i_ratio * 100.0
            ),
            Verdict::Yellow => format!(
                "Inter-predicted frames are {:.1}% the size of keyframes. \
                AV1 is finding some structure but there's moderate headroom. \
                A learned predictor might help, especially if your content \
                has long-range repetition AV1's 7-frame window misses. \
                Worth a small Phase 1 experiment but the payoff isn't certain.",
                stats.p_to_i_ratio * 100.0
            ),
            Verdict::Red => format!(
                "Inter-predicted frames are only {:.1}% the size of keyframes \
                — AV1's inter-prediction is already extracting most of the \
                structure in this content. A learned predictor would have to \
                be very good to beat what's already there. Engineering effort \
                probably better spent on a different lever (cross-file dedup, \
                content-specialized post-filter, etc).",
                stats.p_to_i_ratio * 100.0
            ),
        }
    }
}

/// Apply the heuristic thresholds. Cutoffs are calibrated against the 
/// general intuition discussed in research:
///
/// - P/I < 15% : AV1's inter-prediction is finding huge amounts of 
///   structure (typical of static screen recordings, surveillance camera 
///   footage with little change). A learned predictor's ceiling on top 
///   is small.
/// - 15-30% : moderate inter-frame structure remaining
/// - > 30% : significant inter-frame structure NOT being captured by 
///   AV1's 7-frame window
pub fn verdict_for(stats: &PredictionStats) -> Verdict {
    if stats.keyframe_count == 0 || stats.inter_count == 0 {
        // Insufficient data to judge. Default to Yellow rather than 
        // giving false confidence.
        return Verdict::Yellow;
    }
    let r = stats.p_to_i_ratio;
    if r < 0.15 { Verdict::Red }
    else if r < 0.30 { Verdict::Yellow }
    else { Verdict::Green }
}

/// Run the Phase 0 analysis on a short sample of the source.
///
/// `cancel` is checked between subprocess polls so the user can abort.
///
/// Currently only the SvtAv1 backend is supported — VVenC's stat output 
/// format is different and we'd need separate parsing. Phase 0 is about 
/// "does AV1 leave structure on the table"; VVC is a different question.
pub fn run_analysis(
    source: &Path,
    info: &VideoInfo,
    settings: &EncoderSettings,
    sample_seconds: f64,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<PredictionStats, PredictionError> {
    if settings.kind != EncoderKind::SvtAv1 {
        return Err(PredictionError::UnsupportedEncoder(settings.kind));
    }
    
    // Pick a sample range from the middle of the source — same heuristic 
    // as sample::pick_sample_range so the user's intuitions transfer.
    let (start_frame, end_frame) = crate::sample::pick_sample_range(info, sample_seconds);
    let frame_count = end_frame.saturating_sub(start_frame);
    if frame_count < 30 {
        return Err(PredictionError::NoStats);
    }
    
    // Trim with ffmpeg, pipe Y4M to SvtAv1EncApp with stat-report on. 
    // Output goes to a temp file we discard after parsing stats.
    let temp_dir = std::env::temp_dir().join(format!(
        "av1tuner_pred_{}_{}",
        std::process::id(),
        core_kit::time::now_secs(),
    ));
    std::fs::create_dir_all(&temp_dir)?;
    let output_path = temp_dir.join("sample.ivf");
    
    let ffmpeg_bin = which::which("ffmpeg").map_err(|_| {
        PredictionError::EncodeFailed("ffmpeg not on PATH".into())
    })?;
    let svt_bin = which::which("SvtAv1EncApp").map_err(|_| {
        PredictionError::EncodeFailed("SvtAv1EncApp not on PATH".into())
    })?;
    
    let trim_filter = format!(
        "trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS",
        start_frame, end_frame
    );
    
    let mut ffmpeg = Command::new(&ffmpeg_bin)
        .args([
            "-hide_banner", "-loglevel", "error", "-nostdin",
            "-i",
        ])
        .arg(source)
        .args([
            "-vf", &trim_filter,
            "-pix_fmt", &settings.pixel_format,
            "-f", "yuv4mpegpipe",
            "-strict", "-1",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    
    let ffmpeg_stdout = ffmpeg.stdout.take().expect("ffmpeg stdout piped");
    
    let mut svt = Command::new(&svt_bin)
        .args([
            "-i", "stdin",
            "-b", &output_path.to_string_lossy(),
            "--preset", &settings.preset.to_string(),
            "--crf", &settings.svt_crf.to_string(),
            // Critical: enables the per-frame stat output we parse below
            "--enable-stat-report", "1",
        ])
        .stdin(Stdio::from(ffmpeg_stdout))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    
    let svt_stderr = svt.stderr.take().expect("svt stderr piped");
    let stderr_handle = std::thread::Builder::new()
        .name("pred-stderr".into())
        .spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let _ = std::io::BufReader::new(svt_stderr).read_to_string(&mut buf);
            buf
        })
        .expect("spawn stderr drain");
    
    // Poll for cancel.
    loop {
        if cancel() {
            let _ = ffmpeg.kill();
            let _ = svt.kill();
            let _ = ffmpeg.wait();
            let _ = svt.wait();
            let _ = stderr_handle.join();
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(PredictionError::Cancelled);
        }
        let svt_done = svt.try_wait()?;
        if svt_done.is_some() { break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    // Drain ffmpeg too — it might still be writing after svt exited.
    let _ = ffmpeg.wait();
    let svt_status = svt.wait()?;
    
    let stderr_text = stderr_handle.join().unwrap_or_default();
    
    if !svt_status.success() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(PredictionError::EncodeFailed(format!(
            "SvtAv1EncApp exited with status {:?}",
            svt_status.code()
        )));
    }
    
    // Parse the stat output.
    let parse_result = parse_stat_report(&stderr_text);
    let (frames, stat_report_parsed) = match parse_result {
        Some(f) if !f.is_empty() => (f, true),
        _ => {
            // Fall back: read the output IVF and compute per-frame 
            // bytes from the bitstream itself. Less precise (we don't 
            // know I vs P from the bitstream without a parser) but at 
            // least gives us total bytes.
            let estimated = estimate_from_bytes(&output_path, frame_count)?;
            (estimated, false)
        }
    };
    
    let _ = std::fs::remove_dir_all(&temp_dir);
    
    compute_stats(&frames, stat_report_parsed)
}

/// Try to parse SVT-AV1's --enable-stat-report output. The format 
/// varies across versions but typical lines look like:
///
///   POC 0 ( I  ) ... bits 102400 ...
///   POC 1 ( P  ) ... bits  12345 ...
///
/// or
///
///   Pic Num 0  pic_type I_SLICE  bits 102400 ...
///
/// We scan for POC/Pic Num + bits/type and accept either layout.
fn parse_stat_report(text: &str) -> Option<Vec<FrameStat>> {
    let mut frames = Vec::new();
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        // Look for a line that mentions POC and bits.
        let has_poc = lower.contains("poc ") || lower.contains("pic num");
        let has_bits = lower.contains("bits");
        if !(has_poc && has_bits) { continue; }
        
        // Extract POC. After "poc " or "pic num " comes a number.
        let poc = extract_after(&lower, "poc ")
            .or_else(|| extract_after(&lower, "pic num"))
            .and_then(|s| s.trim().split_whitespace().next().map(|t| t.to_string()))
            .and_then(|s| s.parse::<u32>().ok())?;
        
        // Extract bits count. After "bits" comes a number (possibly 
        // with a colon between).
        let bits = extract_after(&lower, "bits")
            .and_then(|s| {
                let s = s.trim_start_matches(':').trim();
                s.split_whitespace().next().map(|t| t.to_string())
            })
            .and_then(|s| s.parse::<u64>().ok())?;
        let bytes = bits.div_ceil(8);
        
        // Identify slice type. Look for I/I_SLICE/keyframe vs P/B.
        // Strategy: any token "i" alone, "i_slice", or "key" → keyframe.
        let is_keyframe = lower.contains("i_slice") 
            || lower.contains("key") 
            || lower.contains(" ( i ")
            || lower.contains("( i  )");
        
        frames.push(FrameStat { poc, bytes, is_keyframe });
    }
    if frames.is_empty() { None } else { Some(frames) }
}

fn extract_after<'a>(s: &'a str, needle: &str) -> Option<&'a str> {
    s.find(needle).map(|i| &s[i + needle.len()..])
}

/// Fallback: if we couldn't parse per-frame stats, measure the total 
/// output file size and assume the first frame is a keyframe + the 
/// average inter rate is (rest / (frame_count - 1)). Coarse but 
/// usable when the stat report isn't there.
fn estimate_from_bytes(
    output: &Path,
    frame_count: u64,
) -> Result<Vec<FrameStat>, PredictionError> {
    let total = std::fs::metadata(output)?.len();
    if frame_count < 2 {
        return Err(PredictionError::NoStats);
    }
    // Without a bitstream parser, we don't know per-frame sizes. The 
    // best we can do: assume a typical I-frame/total ratio of 0.3 for 
    // a default sample (this is rough but better than nothing). The 
    // caller will see stat_report_parsed=false and treat the verdict 
    // accordingly.
    let estimated_keyframe = (total as f64 * 0.30) as u64;
    let estimated_inter = ((total - estimated_keyframe) as f64 
        / (frame_count - 1) as f64) as u64;
    
    let mut frames = Vec::with_capacity(frame_count as usize);
    frames.push(FrameStat {
        poc: 0,
        bytes: estimated_keyframe,
        is_keyframe: true,
    });
    for i in 1..frame_count {
        frames.push(FrameStat {
            poc: i as u32,
            bytes: estimated_inter,
            is_keyframe: false,
        });
    }
    Ok(frames)
}

fn compute_stats(
    frames: &[FrameStat],
    stat_report_parsed: bool,
) -> Result<PredictionStats, PredictionError> {
    if frames.is_empty() {
        return Err(PredictionError::NoStats);
    }
    let total_frames = frames.len() as u32;
    
    let keyframes: Vec<&FrameStat> = frames.iter().filter(|f| f.is_keyframe).collect();
    let inter: Vec<&FrameStat> = frames.iter().filter(|f| !f.is_keyframe).collect();
    
    let avg_keyframe_bytes = if keyframes.is_empty() { 0.0 } else {
        keyframes.iter().map(|f| f.bytes as f64).sum::<f64>() / keyframes.len() as f64
    };
    let avg_inter_bytes = if inter.is_empty() { 0.0 } else {
        inter.iter().map(|f| f.bytes as f64).sum::<f64>() / inter.len() as f64
    };
    let p_to_i_ratio = if avg_keyframe_bytes > 0.0 {
        avg_inter_bytes / avg_keyframe_bytes
    } else { 0.0 };
    
    // Coefficient of variation for inter frames.
    let inter_size_cv = if inter.len() >= 2 && avg_inter_bytes > 0.0 {
        let var = inter.iter()
            .map(|f| (f.bytes as f64 - avg_inter_bytes).powi(2))
            .sum::<f64>() / inter.len() as f64;
        var.sqrt() / avg_inter_bytes
    } else { 0.0 };
    
    Ok(PredictionStats {
        total_frames,
        keyframe_count: keyframes.len() as u32,
        inter_count: inter.len() as u32,
        avg_keyframe_bytes,
        avg_inter_bytes,
        p_to_i_ratio,
        inter_size_cv,
        stat_report_parsed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    
    fn frame(poc: u32, bytes: u64, key: bool) -> FrameStat {
        FrameStat { poc, bytes, is_keyframe: key }
    }
    
    #[test]
    fn compute_stats_basic() {
        let frames = vec![
            frame(0, 100_000, true),
            frame(1, 10_000, false),
            frame(2, 12_000, false),
            frame(3, 8_000, false),
        ];
        let s = compute_stats(&frames, true).unwrap();
        assert_eq!(s.keyframe_count, 1);
        assert_eq!(s.inter_count, 3);
        assert_eq!(s.avg_keyframe_bytes, 100_000.0);
        assert!((s.avg_inter_bytes - 10_000.0).abs() < 0.1);
        assert!((s.p_to_i_ratio - 0.10).abs() < 0.001);
    }
    
    #[test]
    fn verdict_thresholds() {
        let mut s = PredictionStats {
            total_frames: 10, keyframe_count: 1, inter_count: 9,
            avg_keyframe_bytes: 100_000.0, avg_inter_bytes: 10_000.0,
            p_to_i_ratio: 0.10, inter_size_cv: 0.0, stat_report_parsed: true,
        };
        assert_eq!(verdict_for(&s), Verdict::Red);
        s.p_to_i_ratio = 0.22;
        assert_eq!(verdict_for(&s), Verdict::Yellow);
        s.p_to_i_ratio = 0.40;
        assert_eq!(verdict_for(&s), Verdict::Green);
    }
    
    #[test]
    fn verdict_handles_no_data() {
        let s = PredictionStats {
            total_frames: 0, keyframe_count: 0, inter_count: 0,
            avg_keyframe_bytes: 0.0, avg_inter_bytes: 0.0,
            p_to_i_ratio: 0.0, inter_size_cv: 0.0, stat_report_parsed: false,
        };
        // Doesn't return Green/Red on garbage input — Yellow is the 
        // safe fallback.
        assert_eq!(verdict_for(&s), Verdict::Yellow);
    }
    
    #[test]
    fn parse_handles_known_format() {
        let text = "
            POC 0 ( I  ) bits 102400 PSNR 40.5
            POC 1 ( P  ) bits 12345  PSNR 38.2
            POC 2 ( P  ) bits 11200  PSNR 38.1
        ";
        let frames = parse_stat_report(text).expect("should parse");
        assert_eq!(frames.len(), 3);
        assert!(frames[0].is_keyframe);
        assert!(!frames[1].is_keyframe);
        assert_eq!(frames[0].bytes, 102400 / 8 + 1);  // div_ceil
    }
    
    #[test]
    fn parse_returns_none_on_garbage() {
        assert!(parse_stat_report("hello world\nnothing useful here").is_none());
    }
}
