//! Analysis mode: bscale's stage A over a few windows of a title.
//!
//! For each title: three 2-second windows (at 20%, 50% and 80% of its
//! length, so one static intro does not speak for the whole thing), decoded
//! by ffmpeg to RGB at no more than 960 on the long edge, fed to
//! `d2x_core::pipeline::analyze_only` -- the same gate bscale uses to decide
//! which blocks go through its CNN -- plus a softness measure. No upscaler
//! runs; this is the "what would it cost" pass.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use d2x_core::pipeline::{FrameSource, PipelineConfig, SourceInfo};
use d2x_core::Frame;

use crate::enhance_plan::TitleFacts;

/// Frames from one ffmpeg window, as bscale's `FrameSource`.
pub struct FfmpegWindow {
    child: std::process::Child,
    w: u32,
    h: u32,
    fps: f64,
}

/// ffmpeg arguments for an RGB window. Pure.
pub fn window_args(input: &str, start: Duration, len: Duration, w: u32, h: u32) -> Vec<String> {
    let mut a: Vec<String> = ["-v", "error", "-nostdin"].iter().map(|s| s.to_string()).collect();
    a.extend(["-ss".into(), format!("{:.3}", start.as_secs_f64()), "-t".into(), format!("{:.3}", len.as_secs_f64())]);
    a.extend(["-i".into(), input.to_string()]);
    a.extend(["-map", "0:v:0", "-an", "-sn", "-dn"].iter().map(|s| s.to_string()));
    a.extend(["-vf".into(), format!("scale={w}:{h}:flags=area")]);
    a.extend(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"].iter().map(|s| s.to_string()));
    a
}

impl FfmpegWindow {
    pub fn open(input: &str, start: Duration, len: Duration, w: u32, h: u32, fps: f64) -> Result<Self, String> {
        let child = Command::new("ffmpeg")
            .args(window_args(input, start, len, w, h))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("ffmpeg: {e}"))?;
        Ok(Self { child, w, h, fps })
    }
}

impl FrameSource for FfmpegWindow {
    fn info(&self) -> SourceInfo {
        SourceInfo { width: self.w, height: self.h, fps: ((self.fps * 1000.0).round() as u32, 1000), frame_count: None }
    }
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        let mut buf = vec![0u8; self.w as usize * self.h as usize * 3];
        let Some(out) = self.child.stdout.as_mut() else { return Ok(None) };
        match out.read_exact(&mut buf) {
            Ok(()) => Ok(Some(Frame::from_rgb(self.w, self.h, buf).map_err(|e| anyhow::anyhow!("{e:?}"))?)),
            Err(_) => Ok(None),
        }
    }
}

