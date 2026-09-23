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
    /// Of `frames`, how many were exact repeats stored as one-byte `R` frames.
    pub repeats: usize,
    /// Of `frames`, how many were Quiltstone patch frames (`M`).
    pub patches: usize,
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
    let workers = encode_workers();
    let batch_size = (workers * 2).max(1);
    let mut n = 0usize;
    let mut psnr_sum = 0.0;
    let mut psnr_n = 0usize;
    // EXACT REPEATS become `R` frames: a one-byte payload the decoder resolves
    // to the previous real frame. Game captures are full of them -- a 30 fps
    // game recorded into a 60 fps file repeats every other frame; pauses,
    // menus and loading screens repeat for seconds -- and each was being
    // re-encoded in full. Detection is sequential (each frame against the one
    // before), so it runs BEFORE the parallel encode, which then only sees the
    // frames that differ.
    let mut prev: Option<Vec<u8>> = None;
    let mut run = 0usize;
    let mut repeats = 0usize;
    // QUILTSTONE PATCH FRAMES between keyframes (see `patch`): each block is
    // judged against the source pixels it had when last SENT (`reference`), so
    // skipping cannot accumulate error. `since_key` bounds seek cost.
    let tol = crate::patch::tolerance(quality);
    let n_blocks = { let (c, r) = crate::patch::grid(w, h); c * r };
    let mut reference: Option<Vec<u8>> = None;
    let mut since_key = 0usize;
    let mut patches = 0usize;
    // BATCHED, PARALLEL. Every frame here is an intra DCT frame, so frames are
    // independent -- the ideal case for encoding several at once. A batch of
    // `2 x workers` frames is read, encoded across `workers` threads, then
    // written IN ORDER (each worker fills fixed slots; see `encode_batch`).
    // Cancellation is checked per batch, so it lands within one batch.
    let result: Result<(), String> = loop {
        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
            break Err(CANCELLED.into());
        }
        let (mut batch, ended) = match read_batch(&mut stdout, fsz, batch_size) {
            Ok(b) => b,
            Err(e) => break Err(e),
        };
        if batch.is_empty() {
            break Ok(());
        }
        let (dups, carried) = mark_repeats(&batch, prev.as_deref(), run);
        run = carried;
        // PLAN, sequentially and cheaply: each non-repeat frame becomes a
        // keyframe or a patch of its changed blocks. The expensive compression
        // below only needs each job's own frame, so it stays parallel.
        let mut jobs: Vec<Job> = Vec::new();
        for (f, dup) in batch.iter().zip(&dups) {
            if *dup {
                since_key += 1;
                continue;
            }
            let job = match reference.as_mut() {
                Some(r) if since_key < KEY_EVERY => {
                    let blocks = crate::patch::changed_blocks(f, r, w, h, tol);
                    if blocks.len() * 100 > n_blocks * KEY_IF_CHANGED_PCT {
                        None
                    } else {
                        crate::patch::update_reference(r, f, w, h, &blocks);
                        Some(Job::Patch(f.as_slice(), blocks))
                    }
                }
                _ => None,
            };
            match job {
                Some(j) => { since_key += 1; patches += 1; jobs.push(j); }
                None => {
                    reference = Some(f.clone());
                    since_key = 1;
                    jobs.push(Job::Key(f.as_slice()));
                }
            }
        }
        let mut encoded = encode_jobs(&jobs, n, w, h, quality, workers, PSNR_EVERY).into_iter();
        let mut failed = None;
        for dup in &dups {
            let (payload, p) = if *dup {
                repeats += 1;
                (vec![b'R'], None)
            } else {
                encoded.next().expect("one encoded result per fresh frame")
            };
            if let Some(p) = p {
                psnr_sum += p;
                psnr_n += 1;
            }
            // 0 = "not measured" for unsampled frames. Nothing reads the
            // per-frame header PSNR back from a file (checked: every PSNR shown
            // in the UI comes from an in-memory encode result).
            if let Err(e) = writer.push(&payload, b'D', p.unwrap_or(0.0)) {
                failed = Some(format!("writing frame {n}: {e}"));
                break;
            }
            n += 1;
            progress(n, estimate.max(n));
        }
        prev = batch.pop();
        if let Some(e) = failed {
            break Err(e);
        }
        if ended {
            break Ok(());
        }
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
    Ok((out, TranscodeReport { frames: n, repeats, patches, w, h, input_bytes, output_bytes,
                               mean_psnr: if psnr_n == 0 { 0.0 } else { psnr_sum / psnr_n as f64 } }))
}

