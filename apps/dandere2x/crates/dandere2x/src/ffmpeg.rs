//! Video I/O through ffmpeg subprocesses.
//!
//! In: `ffmpeg -i src -f rawvideo -pix_fmt rgb24 -` (AV1 decodes via
//! libdav1d, whatever the container). Out: rawvideo rgb24 on stdin →
//! libsvtav1 10-bit, audio/subtitles/metadata mapped through from the
//! source, keyframes forced onto the source's own keyframe times so the
//! output GOPs line up with the real cuts. Colour tags are copied from the
//! probe so BT.709/2020 never drift through the RGB round trip.
//!
//! Precision: the engine is 8-bit RGB. A 10-bit source is dithered to 8 on
//! decode (swscale's default error-diffusion); the output is 10-bit AV1
//! carrying 8-bit-precision content. Real-ESRGAN is 8-bit-trained, so the
//! CNN wouldn't gain from more; only the reused blocks would.

use anyhow::{anyhow, Context};
use d2x_core::frame::Frame;
use d2x_core::pipeline::{FrameSink, FrameSource, SourceInfo};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

#[derive(Debug, Clone, Default)]
pub struct ColorTags {
    pub primaries: Option<String>,
    pub transfer: Option<String>,
    pub space: Option<String>,
    pub range: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub info: SourceInfo,
    pub codec: String,
    pub pix_fmt: String,
    pub color: ColorTags,
    pub duration_s: Option<f64>,
}

fn parse_rate(s: &str) -> Option<(u32, u32)> {
    let (n, d) = s.split_once('/')?;
    let (n, d): (u32, u32) = (n.parse().ok()?, d.parse().ok()?);
    if d == 0 || n == 0 {
        None
    } else {
        Some((n, d))
    }
}

pub fn probe(path: &Path) -> anyhow::Result<Probe> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,r_frame_rate,avg_frame_rate,nb_frames,pix_fmt,codec_name,color_primaries,color_transfer,color_space,color_range:format=duration", "-of", "json"])
        .arg(path)
        .output()
        .context("running ffprobe (is ffmpeg installed?)")?;
    anyhow::ensure!(out.status.success(), "ffprobe failed: {}", String::from_utf8_lossy(&out.stderr));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let s = v.get("streams").and_then(|s| s.get(0)).ok_or_else(|| anyhow!("{}: no video stream", path.display()))?;
    let get = |k: &str| s.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
    let width = s.get("width").and_then(|x| x.as_u64()).ok_or_else(|| anyhow!("no width"))? as u32;
    let height = s.get("height").and_then(|x| x.as_u64()).ok_or_else(|| anyhow!("no height"))? as u32;
    let fps = get("avg_frame_rate").and_then(|r| parse_rate(&r)).or_else(|| get("r_frame_rate").and_then(|r| parse_rate(&r))).unwrap_or((25, 1));
    let duration_s = v.get("format").and_then(|f| f.get("duration")).and_then(|d| d.as_str()).and_then(|d| d.parse::<f64>().ok());
    let nb = get("nb_frames").and_then(|n| n.parse::<u64>().ok()).or_else(|| duration_s.map(|d| (d * fps.0 as f64 / fps.1 as f64).round() as u64));
    let tagged = |k: &str| get(k).filter(|x| x != "unknown" && !x.is_empty());
    Ok(Probe {
        info: SourceInfo { width, height, fps, frame_count: nb },
        codec: get("codec_name").unwrap_or_default(),
        pix_fmt: get("pix_fmt").unwrap_or_default(),
        color: ColorTags { primaries: tagged("color_primaries"), transfer: tagged("color_transfer"), space: tagged("color_space"), range: tagged("color_range") },
        duration_s,
    })
}

