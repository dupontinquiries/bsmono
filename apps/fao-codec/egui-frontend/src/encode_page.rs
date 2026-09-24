//! Encode — the page a tester opens this app for: pick a video, encode it to `.fao`, watch it
//! happen, open the result in the Player.
//!
//! The GTK Encode dialog offers codec versions v2.0–v8 (plus a v9 trailer); those encoders live
//! inside fao_app's main.rs and are not ported. This page encodes with the codec the port has
//! for real — intra DCT, every frame a `'D'` I-frame (`transcode.rs`) — and SAYS so, rather than
//! showing radio buttons that would do the same thing whichever was picked.
//!
//! Layers: [`Job`] runs `transcode_stream` on a worker thread and reports through a channel
//! ([`Msg`]); [`State::poll`] folds messages into what the page shows; [`ui`] only draws and
//! starts/cancels. The worker holds one decoded frame at a time (FrameReader), so a long file
//! costs the compressed output, not the uncompressed video.

use crate::transcode::{self, EncodeOpts, FrameStat, Stop, TranscodeReport, VideoInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Msg {
    Frame(FrameStat, Option<(usize, usize, Vec<u8>)>),
    Done(Result<(TranscodeReport, PathBuf), Stop>),
}

pub struct Job {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    pub started: Instant,
    pub total: Option<usize>,
    pub stats: Vec<FrameStat>,
    pub out: PathBuf,
}

/// Decoded previews go to the UI at most this often: a picture per frame would copy megabytes
/// per frame across the channel for images nobody can see that fast.
pub const PREVIEW_EVERY: Duration = Duration::from_millis(250);

impl Job {
    pub fn start(input: &str, out: &Path, opts: EncodeOpts, max_frames: Option<usize>, total: Option<usize>) -> Job {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (input, out_path, c) = (input.to_string(), out.to_path_buf(), cancel.clone());
        std::thread::spawn(move || {
            let mut last_preview: Option<Instant> = None;
            let mut dims = (0, 0);
            if let Ok(i) = transcode::probe(&input) { dims = (i.w & !1, i.h & !1); }
            let r = transcode::transcode_stream(&input, opts, max_frames, &c, |s, img| {
                let due = last_preview.is_none_or(|t| t.elapsed() >= PREVIEW_EVERY);
                let preview = due.then(|| { last_preview = Some(Instant::now()); (dims.0, dims.1, img.to_vec()) });
                let _ = tx.send(Msg::Frame(s.clone(), preview));
            });
            let done = r.and_then(|(bytes, rep)| transcode::write_atomic(&out_path, &bytes).map(|_| (rep, out_path.clone())).map_err(Stop::Failed));
            let _ = tx.send(Msg::Done(done));
        });
        Job { rx, cancel, started: Instant::now(), total, stats: Vec::new(), out: out.to_path_buf() }
    }

    pub fn cancel(&self) { self.cancel.store(true, Ordering::Relaxed); }

    pub fn fps(&self) -> f64 {
        let t = self.started.elapsed().as_secs_f64();
        if t > 0.0 { self.stats.len() as f64 / t } else { 0.0 }
    }

    pub fn bytes_so_far(&self) -> usize { self.stats.iter().map(|s| s.bytes).sum() }

    pub fn t_frames(&self) -> usize { self.stats.iter().filter(|s| s.marker == b'T').count() }

    pub fn mean_psnr(&self) -> f64 {
        if self.stats.is_empty() { 0.0 } else { self.stats.iter().map(|s| s.psnr).sum::<f64>() / self.stats.len() as f64 }
    }
}

pub enum Outcome {
    Done(TranscodeReport, PathBuf, Duration),
    Cancelled(usize),
    Failed(String),
}

pub struct State {
    pub input: String,
    pub info: Option<Result<VideoInfo, String>>,
    probed: String,
    pub quality: u8,
    /// T-frames (motion + residual against the last I-frame) where they pay.
    pub inter: bool,
    pub max_gop: usize,
    pub max_psnr_drop: f64,
    /// 0 = the whole file.
    pub max_frames: usize,
    pub output: String,
    /// Output path typed by hand: stop deriving it from the input.
    output_edited: bool,
    pub job: Option<Job>,
    pub outcome: Option<Outcome>,
    pub preview: Option<(usize, usize, Vec<u8>)>,
    preview_dirty: bool,
    /// Set once when an encode finishes; taken by `add_finished_to_archive`.
    pub finished: Option<PathBuf>,
    /// What happened with the archive for the last encode ("added to archive X").
    pub archive_note: String,
    tex: Option<egui::TextureHandle>,
}

impl Default for State {
    fn default() -> Self {
        State { input: String::new(), info: None, probed: String::new(), quality: 75, inter: true, max_gop: EncodeOpts::inter(75).max_gop, max_psnr_drop: EncodeOpts::inter(75).max_psnr_drop, max_frames: 0, output: String::new(), output_edited: false, job: None, outcome: None, preview: None, preview_dirty: false, finished: None, archive_note: String::new(), tex: None }
    }
}

/// `clip.mp4` → `clip.fao` beside it.
pub fn default_output(input: &str) -> String {
    let p = Path::new(input.trim());
    if input.trim().is_empty() { return String::new(); }
    p.with_extension("fao").display().to_string()
}

impl State {
    /// Probe when the input path changed and names a file; keep the output path in step
    /// unless it was edited.
    pub fn refresh_input(&mut self) {
        let path = self.input.trim().to_string();
        if path == self.probed { return; }
        self.probed = path.clone();
        if !self.output_edited { self.output = default_output(&path); }
        self.info = if path.is_empty() { None } else if !Path::new(&path).is_file() { Some(Err("no such file".into())) } else { Some(transcode::probe(&path)) };
    }

    pub fn opts(&self) -> EncodeOpts {
        if self.inter {
            EncodeOpts { quality: self.quality, inter: true, max_gop: self.max_gop.max(1), max_psnr_drop: self.max_psnr_drop.max(0.0) }
        } else {
            EncodeOpts::intra(self.quality)
        }
    }

    pub fn can_start(&self) -> Result<(), String> {
        if self.job.is_some() { return Err("already encoding".into()); }
        match &self.info {
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(e.clone()),
            None => return Err("choose a video file".into()),
        }
        let out = self.output.trim();
        if out.is_empty() { return Err("choose an output path".into()); }
        if Path::new(out) == Path::new(self.input.trim()) { return Err("the output would overwrite the input".into()); }
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.can_start()?;
        let max = (self.max_frames > 0).then_some(self.max_frames);
        let total = self.info.as_ref().and_then(|i| i.as_ref().ok()).map(|i| transcode::estimated_frames(i, max));
        self.outcome = None;
        self.preview = None;
        self.job = Some(Job::start(self.input.trim(), Path::new(self.output.trim()), self.opts(), max, total));
        Ok(())
    }

    /// Fold the worker's messages in. Returns true when something changed.
    pub fn poll(&mut self) -> bool {
        let Some(job) = self.job.as_mut() else { return false };
        let mut changed = false;
        let mut finished = None;
        while let Ok(m) = job.rx.try_recv() {
            changed = true;
            match m {
                Msg::Frame(s, p) => {
                    job.stats.push(s);
                    if let Some(p) = p { self.preview = Some(p); self.preview_dirty = true; }
                }
                Msg::Done(r) => finished = Some(r),
            }
        }
        if let Some(r) = finished {
            let job = self.job.take().expect("job");
            self.archive_note.clear();
            self.outcome = Some(match r {
                Ok((rep, path)) => { self.finished = Some(path.clone()); Outcome::Done(rep, path, job.started.elapsed()) }
                Err(Stop::Cancelled) => Outcome::Cancelled(job.stats.len()),
                Err(Stop::Failed(e)) => Outcome::Failed(e),
            });
        }
        changed
    }
}

fn bytes_h(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.2} GB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.2} MB", n as f64 / (1u64 << 20) as f64),
        n if n >= 1 << 10 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{n} B"),
    }
}

