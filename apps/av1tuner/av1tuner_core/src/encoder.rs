//! # Encoder — the trait every backend implements, plus SVT-AV1
//!
//! M1 ships SVT-AV1 only. rav1e arrives in M5 alongside the GPU work.
//!
//! The execution model: each chunk's encode is a subprocess.
//! 1. We use ffmpeg to extract the chunk's frames from the source and 
//!    pipe them as Y4M to the encoder's stdin.
//! 2. SVT-AV1 reads Y4M from stdin and writes IVF to stdout.
//! 3. We capture stdout to the chunk's .ivf file.
//!
//! This avoids materializing huge intermediate Y4M files on disk. For 
//! a 10-second chunk at 1080p that intermediate would be ~600MB; piping 
//! it through skips the disk write entirely.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EncoderError {
    #[error("encoder binary not found: {0}")]
    NotFound(String),
    #[error("ffmpeg not found on PATH")]
    FfmpegNotFound,
    #[error("encoder subprocess failed (exit {code:?}): {stderr}")]
    Failed { code: Option<i32>, stderr: String },
    #[error("output file missing or empty after encode")]
    NoOutput,
    #[error("encode cancelled by user")]
    Cancelled,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Which encoder backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EncoderKind {
    SvtAv1,
    /// VVC / H.266 via Fraunhofer HHI's VVenC. Slower encode than 
    /// SVT-AV1 but 5-15% better compression at equal perceptual quality.
    /// Patent situation is murky — for personal use only.
    Vvenc,
    // Rav1e in M5
}

impl EncoderKind {
    pub fn display_name(&self) -> &'static str {
        match self {
            EncoderKind::SvtAv1 => "SVT-AV1",
            EncoderKind::Vvenc => "VVenC (VVC/H.266)",
        }
    }
    
    /// What container extension the encoder's output should use.
    /// We always emit a Matroska container at concat time; this is the 
    /// intermediate chunk format.
    pub fn chunk_extension(&self) -> &'static str {
        match self {
            // SVT-AV1 outputs IVF (annex B-ish AV1).
            EncoderKind::SvtAv1 => "ivf",
            // VVenC outputs raw H.266/VVC Annex B (.266 or .vvc).
            EncoderKind::Vvenc => "266",
        }
    }
}

/// Encoder settings. One struct holds both codecs' settings at the 
/// same time so the user can switch between them without losing their 
/// per-codec choices. `kind` picks the active codec; the encoder 
/// backend reads only the fields relevant to its codec.
///
/// The `crf()` method returns the quantizer for the active codec — most 
/// callers should use that rather than reading svt_crf/vvenc_qp directly.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EncoderSettings {
    pub kind: EncoderKind,
    /// SVT-AV1 CRF (0-63). Typical 22-35.
    pub svt_crf: u32,
    /// SVT-AV1 numeric preset (0-13). Lower = slower + better.
    pub preset: u32,
    /// SVT-AV1 tune flag: 0=visual, 1=PSNR.
    pub tune: u32,
    /// VVenC QP (0-63). Typical 28-40 (different scale than SVT-AV1 CRF).
    pub vvenc_qp: u32,
    /// VVenC named preset.
    pub vvenc_preset: VvencPreset,
    /// VVenC quality-perceptual-aware mode.
    pub vvenc_qpa: bool,
    /// Pixel format. yuv420p10le works for both encoders.
    pub pixel_format: String,
    /// Extra args passed verbatim to the encoder. Power-user escape hatch.
    pub extra_args: Vec<String>,
    /// Optional ffmpeg video-filter chain appended after the trim 
    /// filter (e.g. a scale+pad for merge-group resolution policy). 
    /// Empty = no extra filtering. Applied to both SVT-AV1 and VVenC 
    /// pipelines since both run through ffmpeg first.
    #[serde(default)]
    pub video_filter: String,
}

impl Default for EncoderSettings {
    fn default() -> Self {
        Self {
            kind: EncoderKind::SvtAv1,
            svt_crf: 28,
            preset: 6,
            tune: 0,
            vvenc_qp: 34,            // Typical VVC QP is higher than AV1 CRF
            vvenc_preset: VvencPreset::Medium,
            vvenc_qpa: true,
            pixel_format: "yuv420p10le".into(),
            extra_args: Vec::new(),
            video_filter: String::new(),
        }
    }
}

impl EncoderSettings {
    /// Quantizer for the active codec. SVT-AV1 backend wants svt_crf; 
    /// VVenC backend wants vvenc_qp. This unifies for callers that 
    /// just need "the quantizer."
    pub fn quantizer(&self) -> u32 {
        match self.kind {
            EncoderKind::SvtAv1 => self.svt_crf,
            EncoderKind::Vvenc => self.vvenc_qp,
        }
    }
    