/// Presentation times of the source's keyframes (decoder skips everything
/// else, so this is fast even on long files).
pub fn keyframe_times(path: &Path) -> anyhow::Result<Vec<f64>> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-skip_frame", "nokey", "-show_entries", "frame=pts_time", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .context("ffprobe keyframes")?;
    anyhow::ensure!(out.status.success(), "ffprobe keyframes failed: {}", String::from_utf8_lossy(&out.stderr));
    Ok(String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().trim_end_matches(',').parse::<f64>().ok()).collect())
}

pub struct FfmpegSource {
    child: Child,
    stdout: ChildStdout,
    info: SourceInfo,
    frame_bytes: usize,
}

/// A closed segment of the source: `[start, end)` in seconds, both on
/// keyframes (or the clip's ends).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub index: usize,
    pub start: f64,
    pub end: Option<f64>,
}

/// Cut a clip into `n` segments of roughly equal length on keyframe times.
pub fn segments(keyframes: &[f64], duration: Option<f64>, n: usize) -> Vec<Segment> {
    let n = n.max(1);
    if keyframes.len() < 2 || n == 1 {
        return vec![Segment { index: 0, start: 0.0, end: None }];
    }
    let total = duration.unwrap_or(*keyframes.last().unwrap());
    let mut cuts = vec![0.0];
    for k in 1..n {
        let target = total * k as f64 / n as f64;
        // nearest keyframe strictly after the previous cut
        let prev = *cuts.last().unwrap();
        if let Some(&t) = keyframes.iter().filter(|&&t| t > prev + 0.5).min_by(|a, b| (*a - target).abs().partial_cmp(&(*b - target).abs()).unwrap()) {
            if t > prev {
                cuts.push(t);
            }
        }
    }
    cuts.dedup();
    cuts.iter().enumerate().map(|(i, &start)| Segment { index: i, start, end: cuts.get(i + 1).copied() }).collect()
}

impl FfmpegSource {
    pub fn open(path: &Path, p: &Probe) -> anyhow::Result<Self> {
        Self::open_segment(path, p, Segment { index: 0, start: 0.0, end: None })
    }

    /// Decode only `[seg.start, seg.end)`. Input-side `-ss` on a keyframe
    /// time is exact; `-to` is relative to the seek.
    pub fn open_segment(path: &Path, p: &Probe, seg: Segment) -> anyhow::Result<Self> {
        let mut vf = String::from("format=rgb24");
        if let Some(space) = &p.color.space {
            let range = p.color.range.as_deref().unwrap_or("tv");
            vf = format!("scale=in_color_matrix={space}:in_range={range},format=rgb24");
        }
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if seg.start > 0.0 {
            cmd.args(["-ss", &format!("{:.6}", seg.start)]);
        }
        cmd.arg("-i").arg(path);
        if let Some(end) = seg.end {
            cmd.args(["-to", &format!("{:.6}", end - seg.start)]);
        }
        let mut child = cmd
            .args(["-map", "0:v:0", "-vf", &vf, "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("spawning ffmpeg decoder")?;
        let stdout = child.stdout.take().unwrap();
        let mut info = p.info;
        if seg.end.is_some() || seg.start > 0.0 {
            let end = seg.end.or(p.duration_s);
            info.frame_count = end.map(|e| ((e - seg.start) * p.info.fps.0 as f64 / p.info.fps.1 as f64).round() as u64);
        }
        Ok(FfmpegSource { child, stdout, info, frame_bytes: (p.info.width as usize) * (p.info.height as usize) * 3 })
    }
}

impl FrameSource for FfmpegSource {
    fn info(&self) -> SourceInfo {
        self.info
    }
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        let mut buf = vec![0u8; self.frame_bytes];
        let mut got = 0;
        while got < self.frame_bytes {
            let n = self.stdout.read(&mut buf[got..])?;
            if n == 0 {
                break;
            }
            got += n;
        }
        if got == 0 {
            let st = self.child.wait()?;
            anyhow::ensure!(st.success(), "ffmpeg decoder exited with {st}");
            return Ok(None);
        }
        anyhow::ensure!(got == self.frame_bytes, "short frame from ffmpeg: {got} of {} bytes", self.frame_bytes);
        Ok(Some(Frame::from_rgb(self.info.width, self.info.height, buf)?))
    }
}

impl Drop for FfmpegSource {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug, Clone)]
pub struct EncodeOptions {
    /// `av1` (libsvtav1) or `x264`/`x265` for comparison.
    pub codec: String,
    pub crf: u32,
    /// SVT-AV1 preset 0..13 (lower = slower/better); x264/x265 preset names map from it.
    pub preset: u32,
    pub ten_bit: bool,
    /// Force keyframes at these times (seconds). Honoured by x264/x265 and by
    /// libsvtav1 only on ffmpeg ≥ 7; older builds ignore it, which is why
    /// `keyint` is also set from the source cadence (see `job::keyint_for`).
    pub keyframes: Vec<f64>,
    /// Extra `-svtav1-params` entries.
    pub svt_params: Vec<String>,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        EncodeOptions { codec: "av1".into(), crf: 24, preset: 6, ten_bit: true, keyframes: Vec::new(), svt_params: vec!["tune=0".into(), "enable-overlays=1".into(), "scd=1".into()] }
    }
}