impl Drop for FfmpegWindow {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// How soft a frame is, 0 (crisp) to 1 (very soft). Pure.
///
/// Mean absolute Laplacian of luma, relative to the mean absolute gradient:
/// a ratio, so a dim or low-contrast frame is not called soft for being dim.
/// Sharp detail has second-derivative energy comparable to its first; a
/// blurred edge keeps its gradient and loses the curvature. Flat frames
/// (almost no gradient) say nothing and return 0.
pub fn softness(f: &Frame) -> f32 {
    let (w, h) = (f.width as usize, f.height as usize);
    if w < 3 || h < 3 {
        return 0.0;
    }
    let y = |x: usize, yy: usize| {
        let p = f.px(x as u32, yy as u32);
        0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
    };
    let (mut lap, mut grad) = (0.0f64, 0.0f64);
    for yy in (1..h - 1).step_by(2) {
        for x in (1..w - 1).step_by(2) {
            let c = y(x, yy);
            let l = y(x - 1, yy) + y(x + 1, yy) + y(x, yy - 1) + y(x, yy + 1) - 4.0 * c;
            lap += l.abs() as f64;
            grad += ((y(x + 1, yy) - y(x - 1, yy)).abs() + (y(x, yy + 1) - y(x, yy - 1)).abs()) as f64;
        }
    }
    if grad < 1e-3 * (w * h) as f64 {
        return 0.0;
    }
    // Crisp content sits near 1.0 on this ratio; a 2-pixel blur near 0.35.
    let ratio = (lap / grad) as f32;
    ((1.0 - ratio) / 0.7).clamp(0.0, 1.0)
}

/// The analysis size: the source, capped at 960 on the long edge, even.
pub fn analysis_size(w: u32, h: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    let s = (960.0 / long as f64).min(1.0);
    ((((w as f64 * s) as u32) & !1).max(2), (((h as f64 * s) as u32) & !1).max(2))
}

/// Analyse one title. Blocking; runs on the analysis thread.
pub fn analyse(input: &str, src_w: u32, src_h: u32, fps: f64, duration: Option<Duration>) -> Result<(TitleFacts, Duration), String> {
    let began = Instant::now();
    let (w, h) = analysis_size(src_w, src_h);
    let len = Duration::from_secs(2);
    let starts: Vec<Duration> = match duration {
        Some(d) if d > Duration::from_secs(10) => [0.2, 0.5, 0.8].iter().map(|f| d.mul_f64(*f)).collect(),
        _ => vec![Duration::ZERO],
    };
    let (mut frac, mut soft, mut n) = (0.0, 0.0f32, 0usize);
    for s in starts {
        // Softness from the first frame of the window.
        let mut probe = FfmpegWindow::open(input, s, Duration::from_millis(200), w, h, fps)?;
        if let Ok(Some(f)) = probe.next_frame() {
            soft += softness(&f);
        }
        drop(probe);
        let src = FfmpegWindow::open(input, s, len, w, h, fps)?;
        // noise: 0. bscale adds uniform noise (default strength 8, a new
        // seed each frame) to every input for its offline encode; with it on,
        // two identical frames differ and the gate redraws everything -- a
        // perfectly still clip measured 100%. The question here is what the
        // CONTENT changes, so the noise is off.
        let cfg = PipelineConfig { noise: 0, ..PipelineConfig::default() };
        let summary = d2x_core::pipeline::analyze_only(Box::new(src), cfg, |_, _, _, _| {})
            .map_err(|e| format!("bscale analysis: {e}"))?;
        if summary.frames > 1 {
            frac += summary.upscaled_pixel_fraction;
            n += 1;
        }
    }
    if n == 0 {
        return Err("no frames analysed".into());
    }
    let facts = TitleFacts { src_w, src_h, fps, residual_fraction: frac / n as f64, softness: soft / n as f32 };
    Ok((facts, began.elapsed()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new("ffmpeg").arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    }

    #[test]
    fn the_window_seeks_before_the_input_and_asks_for_rgb() {
        let a = window_args("x.mp4", Duration::from_secs(60), Duration::from_secs(2), 640, 360);
        let at = |s: &str| a.iter().position(|x| x == s).unwrap();
        assert!(at("-ss") < at("-i") && at("-t") < at("-i"));
        assert_eq!(a[at("-pix_fmt") + 1], "rgb24");
        assert!(a.contains(&"-an".to_string()));
    }

    #[test]
    fn analysis_is_capped_near_960_and_even() {
        assert_eq!(analysis_size(1920, 1080), (960, 540));
        assert_eq!(analysis_size(640, 360), (640, 360), "never upscaled for analysis");
        let (w, h) = analysis_size(1081, 1921);
        assert!(w % 2 == 0 && h % 2 == 0 && h <= 960);
    }

    fn frame_from(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Frame {
        let mut d = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                let v = f(x, y);
                d.extend([v, v, v]);
            }
        }
        Frame::from_rgb(w, h, d).unwrap()
    }

    /// Crisp stripes read crisp, the same stripes blurred read soft, flat
    /// reads as nothing, and dimming does not change the answer.
    #[test]
    fn softness_tells_blur_from_detail_and_ignores_brightness() {
        let crisp = frame_from(64, 64, |x, _| if (x / 2) % 2 == 0 { 200 } else { 40 });
        let blur = frame_from(64, 64, |x, _| (120.0 + 80.0 * ((x as f32) * 0.2).sin()) as u8);
        let flat = frame_from(64, 64, |_, _| 128);
        let dim = frame_from(64, 64, |x, _| if (x / 2) % 2 == 0 { 50 } else { 10 });
        let (sc, sb) = (softness(&crisp), softness(&blur));
        assert!(sc < 0.2, "crisp {sc}");
        assert!(sb > 0.6, "blurred {sb}");
        assert_eq!(softness(&flat), 0.0);
        assert!((softness(&dim) - sc).abs() < 0.05, "brightness changed the answer");
    }

    /// Real ffmpeg and real bscale stage A: a still picture needs the CNN on
    /// almost nothing after its first frame; noise needs it everywhere.
    #[test]
    fn bscale_analysis_separates_still_from_busy() {
        if !have_ffmpeg() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("vk-an-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let still = dir.join("still.mkv");
        let busy = dir.join("busy.mkv");
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=15:duration=3", "-vf", "trim=end_frame=1,loop=loop=60:size=1:start=0,setpts=N/15/TB", "-c:v", "ffv1"]).arg(&still).status().unwrap().success());
        assert!(Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi", "-i", "nullsrc=size=320x180:rate=15,geq=random(1)*255:128:128", "-t", "3", "-c:v", "ffv1"]).arg(&busy).status().unwrap().success());
        let (s, took) = analyse(&still.to_string_lossy(), 320, 180, 15.0, Some(Duration::from_secs(3))).unwrap();
        let (b, _) = analyse(&busy.to_string_lossy(), 320, 180, 15.0, Some(Duration::from_secs(3))).unwrap();
        eprintln!("DIAG residual still {:.3} busy {:.3}; softness still {:.2} busy {:.2}; took {took:?}", s.residual_fraction, b.residual_fraction, s.softness, b.softness);
        assert!(s.residual_fraction < 0.2, "a still picture: {}", s.residual_fraction);
        assert!(b.residual_fraction > 0.7, "noise: {}", b.residual_fraction);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The GPU shader's Rust mirror and bscale's own RCAS must agree: the
    /// shader was ported from bscale's, and this keeps the two honest.
    #[test]
    fn the_shader_mirror_matches_bscales_rcas() {
        let mut seed = 7u32;
        let mut r = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut worst = 0.0f32;
        for _ in 0..5000 {
            let mut px = || [r(), r(), r()];
            let (b, d, e, f, h) = (px(), px(), px(), px(), px());
            for stops in [0.0f32, 0.5, 1.0, 2.0] {
                let ours = vidkit_egui::glow_present::rcas_reference(b, d, e, f, h, (-stops).exp2());
                let theirs = d2x_core::rcas::rcas_pixel(b, d, e, f, h, d2x_core::rcas::Sharpness(stops));
                for c in 0..3 {
                    worst = worst.max((ours[c] - theirs[c]).abs());
                }
            }
        }
        assert!(worst < 1e-4, "shader mirror drifted from bscale's RCAS by {worst}");
    }

}
