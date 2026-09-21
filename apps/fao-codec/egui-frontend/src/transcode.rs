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
    /// The rate EXACTLY as the header will store it. From the stream's own
    /// rational where it fits (30000/1001, 144/1), else `fps_ratio(fps)`. Kept as
    /// a rational because the float path is where 144 fps used to wrap to 12.9.
    pub rate: (u16, u16),
    pub duration_s: f64,
    pub codec: String,
    /// ffprobe's absolute stream index -- handed to ffmpeg as `-map 0:N` so the
    /// stream decoded is the stream probed.
    pub stream: usize,
}

/// Parse `ffprobe -show_streams -show_format` JSON.
///
/// Picks the first VIDEO stream that is not cover art. Two traps, both real:
///   * stream 0 is often AUDIO in an OBS capture -- fao_app's `streams[0]` then
///     reads no width and silently substitutes 640x480, slicing every 1080p frame
///     as 640x480 (confirmed on an audio-first MKV);
///   * an MKV's cover art is a `video` stream with `disposition.attached_pic`, so
///     "first video stream" can be a 300x300 thumbnail while ffmpeg decodes the
///     real picture -- a frame-size mismatch on every frame.
/// No video stream is an ERROR, never a default size.
pub fn parse_probe(json: &str) -> Result<VideoInfo, String> {
    let info: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("ffprobe JSON: {e}"))?;
    let streams = info["streams"].as_array().ok_or("no streams")?;
    let v = streams.iter()
        .find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_u64() != Some(1))
        .ok_or("no video stream (only audio or cover art)")?;
    let w = v["width"].as_u64().ok_or("video stream has no width")? as usize;
    let h = v["height"].as_u64().ok_or("video stream has no height")? as usize;
    let stream = v["index"].as_u64().ok_or("video stream has no index")? as usize;
    let rat = v["avg_frame_rate"].as_str().and_then(parse_ratio).filter(|&(n, d)| n > 0 && d > 0)
        .or_else(|| v["r_frame_rate"].as_str().and_then(parse_ratio).filter(|&(n, d)| n > 0 && d > 0))
        .ok_or("no usable frame rate")?;
    let fps = rat.0 as f64 / rat.1 as f64;
    let rate = fit_u16(rat).unwrap_or_else(|| fao_v1::fps_ratio(fps));
    let duration_s = info["format"]["duration"].as_str().and_then(|s| s.parse().ok())
        .or_else(|| v["duration"].as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0);
    let codec = v["codec_name"].as_str().unwrap_or("?").to_string();
    Ok(VideoInfo { w, h, fps, rate, duration_s, codec, stream })
}

/// "30000/1001" -> (30000, 1001), reduced.
fn parse_ratio(s: &str) -> Option<(u64, u64)> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (u64, u64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
    if d == 0 { return None; }
    let g = gcd(n, d).max(1);
    Some((n / g, d / g))
}

fn gcd(a: u64, b: u64) -> u64 { if b == 0 { a } else { gcd(b, a % b) } }

fn fit_u16((n, d): (u64, u64)) -> Option<(u16, u16)> {
    (n <= u16::MAX as u64 && d <= u16::MAX as u64).then(|| (n as u16, d as u16))
}

/// `ffprobe`, then [`parse_probe`].
pub fn probe(path: &str) -> Result<VideoInfo, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "quiet", "-print_format", "json", "-show_streams", "-show_format"])
        .arg(path).output().map_err(|e| format!("ffprobe: {e}"))?;
    if !out.status.success() { return Err("ffprobe failed (not a media file?)".into()); }
    parse_probe(&String::from_utf8_lossy(&out.stdout))
}