pub struct FfmpegSink {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl FfmpegSink {
    /// `src` is the original file, for audio/subtitle/metadata mapping;
    /// `None` writes a video-only part (segment parallelism muxes audio at
    /// the end).
    pub fn open(out: &Path, src: Option<&Path>, p: &Probe, width: u32, height: u32, opts: &EncodeOptions) -> anyhow::Result<Self> {
        let fps = format!("{}/{}", p.info.fps.0, p.info.fps.1);
        let size = format!("{width}x{height}");
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-y"]);
        cmd.args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &size, "-r", &fps, "-i", "-"]);
        if let Some(src) = src {
            cmd.arg("-i").arg(src);
            cmd.args(["-map", "0:v:0", "-map", "1:a?", "-map", "1:s?", "-map_metadata", "1", "-c:a", "copy", "-c:s", "copy"]);
        } else {
            cmd.args(["-map", "0:v:0"]);
        }
        // RGB -> YUV with the source's matrix/range, then the encoder.
        let space = p.color.space.clone().unwrap_or_else(|| "bt709".into());
        let range = p.color.range.clone().unwrap_or_else(|| "tv".into());
        let pix = if opts.ten_bit { "yuv420p10le" } else { "yuv420p" };
        cmd.args(["-vf", &format!("scale=out_color_matrix={space}:out_range={range},format={pix}")]);
        // Tag the stream with what was actually used, so an untagged source
        // comes out tagged bt709/tv rather than converted-with-709-but-unknown.
        let primaries = p.color.primaries.clone().unwrap_or_else(|| "bt709".into());
        let transfer = p.color.transfer.clone().unwrap_or_else(|| "bt709".into());
        for (flag, tag) in [("-color_primaries", &primaries), ("-color_trc", &transfer), ("-colorspace", &space), ("-color_range", &range)] {
            cmd.args([flag, tag]);
        }
        match opts.codec.as_str() {
            "av1" => {
                cmd.args(["-c:v", "libsvtav1", "-crf", &opts.crf.to_string(), "-preset", &opts.preset.to_string()]);
                if !opts.svt_params.is_empty() {
                    cmd.args(["-svtav1-params", &opts.svt_params.join(":")]);
                }
                if let Some(k) = opts.svt_params.iter().find_map(|p| p.strip_prefix("keyint=")) {
                    // Also as -g so the container-level GOP matches on old wrappers.
                    cmd.args(["-g", k]);
                }
            }
            "x264" | "x265" => {
                let preset = ["veryslow", "slower", "slow", "slow", "medium", "medium", "medium", "fast", "faster", "veryfast", "superfast", "ultrafast", "ultrafast", "ultrafast"][opts.preset.min(13) as usize];
                cmd.args(["-c:v", if opts.codec == "x264" { "libx264" } else { "libx265" }, "-crf", &opts.crf.to_string(), "-preset", preset]);
            }
            other => anyhow::bail!("unknown codec {other}"),
        }
        if !opts.keyframes.is_empty() {
            let list: Vec<String> = opts.keyframes.iter().map(|t| format!("{t:.4}")).collect();
            cmd.args(["-force_key_frames", &list.join(",")]);
        }
        cmd.arg(out);
        let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().context("spawning ffmpeg encoder")?;
        let stdin = child.stdin.take();
        Ok(FfmpegSink { child, stdin })
    }
}

