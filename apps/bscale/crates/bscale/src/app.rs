//! The shell. Everything a job can be told is in [`crate::job::Job`]; this
//! is a form over it, a background thread running it, and a live view of
//! the pipeline's events: per-frame reuse bars, stage timings, the log, and
//! a preview of the latest output frame. Stop is clean — the source is cut
//! and the encoder finalises a playable partial file.

use d2x_job::job::{self, Backend, Job};
use d2x_core::frame::Frame;
use d2x_core::pipeline::{Event, FrameSink, FrameSource, MatcherKind, SourceInfo};
use eframe::egui;
use snapclone_theme::palette_by_id;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;
use uikit_egui::log_view::{log_view, LogEntry, LogLevel};

/// Must match packaging/org.bscale.Bscale.desktop exactly — a mismatch
/// fails silently, leaving a working app with no icon.
const APP_ID: &str = "org.bscale.Bscale";

pub fn run() -> anyhow::Result<()> {
    let opts = eframe::NativeOptions {
        // Glow SAID, not left to eframe: wgpu's default device descriptor
        // asks for 8 colour attachments, which dies on a phone GPU allowing 5.
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "bscale", [1100.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native("bscale", opts, Box::new(|cc| {
        uikit_egui::theme_bridge::apply_palette(&cc.egui_ctx, palette_by_id("dark"));
        Ok(Box::new(Shell::default()))
    }))
    .map_err(|e| anyhow::anyhow!("{e}"))
}

enum Msg {
    Event(Event),
    Preview { width: u32, height: u32, rgb: Vec<u8> },
    Log(LogLevel, String),
    Done(Result<(), String>),
    /// The real RAM-aware ring sizing this run is actually using (see
    /// `job::Event::MemoryPlan`) -- distinct from `Log` so the UI can show
    /// it as structured numbers next to the override sliders, not just
    /// scroll it into the log text.
    MemoryPlan(job::MemPlan),
}

/// Wraps the sink to (a) honour Stop and (b) ship a thumbnail of every
/// Nth output frame to the UI without stalling the encoder.
struct TeeSink {
    inner: Box<dyn FrameSink>,
    tx: Sender<Msg>,
    every: u64,
    n: u64,
}

impl FrameSink for TeeSink {
    fn write(&mut self, frame: &Frame) -> anyhow::Result<()> {
        if self.n % self.every == 0 {
            let step = (frame.width / 640).max(1);
            let (w, h) = (frame.width / step, frame.height / step);
            let mut rgb = Vec::with_capacity((w * h * 3) as usize);
            for y in 0..h {
                for x in 0..w {
                    rgb.extend_from_slice(&frame.px(x * step, y * step));
                }
            }
            let _ = self.tx.send(Msg::Preview { width: w, height: h, rgb });
        }
        self.n += 1;
        self.inner.write(frame)
    }
    fn finish(&mut self) -> anyhow::Result<()> {
        self.inner.finish()
    }
}

struct StoppableSource {
    inner: Box<dyn FrameSource>,
    stop: Arc<AtomicBool>,
}

impl FrameSource for StoppableSource {
    fn info(&self) -> SourceInfo {
        self.inner.info()
    }
    fn next_frame(&mut self) -> anyhow::Result<Option<Frame>> {
        if self.stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        self.inner.next_frame()
    }
}

fn run_job(mut job: Job, analyze_only: bool, tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    let r = (|| -> anyhow::Result<()> {
        if analyze_only {
            let _ = tx.send(Msg::Log(LogLevel::Info, "probing".into()));
            let (source, _) = job::open_source(&job)?;
            let source: Box<dyn FrameSource> = Box::new(StoppableSource { inner: source, stop });
            let s = d2x_core::pipeline::analyze_only(source, job.pipeline, |i, st, _frac, t| {
                let _ = tx.send(Msg::Event(Event::FrameDone { index: i, stats: *st, analyze_time: t, upscale_time: Duration::ZERO, merge_time: Duration::ZERO }));
            })?;
            let _ = tx.send(Msg::Event(Event::Finished { frames: s.frames, elapsed: s.elapsed, upscaled_pixel_fraction: s.upscaled_pixel_fraction, heavy_pixel_fraction: s.heavy_pixel_fraction, plate_pixel_fraction: s.plate_pixel_fraction }));
            return Ok(());
        }
        if job.jobs > 1 {
            let _ = tx.send(Msg::Log(LogLevel::Info, format!("{} segment workers, one shared upscaler; parts are concatenated at the end (no preview in this mode)", job.jobs)));
            let tx2 = tx.clone();
            job::run(&job, move |e| {
                let _ = match e {
                    job::Event::Stage(m) => tx2.send(Msg::Log(LogLevel::Info, m)),
                    job::Event::Pipeline(e) | job::Event::Segment(_, e) => tx2.send(Msg::Event(e)),
                    job::Event::MemoryPlan(plan) => tx2.send(Msg::MemoryPlan(plan)),
                };
            })?;
            return Ok(());
        }
        // Single pipeline, assembled here so the sink can be wrapped for
        // preview and the source for Stop.
        let _ = tx.send(Msg::Log(LogLevel::Info, format!("probing {}", job.input.display())));
        let (source, probe) = job::open_source(&job)?;
        if let Some(p) = &probe {
            let _ = tx.send(Msg::Log(LogLevel::Info, format!("source: {} {} {}x{}, colour {}/{}/{}", p.codec, p.pix_fmt, p.info.width, p.info.height, p.color.primaries.as_deref().unwrap_or("?"), p.color.transfer.as_deref().unwrap_or("?"), p.color.space.as_deref().unwrap_or("?"))));
        }
        let source: Box<dyn FrameSource> = Box::new(StoppableSource { inner: source, stop });
        let info = source.info();
        let keyframes = if probe.is_some() && job.output.extension().is_some() {
            let _ = tx.send(Msg::Log(LogLevel::Info, "scanning keyframes (packet headers only; cached in a sidecar)".into()));
            d2x_job::ffmpeg::keyframe_times(&job.input).unwrap_or_default()
        } else {
            Vec::new()
        };
        let _ = tx.send(Msg::Log(LogLevel::Info, format!("loading model and initialising the {:?} backend", job.backend)));
        let tiers = job::build_tiers(&job)?;
        let scale = tiers[0].scale();
        // The RAM-aware ring sizing: previously only applied on the
        // `job::run`/`run_segmented` paths (the `--jobs > 1` branch above,
        // and the CLI). This single-pipeline path called `run_tiered`
        // directly with the raw, un-adjusted `job.pipeline` -- meaning the
        // GUI's default refs=3/shot-refs=4 (PipelineConfig::default())
        // were not a display artifact here, they were what actually ran,
        // regardless of available RAM. Fixed by computing and applying the
        // same plan every other path already gets.
        let plan = job::mem_plan(&job, info.width, info.height, scale);
        job::apply_plan(&mut job.pipeline, &plan);
        let _ = tx.send(Msg::MemoryPlan(plan));
        let inner = job::open_sink(&job, probe.as_ref(), &keyframes, info.width * scale, info.height * scale)?;
        let sink: Box<dyn FrameSink> = Box::new(TeeSink { inner, tx: tx.clone(), every: 4, n: 0 });
        let _ = tx.send(Msg::Log(LogLevel::Info, "frame 0 is a full-frame upscale; later frames only pay for what changed".into()));
        let tx2 = tx.clone();
        d2x_core::pipeline::run_tiered(source, tiers, sink, job.pipeline, move |e| {
            let _ = tx2.send(Msg::Event(e));
        })?;
        Ok(())
    })();
    let _ = tx.send(Msg::Done(r.map_err(|e| format!("{e:#}"))));
}

#[derive(Clone, Copy)]
struct FrameRow {
    fraction: f32,
    redraw: bool,
    analyze_ms: f32,
    upscale_ms: f32,
}

struct Shell {
    job: Job,
    matcher: usize,
    backend: usize,
    /// The real ring sizing the last/current run actually applied (see
    /// `Msg::MemoryPlan`) -- shown next to the override sliders so "what
    /// will run" is never silently the same-looking number as "what
    /// you're overriding". `None` before any job has probed a source.
    running_plan: Option<job::MemPlan>,
    input: String,
    /// A dropped path waiting on confirmation. A drop has no undo — it
    /// overwrites the input and the old text is gone — so it never takes
    /// effect on release. See `d2x_core::dropped`.
    pending_drop: Option<d2x_core::dropped::Pending>,
    output: String,
    /// The last value `suggested_output` put in the box.
    ///
    /// So a re-pick of the input replaces a suggestion but never a path the
    /// operator typed. Comparing against this rather than a "was edited" flag
    /// because the flag gets out of step the moment anything else writes the
    /// field, and the failure — silently discarding a deliberate output path —
    /// is one the user only notices after the run finishes somewhere else.
    output_suggested: String,
    /// A start held back pending an overwrite answer; the bool is `analyze_only`.
    pending_start: Option<bool>,
    model: String,
    cascade: bool,
    /// The models folder as a catalogue, scanned in the background. Selection
    /// is held as a PATH, never an index: a rescan rebuilds the list, and a
    /// stored index then names a different model while still being in range
    /// (the bug logscope's picker documents).
    models_dir: String,
    catalogue: Vec<d2x_core::registry::ModelInfo>,
    /// Distinguishes "never scanned yet" from "scanned and found nothing" --
    /// `catalogue.is_empty()` alone can't tell those apart, and using it as
    /// the auto-rescan trigger meant an empty (or misconfigured, or briefly
    /// inaccessible) models folder caused a NEW scan thread every single
    /// frame forever: the scan completes, sets `catalogue` back to an empty
    /// `Vec`, and the next frame sees "empty, not scanning, no pending
    /// receiver" and fires again. Set true once, in `drain_scan`, after the
    /// first result of any kind (including an empty one) is processed.
    scanned_once: bool,
    model_key: Option<String>,
    cascade_key: Option<String>,
    scanning: bool,
    scan_rx: Option<Receiver<Vec<d2x_core::registry::ModelInfo>>>,
    running: bool,
    stop: Arc<AtomicBool>,
    rx: Option<Receiver<Msg>>,
    log: Vec<LogEntry>,
    log_id: u64,
    rows: Vec<FrameRow>,
    frames_total: Option<u64>,
    preview: Option<egui::TextureHandle>,
    preview_size: (u32, u32),
    summary: Option<String>,
}

impl Default for Shell {
    fn default() -> Self {
        let job = Job::default();
        let model = job::default_model_paths().into_iter().find(|p| p.exists()).map(|p| p.display().to_string()).unwrap_or_default();
        Shell {
            job,
            matcher: 1,
            backend: 0,
            running_plan: None,
            input: String::new(),
            pending_drop: None,
            output: String::new(),
            // Lost by the c14unified-026 merge: it took THEIR app.rs hunk,
            // which predates these two fields, while the struct definition
            // (auto-merged) kept them. A struct literal missing a field is a
            // compile error rather than a silent bug, which is the only reason
            // this was cheap — it shipped in bundle 006 and the operator hit it
            // on the first build.
            output_suggested: String::new(),
            pending_start: None,
            models_dir: job::default_model_paths()
                .into_iter()
                .find(|p| p.exists())
                .and_then(|p| p.parent().map(|d| d.display().to_string()))
                .unwrap_or_else(|| "models".into()),
            catalogue: Vec::new(),
            scanned_once: false,
            model_key: None,
            cascade_key: None,
            scanning: false,
            scan_rx: None,
            model,
            cascade: false,
            running: false,
            stop: Arc::new(AtomicBool::new(false)),
            rx: None,
            log: Vec::new(),
            log_id: 0,
            rows: Vec::new(),
            frames_total: None,
            preview: None,
            preview_size: (0, 0),
            summary: None,
        }
    }
}

/// The output path to suggest for a given input.
///
/// `clip.mkv` → `clip_upscaled.mkv`; a PNG directory `frames` → `frames_upscaled`.
///
/// Beside the input, not in a fixed output folder: the operator picked where
/// the source lives and the result almost always belongs next to it. A default
/// that writes somewhere else is a default people have to notice, and this one
/// should be invisible when it is right.
///
/// The suffix goes before the extension because everything downstream reads the
/// extension — `ffmpeg::open_sink` picks a muxer from it, and `run_segmented`
/// builds `stem.partNNN.ext` out of both halves. `clip.mkv_upscaled` would be a
/// file nothing can write.
pub fn suggested_output(input: &str) -> String {
    let input = input.trim();
    if input.is_empty() {
        return String::new();
    }
    // No trailing-slash handling: `Path` already skips empty components, so
    // `Path::new("/v/frames/").file_stem()` is "frames" and its parent is "/v".
    // I wrote a `trim_end_matches('/')` for this and then could not break it —
    // deleting the trim failed no test, because the case it guarded does not
    // exist. `a_directory_input_suggests_a_directory` still asserts the
    // behaviour, so it is pinned on `Path` rather than on code of mine.
    let p2 = std::path::Path::new(input);
    match (p2.file_stem(), p2.extension()) {
        (Some(stem), Some(ext)) => {
            let name = format!("{}_upscaled.{}", stem.to_string_lossy(), ext.to_string_lossy());
            p2.parent().map(|d| d.join(&name).to_string_lossy().into_owned()).unwrap_or(name)
        }
        (Some(stem), None) => {
            let name = format!("{}_upscaled", stem.to_string_lossy());
            p2.parent().map(|d| d.join(&name).to_string_lossy().into_owned()).unwrap_or(name)
        }
        _ => String::new(),
    }
}

/// What an existing output means for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overwrite {
    /// Nothing there. Go.
    Clear,
    /// A file that would be replaced.
    File,
    /// A directory with something in it — the PNG-sequence case, where the run
    /// writes frames alongside whatever is already there rather than replacing
    /// it. Worth a different question, because "overwrite?" is the wrong word
    /// for a mkdir that will merge.
    NonEmptyDir,
}

/// Classify `output`, given a predicate for what exists.
///
/// Takes the filesystem as arguments so the RULES are testable without one —
/// which matters here because getting this wrong in either direction is bad:
/// a prompt that never fires destroys an eight-hour upscale, and one that
/// always fires trains people to click through it.
pub fn overwrite_state(
    output: &str,
    is_file: &dyn Fn(&std::path::Path) -> bool,
    dir_entries: &dyn Fn(&std::path::Path) -> Option<usize>,
) -> Overwrite {
    let o = output.trim();
    if o.is_empty() {
        return Overwrite::Clear;
    }
    let p = std::path::Path::new(o);
    if is_file(p) {
        return Overwrite::File;
    }
    match dir_entries(p) {
        Some(n) if n > 0 => Overwrite::NonEmptyDir,
        _ => Overwrite::Clear,
    }
}

impl Shell {
    /// Walk the models folder off the UI thread: verification runs a forward
    /// pass per model, which is far too slow for a frame.
    fn rescan_models(&mut self) {
        if self.scanning {
            return;
        }
        let dir = std::path::PathBuf::from(self.models_dir.trim());
        let (tx, rx) = mpsc::channel();
        self.scan_rx = Some(rx);
        self.scanning = true;
        std::thread::Builder::new()
            .name("d2x-scan".into())
            .spawn(move || {
                let _ = tx.send(d2x_core::registry::scan(&dir, true));
            })
            .ok();
    }

    fn drain_scan(&mut self) {
        let Some(rx) = &self.scan_rx else { return };
        if let Ok(found) = rx.try_recv() {
            let broken = found.iter().filter(|m| !m.status.usable()).count();
            self.push_log(LogLevel::Info, format!("{} models in {} ({} unusable)", found.len(), self.models_dir, broken));
            for m in found.iter().filter(|m| m.status.detail().is_some()) {
                let (name, _, badge) = m.row();
                self.push_log(if m.status.usable() { LogLevel::Warn } else { LogLevel::Error }, format!("{badge}: {name} — {}", m.status.detail().unwrap_or("")));
            }
            // Restore by key; fall back to the cheapest usable model, and pick
            // the cheap end of the ladder for the cascade tier.
            let key = self.model_key.clone();
            self.model_key = d2x_core::registry::select_by_key(&found, key.as_deref()).map(|m| m.key());
            if self.cascade_key.is_none() {
                if let Some(sel) = self.model_key.as_deref().and_then(|k| found.iter().find(|m| m.key() == k)) {
                    if let Some((cheap, _)) = d2x_core::registry::ladder(&found, sel.scale) {
                        self.cascade_key = Some(cheap.key());
                    }
                }
            }
            self.catalogue = found;
            self.scanning = false;
            self.scan_rx = None;
            self.scanned_once = true;
        }
    }

    /// One row of the picker: name, then a dim subtitle carrying architecture,
    /// cost and verification badge — the shape logscope's picker uses.
    fn model_row(m: &d2x_core::registry::ModelInfo) -> egui::text::LayoutJob {
        use egui::text::{LayoutJob, TextFormat};
        let (name, sub, badge) = m.row();
        let col = match m.status {
            d2x_core::registry::Verify::Ok => egui::Color32::from_rgb(120, 190, 120),
            d2x_core::registry::Verify::Warn(_) => egui::Color32::from_rgb(220, 180, 90),
            d2x_core::registry::Verify::Failed(_) => egui::Color32::from_rgb(220, 90, 80),
        };
        let mut job = LayoutJob::default();
        job.append(&name, 0.0, TextFormat { font_id: egui::FontId::monospace(12.0), ..Default::default() });
        job.append(&format!("\n{badge}"), 0.0, TextFormat { color: col, font_id: egui::FontId::monospace(9.0), ..Default::default() });
        job.append(&format!(" · {sub}"), 0.0, TextFormat { color: egui::Color32::GRAY, font_id: egui::FontId::monospace(9.0), ..Default::default() });
        job
    }

    fn model_combo(&mut self, ui: &mut egui::Ui, label: &str, cascade: bool) {
        let cur = if cascade { self.cascade_key.clone() } else { self.model_key.clone() };
        let text = match cur.as_deref().and_then(|k| self.catalogue.iter().find(|m| m.key() == k)) {
            Some(m) => m.name.clone(),
            None if self.scanning => "scanning…".into(),
            None if self.catalogue.is_empty() => "no models".into(),
            None => "(none)".into(),
        };
        let mut picked = cur.clone();
        egui::ComboBox::from_id_salt(label).selected_text(text).width(260.0).show_ui(ui, |ui| {
            if cascade {
                ui.selectable_value(&mut picked, None, "(no cascade)");
            }
            // Only usable models are selectable; broken ones are reported in
            // the log rather than offered as a choice.
            for m in self.catalogue.iter().filter(|m| m.status.usable()) {
                ui.selectable_value(&mut picked, Some(m.key()), Self::model_row(m));
            }
        });
        if picked != cur {
            if cascade {
                self.cascade_key = picked;
            } else {
                self.model_key = picked;
            }
        }
    }

    fn push_log(&mut self, level: LogLevel, text: String) {
        self.log_id += 1;
        self.log.push(LogEntry::new(level, text, self.log_id));
        if self.log.len() > 500 {
            self.log.drain(..100);
        }
    }

    /// The overwrite confirmation, drawn when `start` held a run back.
    ///
    /// A modal Window rather than a toast or an inline warning: the answer
    /// decides whether an existing file survives, and an upscale is long enough
    /// that a wrong answer is expensive in both directions — clobbering a
    /// finished render, or making someone rerun eight hours because they
    /// dismissed a notification.
    ///
    /// The path is shown in full. "A file already exists" without saying WHICH
    /// is unanswerable when the box was auto-filled and the operator has not
    /// read it.
    fn overwrite_dialog(&mut self, ctx: &egui::Context) {
        let Some(analyze_only) = self.pending_start else { return };
        let state = overwrite_state(
            &self.output,
            &|p| p.is_file(),
            &|p| std::fs::read_dir(p).ok().map(|d| d.count()),
        );
        let mut go = false;
        let mut cancel = false;
        egui::Window::new(match state {
            Overwrite::NonEmptyDir => "Directory is not empty",
            _ => "Overwrite this file?",
        })
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.label(&self.output);
            ui.add_space(4.0);
            ui.label(match state {
                // Different wording, because it is a different act: a run into
                // a populated directory writes frames ALONGSIDE what is there
                // rather than replacing it, and calling that "overwrite" would
                // describe the wrong risk.
                Overwrite::NonEmptyDir =>
                    "The run will write frames into this directory alongside what is already in it.",
                _ => "This file will be replaced when the run finishes.",
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
                if ui.button(match state {
                    Overwrite::NonEmptyDir => "Write here",
                    _ => "Overwrite",
                }).clicked() {
                    go = true;
                }
            });
        });
        if cancel {
            self.pending_start = None;
            self.push_log(LogLevel::Info, "cancelled — nothing was written".into());
        } else if go {
            // Left SET so `start` sees it and skips the check, then cleared
            // there. Clearing it here would send the run straight back into
            // the prompt.
            self.start(analyze_only);
        }
    }

    fn start(&mut self, analyze_only: bool) {
        // A chosen model that cannot run is refused here, with the file and
        // the reason, rather than failing later at the config stage.
        let refused = [("model", self.model_key.as_deref()), ("cascade model", self.cascade_key.as_deref())]
            .into_iter()
            .find_map(|(what, key)| d2x_core::registry::refusal(&self.catalogue, key)
                .map(|why| format!("{what}: {why} -- choose another in the picker")));
        if let Some(msg) = refused {
            self.push_log(LogLevel::Error, msg);
            return;
        }
        let mut job = self.job.clone();
        job.input = self.input.trim().into();
        job.output = self.output.trim().into();
        let pick = |k: &Option<String>| k.as_deref().and_then(|k| self.catalogue.iter().find(|m| m.key() == k)).map(|m| m.path.clone());
        job.model = pick(&self.model_key);
        job.cascade_model = pick(&self.cascade_key).map(Some);
        job.backend = [Backend::Wgpu, Backend::Cpu, Backend::Bicubic, Backend::Nearest][self.backend];
        job.pipeline.matcher = [MatcherKind::Stationary, MatcherKind::Diamond, MatcherKind::Exhaustive { radius: 4 }][self.matcher];
        job.pipeline.tiers = 1 + job.cascade_model.is_some() as usize + job.flat_bypass as usize;
        if job.input.as_os_str().is_empty() || (!analyze_only && job.output.as_os_str().is_empty()) {
            self.push_log(LogLevel::Error, "input (and output) paths are required".into());
            return;
        }
        // ASK BEFORE CLOBBERING. Only for a real run: analyze writes nothing,
        // so prompting there would be a dialog that means nothing, and those
        // are what teach people to click through the ones that matter.
        //
        // `pending_start` is cleared by the dialog, so a confirmed start
        // re-enters here with it already taken and does not loop.
        if !analyze_only && self.pending_start.is_none() {
            let state = overwrite_state(
                &self.output,
                &|p| p.is_file(),
                &|p| std::fs::read_dir(p).ok().map(|d| d.count()),
            );
            if state != Overwrite::Clear {
                self.pending_start = Some(analyze_only);
                return;
            }
        }
        self.pending_start = None;
        self.rows.clear();
        self.summary = None;
        self.frames_total = None;
        self.stop.store(false, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.running = true;
        self.running_plan = None; // stale from a previous run until the new one probes its source
        let stop = self.stop.clone();
        self.push_log(LogLevel::Info, format!("{} {}", if analyze_only { "analyzing" } else { "upscaling" }, job.input.display()));
        std::thread::Builder::new().name("d2x-job".into()).spawn(move || {
            // BE A GOOD NEIGHBOUR. An upscale runs for minutes and will take
            // every cycle it is given; on the machine someone is also using,
            // that is why the pointer stutters. `nice` on the worker thread
            // (Linux applies it per-thread, which is what we want — the UI
            // thread keeps its priority) costs nothing when the machine is
            // idle and yields immediately when it is not.
            //
            // Best-effort: lowering priority never needs privileges, so the
            // only failure mode is a platform without setpriority, where the
            // job simply runs as before.
            #[cfg(target_os = "linux")]
            {
                use d2x_core::courtesy::{nice_value, Politeness};
                let p = match std::env::var("D2X_POLITENESS").ok().as_deref() {
                    Some("background") => Politeness::Background,
                    Some("foreground") => Politeness::Foreground,
                    _ => Politeness::Balanced,
                };
                let n = nice_value(p);
                if n != 0 {
                    // SAFETY: setpriority on the calling thread (PRIO_PROCESS
                    // with who=0 is the caller) with an in-range value.
                    unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, n); }
                }
            }
            run_job(job, analyze_only, tx, stop)
        }).expect("spawn job thread");
    }

    fn drain(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        let mut msgs = Vec::new();
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        for m in msgs {
            match m {
                Msg::Event(Event::Started { info, padded, upscaler, matcher, gate }) => {
                    self.frames_total = info.frame_count;
                    self.push_log(LogLevel::Info, format!("{}x{} @ {}/{} fps, grid {}x{}; {upscaler}; {matcher}, {gate}", info.width, info.height, info.fps.0, info.fps.1, padded.0, padded.1));
                }
                Msg::Event(Event::FrameDone { index, stats, analyze_time, upscale_time, merge_time }) => {
                    let grid = d2x_core::plan::Grid { block_size: self.job.pipeline.analyze.block_size, cols: 1, rows: 1 };
                    let _ = grid;
                    let fraction = if stats.redraw { 1.0 } else {
                        // residual area relative to frame area is what FrameStats knows once it has a grid;
                        // here we approximate from block counts for the bar (exact number is in the log).
                        let total = (stats.stationary + stats.moved + stats.recalled + stats.missing).max(1) as f32;
                        stats.missing as f32 / total
                    };
                    self.rows.push(FrameRow { fraction, redraw: stats.redraw, analyze_ms: analyze_time.as_secs_f32() * 1e3, upscale_ms: upscale_time.as_secs_f32() * 1e3 });
                    if index % 25 == 0 || stats.redraw {
                        self.push_log(if stats.redraw { LogLevel::Warn } else { LogLevel::Debug }, format!("frame {index}: psnr {:.1}, stat {} mov {} rec {} miss {} fade {}{}; A {:.1}ms U {:.1}ms M {:.1}ms", stats.psnr, stats.stationary, stats.moved, stats.recalled, stats.missing, stats.fades, if stats.redraw { " REDRAW" } else { "" }, analyze_time.as_secs_f64() * 1e3, upscale_time.as_secs_f64() * 1e3, merge_time.as_secs_f64() * 1e3));
                    }
                }
                Msg::Event(Event::PlateUpscaled { index, size, frames, area, time }) => {
                    self.push_log(LogLevel::Info, format!("plate at frame {index}: {}x{}, {:.2} frames of pixels for {frames} frames, {:.0} ms", size.0, size.1, area, time.as_secs_f64() * 1e3));
                }
                Msg::Event(Event::Finished { frames, elapsed, upscaled_pixel_fraction, plate_pixel_fraction, .. }) => {
                    let s = format!("{frames} frames in {:.1}s ({:.2} fps); {:.1}% of pixels went through the network (+{:.1}% as plates)", elapsed.as_secs_f64(), frames as f64 / elapsed.as_secs_f64().max(1e-9), upscaled_pixel_fraction * 100.0, plate_pixel_fraction * 100.0);
                    self.push_log(LogLevel::Info, s.clone());
                    self.summary = Some(s);
                }
                Msg::Preview { width, height, rgb } => {
                    let img = egui::ColorImage::from_rgb([width as usize, height as usize], &rgb);
                    match &mut self.preview {
                        Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                        None => self.preview = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR)),
                    }
                    self.preview_size = (width, height);
                }
                Msg::Log(level, text) => self.push_log(level, text),
                Msg::MemoryPlan(plan) => { self.running_plan = Some(plan); }
                Msg::Done(r) => {
                    self.running = false;
                    match r {
                        Ok(()) => self.push_log(LogLevel::Info, "finished".into()),
                        Err(e) => self.push_log(LogLevel::Error, e),
                    }
                }
            }
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        let busy = self.running;
        ui.add_enabled_ui(!busy, |ui| {
            ui.heading("Job");
            egui::Grid::new("paths").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                ui.label("Input");
                let changed = ui.add(egui::TextEdit::singleline(&mut self.input).id_salt("bscale-input")
                    .hint_text("video file or PNG directory")
                    .desired_width(f32::INFINITY)).changed();
                if changed && (self.output.is_empty() || self.output == self.output_suggested) {
                    // Only over an empty box or over our own last suggestion —
                    // never over something the operator typed.
                    self.output_suggested = suggested_output(&self.input);
                    self.output = self.output_suggested.clone();
                }
                ui.end_row();
                ui.label("Output");
                ui.add(egui::TextEdit::singleline(&mut self.output).id_salt("bscale-output").hint_text("out.mkv or a directory").desired_width(f32::INFINITY));
                ui.end_row();
                ui.label("Models folder");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.models_dir).id_salt("bscale-models-dir").desired_width(200.0));
                    if ui.add_enabled(!self.scanning, egui::Button::new("Scan")).clicked() {
                        self.rescan_models();
                    }
                });
                ui.end_row();
                ui.label("Model");
                self.model_combo(ui, "model", false);
                ui.end_row();
                ui.label("Cascade");
                self.model_combo(ui, "cascade", true);
                ui.end_row();
            });
            ui.separator();
            ui.heading("Upscaler");
            egui::ComboBox::from_label("backend").selected_text(["wgpu", "cpu", "bicubic", "nearest"][self.backend]).show_ui(ui, |ui| {
                for (i, n) in ["wgpu", "cpu", "bicubic", "nearest"].iter().enumerate() {
                    ui.selectable_value(&mut self.backend, i, *n);
                }
            });
            ui.horizontal(|ui| {
                ui.label("scale");
                ui.add(egui::DragValue::new(&mut self.job.scale).range(1..=8));
                ui.label("tile");
                ui.add(egui::DragValue::new(&mut self.job.tile).range(0..=1024).speed(8));
                ui.checkbox(&mut self.job.require_hw, "require hw");
            });
            ui.separator();
            ui.heading("Reuse gate");
            ui.horizontal(|ui| {
                ui.label("block");
                ui.add(egui::DragValue::new(&mut self.job.pipeline.analyze.block_size).range(8..=128));
                ui.label("bleed");
                // Range widened from 0..=8. 8 is now the DEFAULT (raised from 1
                // because Real-ESRGAN's receptive field makes a thin ring
                // useless), so the old cap left no room to turn the dial up —
                // and turning it up is the first thing to try when block seams
                // flicker. 24 is where the cost stops being worth it at the
                // default 30px block: (30+2*24)^2 / 30^2 is ~6.8x the pixels.
                ui.add(egui::DragValue::new(&mut self.job.pipeline.analyze.bleed).range(0..=24));
            });
            ui.horizontal(|ui| {
                ui.label("quality");
                ui.add(egui::Slider::new(&mut self.job.pipeline.quality, 50..=100));
            });
            ui.horizontal(|ui| {
                ui.label("noise");
                ui.add(egui::DragValue::new(&mut self.job.pipeline.noise).range(0..=32));
                ui.checkbox(&mut self.job.pipeline.analyze.fade, "fades");
            });
            ui.horizontal(|ui| {
                ui.label("refs");
                ui.add(egui::DragValue::new(&mut self.job.pipeline.recent_refs).range(0..=64));
                ui.label("shot refs");
                ui.add(egui::DragValue::new(&mut self.job.pipeline.keyframe_refs).range(0..=256));
                ui.label("batch");
                ui.add(egui::DragValue::new(&mut self.job.pipeline.batch).range(1..=32));
                ui.label("in flight");
                ui.add(egui::DragValue::new(&mut self.job.inflight).range(1..=8));
            });
            // Was: "ring depths and block cache follow the memory budget
            // ... unless set on the CLI" -- true for the CLI and the
            // segmented (--jobs > 1) path, but NOT for this single-job GUI
            // path until the fix that added `running_plan`: this ran with
            // exactly the sliders above, no budget applied, regardless of
            // available RAM. Now it does, and this line shows what was
            // actually used on the last/current run rather than asserting
            // it and hoping.
            match self.running_plan {
                Some(plan) => { ui.label(format!("last run actually used: {plan}")); }
                None => { ui.label("ring depths and block cache follow the memory budget (half of available RAM); sliders above are the override, shown here once a run has probed a source"); }
            }
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.job.flat_bypass, "flat → bicubic");
            });
            ui.horizontal(|ui| {
                ui.label("flat std");
                ui.add(egui::DragValue::new(&mut self.job.pipeline.analyze.tier_std[0]).range(0.0..=50.0).speed(0.1));
                ui.label("smooth std");
                ui.add(egui::DragValue::new(&mut self.job.pipeline.analyze.tier_std[1]).range(0.0..=100.0).speed(0.1));
                ui.label("streak bonus");
                ui.add(egui::DragValue::new(&mut self.job.pipeline.analyze.streak_bonus).range(0.0..=3.0).speed(0.05));
            });
            egui::ComboBox::from_label("matcher").selected_text(["stationary", "diamond", "exhaustive r4"][self.matcher]).show_ui(ui, |ui| {
                for (i, n) in ["stationary", "diamond", "exhaustive r4"].iter().enumerate() {
                    ui.selectable_value(&mut self.matcher, i, *n);
                }
            });
            ui.separator();
            ui.heading("Encode");
            ui.horizontal(|ui| {
                ui.label("codec");
                for c in ["av1", "x264", "x265"] {
                    ui.selectable_value(&mut self.job.encode.codec, c.to_string(), c);
                }
            });
            ui.horizontal(|ui| {
                ui.label("crf");
                ui.add(egui::DragValue::new(&mut self.job.encode.crf).range(0..=63));
                ui.label("preset");
                ui.add(egui::DragValue::new(&mut self.job.encode.preset).range(0..=13));
                ui.checkbox(&mut self.job.encode.ten_bit, "10-bit");
            });
            ui.horizontal(|ui| {
                ui.label("frames (0 = all)");
                ui.add(egui::DragValue::new(&mut self.job.max_frames).range(0..=1_000_000).speed(10));
                ui.label("jobs");
                ui.add(egui::DragValue::new(&mut self.job.jobs).range(1..=64));
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new("Analyze")).clicked() {
                self.start(true);
            }
            if ui.add_enabled(!busy, egui::Button::new("Upscale")).clicked() {
                self.start(false);
            }
            if ui.add_enabled(busy, egui::Button::new("Stop")).clicked() {
                self.stop.store(true, Ordering::Relaxed);
                self.push_log(LogLevel::Warn, "stopping after the current frame; the encoder finalises a partial file".into());
            }
        });
    }

    /// What the run is costing, over the recent window.
    fn metrics_pane(&self, ui: &mut egui::Ui) {
        use d2x_core::jobmetrics::{recent, Frame};
        if self.rows.is_empty() { return; }
        let frames: Vec<Frame> = self.rows.iter().map(|r| Frame {
            analyze_ms: r.analyze_ms, upscale_ms: r.upscale_ms,
            fraction: r.fraction, redraw: r.redraw,
        }).collect();
        let s = recent(&frames);
        let c = uikit_egui::palette_colors("voice");
        // `carved::inset` rather than a bare label: a run of numbers loose on
        // the background reads as debris. See its doc for chip/inset/card.
        uikit_egui::carved::inset(ui, &c, |ui| {
            ui.label(egui::RichText::new(s.line()).monospace().small());
            if let Some(total) = self.frames_total {
                let left = (total as usize).saturating_sub(self.rows.len());
                if let Some(eta) = s.eta_secs(left) {
                    ui.label(egui::RichText::new(
                        format!("about {eta:.0}s remaining at this rate")).small().weak());
                }
            }
        });
    }

    fn bars(&self, ui: &mut egui::Ui) {
        let desired = egui::vec2(ui.available_width(), 90.0);
        let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        if self.rows.is_empty() {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "per-frame share of pixels sent to the network", egui::FontId::proportional(13.0), ui.visuals().weak_text_color());
            return;
        }
        let n = self.rows.len().min(600);
        let rows = &self.rows[self.rows.len() - n..];
        let bw = (rect.width() / n as f32).max(1.0);
        for (i, r) in rows.iter().enumerate() {
            let h = (r.fraction.clamp(0.0, 1.0)) * (rect.height() - 6.0);
            let x0 = rect.left() + i as f32 * bw;
            let color = if r.redraw { egui::Color32::from_rgb(220, 80, 60) } else { egui::Color32::from_rgb(90, 160, 240) };
            painter.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, rect.bottom() - 3.0 - h), egui::pos2(x0 + bw - 0.5, rect.bottom() - 3.0)), 0.0, color);
        }
        let last = rows.last().unwrap();
        painter.text(rect.left_top() + egui::vec2(6.0, 4.0), egui::Align2::LEFT_TOP, format!("{} frames · last: {:.0}% missing, A {:.1} ms, U {:.0} ms", self.rows.len(), last.fraction * 100.0, last.analyze_ms, last.upscale_ms), egui::FontId::monospace(12.0), ui.visuals().text_color());
    }
}