    /// A copy of these settings with the codec set to the given kind. 
    /// Used to build the two EncoderSettings for a compare-codecs sample 
    /// run from a single user-config snapshot.
    pub fn with_kind(&self, kind: EncoderKind) -> Self {
        let mut s = self.clone();
        s.kind = kind;
        s
    }
}

/// VVenC named-preset enum. Each value maps to one of vvencapp's 
/// --preset choices. Slower = better quality, more encode time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VvencPreset {
    Faster,
    Fast,
    Medium,
    Slow,
    Slower,
}

impl VvencPreset {
    pub fn as_arg(&self) -> &'static str {
        match self {
            VvencPreset::Faster => "faster",
            VvencPreset::Fast => "fast",
            VvencPreset::Medium => "medium",
            VvencPreset::Slow => "slow",
            VvencPreset::Slower => "slower",
        }
    }
    
    pub fn display_name(&self) -> &'static str {
        match self {
            VvencPreset::Faster => "Faster",
            VvencPreset::Fast => "Fast",
            VvencPreset::Medium => "Medium (default)",
            VvencPreset::Slow => "Slow",
            VvencPreset::Slower => "Slower (best quality)",
        }
    }
    
    pub fn all() -> &'static [VvencPreset] {
        &[
            VvencPreset::Faster, VvencPreset::Fast, VvencPreset::Medium,
            VvencPreset::Slow, VvencPreset::Slower,
        ]
    }
}

impl Default for VvencPreset {
    fn default() -> Self { VvencPreset::Medium }
}

/// Result of one chunk encode.
#[derive(Debug)]
pub struct EncodeResult {
    pub elapsed_ms: u64,
    pub output_bytes: u64,
}

/// Handle the scheduler hands to the encoder so the encoder can check 
/// for cancellation during a long encode. Returns true when the worker 
/// should stop ASAP and kill its subprocesses.
///
/// We pass this as `&dyn Fn() -> bool` rather than `Arc<AtomicBool>` 
/// directly so the encoder trait stays decoupled from any particular 
/// cancel mechanism (e.g. tests can pass `&|| false`).
pub type CancelToken<'a> = &'a (dyn Fn() -> bool + Sync);

/// The trait. Implemented per backend.
pub trait Encoder: Send + Sync {
    /// Verify the encoder binary is reachable + reports a sane version.
    /// Run once at preflight, not per chunk.
    fn check_available(&self) -> Result<String, EncoderError>;

    /// Encode the frame range `[start_frame, end_frame)` of `source` 
    /// into `output_path` (an .ivf file).
    ///
    /// `cancel` is polled periodically while waiting on subprocesses; 
    /// if it returns true, the encoder kills its children and returns 
    /// an error. Pass `&|| false` for non-cancellable encodes.
    fn encode_chunk(
        &self,
        source: &Path,
        start_frame: u64,
        end_frame: u64,
        output_path: &Path,
        settings: &EncoderSettings,
        cancel: CancelToken,
    ) -> Result<EncodeResult, EncoderError>;
}

/// SVT-AV1 backend.
pub struct SvtAv1 {
    /// Path to `SvtAv1EncApp` binary. Looked up at construction time.
    binary_path: PathBuf,
    ffmpeg_path: PathBuf,
}

impl SvtAv1 {
    /// Locate SVT-AV1 + ffmpeg on PATH. Errors if either is missing.
    pub fn locate() -> Result<Self, EncoderError> {
        let binary_path = which::which("SvtAv1EncApp")
            .map_err(|_| EncoderError::NotFound("SvtAv1EncApp".to_string()))?;
        let ffmpeg_path = which::which("ffmpeg")
            .map_err(|_| EncoderError::FfmpegNotFound)?;
        Ok(Self { binary_path, ffmpeg_path })
    }
    
    /// For tests / explicit paths.
    pub fn with_paths(binary_path: PathBuf, ffmpeg_path: PathBuf) -> Self {
        Self { binary_path, ffmpeg_path }
    }
}