/// The error a cancelled transcode returns, so callers can tell it from a failure.
pub const CANCELLED: &str = "cancelled";

/// Longest run of repeat (`R`) frames before a real frame is re-encoded.
/// The decoder walks back from an `R` to the last real frame but only up to
/// `decode::MAX_R_WALKBACK` (600) frames, then reports `RepeatUnresolved` -- so
/// a long static stretch (a paused game) must still get a real frame now and
/// then. 300 keeps well inside that: 5 s at 60 fps.
pub const MAX_REPEAT_RUN: usize = 300;

/// Which frames of `batch` are exact repeats of the frame before them, given
/// the last frame of the previous batch (`prev`) and the repeat run so far.
/// Returns the flags and the run length carried out of the batch. Pure.
fn mark_repeats(batch: &[Vec<u8>], prev: Option<&[u8]>, mut run: usize) -> (Vec<bool>, usize) {
    let mut out = Vec::with_capacity(batch.len());
    let mut before = prev;
    for f in batch {
        let dup = before == Some(f.as_slice()) && run < MAX_REPEAT_RUN;
        run = if dup { run + 1 } else { 0 };
        out.push(dup);
        before = Some(f.as_slice());
    }
    (out, run)
}

/// A keyframe at least this often, so seeking into a run decodes at most this
/// many (cheap) patch frames forward from its keyframe.
pub const KEY_EVERY: usize = 60;
/// Past this share of blocks changed, a patch saves nothing: send a keyframe,
/// which also resets every block's reference exactly.
pub const KEY_IF_CHANGED_PCT: usize = 70;

/// One frame's encoding work.
pub(crate) enum Job<'a> {
    Key(&'a [u8]),
    Patch(&'a [u8], Vec<usize>),
}

/// Measure PSNR on one frame in this many. Measuring means DECODING the frame
/// again and comparing -- pure overhead in a batch encode, only there for the
/// report. Sampled, the report's mean is an estimate; frame 0 is always in it.
const PSNR_EVERY: usize = 8;

/// Encode threads: the machine's parallelism, capped -- beyond 8 the batch's
/// memory (two raw frames per worker) grows faster than the gain.
fn encode_workers() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 8)
}

/// Read up to `max` frames. `(frames, ended)`: `ended` when ffmpeg's output
/// finished within this batch.
fn read_batch(r: &mut impl std::io::Read, fsz: usize, max: usize) -> Result<(Vec<Vec<u8>>, bool), String> {
    let mut batch = Vec::with_capacity(max);
    while batch.len() < max {
        let mut buf = vec![0u8; fsz];
        if !read_frame(r, &mut buf)? {
            return Ok((batch, true));
        }
        batch.push(buf);
    }
    Ok((batch, false))
}

/// One frame: its DCT payload, and its PSNR if this frame is sampled.
fn encode_one(rgb: &[u8], w: usize, h: usize, quality: u8, measure: bool) -> (Vec<u8>, Option<f64>) {
    let yuv = crate::decode::rgb_to_yuv420p(rgb, w, h);
    let payload = fao_v1::compress_frame_dct(&yuv, w, h, quality);
    let p = measure.then(|| {
        let recon = fao_v1::decompress_frame_dct(&payload, w, h).unwrap_or_else(|| yuv.clone());
        psnr(rgb, &crate::decode::yuv_to_rgb(&recon, w, h))
    });
    (payload, p)
}

