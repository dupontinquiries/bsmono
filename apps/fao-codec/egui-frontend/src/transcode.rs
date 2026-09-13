//! transcode — a real video file in, a real `.fao` out, opened straight in the Player.
//!
//! Every other page that touches the encoder (Encode Lab, seq_lab, the Player's own tests)
//! works on synthetic frames. Nothing in this port had ever put a real video file through the
//! codec end to end. This is that pipeline, and it is deliberately the SIMPLEST one that is
//! real: every frame is coded independently with `compress_frame_dct` (marker `'D'`, w×h RGB
//! in, quality in `1..=100`) — the exact function `lab::run_source` already exercises and
//! `decode.rs`'s own round-trip test decodes back. No T-frames, no K-frame dictionary, no
//! `VideoEncoder`'s scene-change/GOP logic: `VideoEncoder::encode_frame` does not expose the
//! compressed bytes it discards after reconstructing (only the reconstruction and a summary
//! `FrameResult` cross its API), so wiring it up is separate work, not a detail of this slice.
//! What this buys instead: a video that survives ffmpeg → fao → ffmpeg-eye-check TODAY, and a
//! known, honest floor to chase real inter-frame compression down from — intra-only DCT is
//! roughly what AV1's own I-frames cost, so the ratio this reports is the right one to compare
//! `V2Encoder`'s inter-frame savings against once that wiring exists.
use fao_codec_lib as fao_v1;
use std::process::{Command, Stdio};

#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub w: usize,
    pub h: usize,
    pub fps: f64,
    pub duration_s: f64,
    pub codec: String,
}

/// `ffprobe -show_streams`, the same call `fao_app::probe_video` makes, parsed with `serde_json`
/// rather than the ad hoc string slicing this needed anyway once `fps_num`/`fps_den` (an `N/D`
/// string in ffprobe's own output) had to be split.
pub fn probe(path: &str) -> Result<VideoInfo, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path).output().map_err(|e| format!("ffprobe: {e}"))?;
    if !out.status.success() { return Err("ffprobe failed (not a media file?)".into()); }
    let info: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe JSON: {e}"))?;
    let streams = info["streams"].as_array().ok_or("no streams")?;
    let v = streams.iter().find(|s| s["codec_type"] == "video").ok_or("no video stream")?;
    let w = v["width"].as_u64().ok_or("no width")? as usize;
    let h = v["height"].as_u64().ok_or("no height")? as usize;
    let fps = v["avg_frame_rate"].as_str().and_then(parse_rational)
        .filter(|f| *f > 0.0 && *f < 1000.0)
        .or_else(|| v["r_frame_rate"].as_str().and_then(parse_rational))
        .unwrap_or(30.0);
    let duration_s = info["format"]["duration"].as_str().and_then(|s| s.parse().ok())
        .or_else(|| v["duration"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0);
    let codec = v["codec_name"].as_str().unwrap_or("?").to_string();
    Ok(VideoInfo { w, h, fps, duration_s, codec })
}

fn parse_rational(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d != 0.0).then_some(n / d)
}