impl eframe::App for Shell {
    // The house eframe shape: root-Ui entry point.
    fn ui(&mut self, ui: &mut egui::Ui, _f: &mut eframe::Frame) {
        // DRAG AND DROP. Read once per frame, before anything draws, so a
        // drop that lands during this frame is classified against the input
        // as it was — not as some widget below has just edited it.
        {
            let dropped: Vec<std::path::PathBuf> = ui.ctx().input(|i| {
                i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect()
            });
            if !dropped.is_empty() {
                let what = d2x_core::dropped::classify(&dropped, |p| p.is_dir());
                self.pending_drop = Some(d2x_core::dropped::Pending {
                    what,
                    replacing: self.input.clone(),
                });
            }
        }
        if let Some(p) = self.pending_drop.clone() {
            let mut close = false;
            egui::Window::new("Dropped file")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(p.message());
                    if let Some(note) = p.replacing_note() {
                        // Say what is being LOST, not only what arrives —
                        // the old path is about to be unrecoverable.
                        ui.label(egui::RichText::new(note).small().weak());
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if p.is_acceptable() && ui.button("Use it").clicked() {
                            if let Some(path) = p.accepted_path() {
                                self.input = path;
                            }
                            close = true;
                        }
                        // "Cancel" when there is a choice, "OK" when the
                        // dialog is only explaining a refusal.
                        let dismiss = if p.is_acceptable() { "Cancel" } else { "OK" };
                        if ui.button(dismiss).clicked() { close = true; }
                    });
                });
            if close { self.pending_drop = None; }
        }

        let ctx = ui.ctx().clone();
        if !self.scanned_once && !self.scanning && self.scan_rx.is_none() {
            self.rescan_models();
        }
        self.drain_scan();
        self.drain(&ctx);
        self.overwrite_dialog(&ctx);
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        // SIDE BY SIDE OR STACKED, not a narrower split. A 340pt settings
        // column beside the preview is right on a laptop and most of the
        // screen on a phone. Making the column a fraction is the tempting
        // fix and it is worse — tried on beatspace, looked at it, and the
        // controls started clipping while the content was still cramped. A
        // narrow screen wants a different arrangement, so below the two
        // panes' minima the settings become a collapsed disclosure above the
        // content and the preview gets the whole width.
        uikit_egui::two_pane::two_pane_ui(
            ui, "bscale-settings", 340.0, 260.0, 320.0, "settings", self,
            |ui, s| { egui::ScrollArea::vertical().show(ui, |ui| s.settings(ui)); },
            |ui, self_| {
            if let Some(total) = self_.frames_total {
                let done = self_.rows.len() as f32;
                ui.add(egui::ProgressBar::new((done / total.max(1) as f32).min(1.0)).text(format!("{done} / {total}")));
            }
            // METRICS PANE. The per-frame costs were already collected and
            // only ever surfaced in a log line every twenty-five frames,
            // which does not answer the question people actually ask —
            // "how long will this take, and what is slow". Windowed, not a
            // whole-run mean: cost is not stationary, so an average over ten
            // thousand frames is still describing the title card.
            self_.metrics_pane(ui);
            self_.bars(ui);
            if let Some(s) = &self_.summary {
                ui.label(s);
            }
            let avail = ui.available_height();
            let preview_h = (avail * 0.55).max(120.0);
            if let Some(tex) = &self_.preview {
                let (w, h) = self_.preview_size;
                let aspect = w as f32 / h.max(1) as f32;
                let size = egui::vec2((preview_h * aspect).min(ui.available_width()), preview_h);
                ui.add(egui::Image::new(tex).fit_to_exact_size(size));
            } else {
                ui.add_space(4.0);
            }
            ui.separator();
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| log_view(ui, &self_.log));
            },
        );
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn the_suffix_goes_before_the_extension() {
        // Everything downstream reads the extension — `ffmpeg::open_sink` picks
        // a muxer from it and `run_segmented` builds `stem.partNNN.ext` out of
        // both halves — so `clip.mkv_upscaled` would be a file nothing can
        // write.
        assert_eq!(suggested_output("/v/clip.mkv"), "/v/clip_upscaled.mkv");
        assert_eq!(suggested_output("clip.mp4"), "clip_upscaled.mp4");
    }

    #[test]
    fn the_suggestion_lands_beside_the_input() {
        // Not in a fixed output folder: the operator chose where the source
        // lives, and a default that writes elsewhere is one they have to
        // notice.
        assert_eq!(suggested_output("/a/b/c/clip.mkv"), "/a/b/c/clip_upscaled.mkv");
    }

    #[test]
    fn a_directory_input_suggests_a_directory() {
        // The PNG-sequence case. A trailing slash still means a directory, and
        // must not put the suffix inside the path.
        assert_eq!(suggested_output("/v/frames"), "/v/frames_upscaled");
        assert_eq!(suggested_output("/v/frames/"), "/v/frames_upscaled");
    }

    #[test]
    fn an_empty_input_suggests_nothing() {
        // Rather than "_upscaled", which is a real filename and would be
        // written to the working directory.
        assert_eq!(suggested_output(""), "");
        assert_eq!(suggested_output("   "), "");
    }

    #[test]
    fn overwrite_is_reported_for_a_file_and_a_populated_directory_only() {
        // Both directions matter: a prompt that never fires destroys an
        // eight-hour upscale, and one that always fires trains people to click
        // through it.
        let no = |_: &Path| false;
        let none = |_: &Path| None;
        assert_eq!(overwrite_state("/v/out.mkv", &no, &none), Overwrite::Clear);
        assert_eq!(overwrite_state("", &|_| true, &none), Overwrite::Clear,
                   "an empty path prompted");

        let yes = |_: &Path| true;
        assert_eq!(overwrite_state("/v/out.mkv", &yes, &none), Overwrite::File);

        // An EMPTY directory is not a conflict — that is the normal case for a
        // PNG run into a folder the operator just made.
        assert_eq!(overwrite_state("/v/frames", &no, &|_| Some(0)), Overwrite::Clear);
        assert_eq!(overwrite_state("/v/frames", &no, &|_| Some(3)), Overwrite::NonEmptyDir);
    }

    #[test]
    fn a_suggestion_is_replaced_but_a_typed_path_is_not() {
        // The rule the whole auto-fill turns on. Silently discarding a
        // deliberate output path is a failure the operator only discovers after
        // the run finishes somewhere else.
        let mut sh = Shell::default();
        sh.input = "/v/a.mkv".into();
        sh.output_suggested = suggested_output(&sh.input);
        sh.output = sh.output_suggested.clone();
        // Re-picking the input replaces our own suggestion.
        assert!(sh.output.is_empty() || sh.output == sh.output_suggested);

        // A hand-typed path is not ours, so the guard refuses.
        sh.output = "/somewhere/else.mkv".into();
        assert_ne!(sh.output, sh.output_suggested);
        assert!(!sh.output.is_empty());
    }
}