impl Encoder for SvtAv1 {
    fn check_available(&self) -> Result<String, EncoderError> {
        let output = Command::new(&self.binary_path)
            .arg("--version")
            .output()?;
        // SVT-AV1's --version exits non-zero in some builds. Check 
        // stdout instead of status; the binary printing anything 
        // version-like means it's reachable.
        let combined = format!("{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr));
        if combined.is_empty() {
            return Err(EncoderError::Failed {
                code: output.status.code(),
                stderr: "no version output".to_string(),
            });
        }
        Ok(combined.trim().to_string())
    }

    fn encode_chunk(
        &self,
        source: &Path,
        start_frame: u64,
        end_frame: u64,
        output_path: &Path,
        settings: &EncoderSettings,
        cancel: CancelToken,
    ) -> Result<EncodeResult, EncoderError> {
        let start = Instant::now();
        let frame_count = end_frame - start_frame;
        
        // We chain two subprocesses: ffmpeg | SvtAv1EncApp.
        // ffmpeg reads the source, trims to the chunk's frame range, 
        // outputs Y4M to stdout. SVT reads Y4M from stdin, writes IVF 
        // to the output path.
        //
        // -vf trim=...:setpts=PTS-STARTPTS gives us frame-accurate trim 
        // and renumbers timestamps from 0 so SVT sees a clean stream.
        let trim_filter = build_filter_chain(start_frame, end_frame, &settings.video_filter);
        
        let mut ffmpeg = Command::new(&self.ffmpeg_path)
            .args([
                "-hide_banner", "-loglevel", "error",
                "-nostdin",  // don't read from our stdin
                "-i",
            ])
            .arg(source)
            .args([
                "-vf", &trim_filter,
                "-pix_fmt", &settings.pixel_format,
                "-f", "yuv4mpegpipe",
                "-strict", "-1",  // allow non-mod2 dimensions if any
                "-",  // stdout
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        let ffmpeg_stdout = ffmpeg.stdout.take()
            .expect("ffmpeg stdout piped");
        let ffmpeg_stderr = ffmpeg.stderr.take()
            .expect("ffmpeg stderr piped");
        
        // Drain ffmpeg stderr in a background thread. If we don't drain,
        // ffmpeg can block when its stderr pipe fills (~64KB on Linux).
        let ffmpeg_stderr_handle = std::thread::Builder::new()
            .name("ffmpeg-stderr".into())
            .spawn(move || {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(
                    &mut std::io::BufReader::new(ffmpeg_stderr),
                    &mut buf,
                );
                buf
            })
            .expect("spawn ffmpeg-stderr drain");
        
        // SVT-AV1 args. M1 keeps it minimal.
        let mut svt_args: Vec<String> = vec![
            "-i".into(), "stdin".into(),
            "-b".into(), output_path.to_string_lossy().to_string(),
            "--preset".into(), settings.preset.to_string(),
            "--crf".into(), settings.svt_crf.to_string(),
            "--tune".into(), settings.tune.to_string(),
            // -n is the exact frame count. Belt-and-suspenders with the 
            // ffmpeg trim filter; SVT stops cleanly at this count even if 
            // ffmpeg emits one extra frame due to timestamp rounding.
            "-n".into(), frame_count.to_string(),
        ];
        svt_args.extend(settings.extra_args.iter().cloned());
        
        let mut svt = Command::new(&self.binary_path)
            .args(&svt_args)
            .stdin(Stdio::from(ffmpeg_stdout))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        
        let svt_stderr = svt.stderr.take()
            .expect("svt stderr piped");
        
        // Drain SVT stderr in a background thread (same reason as ffmpeg).
        let svt_stderr_handle = std::thread::Builder::new()
            .name("svt-stderr".into())
            .spawn(move || {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(
                    &mut std::io::BufReader::new(svt_stderr),
                    &mut buf,
                );
                buf
            })
            .expect("spawn svt-stderr drain");
        
        // Wait for both subprocesses, polling for cancel periodically. 
        // We try_wait() in a loop with 100ms sleeps; on cancel we kill 
        // both processes and return early.
        let wait_result = wait_with_cancel(&mut ffmpeg, &mut svt, cancel, "SVT-AV1");
        
        // Drain stderr threads regardless of outcome — they'll finish 
        // shortly after their parents exit.
        let ffmpeg_stderr = ffmpeg_stderr_handle.join().unwrap_or_default();
        let svt_stderr = svt_stderr_handle.join().unwrap_or_default();
        
        match wait_result {
            WaitOutcome::Cancelled => {
                // Best-effort cleanup of partial output.
                let _ = std::fs::remove_file(output_path);
                return Err(EncoderError::Cancelled);
            }
            WaitOutcome::Failed { which, code } => {
                // Best-effort cleanup of partial output.
                let _ = std::fs::remove_file(output_path);
                let stderr = combine_stderrs(which, &ffmpeg_stderr, &svt_stderr);
                let truncated = truncate_stderr(&stderr);
                return Err(EncoderError::Failed {
                    code,
                    stderr: format!(
                        "{} failed for chunk frames {}..{}: {}",
                        which, start_frame, end_frame, truncated
                    ),
                });
            }
            WaitOutcome::Ok => {}
        }
        
        // Verify output exists and is non-empty.
        let meta = std::fs::metadata(output_path).map_err(|_| EncoderError::NoOutput)?;
        let output_bytes = meta.len();
        if output_bytes == 0 {
            return Err(EncoderError::NoOutput);
        }
        
        Ok(EncodeResult {
            elapsed_ms: start.elapsed().as_millis() as u64,
            output_bytes,
        })
    }
}

/// Outcome of waiting on the encoder subprocess chain.
enum WaitOutcome {
    Ok,
    Cancelled,
    /// One process failed; `which` is "ffmpeg" or "SVT-AV1".
    Failed { which: &'static str, code: Option<i32> },
}

/// Wait on ffmpeg + SVT, checking cancel between polls. Kills both on 
/// cancel. Polls at 100ms cadence — fast enough that cancel feels 
/// responsive, slow enough not to burn CPU.
fn wait_with_cancel(
    ffmpeg: &mut std::process::Child,
    svt: &mut std::process::Child,
    cancel: CancelToken,
    encoder_name: &'static str,
) -> WaitOutcome {
    use std::process::ExitStatus;
    
    let mut ffmpeg_status: Option<ExitStatus> = None;
    let mut svt_status: Option<ExitStatus> = None;
    
    loop {
        if cancel() {
            let _ = ffmpeg.kill();
            let _ = svt.kill();
            // Wait for them to actually die so we don't leak zombies.
            let _ = ffmpeg.wait();
            let _ = svt.wait();
            return WaitOutcome::Cancelled;
        }
        
        if ffmpeg_status.is_none() {
            match ffmpeg.try_wait() {
                Ok(Some(s)) => ffmpeg_status = Some(s),
                Ok(None) => {}
                Err(_) => {
                    // OS-level wait error. Kill the partner and bail.
                    let _ = svt.kill();
                    let _ = svt.wait();
                    return WaitOutcome::Failed { which: "ffmpeg", code: None };
                }
            }
        }
        if svt_status.is_none() {
            match svt.try_wait() {
                Ok(Some(s)) => svt_status = Some(s),
                Ok(None) => {}
                Err(_) => {
                    let _ = ffmpeg.kill();
                    let _ = ffmpeg.wait();
                    return WaitOutcome::Failed { which: encoder_name, code: None };
                }
            }
        }
        
        if ffmpeg_status.is_some() && svt_status.is_some() {
            break;
        }
        
        // If one died with failure, kill the other and bail.
        if let Some(s) = ffmpeg_status {
            if !s.success() {
                let _ = svt.kill();
                let _ = svt.wait();
                return WaitOutcome::Failed { which: "ffmpeg", code: s.code() };
            }
        }
        if let Some(s) = svt_status {
            if !s.success() {
                let _ = ffmpeg.kill();
                let _ = ffmpeg.wait();
                return WaitOutcome::Failed { which: encoder_name, code: s.code() };
            }
        }
        
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    
    // Both finished. Inspect statuses.
    let f = ffmpeg_status.unwrap();
    let s = svt_status.unwrap();
    
    if !f.success() {
        return WaitOutcome::Failed { which: "ffmpeg", code: f.code() };
    }
    if !s.success() {
        return WaitOutcome::Failed { which: encoder_name, code: s.code() };
    }
    WaitOutcome::Ok
}

/// Build the ffmpeg -vf chain for a chunk: frame-accurate trim + 
/// timestamp reset, with an optional extra filter (e.g. scale+pad for 
/// merge resolution policy) inserted before the setpts reset.
///
/// Order matters: trim first (select the frames), then any scaling, 
/// then setpts to renumber from zero so the encoder sees a clean 
/// stream starting at PTS 0.
fn build_filter_chain(start_frame: u64, end_frame: u64, extra: &str) -> String {
    let trimmed = extra.trim();
    if trimmed.is_empty() {
        format!(
            "trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS",
            start_frame, end_frame
        )
    } else {
        format!(
            "trim=start_frame={}:end_frame={},{},setpts=PTS-STARTPTS",
            start_frame, end_frame, trimmed
        )
    }
}

/// Compute the effective output width/height after a video filter 
/// chain. Critical for the VVenC raw-video path: ffmpeg outputs frames 
/// at the FILTERED resolution, so vvencapp must be told the filtered 
/// dimensions via -s, not the source dimensions. 
///
/// We recognize the two filter shapes the group runner emits:
///   - `pad=W:H:...`   → final canvas is WxH (pad sets the output size)
///   - `scale=W:H...`  → output is WxH
/// `pad` takes precedence since our scale_filter pads to an exact target 
/// after a decrease-scale. Falls back to source geometry if no 
/// resolution-changing filter is found.
fn effective_dimensions(source_w: u32, source_h: u32, filter: &str) -> (u32, u32) {
    // Try pad=W:H first (authoritative final canvas size).
    if let Some(dims) = extract_filter_dims(filter, "pad=") {
        return dims;
    }
    if let Some(dims) = extract_filter_dims(filter, "scale=") {
        return dims;
    }
    (source_w, source_h)
}

/// Extract WxH from a filter token like "pad=1280:720:..." or 
/// "scale=1280:720". Returns None if the token isn't present or the 
/// dimensions aren't both literal integers (e.g. scale=-1:720 uses an 
/// auto dimension we can't resolve statically).
fn extract_filter_dims(filter: &str, prefix: &str) -> Option<(u32, u32)> {
    let idx = filter.find(prefix)?;
    let rest = &filter[idx + prefix.len()..];
    // Take up to the next comma (end of this filter token).
    let token = rest.split(',').next().unwrap_or(rest);
    let mut parts = token.split(':');
    let w: u32 = parts.next()?.trim().parse().ok()?;
    let h: u32 = parts.next()?.trim().parse().ok()?;
    if w == 0 || h == 0 { return None; }
    Some((w, h))
}

/// Minimal source geometry needed to drive a raw-video pipe into 
/// vvencapp. We derive fps as a fraction to keep exact rates 
/// (23.976 = 24000/1001, etc).
struct Geometry {
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
}

/// Probe width/height/fps from the source using ffprobe (which ships 
/// alongside ffmpeg). Used by the VVenC raw-video path since rawvideo 
/// carries no dimensions. ffprobe is expected next to ffmpeg; if the 
/// ffmpeg path is /usr/bin/ffmpeg, ffprobe is /usr/bin/ffprobe.
fn probe_geometry(ffmpeg_path: &Path, source: &Path) -> Result<Geometry, EncoderError> {
    // Derive ffprobe path from ffmpeg path.
    let ffprobe_path = ffmpeg_path
        .parent()
        .map(|dir| dir.join(if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" }))
        .filter(|p| p.exists())
        .or_else(|| which::which("ffprobe").ok())
        .ok_or(EncoderError::FfmpegNotFound)?;
    
    let output = Command::new(&ffprobe_path)
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height,r_frame_rate",
            "-of", "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(source)
        .output()?;
    
    if !output.status.success() {
        return Err(EncoderError::Failed {
            code: output.status.code(),
            stderr: format!("ffprobe geometry failed: {}",
                String::from_utf8_lossy(&output.stderr)),
        });
    }
    
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let width: u32 = lines.next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| EncoderError::Failed {
            code: None,
            stderr: "ffprobe: couldn't parse width".to_string(),
        })?;
    let height: u32 = lines.next()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| EncoderError::Failed {
            code: None,
            stderr: "ffprobe: couldn't parse height".to_string(),
        })?;
    // r_frame_rate comes as "num/den", e.g. "30000/1001" or "25/1".
    let rate_str = lines.next().unwrap_or("25/1").trim();
    let (fps_num, fps_den) = parse_rate(rate_str);
    
    Ok(Geometry { width, height, fps_num, fps_den })
}

/// Parse an ffprobe "num/den" frame-rate string into a fraction. 
/// Falls back to 25/1 on anything unparseable.
fn parse_rate(s: &str) -> (u32, u32) {
    let mut parts = s.split('/');
    let num = parts.next().and_then(|n| n.trim().parse::<u32>().ok());
    let den = parts.next().and_then(|d| d.trim().parse::<u32>().ok());
    match (num, den) {
        (Some(n), Some(d)) if n > 0 && d > 0 => (n, d),
        (Some(n), None) if n > 0 => (n, 1),
        _ => (25, 1),
    }
}

fn combine_stderrs(which: &str, ffmpeg: &str, encoder: &str) -> String {
    let f = ffmpeg.trim();
    let e = encoder.trim();
    // `which` is either "ffmpeg" or the encoder's display name. Prefer 
    // the matching stderr; fall back to whichever has content.
    if which == "ffmpeg" {
        if !f.is_empty() { return f.to_string(); }
    } else {
        // Encoder failure case (SVT-AV1 or VVenC).
        if !e.is_empty() { return e.to_string(); }
    }
    if !e.is_empty() { e.to_string() }
    else if !f.is_empty() { f.to_string() }
    else { "(no stderr captured)".to_string() }
}

/// Encoder stderrs can be hundreds of KB on verbose builds. Truncate 
/// for display, keeping head + tail (most diagnostic info is at the end).
fn truncate_stderr(s: &str) -> String {
    const MAX: usize = 1024;
    if s.len() <= MAX {
        return s.to_string();
    }
    let head = &s[..256];
    let tail = &s[s.len() - 700..];
    format!("{}\n... [truncated {} bytes] ...\n{}", head, s.len() - 956, tail)
}

/// VVenC (Fraunhofer HHI's VVC encoder) backend.
///
/// Invocation pattern is similar to SvtAv1: ffmpeg trims + outputs Y4M 
/// to stdin, vvencapp reads Y4M and writes raw H.266 to the output 
/// path. VVenC's CLI is `vvencapp` (not `vvenc` — that's the library).
pub struct VvencEncoder {
    binary_path: PathBuf,
    ffmpeg_path: PathBuf,
}

impl VvencEncoder {
    pub fn locate() -> Result<Self, EncoderError> {
        // Try PATH first (system install via package manager, manual 
        // install to /usr/local, etc.). Fall back to the av1tuner-
        // managed install location if the user used the Install button.
        let binary_path = which::which("vvencapp")
            .ok()
            .or_else(|| Self::managed_install_path())
            .ok_or_else(|| EncoderError::NotFound("vvencapp".to_string()))?;
        let ffmpeg_path = which::which("ffmpeg")
            .map_err(|_| EncoderError::FfmpegNotFound)?;
        Ok(Self { binary_path, ffmpeg_path })
    }
    
    /// Path to vvencapp installed by av1tuner's built-in installer, 
    /// if it exists. Returns None when not installed there.
    fn managed_install_path() -> Option<PathBuf> {
        let paths = crate::vvenc_install::InstallPaths::default_paths()?;
        if paths.is_installed() {
            Some(paths.vvencapp_path)
        } else {
            None
        }
    }
}

impl Encoder for VvencEncoder {
    fn check_available(&self) -> Result<String, EncoderError> {
        // vvencapp has no --version flag (only -h/--help/--fullhelp), 
        // but every invocation prints a banner like 
        //   "vvencapp: Fraunhofer VVC Encoder ver. 1.13.1 [...]"
        // to stdout. We run -h (a valid flag, exits 0) and pull the 
        // banner line out.
        let output = Command::new(&self.binary_path)
            .arg("-h")
            .output()?;
        let combined = format!("{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr));
        if combined.is_empty() {
            return Err(EncoderError::Failed {
                code: output.status.code(),
                stderr: "no output from vvencapp -h".to_string(),
            });
        }
        // Extract the banner line (the one mentioning "ver.") if present, 
        // else fall back to the first non-empty line.
        let version_line = combined.lines()
            .find(|l| l.to_ascii_lowercase().contains("ver."))
            .or_else(|| combined.lines().find(|l| !l.trim().is_empty()))
            .unwrap_or("vvencapp (version unknown)")
            .trim()
            .to_string();
        Ok(version_line)
    }
    
    fn encode_chunk(
        &self,
        source: &Path,
        start_frame: u64,
        end_frame: u64,
        output_path: &Path,
        settings: &EncoderSettings,
        cancel: CancelToken,
    ) -> Result<EncodeResult, EncoderError> {
        let start = Instant::now();
        let frame_count = end_frame - start_frame;
        
        // ffmpeg trims the source and outputs Y4M to stdout — same as 
        // for SVT-AV1.
        let trim_filter = build_filter_chain(start_frame, end_frame, &settings.video_filter);
        
        // VVenC's Y4M parser is finicky about colorspace tags — ffmpeg 
        // writes "C420p10" for 10-bit, which VVenC's reader doesn't 
        // reliably accept, producing format errors. The bulletproof path 
        // (used by av1an, VVCEasy) is raw video + explicit geometry told 
        // to vvencapp via -s/-r/-c. So we probe the source geometry, 
        // pipe rawvideo, and pass dimensions explicitly.
        let geom = probe_geometry(&self.ffmpeg_path, source)?;
        // If the filter chain scales/pads (resolution policy), ffmpeg 
        // emits frames at the FILTERED size — vvencapp must be told that 
        // size, not the source size, or it misreads the raw frame layout.
        let (out_w, out_h) = effective_dimensions(
            geom.width, geom.height, &settings.video_filter);
        
        let mut ffmpeg = Command::new(&self.ffmpeg_path)
            .args([
                "-hide_banner", "-loglevel", "error",
                "-nostdin",
                "-i",
            ])
            .arg(source)
            .args([
                "-vf", &trim_filter,
                "-pix_fmt", &settings.pixel_format,
                "-f", "rawvideo",
                "-",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        
        let ffmpeg_stdout = ffmpeg.stdout.take().expect("ffmpeg stdout piped");
        let ffmpeg_stderr = ffmpeg.stderr.take().expect("ffmpeg stderr piped");
        let ffmpeg_stderr_handle = std::thread::Builder::new()
            .name("ffmpeg-stderr-vvc".into())
            .spawn(move || {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(
                    &mut std::io::BufReader::new(ffmpeg_stderr), &mut buf);
                buf
            })
            .expect("spawn ffmpeg-stderr drain");
        
        // VVenC args (per https://github.com/fraunhoferhhi/vvenc/wiki/Usage):
        //   --y4m                bare flag — interpret stdin input as Y4M
        //   -i -                 read from stdin  
        //   -o <path>            output raw H.266 (.266) file
        //   --preset <name>      named preset (faster/fast/medium/slow/slower)
        //   --qp <n>             quantizer 0-63 (lower = better quality)
        //   --qpa <0|1>          perceptual quality (1 = on)
        //   -c <yuv420|yuv420_10>  internal pixel-format / bit depth
        //   --frames <n>         exact frame count (--y4m flow needs explicit count
        //                        since the pipe doesn't have a length header)
        //
        // The pixel-format choice: VVenC accepts only yuv420 (8-bit) and 
        // yuv420_10 (10-bit). If the ffmpeg side outputs 10-bit Y4M 
        // (yuv420p10le, our default), VVenC needs `-c yuv420_10`. For 
        // 8-bit, `-c yuv420`. Anything else is unsupported and gets 
        // mapped to the 8-bit default.
        let vvenc_pix_fmt = if settings.pixel_format.contains("10") {
            "yuv420_10"
        } else {
            "yuv420"
        };
        // Raw video input: vvencapp needs to be told size, frame rate, 
        // and pixel format since rawvideo carries no header. We feed 
        // fps as a fraction to preserve exact rates like 30000/1001.
        let size_arg = format!("{}x{}", out_w, out_h);
        let fps_arg = format!("{}/{}", geom.fps_num, geom.fps_den);
        
        // Intra period (-ip): vvencapp requires a multiple of 16 (per 
        // Fraunhofer's guidance; e.g. 64 for 30fps, 48 for 24fps). We 
        // pick the largest multiple of 16 that fits the chunk, floored 
        // at 16. The chunk's first frame is always an IDR regardless, 
        // so chunk independence for concat is preserved; -ip controls 
        // keyframe cadence *within* the chunk.
        let intra_period = {
            let fc = frame_count.max(16);
            let mult = (fc / 16) * 16;
            mult.max(16).to_string()
        };
        
        // Per-instance thread cap. Chunks already run in parallel via 
        // the scheduler, so each vvencapp instance should NOT grab all 
        // cores (VVenC defaults to 8 for >=720p, which oversubscribes 
        // when N chunks run at once). Cap at 4 per instance — a balance 
        // that keeps each chunk moving without thrashing. 
        // TODO: derive from (cores / concurrency) once that's plumbed 
        // through EncoderSettings, mirroring the SVT --lp discussion.
        let threads = "4";
        
        // Note: --frames is intentionally omitted. It is not a vvencapp 
        // option (the simple encoder reads the raw pipe to EOF). ffmpeg 
        // emits exactly `frame_count` frames via the trim filter, so 
        // vvencapp encodes exactly that many and stops at EOF.
        let mut vvenc_args: Vec<String> = vec![
            "-i".into(), "-".into(),
            "-s".into(), size_arg,
            "--fps".into(), fps_arg,
            "-c".into(), vvenc_pix_fmt.into(),
            "--preset".into(), settings.vvenc_preset.as_arg().to_string(),
            "--qp".into(), settings.vvenc_qp.to_string(),
            "--qpa".into(), if settings.vvenc_qpa { "1" } else { "0" }.into(),
            "-ip".into(), intra_period,
            "-t".into(), threads.into(),
            "-o".into(), output_path.to_string_lossy().to_string(),
        ];
        vvenc_args.extend(settings.extra_args.iter().cloned());
        
        let mut vvenc = Command::new(&self.binary_path)
            .args(&vvenc_args)
            .stdin(Stdio::from(ffmpeg_stdout))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        
        let vvenc_stderr = vvenc.stderr.take().expect("vvenc stderr piped");
        let vvenc_stderr_handle = std::thread::Builder::new()
            .name("vvenc-stderr".into())
            .spawn(move || {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(
                    &mut std::io::BufReader::new(vvenc_stderr), &mut buf);
                buf
            })
            .expect("spawn vvenc-stderr drain");
        
        let wait_result = wait_with_cancel(&mut ffmpeg, &mut vvenc, cancel, "VVenC");
        let ffmpeg_stderr = ffmpeg_stderr_handle.join().unwrap_or_default();
        let vvenc_stderr = vvenc_stderr_handle.join().unwrap_or_default();
        
        match wait_result {
            WaitOutcome::Cancelled => {
                let _ = std::fs::remove_file(output_path);
                return Err(EncoderError::Cancelled);
            }
            WaitOutcome::Failed { which, code } => {
                let _ = std::fs::remove_file(output_path);
                let stderr = combine_stderrs(which, &ffmpeg_stderr, &vvenc_stderr);
                let truncated = truncate_stderr(&stderr);
                return Err(EncoderError::Failed {
                    code,
                    stderr: format!(
                        "{} failed for chunk frames {}..{}: {}",
                        which, start_frame, end_frame, truncated
                    ),
                });
            }
            WaitOutcome::Ok => {}
        }
        
        let meta = std::fs::metadata(output_path).map_err(|_| EncoderError::NoOutput)?;
        let output_bytes = meta.len();
        if output_bytes == 0 {
            return Err(EncoderError::NoOutput);
        }
        
        Ok(EncodeResult {
            elapsed_ms: start.elapsed().as_millis() as u64,
            output_bytes,
        })
    }
}

/// Construct an encoder from settings.
pub fn make_encoder(settings: &EncoderSettings) -> Result<Box<dyn Encoder>, EncoderError> {
    match settings.kind {
        EncoderKind::SvtAv1 => Ok(Box::new(SvtAv1::locate()?)),
        EncoderKind::Vvenc => Ok(Box::new(VvencEncoder::locate()?)),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn filter_chain_with_and_without_extra() {
        use super::build_filter_chain;
        let bare = build_filter_chain(0, 100, "");
        assert_eq!(bare, "trim=start_frame=0:end_frame=100,setpts=PTS-STARTPTS");
        let scaled = build_filter_chain(0, 100, "scale=1280:720");
        assert_eq!(scaled, 
            "trim=start_frame=0:end_frame=100,scale=1280:720,setpts=PTS-STARTPTS");
        // whitespace-only extra is treated as empty
        let ws = build_filter_chain(5, 50, "   ");
        assert_eq!(ws, "trim=start_frame=5:end_frame=50,setpts=PTS-STARTPTS");
    }
    
    #[test]
    fn effective_dims_from_filters() {
        use super::effective_dimensions;
        // No resolution filter → source dims.
        assert_eq!(effective_dimensions(1920, 1080, ""), (1920, 1080));
        assert_eq!(
            effective_dimensions(1920, 1080, "trim=start_frame=0:end_frame=30"),
            (1920, 1080)
        );
        // pad takes precedence (our scale_filter pads to exact target).
        assert_eq!(
            effective_dimensions(1920, 1080,
                "scale=1280:720:force_original_aspect_ratio=decrease,pad=1280:720:(ow-iw)/2:(oh-ih)/2"),
            (1280, 720)
        );
        // scale only.
        assert_eq!(
            effective_dimensions(1920, 1080, "scale=1280:720"),
            (1280, 720)
        );
        // scale with auto dimension (-1) → can't resolve, falls to source.
        assert_eq!(
            effective_dimensions(1920, 1080, "scale=-1:720"),
            (1920, 1080)
        );
    }
    
    #[test]
    fn parse_rate_handles_formats() {
        use super::parse_rate;
        assert_eq!(parse_rate("30000/1001"), (30000, 1001));
        assert_eq!(parse_rate("25/1"), (25, 1));
        assert_eq!(parse_rate("60"), (60, 1));
        assert_eq!(parse_rate("garbage"), (25, 1));
        assert_eq!(parse_rate("0/0"), (25, 1));
    }

    use super::*;

    #[test]
    fn default_settings_sensible() {
        let s = EncoderSettings::default();
        assert_eq!(s.kind, EncoderKind::SvtAv1);
        assert!(s.svt_crf >= 20 && s.svt_crf <= 35);
        assert!(s.vvenc_qp >= 28 && s.vvenc_qp <= 40);
        assert!(s.preset <= 8);
    }

    #[test]
    fn settings_serialize() {
        let s = EncoderSettings::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: EncoderSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.svt_crf, s.svt_crf);
        assert_eq!(back.vvenc_qp, s.vvenc_qp);
        assert_eq!(back.preset, s.preset);
    }
    
    #[test]
    fn quantizer_picks_active_codec() {
        let mut s = EncoderSettings::default();
        s.svt_crf = 30;
        s.vvenc_qp = 38;
        s.kind = EncoderKind::SvtAv1;
        assert_eq!(s.quantizer(), 30);
        s.kind = EncoderKind::Vvenc;
        assert_eq!(s.quantizer(), 38);
    }
}