/// Decode every frame to packed RGB8 in ONE ffmpeg process (a `rawvideo` pipe to stdout) rather
/// than fao_app's one-`Command`-per-frame pattern, which is fine for a single still but not for
/// a sequence. Width/height are rounded down to even numbers — the encoder's chroma planes are
/// half-size in both axes, the same crop `lab::Source::load` applies to a loaded image.
/// `max_frames` bounds memory for the egui page (a 1080p minute is ~11 GB uncompressed); `None`
/// decodes the whole file.
pub fn decode_frames_rgb(path: &str, w: usize, h: usize, max_frames: Option<usize>) -> Result<Vec<Vec<u8>>, String> {
    let (w, h) = (w & !1, h & !1);
    if w < 8 || h < 8 { return Err(format!("{w}x{h} is too small to encode")); }
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-i").arg(path);
    if let Some(n) = max_frames { cmd.args(["-vframes", &n.to_string()]); }
    cmd.args(["-vf", &format!("scale={w}:{h}")])
       .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-v", "error", "-"])
       .stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = cmd.output().map_err(|e| format!("ffmpeg: {e}"))?;
    if !out.status.success() {
        return Err(format!("ffmpeg decode failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let fsz = w * h * 3;
    if out.stdout.is_empty() { return Err("ffmpeg produced no frames".into()); }
    if out.stdout.len() % fsz != 0 {
        return Err(format!("ffmpeg output {} bytes, not a multiple of the {fsz}-byte frame size", out.stdout.len()));
    }
    Ok(out.stdout.chunks_exact(fsz).map(|c| c.to_vec()).collect())
}

#[derive(Clone, Debug, PartialEq)]
pub struct TranscodeReport {
    pub frames: usize,
    pub w: usize,
    pub h: usize,
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub mean_psnr: f64,
}

/// Probe, decode, encode every frame intra (see the module doc for why), and return a real
/// `.fao` file's bytes plus a report. `progress(done, total)` is called after every frame so a
/// caller on a worker thread can report back to the UI without this module knowing about egui.
pub fn transcode(path: &str, quality: u8, max_frames: Option<usize>,
                  mut progress: impl FnMut(usize, usize)) -> Result<(Vec<u8>, TranscodeReport), String> {
    let info = probe(path)?;
    let input_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let frames = decode_frames_rgb(path, info.w, info.h, max_frames)?;
    let (w, h) = (info.w & !1, info.h & !1);
    let n = frames.len();
    let mut container_frames = Vec::with_capacity(n);
    let mut psnr_sum = 0.0;
    for (i, rgb) in frames.iter().enumerate() {
        let yuv = decode_yuv_from_rgb(rgb, w, h);
        let payload = fao_v1::compress_frame_dct(&yuv, w, h, quality);
        let recon_yuv = fao_v1::decompress_frame_dct(&payload, w, h).unwrap_or_else(|| yuv.clone());
        let recon_rgb = crate::decode::yuv_to_rgb(&recon_yuv, w, h);
        psnr_sum += psnr(rgb, &recon_rgb);
        let plen = payload.len();
        container_frames.push((Vec::new(), payload, b'D', plen, 0.0, 0.0));
        progress(i + 1, n);
    }
    let mut out = Vec::new();
    fao_v1::write_fao_container(&mut out, w, h, info.fps, quality as f32 / 100.0, &container_frames)
        .map_err(|e| format!("writing the container: {e}"))?;
    let report = TranscodeReport {
        frames: n, w, h, input_bytes, output_bytes: out.len() as u64,
        mean_psnr: if n > 0 { psnr_sum / n as f64 } else { 0.0 },
    };
    Ok((out, report))
}

fn decode_yuv_from_rgb(rgb: &[u8], w: usize, h: usize) -> Vec<u8> {
    crate::decode::rgb_to_yuv420p(rgb, w, h)
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let n = a.len().min(b.len()).max(1);
    let mse: f64 = a.iter().zip(b).map(|(x, y)| { let d = *x as f64 - *y as f64; d * d }).sum::<f64>() / n as f64;
    if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    /// `ffmpeg`'s own `testsrc` filter generates a real MP4 with no fixture file needed — a
    /// moving gradient/checkerboard, exactly the kind of synthetic-but-real input this pipeline
    /// should be exercised against before a hand-picked video ever touches it.
    fn make_test_video(path: &std::path::Path, w: usize, h: usize, n: usize, fps: u32) {
        let status = Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi"])
            .arg("-i").arg(format!("testsrc=size={w}x{h}:rate={fps}:duration={:.2}", n as f64 / fps as f64))
            .args(["-frames:v", &n.to_string(), "-pix_fmt", "yuv420p"])
            .arg(path).status().expect("spawn ffmpeg");
        assert!(status.success(), "ffmpeg testsrc generation failed");
    }

    #[test]
    fn probes_a_real_file_it_generated_itself() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-transcode-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("probe.mp4");
        make_test_video(&path, 64, 64, 8, 10);
        let info = probe(path.to_str().unwrap()).expect("probe");
        assert_eq!((info.w, info.h), (64, 64));
        assert!((info.fps - 10.0).abs() < 0.5, "{}", info.fps);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_real_video_file_transcodes_to_a_player_openable_fao_with_the_right_frame_count() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-transcode-test-{}-2", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("in.mp4");
        make_test_video(&path, 64, 48, 6, 12);

        let mut calls = Vec::new();
        let (bytes, report) = transcode(path.to_str().unwrap(), 80, None, |done, total| calls.push((done, total)))
            .expect("transcode");
        assert_eq!(report.frames, 6);
        assert_eq!((report.w, report.h), (64, 48));
        assert!(report.output_bytes > 0);
        assert_eq!(calls.last(), Some(&(6, 6)), "progress should reach done==total");
        assert!(report.mean_psnr > 20.0, "quality 80 intra-DCT should not be unwatchable: {}", report.mean_psnr);

        // The exact loop the Player runs: FaoFile::parse -> decode_rgb for every frame.
        let f = crate::decode::FaoFile::parse(bytes).expect("the transcoded bytes parse as a .fao");
        assert_eq!(f.frame_count(), 6);
        assert_eq!((f.width(), f.height()), (64, 48));
        for i in 0..f.frame_count() {
            let rgb = f.decode_rgb(i).unwrap_or_else(|g| panic!("frame {i}: {g}"));
            assert_eq!(rgb.len(), 64 * 48 * 3);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_max_frames_cap_is_honored() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-transcode-test-{}-3", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("in.mp4");
        make_test_video(&path, 32, 32, 10, 10);
        let (_, report) = transcode(path.to_str().unwrap(), 70, Some(3), |_, _| {}).expect("transcode");
        assert_eq!(report.frames, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_a_clean_error_not_a_panic() {
        if !have_ffmpeg() { return; }
        assert!(probe("/nonexistent/path/that/does/not/exist.mp4").is_err());
        assert!(transcode("/nonexistent/path/that/does/not/exist.mp4", 70, None, |_, _| {}).is_err());
    }

    /// Isolates which stage introduces error, on an actual ffmpeg-decoded frame (not the
    /// synthetic gradient decode.rs's own test uses, which never compares against a true
    /// original — that's how the run-length filler bug fixed alongside this survived).
    #[test]
    fn a_real_frame_survives_dct_round_trip_at_every_quality() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-iso-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("in.mp4");
        make_test_video(&path, 64, 48, 1, 10);
        let frames = decode_frames_rgb(path.to_str().unwrap(), 64, 48, Some(1)).expect("decode");
        let rgb = &frames[0];
        let (w, h) = (64usize, 48usize);
        let yuv = decode_yuv_from_rgb(rgb, w, h);
        for q in [10u8, 50, 80, 95] {
            fao_v1::DEBUG_ESCAPES.store(0, std::sync::atomic::Ordering::Relaxed);
            fao_v1::DEBUG_OVERFLOWS.store(0, std::sync::atomic::Ordering::Relaxed);
            let payload = fao_v1::compress_frame_dct(&yuv, w, h, q);
            let recon_yuv = fao_v1::decompress_frame_dct(&payload, w, h).unwrap();
            eprintln!("DEBUG q={q} payload_len={} escapes={} overflows={}", payload.len(),
                fao_v1::DEBUG_ESCAPES.load(std::sync::atomic::Ordering::Relaxed),
                fao_v1::DEBUG_OVERFLOWS.load(std::sync::atomic::Ordering::Relaxed));
            // per-plane psnr
            let y_sz = w*h; let uv = y_sz/4;
            eprintln!("DEBUG q={q} Yp={:.2} Up={:.2} Vp={:.2}", psnr(&yuv[..y_sz], &recon_yuv[..y_sz]),
                psnr(&yuv[y_sz..y_sz+uv], &recon_yuv[y_sz..y_sz+uv]),
                psnr(&yuv[y_sz+uv..], &recon_yuv[y_sz+uv..]));
            let p = psnr(&yuv, &recon_yuv);
            // Before the filler fix this was a cliff: 23 / 29 / 7.1 / 7.4 dB — worse at HIGHER
            // quality, which is backwards for any real codec. 20 dB is a floor, not a target.
            assert!(p > 20.0, "q={q}: PSNR {p:.2} dB — DCT round trip should not degrade with quality");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