impl FrameSink for FfmpegSink {
    fn write(&mut self, frame: &Frame) -> anyhow::Result<()> {
        self.stdin.as_mut().ok_or_else(|| anyhow!("encoder closed"))?.write_all(&frame.data).context("writing frame to ffmpeg encoder")
    }
    fn finish(&mut self) -> anyhow::Result<()> {
        drop(self.stdin.take());
        let st = self.child.wait()?;
        anyhow::ensure!(st.success(), "ffmpeg encoder exited with {st}");
        Ok(())
    }
}

impl Drop for FfmpegSink {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------------------
// Image sequences (a directory of PNGs), for debugging and tests.

pub struct PngDirSource {
    files: Vec<PathBuf>,
    next: usize,
    info: SourceInfo,
}

impl PngDirSource {
    pub fn open(dir: &Path, fps: (u32, u32)) -> anyhow::Result<Self> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().map_or(false, |e| e.eq_ignore_ascii_case("png"))).collect();
        files.sort();
        anyhow::ensure!(!files.is_empty(), "{}: no .png files", dir.display());
        let first = image::open(&files[0])?.into_rgb8();
        Ok(PngDirSource { info: SourceInfo { width: first.width(), height: first.height(), fps, frame_count: Some(files.len() as u64) }, files, next: 0 })
    }
}

impl FrameSource for PngDirSource {
    fn info(&self) -> SourceInfo {
        self.info
    }
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        let Some(p) = self.files.get(self.next) else { return Ok(None) };
        self.next += 1;
        let img = image::open(p)?.into_rgb8();
        Ok(Some(Frame::from_rgb(img.width(), img.height(), img.into_raw())?))
    }
}

pub struct PngDirSink {
    dir: PathBuf,
    index: u64,
}

impl PngDirSink {
    pub fn open(dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(dir)?;
        Ok(PngDirSink { dir: dir.to_path_buf(), index: 0 })
    }
}

impl FrameSink for PngDirSink {
    fn write(&mut self, frame: &Frame) -> anyhow::Result<()> {
        let p = self.dir.join(format!("frame{:06}.png", self.index));
        self.index += 1;
        image::RgbImage::from_raw(frame.width, frame.height, frame.data.clone()).ok_or_else(|| anyhow!("bad frame"))?.save(&p)?;
        Ok(())
    }
    fn finish(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Stream-copy concatenate video-only parts and mux audio/subtitles/metadata
/// from the source. Parts must share encoder settings and dimensions, which
/// segment parallelism guarantees.
pub fn concat_parts(parts: &[PathBuf], src: &Path, out: &Path) -> anyhow::Result<()> {
    let list = out.with_extension("parts.txt");
    let mut text = String::new();
    for p in parts {
        let abs = std::fs::canonicalize(p)?;
        text.push_str(&format!("file '{}'\n", abs.display().to_string().replace('\'', "'\\''")));
    }
    std::fs::write(&list, text)?;
    let st = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list)
        .arg("-i")
        .arg(src)
        .args(["-map", "0:v:0", "-map", "1:a?", "-map", "1:s?", "-map_metadata", "1", "-c", "copy"])
        .arg(out)
        .status()
        .context("ffmpeg concat")?;
    let _ = std::fs::remove_file(&list);
    anyhow::ensure!(st.success(), "ffmpeg concat exited with {st}");
    Ok(())
}