/// Index slots to reserve: duration x rate with a quarter's margin plus a
/// floor, since variable-rate captures run over their average. Or exactly the
/// cap, when one is set.
pub fn reserve_for(info: &VideoInfo, max_frames: Option<usize>) -> u64 {
    if let Some(n) = max_frames { return n.max(1) as u64; }
    if info.duration_s > 0.0 && info.fps > 0.0 {
        (info.duration_s * info.fps * 1.25).ceil() as u64 + 1024
    } else {
        // Unknown length: 16 bytes a slot, so 4 MB of index covers ~73 min at 60.
        262_144
    }
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

/// Transcode into any `Write + Seek`, ONE FRAME AT A TIME.
///
/// The old path called `ffmpeg ... .output()`, which buffers ffmpeg's ENTIRE
/// decoded output, then copied it into a Vec per frame, kept every payload for
/// the container, and returned the file as one Vec -- its own doc put a 1080p
/// minute at ~11 GB, so an hour-long match could not run at all. This reads
/// frames off ffmpeg's pipe with `read_exact` and writes each straight into a
/// `FaoStreamWriter`: memory is one frame, whatever the length.
pub fn transcode_to_writer<W: std::io::Write + std::io::Seek>(
    path: &str, out: W, quality: u8, max_frames: Option<usize>,
    progress: impl FnMut(usize, usize),
) -> Result<(W, TranscodeReport), String> {
    transcode_cancellable(path, out, quality, max_frames, progress, None)
}

/// [`transcode_to_writer`], stoppable between frames by setting `cancel`.
///
/// A folder of matches is hours of work; a job that cannot be stopped holds the
/// machine hostage. Checked once per frame, so it stops within one frame's
/// encode, and the partial output is discarded by the caller rather than left
/// behind looking like a finished file.
pub fn transcode_cancellable<W: std::io::Write + std::io::Seek>(
    path: &str, out: W, quality: u8, max_frames: Option<usize>,
    mut progress: impl FnMut(usize, usize), cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(W, TranscodeReport), String> {
    use std::io::Read;
    let info = probe(path)?;
    let input_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let (w, h) = (info.w & !1, info.h & !1);
    if w < 8 || h < 8 { return Err(format!("{w}x{h} is too small to encode")); }
    let reserve = reserve_for(&info, max_frames);
    let estimate = reserve.saturating_sub(1024).max(1) as usize;

    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-i"]).arg(path)
       .args(["-map", &format!("0:{}", info.stream)]);
    if let Some(n) = max_frames { cmd.args(["-frames:v", &n.to_string()]); }
    // CROP an odd edge, never scale: every even size round-trips through this
    // codec unchanged (tested down to 18x16 and 1366x768), and a scale filter
    // would resample every pixel of a frame that needed no change.
    if (w, h) != (info.w, info.h) { cmd.args(["-vf", &format!("crop={w}:{h}:0:0")]); }
    cmd.args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
       .stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("ffmpeg: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no ffmpeg stdout")?;
    // DRAIN stderr on a thread: undrained, a chatty decode fills the pipe and
    // ffmpeg blocks writing to it while we block reading stdout -- deadlock.
    let mut stderr = child.stderr.take().ok_or("no ffmpeg stderr")?;
    let err_thread = std::thread::spawn(move || {
        let mut e = String::new();
        let _ = stderr.read_to_string(&mut e);
        e
    });

    let mut writer = crate::export::FaoStreamWriter::new(out, w, h, info.rate.0, info.rate.1, quality, reserve)
        .map_err(|e| format!("starting the file: {e}"))?;
    let fsz = w * h * 3;
    let mut rgb = vec![0u8; fsz];
    let mut n = 0usize;
    let mut psnr_sum = 0.0;
    let result: Result<(), String> = loop {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            break Err(CANCELLED.into());
        }
        match read_frame(&mut stdout, &mut rgb) {
            Ok(false) => break Ok(()),
            Ok(true) => {}
            Err(e) => break Err(e),
        }
        let yuv = crate::decode::rgb_to_yuv420p(&rgb, w, h);
        let payload = fao_v1::compress_frame_dct(&yuv, w, h, quality);
        let recon = fao_v1::decompress_frame_dct(&payload, w, h).unwrap_or_else(|| yuv.clone());
        let p = psnr(&rgb, &crate::decode::yuv_to_rgb(&recon, w, h));
        psnr_sum += p;
        if let Err(e) = writer.push(&payload, b'D', p) { break Err(format!("writing frame {n}: {e}")); }
        n += 1;
        progress(n, estimate.max(n));
    };
    if result.is_err() { let _ = child.kill(); }
    let status = child.wait().map_err(|e| format!("ffmpeg wait: {e}"))?;
    let errtext = err_thread.join().unwrap_or_default();
    result?;
    if !status.success() {
        return Err(format!("ffmpeg decode failed: {}", errtext.trim()));
    }
    if n == 0 { return Err("ffmpeg produced no frames".into()); }
    progress(n, n);
    let mut out = writer.finish().map_err(|e| format!("finishing the file: {e}"))?;
    let output_bytes = out.stream_position().unwrap_or(0);
    Ok((out, TranscodeReport { frames: n, w, h, input_bytes, output_bytes,
                               mean_psnr: psnr_sum / n as f64 }))
}

/// The error a cancelled transcode returns, so callers can tell it from a failure.
pub const CANCELLED: &str = "cancelled";

/// Read exactly one frame. `Ok(false)` at a clean end; a PARTIAL frame is an
/// error, not a frame padded with the previous one's bytes.
fn read_frame(r: &mut impl std::io::Read, buf: &mut [u8]) -> Result<bool, String> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) => return if got == 0 { Ok(false) }
                            else { Err(format!("ffmpeg stopped mid-frame ({got} of {} bytes)", buf.len())) },
            Ok(k) => got += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(format!("reading ffmpeg: {e}")),
        }
    }
    Ok(true)
}

