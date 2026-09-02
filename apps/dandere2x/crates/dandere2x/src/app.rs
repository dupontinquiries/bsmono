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

/// Must match packaging/org.dandere2x.Dandere2x.desktop exactly — a mismatch
/// fails silently, leaving a working app with no icon.
const APP_ID: &str = "org.dandere2x.Dandere2x";

pub fn run() -> anyhow::Result<()> {
    let opts = eframe::NativeOptions {
        // Glow SAID, not left to eframe: wgpu's default device descriptor
        // asks for 8 colour attachments, which dies on a phone GPU allowing 5.
        renderer: eframe::Renderer::Glow,
        viewport: uikit_egui::desktop::viewport(APP_ID, "dandere2x", [1100.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native("dandere2x", opts, Box::new(|cc| {
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

fn run_job(job: Job, analyze_only: bool, tx: Sender<Msg>, stop: Arc<AtomicBool>) {
    let r = (|| -> anyhow::Result<()> {
        if analyze_only {
            let _ = tx.send(Msg::Log(LogLevel::Info, "probing".into()));
            let (source, _) = job::open_source(&job)?;
            let source: Box<dyn FrameSource> = Box::new(StoppableSource { inner: source, stop });
            let s = d2x_core::pipeline::analyze_only(source, job.pipeline, |i, st, _frac, t| {
                let _ = tx.send(Msg::Event(Event::FrameDone { index: i, stats: *st, analyze_time: t, upscale_time: Duration::ZERO, merge_time: Duration::ZERO }));
            })?;
            let _ = tx.send(Msg::Event(Event::Finished { frames: s.frames, elapsed: s.elapsed, upscaled_pixel_fraction: s.upscaled_pixel_fraction }));
            return Ok(());
        }
        if job.jobs > 1 {
            let _ = tx.send(Msg::Log(LogLevel::Info, format!("{} segment workers, one shared upscaler; parts are concatenated at the end (no preview in this mode)", job.jobs)));
            let tx2 = tx.clone();
            job::run(&job, move |e| {
                let _ = match e {
                    job::Event::Stage(m) => tx2.send(Msg::Log(LogLevel::Info, m)),
                    job::Event::Pipeline(e) | job::Event::Segment(_, e) => tx2.send(Msg::Event(e)),
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
        let upscaler = job::build_upscaler(&job)?;
        let scale = upscaler.scale();
        let inner = job::open_sink(&job, probe.as_ref(), &keyframes, info.width * scale, info.height * scale)?;
        let sink: Box<dyn FrameSink> = Box::new(TeeSink { inner, tx: tx.clone(), every: 4, n: 0 });
        let _ = tx.send(Msg::Log(LogLevel::Info, "frame 0 is a full-frame upscale; later frames only pay for what changed".into()));
        let tx2 = tx.clone();
        d2x_core::pipeline::run(source, upscaler, sink, job.pipeline, move |e| {
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
    input: String,
    output: String,
    model: String,
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
            input: String::new(),
            output: String::new(),
            model,
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

impl Shell {
    fn push_log(&mut self, level: LogLevel, text: String) {
        self.log_id += 1;
        self.log.push(LogEntry::new(level, text, self.log_id));
        if self.log.len() > 500 {
            self.log.drain(..100);
        }
    }

    fn start(&mut self, analyze_only: bool) {
        let mut job = self.job.clone();
        job.input = self.input.trim().into();
        job.output = self.output.trim().into();
        job.model = if self.model.trim().is_empty() { None } else { Some(self.model.trim().into()) };
        job.backend = [Backend::Wgpu, Backend::Cpu, Backend::Bicubic, Backend::Nearest][self.backend];
        job.pipeline.matcher = [MatcherKind::Stationary, MatcherKind::Diamond, MatcherKind::Exhaustive { radius: 4 }][self.matcher];
        if job.input.as_os_str().is_empty() || (!analyze_only && job.output.as_os_str().is_empty()) {
            self.push_log(LogLevel::Error, "input (and output) paths are required".into());
            return;
        }
        self.rows.clear();
        self.summary = None;
        self.frames_total = None;
        self.stop.store(false, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.running = true;
        let stop = self.stop.clone();
        self.push_log(LogLevel::Info, format!("{} {}", if analyze_only { "analyzing" } else { "upscaling" }, job.input.display()));
        std::thread::Builder::new().name("d2x-job".into()).spawn(move || run_job(job, analyze_only, tx, stop)).expect("spawn job thread");
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
                Msg::Event(Event::Finished { frames, elapsed, upscaled_pixel_fraction }) => {
                    let s = format!("{frames} frames in {:.1}s ({:.2} fps); {:.1}% of pixels went through the network", elapsed.as_secs_f64(), frames as f64 / elapsed.as_secs_f64().max(1e-9), upscaled_pixel_fraction * 100.0);
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
                ui.add(egui::TextEdit::singleline(&mut self.input).hint_text("video file or PNG directory").desired_width(f32::INFINITY));
                ui.end_row();
                ui.label("Output");
                ui.add(egui::TextEdit::singleline(&mut self.output).hint_text("out.mkv or a directory").desired_width(f32::INFINITY));
                ui.end_row();
                ui.label("Model");
                ui.add(egui::TextEdit::singleline(&mut self.model).hint_text(".safetensors / .json").desired_width(f32::INFINITY));
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
                ui.add(egui::DragValue::new(&mut self.job.pipeline.analyze.bleed).range(0..=8));
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
        let ctx = ui.ctx().clone();
        self.drain(&ctx);
        if self.running {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        egui::Panel::left("settings").resizable(true).default_size(340.0).show_inside(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.settings(ui));
        });
        egui::CentralPanel::default().show_inside(ui, |ui| {
            if let Some(total) = self.frames_total {
                let done = self.rows.len() as f32;
                ui.add(egui::ProgressBar::new((done / total.max(1) as f32).min(1.0)).text(format!("{done} / {total}")));
            }
            self.bars(ui);
            if let Some(s) = &self.summary {
                ui.label(s);
            }
            let avail = ui.available_height();
            let preview_h = (avail * 0.55).max(120.0);
            if let Some(tex) = &self.preview {
                let (w, h) = self.preview_size;
                let aspect = w as f32 / h.max(1) as f32;
                let size = egui::vec2((preview_h * aspect).min(ui.available_width()), preview_h);
                ui.add(egui::Image::new(tex).fit_to_exact_size(size));
            } else {
                ui.add_space(4.0);
            }
            ui.separator();
            egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| log_view(ui, &self.log));
        });
    }
}
