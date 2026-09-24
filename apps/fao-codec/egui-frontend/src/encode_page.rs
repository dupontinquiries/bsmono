//! Encode — the page a tester opens this app for: pick a video, encode it to `.fao`, watch it
//! happen, open the result in the Player.
//!
//! The encoder is `transcode::transcode_cancellable_with` (the streaming path: one frame in
//! memory, output written as it goes), with its COMPACT (motion-predicted from the previous
//! decoded frame) or FAST (patches coded from scratch, parallel) mode. The GTK dialog's codec
//! versions v2.0–v8 and the v9 trailer are not ported; the page says so.
//!
//! Layers: [`Job`] runs the transcode on a worker thread and reports through a channel
//! ([`Msg`]); [`State::poll`] folds messages into what the page shows; [`ui`] only draws and
//! starts/cancels. The output goes to `<name>.fao.part` and is renamed only on success, so a
//! crash mid-encode never leaves a half file under the real name.

use crate::transcode::{self, TranscodeReport, VideoInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Msg {
    Progress(usize, usize),
    Done(Result<(TranscodeReport, PathBuf), String>),
}

pub struct Job {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    pub started: Instant,
    pub total: Option<usize>,
    pub done: usize,
    pub out: PathBuf,
}

/// Frames a file should yield: duration × fps, capped. The stream's actual end is what counts.
pub fn estimated_frames(info: &VideoInfo, max_frames: Option<usize>) -> usize {
    let n = (info.duration_s * info.fps).round().max(0.0) as usize;
    match max_frames { Some(m) => n.min(m), None => n }
}

impl Job {
    pub fn start(input: &str, out: &Path, quality: u8, motion: bool, max_frames: Option<usize>, total: Option<usize>) -> Job {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (input, out_path, c) = (input.to_string(), out.to_path_buf(), cancel.clone());
        std::thread::spawn(move || {
            let part = out_path.with_extension("fao.part");
            let r = std::fs::File::create(&part).map_err(|e| format!("creating {}: {e}", part.display())).and_then(|f| {
                let ptx = tx.clone();
                transcode::transcode_cancellable_with(&input, std::io::BufWriter::new(f), quality, max_frames,
                    move |done, total| { let _ = ptx.send(Msg::Progress(done, total)); }, Some(&c), motion)
                    .and_then(|(w, rep)| w.into_inner().map_err(|e| format!("writing {}: {e}", part.display())).map(|_| rep))
            });
            let done = match r {
                Ok(rep) => std::fs::rename(&part, &out_path).map(|_| (rep, out_path.clone())).map_err(|e| format!("{}: {e}", out_path.display())),
                Err(e) => { let _ = std::fs::remove_file(&part); Err(e) }
            };
            let _ = tx.send(Msg::Done(done));
        });
        Job { rx, cancel, started: Instant::now(), total, done: 0, out: out.to_path_buf() }
    }

    pub fn cancel(&self) { self.cancel.store(true, Ordering::Relaxed); }

    pub fn fps(&self) -> f64 {
        let t = self.started.elapsed().as_secs_f64();
        if t > 0.0 { self.done as f64 / t } else { 0.0 }
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
    /// COMPACT (motion from the previous decoded frame; smaller, one core) or FAST.
    pub motion: bool,
    /// 0 = the whole file.
    pub max_frames: usize,
    pub output: String,
    /// Output path typed by hand: stop deriving it from the input.
    output_edited: bool,
    pub job: Option<Job>,
    pub outcome: Option<Outcome>,
    /// Set once when an encode finishes; taken by `add_finished_to_archive`.
    pub finished: Option<PathBuf>,
    /// What happened with the archive for the last encode ("added to archive X").
    pub archive_note: String,
}

impl Default for State {
    fn default() -> Self {
        State { input: String::new(), info: None, probed: String::new(), quality: 75, motion: transcode::MOTION_DEFAULT, max_frames: 0, output: String::new(), output_edited: false, job: None, outcome: None, finished: None, archive_note: String::new() }
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
        let total = self.info.as_ref().and_then(|i| i.as_ref().ok()).map(|i| estimated_frames(i, max));
        self.outcome = None;
        self.job = Some(Job::start(self.input.trim(), Path::new(self.output.trim()), self.quality, self.motion, max, total));
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
                Msg::Progress(done, total) => {
                    job.done = done;
                    if total > 0 { job.total = Some(total); }
                }
                Msg::Done(r) => finished = Some(r),
            }
        }
        if let Some(r) = finished {
            let job = self.job.take().expect("job");
            self.archive_note.clear();
            self.outcome = Some(match r {
                Ok((rep, path)) => { self.finished = Some(path.clone()); Outcome::Done(rep, path, job.started.elapsed()) }
                Err(e) if e == transcode::CANCELLED => Outcome::Cancelled(job.done),
                Err(e) => Outcome::Failed(e),
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
    ui.label(egui::RichText::new("A video file in, a .fao out: key frames, patch frames for the blocks that changed, \
        one-byte repeats. Compact predicts patches from the previous frame (smaller, one core); fast codes them \
        from scratch (parallel). The GTK app's codec versions v2.0–v8 and the v9 trailer are not ported yet.").weak().small());
    ui.add_space(8.0);

    let running = s.job.is_some();
    ui.add_enabled_ui(!running, |ui| {
        ui.horizontal(|ui| {
            ui.label("Video");
            ui.add(egui::TextEdit::singleline(&mut s.input).id_salt("encode-input").hint_text("/path/to/clip.mp4").desired_width(360.0));
        });
        s.refresh_input();
        match &s.info {
            Some(Ok(i)) => { ui.label(format!("{}×{} · {:.2} fps · {:.1} s · {} · about {} frames", i.w, i.h, i.fps, i.duration_s, i.codec, estimated_frames(i, None))); }
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
            ui.radio_value(&mut s.motion, true, "Compact (motion)");
            ui.radio_value(&mut s.motion, false, "Fast (parallel)");
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
        let done = j.done;
        let frac = j.total.filter(|t| *t > 0).map(|t| (done as f32 / t as f32).min(1.0));
        let eta = match (j.total, j.fps()) { (Some(t), f) if f > 0.0 && t > done => format!(" · ~{:.0} s left", (t - done) as f64 / f), _ => String::new() };
        let text = format!("{done}{} frames · {:.1} fps{eta}", j.total.map(|t| format!(" / ~{t}")).unwrap_or_default(), j.fps());
        ui.add(egui::ProgressBar::new(frac.unwrap_or(0.0)).text(text).animate(frac.is_none()));
    }

    match &s.outcome {
        Some(Outcome::Done(r, path, took)) => {
            let ratio = if r.output_bytes > 0 { r.input_bytes as f64 / r.output_bytes as f64 } else { 0.0 };
            let bpp = r.output_bytes as f64 * 8.0 / (r.w * r.h * r.frames).max(1) as f64;
            ui.label(egui::RichText::new(format!("Done: {} frames {}×{} in {:.1} s", r.frames, r.w, r.h, took.as_secs_f64())).strong());
            ui.label(format!("{} → {} ({:.2}× vs the input file) · {:.3} bpp · mean PSNR {:.2} dB · {} patch frames, {} repeats", bytes_h(r.input_bytes), bytes_h(r.output_bytes), ratio, bpp, r.mean_psnr, r.patches, r.repeats));
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
        app.encode.outcome = Some(Outcome::Done(TranscodeReport { frames: 3, repeats: 1, patches: 1, w: 64, h: 48, input_bytes: 5000, output_bytes: 2000, mean_psnr: 38.5 }, PathBuf::from("/tmp/x.fao"), Duration::from_millis(900)));
        frame(&mut app);
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