/// Transcode to a file on disk -- the path the Media tab uses for real matches.
pub fn transcode_to_file(path: &str, out_path: &std::path::Path, quality: u8, max_frames: Option<usize>,
                         progress: impl FnMut(usize, usize)) -> Result<TranscodeReport, String> {
    transcode_to_file_cancellable(path, out_path, quality, max_frames, progress, None)
}

/// [`transcode_to_file`], stoppable. On any error -- cancellation included --
/// the partial file is REMOVED, so a half-written match never sits in the
/// folder looking like a finished one.
pub fn transcode_to_file_cancellable(path: &str, out_path: &std::path::Path, quality: u8,
        max_frames: Option<usize>, progress: impl FnMut(usize, usize),
        cancel: Option<&std::sync::atomic::AtomicBool>) -> Result<TranscodeReport, String> {
    let f = std::fs::File::create(out_path).map_err(|e| format!("creating {}: {e}", out_path.display()))?;
    let r = transcode_cancellable(path, std::io::BufWriter::new(f), quality, max_frames, progress, cancel)
        .map(|(_, rep)| rep);
    if r.is_err() { let _ = std::fs::remove_file(out_path); }
    r
}

/// The in-memory form, for small previews: the SAME streaming loop into a
/// Vec, so there is one frame path, not two.
pub fn transcode(path: &str, quality: u8, max_frames: Option<usize>,
                 progress: impl FnMut(usize, usize)) -> Result<(Vec<u8>, TranscodeReport), String> {
    transcode_to_writer(path, std::io::Cursor::new(Vec::new()), quality, max_frames, progress)
        .map(|(c, r)| (c.into_inner(), r))
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

    /// One raw source frame, for tests only. The public in-memory decoder is gone
    /// -- unbounded, it was how an hour of 1080p became terabytes -- so a test that
    /// needs a single frame asks ffmpeg for exactly one.
    fn first_frame(path: &std::path::Path, w: usize, h: usize) -> Vec<u8> {
        let out = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path)
            .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output().expect("ffmpeg");
        assert_eq!(out.stdout.len(), w * h * 3, "one {w}x{h} frame");
        out.stdout
    }

    // ── the probe traps, pure ──

    fn stream(i: u64, kind: &str, w: u64, h: u64, rate: &str, attached: u64) -> String {
        format!(r#"{{"index":{i},"codec_type":"{kind}","codec_name":"x","width":{w},"height":{h},
                    "avg_frame_rate":"{rate}","disposition":{{"attached_pic":{attached}}}}}"#)
    }

    /// Stream 0 is audio in an OBS capture. fao_app took streams[0] and
    /// substituted 640x480; this must find the video.
    #[test]
    fn an_audio_first_file_probes_its_video_stream() {
        let j = format!(r#"{{"streams":[{{"index":0,"codec_type":"audio"}},{}],"format":{{"duration":"2.0"}}}}"#,
                        stream(1, "video", 1920, 1080, "60/1", 0));
        let v = parse_probe(&j).unwrap();
        assert_eq!((v.w, v.h, v.stream), (1920, 1080, 1));
    }

    /// MKV cover art is a `video` stream with attached_pic. Taking it would size
    /// every frame as a thumbnail while ffmpeg decodes the real picture.
    #[test]
    fn cover_art_is_skipped() {
        let j = format!(r#"{{"streams":[{},{}],"format":{{}}}}"#,
                        stream(0, "video", 300, 300, "0/0", 1), stream(1, "video", 2560, 1440, "144/1", 0));
        let v = parse_probe(&j).unwrap();
        assert_eq!((v.w, v.h, v.stream), (2560, 1440, 1));
    }

    /// No video is an ERROR -- never a made-up size.
    #[test]
    fn audio_only_is_an_error_not_640x480() {
        let j = r#"{"streams":[{"index":0,"codec_type":"audio"}],"format":{}}"#;
        assert!(parse_probe(j).unwrap_err().contains("no video stream"));
    }

    /// The rate stays an exact rational: NTSC and 144 fps both survive.
    #[test]
    fn rates_are_kept_exact() {
        let ntsc = parse_probe(&format!(r#"{{"streams":[{}],"format":{{}}}}"#,
                                        stream(0, "video", 64, 64, "30000/1001", 0))).unwrap();
        assert_eq!(ntsc.rate, (30000, 1001));
        let hi = parse_probe(&format!(r#"{{"streams":[{}],"format":{{}}}}"#,
                                      stream(0, "video", 64, 64, "144/1", 0))).unwrap();
        assert_eq!(hi.rate, (144, 1));
    }

    // ── end to end, real ffmpeg ──

    /// A 144 fps capture keeps 144 fps. Through the old float path it was
    /// written as 12.9 and played eleven times too slow.
    #[test]
    fn a_144fps_capture_keeps_its_rate_on_disk() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-144-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        make_test_video(&src, 64, 48, 12, 144);
        let out = dir.join("out.fao");
        let rep = transcode_to_file(src.to_str().unwrap(), &out, 80, None, |_, _| {}).unwrap();
        assert_eq!(rep.frames, 12);
        let f = crate::decode::FaoFile::open_stream(Box::new(std::fs::File::open(&out).unwrap())).unwrap();
        assert!((f.fps() - 144.0).abs() < 1e-6, "stored as {} fps", f.fps());
        assert_eq!(f.frame_count(), 12);
        assert!(f.decode_rgb(11).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real bug file: audio first. Transcodes at its true size.
    #[test]
    fn an_audio_first_mkv_transcodes_at_its_real_size() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-af-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("af.mkv");
        let ok = Command::new("ffmpeg").args(["-v", "error", "-y",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=1",
            "-f", "lavfi", "-i", "testsrc=size=320x240:rate=30:duration=0.3",
            "-map", "0:a", "-map", "1:v", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
            .arg(&src).status().unwrap().success();
        assert!(ok, "fixture");
        let (bytes, rep) = transcode(src.to_str().unwrap(), 80, None, |_, _| {}).unwrap();
        assert_eq!((rep.w, rep.h), (320, 240), "not the 640x480 fao_app would have guessed");
        let f = crate::decode::FaoFile::parse(bytes).unwrap();
        assert_eq!((f.width(), f.height()), (320, 240));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Cancelling stops the job AND removes the partial file -- a half-written
    /// match must not be left looking like a finished one.
    #[test]
    fn a_cancelled_transcode_leaves_no_partial_file() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        make_test_video(&src, 64, 48, 30, 30);
        let out = dir.join("out.fao");
        let stop = std::sync::atomic::AtomicBool::new(false);
        let r = transcode_to_file_cancellable(src.to_str().unwrap(), &out, 80, None,
            |done, _| if done == 5 { stop.store(true, std::sync::atomic::Ordering::Relaxed) }, Some(&stop));
        assert_eq!(r.unwrap_err(), CANCELLED);
        assert!(!out.exists(), "the partial file must be removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

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
        let rgb = &first_frame(&path, 64, 48);
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