/// Per-frame bytes as a line, so a cost spike (a scene cut, a noisy stretch) is visible.
fn sparkline(ui: &mut egui::Ui, stats: &[FrameStat]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width().min(520.0), 56.0), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, ui.visuals().weak_text_color()), egui::StrokeKind::Inside);
    if stats.len() < 2 { return; }
    let max = stats.iter().map(|s| s.bytes).max().unwrap_or(1).max(1) as f32;
    let n = stats.len() as f32 - 1.0;
    let pts: Vec<egui::Pos2> = stats.iter().enumerate().map(|(i, s)| {
        egui::pos2(rect.left() + rect.width() * i as f32 / n, rect.bottom() - rect.height() * (s.bytes as f32 / max) * 0.9)
    }).collect();
    // I-frames as ticks along the bottom: where the anchors are, and why the line jumps there.
    for (i, s) in stats.iter().enumerate() {
        if s.marker != b'T' {
            let x = rect.left() + rect.width() * i as f32 / n;
            p.line_segment([egui::pos2(x, rect.bottom()), egui::pos2(x, rect.bottom() - 6.0)], egui::Stroke::new(1.0, ui.visuals().warn_fg_color));
        }
    }
    p.add(egui::Shape::line(pts, egui::Stroke::new(1.5, ui.visuals().selection.bg_fill)));
}