/// Encode a batch across `workers` threads, results IN INPUT ORDER.
///
/// Each worker takes a CONTIGUOUS run of frames and writes into the matching
/// run of output slots, so order is fixed by index, not by which thread
/// finishes first -- the output is byte-identical to encoding one at a time
/// (tested). `first` is the batch's first frame number, for PSNR sampling.
fn encode_batch<T: AsRef<[u8]> + Sync>(batch: &[T], first: usize, w: usize, h: usize, quality: u8,
                workers: usize, psnr_every: usize) -> Vec<(Vec<u8>, Option<f64>)> {
    let jobs: Vec<Job> = batch.iter().map(|f| Job::Key(f.as_ref())).collect();
    encode_jobs(&jobs, first, w, h, quality, workers, psnr_every)
}

/// One job: a keyframe (PSNR if sampled) or a patch frame (never measured --
/// its quality is the keyframe path's, on fewer blocks).
fn encode_job(job: &Job, w: usize, h: usize, quality: u8, measure: bool) -> (Vec<u8>, Option<f64>) {
    match job {
        Job::Key(f) => encode_one(f, w, h, quality, measure),
        Job::Patch(f, blocks) => (crate::patch::encode(f, w, h, blocks, quality), None),
    }
}

/// `encode_batch` over mixed jobs: same contiguous-run, fixed-slot scheme, so
/// results come back in job order whichever thread finishes first.
fn encode_jobs(jobs: &[Job], first: usize, w: usize, h: usize, quality: u8,
               workers: usize, psnr_every: usize) -> Vec<(Vec<u8>, Option<f64>)> {
    let every = psnr_every.max(1);
    if workers <= 1 || jobs.len() <= 1 {
        return jobs.iter().enumerate()
            .map(|(k, j)| encode_job(j, w, h, quality, (first + k) % every == 0))
            .collect();
    }
    let chunk = jobs.len().div_ceil(workers);
    let mut out: Vec<Option<(Vec<u8>, Option<f64>)>> = (0..jobs.len()).map(|_| None).collect();
    std::thread::scope(|sc| {
        for (ci, (js, slots)) in jobs.chunks(chunk).zip(out.chunks_mut(chunk)).enumerate() {
            sc.spawn(move || {
                for (k, (j, slot)) in js.iter().zip(slots.iter_mut()).enumerate() {
                    let idx = first + ci * chunk + k;
                    *slot = Some(encode_job(j, w, h, quality, idx % every == 0));
                }
            });
        }
    });
    out.into_iter().map(|o| o.expect("every slot is filled by its worker")).collect()
}

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
            "-map", "0:a", "-map", "1:v", "-c:v", "mpeg4", "-pix_fmt", "yuv420p", "-c:a", "flac"])
            // Native encoders only: libx264 is absent from Fedora's ffmpeg-free, and
            // flac sidesteps whether a build has AAC. The test needs only an audio
            // stream FIRST, not a particular codec.
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

    /// BENCHMARK, not a check: 1080p60 transcode speed and size, for sizing a
    /// real batch ("hundreds of GB of matches"). Ignored by default; run in
    /// release, since debug speed says nothing:
    ///
    ///     cargo test --release -- --ignored --nocapture bench_1080p
    ///
    /// The source is ffmpeg's synthetic `testsrc2`, encoded with whatever H.264
    /// or MPEG-4 encoder this ffmpeg has -- so the SPEED is representative (the
    /// DCT path's cost is per block, not per content), but the SIZE RATIO is
    /// only indicative: synthetic motion compresses differently from gameplay.
    #[test]
    #[ignore = "benchmark: cargo test --release -- --ignored --nocapture bench_1080p"]
    fn bench_1080p_transcode_speed_and_size() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-bench-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mp4");
        // No -c:v: ffmpeg picks its default (libx264 where present, mpeg4 on
        // builds without it, e.g. Fedora's ffmpeg-free) -- works everywhere.
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i",
            "testsrc2=size=1920x1080:rate=60:duration=2", "-pix_fmt", "yuv420p"])
            .arg(&src).status().unwrap().success());
        let out = dir.join("out.fao");
        let t = std::time::Instant::now();
        let rep = transcode_to_file(src.to_str().unwrap(), &out, 80, None, |_, _| {}).unwrap();
        let secs = t.elapsed().as_secs_f64();
        let in_b = std::fs::metadata(&src).unwrap().len();
        eprintln!("BENCH 1080p60: {} frames in {:.2} s = {:.1} fps ({:.2}x realtime); \
                   source {:.2} MB -> fao {:.2} MB ({:.1}x); mean PSNR {:.1} dB",
                  rep.frames, secs, rep.frames as f64 / secs, (rep.frames as f64 / 60.0) / secs,
                  in_b as f64 / 1e6, rep.output_bytes as f64 / 1e6,
                  rep.output_bytes as f64 / in_b as f64, rep.mean_psnr);
        assert_eq!(rep.frames, 120);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn frames(n: usize, w: usize, h: usize) -> Vec<Vec<u8>> {
        (0..n).map(|k| (0..w * h * 3).map(|i| ((i * 13 + k * 57 + (i / w) * 3) % 256) as u8).collect()).collect()
    }

    /// PARALLEL IS BYTE-IDENTICAL TO SEQUENTIAL. 7 frames over 4 workers gives
    /// uneven runs (2,2,2,1), and the batch starts at frame 5 rather than 0 --
    /// the cases where a slot-indexing mistake would reorder or mis-sample.
    #[test]
    fn a_parallel_batch_matches_one_at_a_time_exactly() {
        let (w, h) = (48, 32);
        let batch = frames(7, w, h);
        let seq = encode_batch(&batch, 5, w, h, 80, 1, 3);
        for workers in [2, 3, 4, 8] {
            let par = encode_batch(&batch, 5, w, h, 80, workers, 3);
            assert_eq!(par.len(), seq.len());
            for (i, (a, b)) in par.iter().zip(&seq).enumerate() {
                assert_eq!(a.0, b.0, "payload {i} with {workers} workers");
                assert_eq!(a.1.is_some(), b.1.is_some(), "sampling {i} with {workers} workers");
            }
        }
    }

    /// Frames sampled for PSNR are exactly those whose GLOBAL index is a
    /// multiple of `every` -- so batch boundaries do not shift the sample.
    #[test]
    fn psnr_is_sampled_by_global_frame_index() {
        let (w, h) = (32, 16);
        let got: Vec<bool> = encode_batch(&frames(6, w, h), 5, w, h, 80, 3, 4)
            .iter().map(|(_, p)| p.is_some()).collect();
        // Global indices 5..=10; multiples of 4 there: 8 only.
        assert_eq!(got, vec![false, false, false, true, false, false]);
        let first: Vec<bool> = encode_batch(&frames(3, w, h), 0, w, h, 80, 2, 8)
            .iter().map(|(_, p)| p.is_some()).collect();
        assert_eq!(first, vec![true, false, false], "frame 0 is always measured");
    }

    #[test]
    fn a_batch_reader_reports_where_the_stream_ends() {
        let data = vec![7u8; 10 * 3];
        let mut r = std::io::Cursor::new(data);
        let (b, ended) = read_batch(&mut r, 10, 2).unwrap();
        assert_eq!((b.len(), ended), (2, false));
        let (b, ended) = read_batch(&mut r, 10, 2).unwrap();
        assert_eq!((b.len(), ended), (1, true), "one frame left, then the end");
    }

    #[test]
    fn repeats_are_marked_against_the_previous_frame_across_batches() {
        let (a, b) = (vec![1u8; 4], vec![2u8; 4]);
        // prev = a: [a, a, b, b, a] -> repeats at 0,1 (vs prev / vs a), 3 (b after b).
        let (d, run) = mark_repeats(&[a.clone(), a.clone(), b.clone(), b.clone(), a.clone()], Some(&a), 0);
        assert_eq!(d, vec![true, true, false, true, false]);
        assert_eq!(run, 0, "the last frame differed, so no run carries out");
        let (d, _) = mark_repeats(&[a.clone()], None, 0);
        assert_eq!(d, vec![false], "the first frame of a file is never a repeat");
    }

    /// The run cap: at MAX_REPEAT_RUN repeats in a row, the next identical
    /// frame is encoded for real, and the run restarts -- including across a
    /// batch boundary, via the carried run length.
    #[test]
    fn a_long_run_of_repeats_is_broken_by_a_real_frame() {
        let a = vec![9u8; 4];
        let (d, run) = mark_repeats(&vec![a.clone(); 3], Some(&a), MAX_REPEAT_RUN - 2);
        assert_eq!(d, vec![true, true, false], "two more repeats reach the cap; the third is real");
        assert_eq!(run, 0);
    }

    fn make(path: &std::path::Path, args: &[&str]) {
        let ok = Command::new("ffmpeg").args(["-v", "error", "-y"]).args(args).arg(path).status().unwrap().success();
        assert!(ok, "fixture {}", path.display());
    }

    /// REAL: a 30 fps clip stored at 60 fps -- every other frame a duplicate,
    /// exactly what a 30 fps game recorded at 60 looks like. Lossless (ffv1),
    /// so duplicates are byte-identical after decoding. Exactly the frames
    /// ffmpeg's own hashes call repeats become one-byte R frames, and every frame still decodes, each repeat to
    /// the same picture as the frame before it.
    #[test]
    fn a_30fps_clip_at_60fps_stores_its_duplicates_as_repeats() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-rep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mkv");
        make(&src, &["-f", "lavfi", "-i", "testsrc2=size=64x48:rate=30:duration=1", "-r", "60", "-c:v", "ffv1"]);
        let out = dir.join("out.fao");
        let rep = transcode_to_file(src.to_str().unwrap(), &out, 80, None, |_, _| {}).unwrap();
        assert_eq!(rep.frames, 60);
        // GROUND TRUTH FROM FFMPEG, not a guess. A first version asserted
        // "about half" (~30) and got 47 -- and ffmpeg's own per-frame hashes
        // agreed: at 64x48 testsrc2's slow motion collapses 30 source frames to
        // 13 distinct pictures. So: hash every decoded frame in the SAME format
        // the transcoder compares (rgb24) and count consecutive equal hashes.
        let hashes = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&src)
            .args(["-pix_fmt", "rgb24", "-f", "framemd5", "-"]).output().unwrap();
        let hs: Vec<String> = String::from_utf8_lossy(&hashes.stdout).lines()
            .filter(|l| !l.starts_with('#')).map(|l| l.rsplit(',').next().unwrap().trim().to_string()).collect();
        assert_eq!(hs.len(), 60);
        let expected = hs.windows(2).filter(|w| w[0] == w[1]).count();
        assert!(expected >= 30, "the fixture must actually contain duplicates: {expected}");
        assert_eq!(rep.repeats, expected, "exactly the frames ffmpeg says repeat");
        let f = crate::decode::FaoFile::parse(std::fs::read(&out).unwrap()).unwrap();
        let mut repeats_seen = 0;
        for i in 0..f.frame_count() {
            let px = f.decode_rgb(i).expect("every frame decodes");
            if f.marker(i) == Some(b'R') {
                repeats_seen += 1;
                assert_eq!(px, f.decode_rgb(i - 1).unwrap(), "repeat {i} shows the frame before it");
            }
        }
        assert_eq!(repeats_seen, rep.repeats);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REAL: a static picture held for 320 frames -- a paused game. Real frames
    /// at 0 and 301 (the cap), repeats between, and the LAST frame still decodes:
    /// its walk back is 18 frames, far inside the decoder's 600.
    #[test]
    fn a_long_static_stretch_stays_decodable_to_the_end() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-static-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("in.mkv");
        make(&src, &["-f", "lavfi", "-i", "color=c=0x336699:size=64x48:rate=60", "-frames:v", "320", "-c:v", "ffv1"]);
        let out = dir.join("out.fao");
        let rep = transcode_to_file(src.to_str().unwrap(), &out, 80, None, |_, _| {}).unwrap();
        assert_eq!((rep.frames, rep.repeats), (320, 318), "real frames at 0 and at the cap");
        let f = crate::decode::FaoFile::parse(std::fs::read(&out).unwrap()).unwrap();
        assert_ne!(f.marker(MAX_REPEAT_RUN + 1), Some(b'R'), "a real frame at the cap");
        assert_eq!(f.decode_rgb(319).unwrap(), f.decode_rgb(0).unwrap(), "the last frame decodes to the picture");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A DETAILED static background with a small moving, animating box -- the
    /// shape of game footage patches are for (a textured scene and HUD that sit
    /// still, a part that moves). Lossless (ffv1), so decoded frames can be
    /// compared with the true source.
    ///
    /// DETAILED ON PURPOSE. A first version used a flat grey background, and
    /// patches came out LARGER than whole keyframes (~1,350 vs 1,125 bytes):
    /// in a DCT codec flat blocks are nearly free, so a keyframe of a flat
    /// scene is already mostly the moving box, and a patch carries that same
    /// detail plus its own fixed overhead. Patches win where keyframes keep
    /// paying for STATIC DETAIL -- `geq` draws a pattern that is a pure
    /// function of position: detailed, and truly static.
    fn moving_box_source(dir: &std::path::Path) -> (std::path::PathBuf, Vec<Vec<u8>>, usize, usize) {
        let (w, h) = (160usize, 96usize);
        let src = dir.join("box.mkv");
        make(&src, &["-f", "lavfi", "-i",
            "nullsrc=s=160x96:r=30,geq=r='mod(X*7+Y*3,256)':g='mod(X*3+Y*11,256)':b='mod(X*13+Y*5,256)'[bg];\
             testsrc2=s=32x32:r=30[fg];[bg][fg]overlay=x='t*30':y=30",
            "-t", "3", "-c:v", "ffv1"]);
        let raw = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&src)
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]).output().unwrap().stdout;
        let frames: Vec<Vec<u8>> = raw.chunks(w * h * 3).map(|c| c.to_vec()).collect();
        (src, frames, w, h)
    }

    /// PATCHES ARE USED, AND ARE SMALL: most frames become patch frames, and
    /// the file is a fraction of what keyframes alone would take.
    #[test]
    fn a_mostly_static_scene_becomes_small_patch_frames() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-patch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (src, frames, w, h) = moving_box_source(&dir);
        let out = dir.join("out.fao");
        let rep = transcode_to_file(src.to_str().unwrap(), &out, 80, None, |_, _| {}).unwrap();
        assert_eq!(rep.frames, frames.len());
        let keys = rep.frames - rep.repeats - rep.patches;
        assert!(keys >= 2 && keys <= 3, "a keyframe per {KEY_EVERY} frames over 90: {keys}");
        // Everything else is a patch or a REPEAT -- and this source has exact
        // repeats (the overlay's frame sync duplicates frames). A first version
        // asserted 80+ patches and got 54; ffmpeg's framemd5 counts 34 repeats,
        // and 54 + 34 + 2 keyframes = 90. So: repeats must match ffmpeg's own
        // count, and every other non-keyframe must be a patch.
        let hashes = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&src)
            .args(["-pix_fmt", "rgb24", "-f", "framemd5", "-"]).output().unwrap();
        let hs: Vec<String> = String::from_utf8_lossy(&hashes.stdout).lines()
            .filter(|l| !l.starts_with('#')).map(|l| l.rsplit(',').next().unwrap().trim().to_string()).collect();
        let expected_repeats = hs.windows(2).filter(|w| w[0] == w[1]).count();
        assert_eq!(rep.repeats, expected_repeats, "repeats per ffmpeg");
        assert_eq!(rep.patches, rep.frames - rep.repeats - keys, "every other non-keyframe is a patch");
        assert!(rep.patches >= 40, "and there are plenty of them: {}", rep.patches);
        let f = crate::decode::FaoFile::parse(std::fs::read(&out).unwrap()).unwrap();
        let key_bytes = f.payload(0).unwrap().len();
        let all_keys = key_bytes * rep.frames;
        assert!((rep.output_bytes as usize) * 3 < all_keys,
                "patches must cut the file well below keyframes-only: {} vs ~{}", rep.output_bytes, all_keys);
        let _ = (w, h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// NO DRIFT: every decoded frame stays close to its TRUE source, and the
    /// last frame before a keyframe is no worse than the first after one. A
    /// codec predicting from the previous SOURCE would degrade across the run.
    #[test]
    fn patch_frames_do_not_drift_from_the_source() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-drift-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (src, frames, w, h) = moving_box_source(&dir);
        let out = dir.join("out.fao");
        transcode_to_file(src.to_str().unwrap(), &out, 80, None, |_, _| {}).unwrap();
        let f = crate::decode::FaoFile::parse(std::fs::read(&out).unwrap()).unwrap();
        let ps: Vec<f64> = (0..f.frame_count()).map(|i| psnr(&frames[i], &f.decode_rgb(i).unwrap())).collect();
        let worst = ps.iter().cloned().fold(f64::INFINITY, f64::min);
        // RELATIVE TO THE KEYFRAME, not an absolute floor: this pattern is hard
        // for 8x8 DCT, so a keyframe itself measures ~26 dB. Measured: key 26.08,
        // frame 30 26.03, frame 59 26.00, worst 25.90 -- flat across the run.
        let key = ps[0];
        assert!(worst > key - 1.0, "no frame much worse than a keyframe: key {key:.2}, worst {worst:.2} dB");
        let (first, last) = (ps[1], ps[KEY_EVERY - 1]);
        assert!(last > first - 1.0, "no degradation across a run: frame 1 {first:.2} dB, frame {} {last:.2} dB", KEY_EVERY - 1);
        let _ = (w, h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// THE TEST THAT SEPARATES THE DESIGNS: a slow fade, one level per frame --
    /// under the skip tolerance (2 at quality 80). Judging each block against
    /// the PREVIOUS frame would skip every step and the error would climb a
    /// level per frame, silently, for the whole run. Judging it against what was
    /// LAST SENT resends after a few frames, so error stays bounded.
    #[test]
    fn a_slow_fade_under_the_tolerance_does_not_accumulate() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-fade-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (64usize, 48usize);
        let src = dir.join("fade.mkv");
        make(&src, &["-f", "lavfi", "-i", "nullsrc=s=64x48:r=30,geq=r='60+N':g='60+N':b='60+N'",
                     "-frames:v", "59", "-c:v", "ffv1"]);
        let raw = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&src)
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"]).output().unwrap().stdout;
        let frames: Vec<&[u8]> = raw.chunks(w * h * 3).collect();
        let out = dir.join("out.fao");
        transcode_to_file(src.to_str().unwrap(), &out, 80, None, |_, _| {}).unwrap();
        let f = crate::decode::FaoFile::parse(std::fs::read(&out).unwrap()).unwrap();
        let last = f.frame_count() - 1;
        let err = f.decode_rgb(last).unwrap().iter().zip(frames[last]).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(err <= 6, "the fade's last frame is within a few levels of its source, not {last} behind: max error {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SEEKING into a run -- a fresh reader asked for frame 45 first -- decodes
    /// the same picture as playing 0..=45 in order.
    #[test]
    fn seeking_into_a_run_matches_playing_up_to_it() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-seek-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (src, _frames, _w, _h) = moving_box_source(&dir);
        let out = dir.join("out.fao");
        transcode_to_file(src.to_str().unwrap(), &out, 80, None, |_, _| {}).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let played = crate::decode::FaoFile::parse(bytes.clone()).unwrap();
        let mut last = Vec::new();
        for i in 0..=45 { last = played.decode_rgb(i).unwrap(); }
        let seeked = crate::decode::FaoFile::parse(bytes).unwrap();
        assert_eq!(seeked.marker(45), Some(crate::patch::MARKER), "frame 45 is a patch, so the seek walks back");
        assert_eq!(seeked.decode_rgb(45).unwrap(), last);
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