/// fao_app's Export adds the file to the active archive; so does a finished encode here.
pub fn add_finished_to_archive(app: &mut crate::App) {
    let Some(path) = app.encode.finished.take() else { return };
    if !app.archive.is_open() { return; }
    let name = app.archive.archive.as_ref().map(|a| a.name.clone()).unwrap_or_default();
    app.encode.archive_note = match app.archive.add(&path, app.encode.input.trim()) {
        Ok(_) => format!("added to archive {name}"),
        Err(e) => format!("not added to archive {name}: {e}"),
    };
}

pub fn ui(app: &mut crate::App, ui: &mut egui::Ui) {
    if app.encode.poll() || app.encode.job.is_some() {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    add_finished_to_archive(app);
    let s = &mut app.encode;
    ui.heading("Encode");
    ui.label(egui::RichText::new("A video file in, a .fao out. I-frames: FAO DCT ('D'). T-frames: v2.1 predictive \
        ('T', motion vectors + residual against the last I-frame), used where smaller and close enough in quality. \
        The GTK app's other codec versions (v3.0–v8, the v9 trailer) are not ported yet.").weak().small());
    ui.add_space(8.0);

    let running = s.job.is_some();
    ui.add_enabled_ui(!running, |ui| {
        ui.horizontal(|ui| {
            ui.label("Video");
            ui.add(egui::TextEdit::singleline(&mut s.input).id_salt("encode-input").hint_text("/path/to/clip.mp4").desired_width(360.0));
        });
        s.refresh_input();
        match &s.info {
            Some(Ok(i)) => { ui.label(format!("{}×{} · {:.2} fps · {:.1} s · {} · about {} frames", i.w, i.h, i.fps, i.duration_s, i.codec, transcode::estimated_frames(i, None))); }
            Some(Err(e)) => { ui.colored_label(ui.visuals().warn_fg_color, e); }
            None => {}
        }
        ui.horizontal(|ui| {
            ui.label("Quality");
            ui.add(egui::Slider::new(&mut s.quality, 1..=100));
            ui.label("Frames");
            ui.add(egui::DragValue::new(&mut s.max_frames).range(0..=1_000_000));
            ui.label(egui::RichText::new(if s.max_frames == 0 { "(all)" } else { "(first N)" }).weak());
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut s.inter, "Inter frames (T)");
            ui.add_enabled_ui(s.inter, |ui| {
                ui.label("GOP ≤");
                ui.add(egui::DragValue::new(&mut s.max_gop).range(1..=10_000));
                ui.label("max PSNR drop");
                ui.add(egui::DragValue::new(&mut s.max_psnr_drop).range(0.0..=30.0).speed(0.1).suffix(" dB"));
            });
        });
        ui.horizontal(|ui| {
            ui.label("Output");
            if ui.add(egui::TextEdit::singleline(&mut s.output).id_salt("encode-output").desired_width(360.0)).changed() {
                s.output_edited = true;
            }
        });
    });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if running {
            if ui.button("Cancel").clicked() {
                if let Some(j) = &s.job { j.cancel(); }
            }
        } else {
            let ready = s.can_start();
            let b = ui.add_enabled(ready.is_ok(), egui::Button::new("Encode"));
            if b.clicked() {
                if let Err(e) = s.start() { s.outcome = Some(Outcome::Failed(e)); }
            }
            if let Err(why) = ready { ui.label(egui::RichText::new(why).weak()); }
        }
    });

    if let Some(j) = &s.job {
        let done = j.stats.len();
        let frac = j.total.filter(|t| *t > 0).map(|t| (done as f32 / t as f32).min(1.0));
        let eta = match (j.total, j.fps()) { (Some(t), f) if f > 0.0 && t > done => format!(" · ~{:.0} s left", (t - done) as f64 / f), _ => String::new() };
        let text = format!("{done}{} frames · {:.1} fps{eta}", j.total.map(|t| format!(" / ~{t}")).unwrap_or_default(), j.fps());
        ui.add(egui::ProgressBar::new(frac.unwrap_or(0.0)).text(text).animate(frac.is_none()));
        if let Some(last) = j.stats.last() {
            ui.label(format!("so far {} · mean PSNR {:.2} dB · I {} / T {} · last frame {} at {:.2} dB", bytes_h(j.bytes_so_far() as u64), j.mean_psnr(), j.stats.len() - j.t_frames(), j.t_frames(), bytes_h(last.bytes as u64), last.psnr));
            ui.label(egui::RichText::new(format!("frame {}: {}", last.index, last.why)).weak().small());
        }
        sparkline(ui, &j.stats);
    }

    match &s.outcome {
        Some(Outcome::Done(r, path, took)) => {
            let ratio = if r.output_bytes > 0 { r.input_bytes as f64 / r.output_bytes as f64 } else { 0.0 };
            let bpp = r.output_bytes as f64 * 8.0 / (r.w * r.h * r.frames).max(1) as f64;
            ui.label(egui::RichText::new(format!("Done: {} frames {}×{} in {:.1} s", r.frames, r.w, r.h, took.as_secs_f64())).strong());
            ui.label(format!("{} → {} ({:.2}× vs the input file) · {:.3} bpp · mean PSNR {:.2} dB · {} I-frames, {} T-frames", bytes_h(r.input_bytes), bytes_h(r.output_bytes), ratio, bpp, r.mean_psnr, r.i_frames, r.t_frames));
            ui.label(egui::RichText::new(path.display().to_string()).monospace().small());
            if !s.archive_note.is_empty() { ui.label(egui::RichText::new(&s.archive_note).small()); }
            if ui.button("Open in Player").clicked() {
                let p = path.display().to_string();
                app.player.path = p.clone();
                app.player.open();
                app.tab = crate::Tab::Player;
                return;
            }
        }
        Some(Outcome::Cancelled(n)) => { ui.label(format!("Cancelled after {n} frames; nothing written.")); }
        Some(Outcome::Failed(e)) => { ui.colored_label(ui.visuals().error_fg_color, format!("Failed: {e}")); }
        None => {}
    }

    let s = &mut app.encode;
    if s.preview_dirty {
        if let Some((w, h, rgb)) = &s.preview {
            if *w > 0 && rgb.len() == w * h * 3 {
                let img = egui::ColorImage::from_rgb([*w, *h], rgb);
                match &mut s.tex {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => s.tex = Some(ui.ctx().load_texture("encode-preview", img, egui::TextureOptions::LINEAR)),
                }
            }
        }
        s.preview_dirty = false;
    }
    if let (Some(t), Some((w, h, _))) = (&s.tex, &s.preview) {
        ui.add_space(6.0);
        ui.label(egui::RichText::new("decoded output (what the .fao will show)").weak().small());
        let avail = ui.available_width().min(640.0);
        let size = egui::vec2(avail, avail * *h as f32 / (*w).max(1) as f32);
        ui.image((t.id(), size));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_video(path: &Path, n: usize) {
        let ok = std::process::Command::new("ffmpeg").args(["-v", "error", "-y", "-f", "lavfi"])
            .arg("-i").arg(format!("testsrc=size=64x48:rate=10:duration={:.1}", n as f64 / 10.0))
            .args(["-frames:v", &n.to_string(), "-pix_fmt", "yuv420p"]).arg(path).status().map(|s| s.success()).unwrap_or(false);
        assert!(ok, "ffmpeg testsrc");
    }
    fn have_ffmpeg() -> bool {
        std::process::Command::new("ffmpeg").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
    }
    fn wait(s: &mut State) {
        let t = Instant::now();
        while s.job.is_some() {
            s.poll();
            assert!(t.elapsed() < Duration::from_secs(60), "encode did not finish");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_real_video_encodes_on_the_worker_and_opens_in_the_player() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-encode-page-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let v = dir.join("clip.mp4");
        make_video(&v, 8);
        let mut app = crate::App::default();
        let s = &mut app.encode;
        s.input = v.display().to_string();
        s.refresh_input();
        assert_eq!(s.output, dir.join("clip.fao").display().to_string(), "output derived from the input");
        assert!(matches!(&s.info, Some(Ok(i)) if i.w == 64 && i.h == 48), "probed");
        s.start().unwrap();
        assert!(s.can_start().is_err(), "one encode at a time");
        wait(s);
        let Some(Outcome::Done(rep, path, _)) = &s.outcome else { panic!("not done") };
        assert_eq!(rep.frames, 8);
        assert!(path.is_file() && !path.with_extension("fao.part").exists(), "written atomically");
        assert!(s.preview.as_ref().is_some_and(|(w, h, p)| *w == 64 && *h == 48 && p.len() == 64 * 48 * 3), "a decoded preview arrived");
        // The same loop the Player runs reads it back.
        let bytes = std::fs::read(path).unwrap();
        let f = crate::decode::FaoFile::parse(bytes).expect("parses");
        assert_eq!(f.frame_count(), 8);
        for i in 0..8 {
            assert!(f.decode_rgb(i).is_ok(), "frame {i} decodes the way the Player decodes it");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancel_leaves_no_output_and_says_how_far_it_got() {
        if !have_ffmpeg() { eprintln!("no ffmpeg; skipping"); return; }
        let dir = std::env::temp_dir().join(format!("fao-encode-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let v = dir.join("long.mp4");
        make_video(&v, 200);
        let mut s = State { input: v.display().to_string(), ..State::default() };
        s.refresh_input();
        s.start().unwrap();
        s.job.as_ref().unwrap().cancel();
        wait(&mut s);
        let Some(Outcome::Cancelled(n)) = s.outcome else { panic!("not cancelled") };
        assert!(n < 200, "stopped early ({n})");
        assert!(!dir.join("long.fao").exists() && !dir.join("long.fao.part").exists(), "nothing written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[allow(deprecated)]
    fn the_page_draws_in_every_state_without_panicking() {
        let mut app = crate::App::default();
        assert!(app.tab == crate::Tab::Encode, "Encode is the first thing a tester sees");
        let ctx = egui::Context::default();
        let frame = |app: &mut crate::App| {
            let _ = ctx.run(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 700.0))), ..Default::default() }, |ctx| app.ui(ctx));
        };
        frame(&mut app); // empty
        app.encode.input = "/nonexistent.mp4".into();
        frame(&mut app); // refused, with a reason
        app.encode.preview = Some((64, 48, vec![128; 64 * 48 * 3]));
        app.encode.preview_dirty = true;
        app.encode.outcome = Some(Outcome::Done(TranscodeReport { frames: 3, w: 64, h: 48, input_bytes: 5000, output_bytes: 2000, mean_psnr: 38.5, i_frames: 1, t_frames: 2 }, PathBuf::from("/tmp/x.fao"), Duration::from_millis(900)));
        frame(&mut app);
        assert!(app.encode.tex.is_some(), "the preview became a texture");
        app.encode.outcome = Some(Outcome::Failed("ffmpeg decode failed: boom".into()));
        frame(&mut app);
        app.encode.outcome = Some(Outcome::Cancelled(7));
        frame(&mut app);
    }

    #[test]
    fn start_is_refused_with_a_reason_until_the_inputs_make_sense() {
        let mut s = State::default();
        assert_eq!(s.can_start().unwrap_err(), "choose a video file");
        s.input = "/nonexistent/clip.mp4".into();
        s.refresh_input();
        assert_eq!(s.can_start().unwrap_err(), "no such file");
        assert_eq!(default_output("/a/b/clip.mov"), "/a/b/clip.fao");
        // A hand-typed output survives an input change.
        s.output = "/tmp/mine.fao".into();
        s.output_edited = true;
        s.input = "/other.mp4".into();
        s.refresh_input();
        assert_eq!(s.output, "/tmp/mine.fao");
    }
}
