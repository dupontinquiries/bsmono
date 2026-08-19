//! The realtime latent-space explorer.
//!
//! Layout: a big square "latent pad" you drag your pointer across. X/Y map to
//! z[0]/z[1] in [-3, 3]; the scroll wheel nudges z[2]. A background thread keeps
//! generating beats from a smoothed latent vector and crossfades between them
//! through the Player, so dragging morphs the sound in realtime. Set A / Set B
//! capture two points and Walk A->B auto-interpolates between them.
//!
//! Threading:
//!   * GUI (this, main thread — eframe requires it) writes `target_z` and reads
//!     back `current_z`, gen time, and a scope buffer via one shared Mutex.
//!   * The generator thread owns the candle Generator, the Mel front-end, the
//!     cpal Stream and the Player. It smooths current_z toward target_z, runs
//!     generate -> Griffin-Lim, and pushes each beat to the Player.

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use candle_core::Device;
use eframe::egui;

use crate::config::Config;
use crate::dsp::{GlState, Mel};
use crate::model::{load_generator, Generator};
use crate::{audio_io, player};

struct Shared {
    target_z: Vec<f32>,
    current_z: Vec<f32>,
    gen_ms: f32,
    scope: Vec<f32>,
    save_request: Option<PathBuf>,
    weights_request: Option<PathBuf>,
    loaded_label: String,
    device_label: String,
    /// Output gain the generator thread applies to its player, 0..1.
    ///
    /// Routed through Shared rather than by handing the GUI an Arc<Player>: the
    /// player is created inside the audio thread and its lock is taken by the
    /// cpal callback on every sample. A GUI holding that lock for a frame is an
    /// audio dropout, so the GUI writes a number here and the generator thread
    /// applies it between beats, where the lock is already being taken.
    gain: f32,
    /// Playback stopped. Distinct from gain 0 — see Player::set_stopped.
    stopped: bool,
}

pub fn run(cfg: Config) -> Result<()> {
    let latent = cfg.latent;
    let shared = Arc::new(Mutex::new(Shared {
        target_z: vec![0.0; latent],
        current_z: vec![0.0; latent],
        gen_ms: 0.0,
        scope: vec![0.0; 512],
        save_request: None,
        weights_request: None,
        loaded_label: "(none)".into(),
        device_label: "?".into(),
        gain: 1.0,
        stopped: false,
    }));
    let stop = Arc::new(AtomicBool::new(false));

    // generator thread (owns audio + model)
    let gen_handle = {
        let shared = shared.clone();
        let stop = stop.clone();
        let cfg = cfg.clone();
        std::thread::spawn(move || {
            if let Err(e) = gen_loop(cfg, shared, stop) {
                eprintln!("[gui] generator thread error: {e:?}");
            }
        })
    };

    let app = App::new(cfg, shared.clone());
    let mut opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([720.0, 500.0]),
        ..Default::default()
    };
    // Ask the device for exactly what THIS adapter offers — never for wgpu's defaults.
    //
    // eframe 0.35 renders through wgpu by default (its default feature list carries
    // `wgpu`, not `glow`), and its stock device_descriptor requests
    // `wgpu::Limits::default()` on every non-GL backend. Those defaults include
    // max_color_attachments = 8, so on an adapter that allows fewer, device creation
    // fails before the first frame ever draws:
    //     WGPU error: Limit 'max_color_attachments' value 8 is better than allowed 5
    // The REQUEST is the bug, not the hardware: egui renders into a single color
    // attachment and beatspace adds none. `adapter.limits()` is supported by
    // definition, which is the same fix already carried in logscope's gpu-gemm
    // ("Requesting the adapter's own limits is always valid") — see
    // maintenance/ASAHI_VULKAN_LESSONS.md.
    if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut opts.wgpu_options.wgpu_setup {
        setup.device_descriptor = Arc::new(|adapter: &eframe::wgpu::Adapter| {
            let info = adapter.get_info();
            // never-silent device selection (same lessons doc): print which GPU was
            // actually chosen, so "it won't start" / "it's slow" is one line of output
            // away from an answer instead of a guess.
            eprintln!(
                "[gpu] adapter: {} [{:?}] {:?}  driver: {} {}",
                info.name, info.backend, info.device_type, info.driver, info.driver_info
            );
            eframe::wgpu::DeviceDescriptor {
                label: Some("beatspace"),
                required_limits: adapter.limits(),
                ..Default::default()
            }
        });
    }
    let res = eframe::run_native("beatspace", opts, Box::new(move |_cc| Ok(Box::new(app))));

    stop.store(true, Ordering::Relaxed);
    let _ = gen_handle.join();
    res.map_err(|e| anyhow!("eframe: {e}"))?;
    Ok(())
}

// ----------------------------------------------------------------------------
// generator / audio thread
// ----------------------------------------------------------------------------

fn gen_loop(cfg: Config, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) -> Result<()> {
    let dev = Device::cuda_if_available(0).unwrap_or(Device::Cpu);
    {
        let mut s = shared.lock().unwrap();
        s.device_label = format!("{dev:?}");
    }

    let mut g = Generator::new(&cfg, &dev)?;
    // try to load the newest checkpoint
    let latest = cfg.ckpt_dir.join("gen_latest.safetensors");
    if latest.exists() {
        if let Err(e) = load_generator(&mut g, &latest, &dev) {
            eprintln!("[gui] failed to load {latest:?}: {e}");
        } else {
            shared.lock().unwrap().loaded_label = latest.display().to_string();
        }
    }

    let mel = Mel::new(&cfg);
    let mut gl = GlState::new(); // persistent phase for warm-started Griffin-Lim

    // audio out (keep the stream alive for the whole thread)
    let (_stream, audio) = player::start(cfg.sr)?;

    let latent = cfg.latent;
    let mut current_z = vec![0.0f32; latent];

    // One beat is `time_frames` hops long. Pace generation to a little under
    // the beat duration so windows overlap (the player crossfades them) and we
    // get continuous morphing without re-rendering audio nobody hears.
    let beat_dur = (cfg.time_frames * cfg.hop) as f32 / cfg.sr as f32;
    let target_period = std::time::Duration::from_secs_f32((beat_dur * 0.55).max(0.05));

    while !stop.load(Ordering::Relaxed) {
        let loop_start = Instant::now();
        // pull target + any pending requests
        let (target, weights_req, save_req) = {
            let mut s = shared.lock().unwrap();
            (
                s.target_z.clone(),
                s.weights_request.take(),
                s.save_request.take(),
            )
        };

        if let Some(path) = weights_req {
            match load_generator(&mut g, &path, &dev) {
                Ok(()) => shared.lock().unwrap().loaded_label = path.display().to_string(),
                Err(e) => eprintln!("[gui] load weights {path:?}: {e}"),
            }
        }

        // exponential smoothing toward target (the "thumb glides" feel)
        for i in 0..latent {
            current_z[i] = 0.9 * current_z[i] + 0.1 * target[i];
        }

        let t0 = Instant::now();
        let (flat, _n_mels, t) = g.generate(&current_z, &dev)?;
        // warm-started, low-iteration inversion: fast and smooth while morphing
        let wav = mel.mel_to_wav_warm(&flat, t, cfg.griffin_iters_rt, &mut gl);
        let gen_ms = t0.elapsed().as_secs_f32() * 1000.0;

        // Apply the GUI's transport in the same lock as the push, between
        // beats. The cpal callback takes this lock per sample, so a GUI thread
        // holding it for a frame is an audible dropout — this is the one place
        // the lock is already being taken for a long-lived reason.
        {
            let (gain, stopped) = {
                let s = shared.lock().unwrap();
                (s.gain, s.stopped)
            };
            let mut p = audio.lock().unwrap();
            p.set_gain(gain);
            p.set_stopped(stopped);
            p.push(&wav);
        }

        if let Some(path) = save_req {
            // render a clean, high-quality version from the same latent (fresh
            // phase, full iterations) rather than saving the realtime preview
            let hq = mel.mel_to_wav(&flat, t, cfg.griffin_iters);
            if let Err(e) = audio_io::save_wav(&path, &hq, cfg.sr) {
                eprintln!("[gui] save wav {path:?}: {e}");
            } else {
                println!("[gui] saved {path:?} ({} iters)", cfg.griffin_iters);
            }
        }

        let scope = audio.lock().unwrap().scope_snapshot();
        {
            let mut s = shared.lock().unwrap();
            s.current_z = current_z.clone();
            s.gen_ms = gen_ms;
            s.scope = scope;
        }

        // pace to the beat period (minus the work we just did) so the gen rate
        // tracks playback instead of spinning the CPU.
        if let Some(rem) = target_period.checked_sub(loop_start.elapsed()) {
            std::thread::sleep(rem);
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------
// GUI
// ----------------------------------------------------------------------------

/// Which page is showing.
///
/// Two, because two exist. A tab bar listing pages that are not built is the
/// overstatement the port audits kept finding elsewhere in this tree.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Morph,
    Train,
}

struct App {
    /// Held for the training page: it needs the data and checkpoint dirs to
    /// launch a run, and Shared carries neither.
    cfg: Config,
    /// The level to restore when unmuting. Dragging a slider to zero to
    /// silence something otherwise loses the level you had chosen.
    last_gain: f32,
    page: Page,
    /// The training dashboard's state. Fed from the trainer's own stdout.
    train: crate::trainpage::TrainPage,
    /// The mel cache on disk. Rescanned on a timer rather than per frame —
    /// a read_dir over thousands of entries every frame is a stall the page
    /// does not need, and the cache changes on the scale of seconds.
    cache: crate::trainpage::CacheView,
    cache_checked: Option<std::time::Instant>,
    /// GPU facts, parsed from the trainer's own output.
    gpu: crate::trainpage::GpuView,
    /// Lines arriving from a running trainer.
    train_rx: Option<std::sync::mpsc::Receiver<String>>,
    train_epochs: String,
    train_batch: String,
    latent: usize,
    shared: Arc<Mutex<Shared>>,
    z_a: Vec<f32>,
    z_b: Vec<f32>,
    walk_t: Option<f32>,
    walk_speed: f32,
    ckpt_text: String,
    save_text: String,
    status: String,
}

const RANGE: f32 = 3.0; // pad maps to [-RANGE, RANGE]

impl App {
    fn new(cfg: Config, shared: Arc<Mutex<Shared>>) -> Self {
        let latent = cfg.latent;
        // Computed before the struct literal moves cfg into it.
        let ckpt_text = cfg
            .ckpt_dir
            .join("gen_latest.safetensors")
            .display()
            .to_string();
        let train_batch = cfg.batch_size.to_string();
        Self {
            last_gain: 1.0,
            page: Page::Morph,
            train: Default::default(),
            cache: Default::default(),
            cache_checked: None,
            gpu: Default::default(),
            train_rx: None,
            train_epochs: "20000".into(),
            train_batch,
            cfg,
            latent,
            shared,
            z_a: vec![0.0; latent],
            z_b: vec![0.0; latent],
            walk_t: None,
            walk_speed: 0.01,
            ckpt_text,
            save_text: "beat.wav".into(),
            status: "drag on the pad to morph".into(),
        }
    }

    fn randomize(&mut self) {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut s = self.shared.lock().unwrap();
        for v in s.target_z.iter_mut() {
            *v = rng.gen_range(-RANGE..RANGE);
        }
        self.walk_t = None;
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.ctx().request_repaint(); // keep animating

        // snapshot shared state
        let (current_z, target_snapshot, gen_ms, scope, loaded, device) = {
            let s = self.shared.lock().unwrap();
            (
                s.current_z.clone(),
                s.target_z.clone(),
                s.gen_ms,
                s.scope.clone(),
                s.loaded_label.clone(),
                s.device_label.clone(),
            )
        };

        // advance an active walk A->B
        if let Some(t) = self.walk_t {
            let nt = t + self.walk_speed;
            let mut s = self.shared.lock().unwrap();
            for i in 0..self.latent {
                s.target_z[i] = self.z_a[i] + (self.z_b[i] - self.z_a[i]) * nt.min(1.0);
            }
            drop(s);
            self.walk_t = if nt >= 1.0 { None } else { Some(nt) };
        }

        // Drain whatever the trainer has printed since the last frame.
        //
        // try_recv in a loop rather than a blocking read: this runs inside the
        // frame, and a trainer that has gone quiet must not freeze the UI —
        // which is the whole reason the run is on a thread.
        if let Some(rx) = &self.train_rx {
            let mut lines = Vec::new();
            while let Ok(l) = rx.try_recv() {
                lines.push(l);
            }
            for l in lines {
                self.gpu.feed_gpu(&l);
                self.train.feed(&l);
            }
        }

        egui::Panel::left("controls")
            .resizable(false)
            .exact_size(250.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading("beatspace");
                ui.label("latent-space beat morpher");
                ui.horizontal(|ui| {
                    let was = self.page;
                    ui.selectable_value(&mut self.page, Page::Morph, "morph");
                    ui.selectable_value(&mut self.page, Page::Train, "train");
                    // ARRIVING AT THE TRAINING PAGE STOPS THE BEAT, once, on the
                    // transition. Training is a long watch and a loop playing
                    // under it is the thing you reach to silence — and the reach
                    // is to the system volume, which silences everything else
                    // too. Doing it on the EDGE rather than every frame is what
                    // lets the operator press play again and be heard.
                    if was != self.page && self.page == Page::Train {
                        self.shared.lock().unwrap().stopped = true;
                        self.status = "playback stopped for training".into();
                    }
                });
                self.transport(ui);
                ui.separator();
                if self.page == Page::Train {
                    // The transport is drawn above, before this branch, so both
                    // pages get the same one rather than a second pair of buttons
                    // that could disagree with it.
                    self.train_controls(ui);
                    return;
                }

                if ui.button("🎲  Randomize all dims").clicked() {
                    self.randomize();
                    self.status = "randomized latent".into();
                }
                ui.horizontal(|ui| {
                    if ui.button("Set A").clicked() {
                        self.z_a = target_snapshot.clone();
                        self.status = "captured A".into();
                    }
                    if ui.button("Set B").clicked() {
                        self.z_b = target_snapshot.clone();
                        self.status = "captured B".into();
                    }
                });
                if ui.button("▶  Walk A → B").clicked() {
                    self.walk_t = Some(0.0);
                    self.status = "walking A → B".into();
                }
                ui.add(
                    egui::Slider::new(&mut self.walk_speed, 0.001..=0.05)
                        .text("walk speed")
                        .logarithmic(true),
                );

                ui.separator();
                ui.label("Generator weights:");
                ui.add(egui::TextEdit::singleline(&mut self.ckpt_text).desired_width(230.0));
                if ui.button("Load weights").clicked() {
                    let p = PathBuf::from(self.ckpt_text.trim());
                    if p.exists() {
                        self.shared.lock().unwrap().weights_request = Some(p);
                        self.status = "loading weights…".into();
                    } else {
                        self.status = "weights path not found".into();
                    }
                }
                ui.label(egui::RichText::new(format!("loaded: {loaded}")).small().weak());

                ui.separator();
                ui.label("Save current beat:");
                ui.add(egui::TextEdit::singleline(&mut self.save_text).desired_width(230.0));
                if ui.button("💾  Save WAV").clicked() {
                    let p = PathBuf::from(self.save_text.trim());
                    self.shared.lock().unwrap().save_request = Some(p);
                    self.status = "saving wav…".into();
                }

                ui.separator();
                let norm = current_z.iter().map(|v| v * v).sum::<f32>().sqrt();
                ui.label(format!("device: {device}"));
                ui.label(format!("gen time: {gen_ms:.0} ms"));
                ui.label(format!("|z|: {norm:.2}"));
                ui.label(format!("z₀ {:+.2}  z₁ {:+.2}  z₂ {:+.2}",
                    current_z.first().copied().unwrap_or(0.0),
                    current_z.get(1).copied().unwrap_or(0.0),
                    current_z.get(2).copied().unwrap_or(0.0)));

                ui.add_space(6.0);
                ui.label(egui::RichText::new(&self.status).italics());
            });

        egui::Panel::bottom("scope")
            .exact_size(90.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("output").small().weak());
                let (resp, painter) =
                    ui.allocate_painter(egui::vec2(ui.available_width(), 60.0), egui::Sense::hover());
                let rect = resp.rect;
                painter.rect_filled(rect, 4.0, egui::Color32::from_gray(18));
                if scope.len() > 1 {
                    let n = scope.len();
                    let mut pts = Vec::with_capacity(n);
                    for (i, v) in scope.iter().enumerate() {
                        let x = rect.left() + rect.width() * (i as f32 / (n - 1) as f32);
                        let y = rect.center().y - v.clamp(-1.0, 1.0) * (rect.height() * 0.45);
                        pts.push(egui::pos2(x, y));
                    }
                    painter.add(egui::Shape::line(
                        pts,
                        egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(120, 220, 160)),
                    ));
                }
            });

        if self.page == Page::Train {
            egui::CentralPanel::default().show(ui, |ui| {
                self.train_body(ui);
            });
            return;
        }

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(4.0);
            ui.label("latent pad — X→z₀  Y→z₁  scroll→z₂");

            let side = ui.available_width().min(ui.available_height()).max(64.0) - 8.0;
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(side, side), egui::Sense::click_and_drag());
            let rect = resp.rect;

            // background grid
            painter.rect_filled(rect, 6.0, egui::Color32::from_gray(24));
            let grid = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(40));
            for k in 1..6 {
                let fx = rect.left() + rect.width() * k as f32 / 6.0;
                let fy = rect.top() + rect.height() * k as f32 / 6.0;
                painter.line_segment([egui::pos2(fx, rect.top()), egui::pos2(fx, rect.bottom())], grid);
                painter.line_segment([egui::pos2(rect.left(), fy), egui::pos2(rect.right(), fy)], grid);
            }
            // center crosshair
            let axis = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(70));
            painter.line_segment(
                [egui::pos2(rect.center().x, rect.top()), egui::pos2(rect.center().x, rect.bottom())],
                axis,
            );
            painter.line_segment(
                [egui::pos2(rect.left(), rect.center().y), egui::pos2(rect.right(), rect.center().y)],
                axis,
            );

            // map helpers
            let to_pad = |z0: f32, z1: f32| -> egui::Pos2 {
                let u = (z0 / RANGE + 1.0) * 0.5; // 0..1
                let v = 1.0 - (z1 / RANGE + 1.0) * 0.5; // invert y
                egui::pos2(rect.left() + u * rect.width(), rect.top() + v * rect.height())
            };

            // pointer drag -> z0,z1
            if resp.dragged() || resp.clicked() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let u = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                    let v = ((p.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                    let z0 = (u * 2.0 - 1.0) * RANGE;
                    let z1 = (1.0 - v) * 2.0 * RANGE - RANGE;
                    let mut s = self.shared.lock().unwrap();
                    if !s.target_z.is_empty() {
                        s.target_z[0] = z0;
                    }
                    if s.target_z.len() > 1 {
                        s.target_z[1] = z1;
                    }
                    self.walk_t = None; // dragging cancels a walk
                }
            }

            // scroll -> z2 (only while hovering the pad)
            if resp.hovered() {
                let dy = ui.input(|i| i.smooth_scroll_delta.y);
                if dy.abs() > 0.0 && self.latent > 2 {
                    let mut s = self.shared.lock().unwrap();
                    let nz = (s.target_z[2] + dy * 0.01).clamp(-RANGE, RANGE);
                    s.target_z[2] = nz;
                }
            }

            // A / B markers
            let pa = to_pad(self.z_a.first().copied().unwrap_or(0.0), self.z_a.get(1).copied().unwrap_or(0.0));
            let pb = to_pad(self.z_b.first().copied().unwrap_or(0.0), self.z_b.get(1).copied().unwrap_or(0.0));
            painter.circle_stroke(pa, 7.0, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(90, 160, 255)));
            painter.text(pa + egui::vec2(10.0, -10.0), egui::Align2::LEFT_CENTER, "A",
                egui::FontId::proportional(14.0), egui::Color32::from_rgb(90, 160, 255));
            painter.circle_stroke(pb, 7.0, egui::Stroke::new(2.0_f32, egui::Color32::from_rgb(255, 150, 90)));
            painter.text(pb + egui::vec2(10.0, -10.0), egui::Align2::LEFT_CENTER, "B",
                egui::FontId::proportional(14.0), egui::Color32::from_rgb(255, 150, 90));
            // A->B guide line
            painter.line_segment([pa, pb], egui::Stroke::new(1.0_f32, egui::Color32::from_gray(60)));

            // current position glow
            let cur = to_pad(current_z.first().copied().unwrap_or(0.0), current_z.get(1).copied().unwrap_or(0.0));
            for (r, a) in [(18.0, 18u8), (12.0, 40), (7.0, 90)] {
                painter.circle_filled(cur, r, egui::Color32::from_rgba_unmultiplied(120, 230, 170, a));
            }
            painter.circle_filled(cur, 4.0, egui::Color32::from_rgb(140, 255, 190));
        });
    }
}

impl App {
    /// The training page's left-panel controls.
    fn train_controls(&mut self, ui: &mut egui::Ui) {
        use crate::trainpage::Phase;
        let running = matches!(self.train.phase, Phase::Preparing { .. } | Phase::Training);

        // THE INPUT DIRECTORY: a picker AND a text field, not one or the other.
        // The picker is what a phone needs; the field is what a path pasted from
        // a terminal needs, and it is also the only way to type a path that is
        // not mounted yet. Both write the same value.
        ui.label(egui::RichText::new("audio library").small());
        ui.horizontal(|ui| {
            let mut dir = self.cfg.data_dir.to_string_lossy().to_string();
            if ui.add_enabled(!running,
                egui::TextEdit::singleline(&mut dir).desired_width(150.0)).changed()
            {
                self.cfg.data_dir = dir.clone().into();
            }
            if ui.add_enabled(!running, egui::Button::new("📁")).clicked() {
                // Blocking pick. The frame stalls while the portal is open,
                // which is correct: the operator is in a modal dialog and an
                // animating window behind it is not worth a thread.
                if let Some(d) = rfd::FileDialog::new()
                    .set_directory(&self.cfg.data_dir)
                    .pick_folder()
                {
                    self.cfg.data_dir = d;
                }
            }
        });
        // Said plainly, because the scan is recursive now and the count is the
        // difference between "this is the right folder" and a silent zero.
        if !running {
            let d = self.cfg.data_dir.clone();
            if !d.is_dir() {
                ui.label(egui::RichText::new("not a directory").small());
            }
        }
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("epochs");
            ui.add_enabled(!running,
                egui::TextEdit::singleline(&mut self.train_epochs).desired_width(70.0));
        });
        ui.horizontal(|ui| {
            ui.label("batch");
            ui.add_enabled(!running,
                egui::TextEdit::singleline(&mut self.train_batch).desired_width(70.0));
        });

        if !running {
            if ui.button("▶  start training").clicked() {
                self.start_training();
            }
        } else if ui.button("■  stop").clicked() {
            // A REQUEST, not a kill. The trainer checks this between epochs and
            // writes a checkpoint on the way out — tearing the thread down here
            // would lose the epochs since the last save, which on a slow CPU
            // run is the work of an hour.
            // WRITTEN TO DISK, not just to this struct. The trainer is a
            // separate process — setting a bool here could never reach it, and
            // the button did nothing for two commits. It polls this file at
            // each epoch boundary and exits having written a checkpoint.
            self.train.stop_requested = true;
            match std::fs::write(self.cfg.ckpt_dir.join("STOP"), b"stop\n") {
                Ok(()) => {
                    self.status = "stop requested — finishing the epoch, then saving".into();
                }
                Err(e) => {
                    // Said out loud. A stop button that silently failed to
                    // write is worse than no button: the operator walks away
                    // believing the run is ending.
                    self.train.stop_requested = false;
                    self.status = format!("could not write the stop file: {e}");
                }
            }
        }

        ui.separator();
        ui.label(egui::RichText::new(self.train.status_line()).small());
        if self.train.stalled() {
            // Said, not acted on: a plateau is sometimes convergence and
            // sometimes a dead run, and the operator knows which.
            ui.label(egui::RichText::new("loss is flat — plateau or converged").small());
        }

        // THE CACHE, with the two numbers that decide whether to wait.
        //
        // Rescanned every two seconds, not every frame: a read_dir over
        // thousands of entries is a real stall, and the cache moves on the
        // scale of seconds.
        let due = self.cache_checked
            .map(|t| t.elapsed().as_secs_f32() > 2.0)
            .unwrap_or(true);
        if due {
            let budget = if self.cfg.cache_batches == 0 { None }
                         else { Some(self.cfg.cache_batches * self.cfg.batch_size.max(1)) };
            self.cache = crate::trainpage::read_cache(&self.cfg.cache_dir, budget);
            self.cache_checked = Some(std::time::Instant::now());
        }
        ui.separator();
        ui.label(egui::RichText::new("cache").small());
        ui.label(egui::RichText::new(self.cache.line()).small());
        if let Some(f) = self.cache.fullness() {
            ui.add(egui::ProgressBar::new(f).desired_height(6.0).show_percentage());
        }
        if self.cache.will_thrash() {
            // Said BEFORE the run. A library larger than the budget means every
            // epoch re-decodes what the last one evicted, and the fix is a
            // bigger cache_batches rather than patience.
            ui.label(egui::RichText::new(
                "library is larger than the cache budget — it will re-decode as it goes").small());
        }

        // GPU, when the trainer has said anything about one. Absent rather
        // than zeroed when it has not: an empty panel reads as "no GPU", and
        // a panel of zeros reads as a broken one.
        if self.gpu.adapter.is_some() || self.gpu.sync_share().is_some() {
            ui.separator();
            ui.label(egui::RichText::new("gpu").small());
            if let Some(a) = &self.gpu.adapter {
                ui.label(egui::RichText::new(a).small());
            }
            if let Some(v) = self.gpu.verdict() {
                ui.label(egui::RichText::new(v).small());
            }
            if let Some(f) = self.gpu.buffer_fullness() {
                ui.add(egui::ProgressBar::new(f).desired_height(6.0)
                    .text("buffer budget"));
            }
        }

        ui.separator();
        if ui.button("copy summary").clicked() {
            ui.ctx().copy_text(self.train.summary());
            self.status = "summary copied".into();
        }
    }

    /// Spawn a trainer and pipe its output into the page.
    ///
    /// Runs the same binary as the CLI rather than calling `train::run` in a
    /// thread. That keeps ONE training path: a GUI that reimplemented the loop
    /// would drift from the command line, and the log the dashboard parses is
    /// the log the terminal shows.
    fn start_training(&mut self) {
        let epochs: u64 = self.train_epochs.trim().parse().unwrap_or(1000);
        let batch: usize = self.train_batch.trim().parse().unwrap_or(16);
        let cfg = self.cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        self.train_rx = Some(rx);
        self.train.start(epochs, batch, "…", &cfg.data_dir.to_string_lossy());

        let exe = std::env::current_exe().unwrap_or_else(|_| "beatspace".into());
        let data = cfg.data_dir.clone();
        let ckpt = cfg.ckpt_dir.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader};
            let child = std::process::Command::new(exe)
                .arg("train")
                .arg("--data-dir").arg(&data)
                .arg("--ckpt-dir").arg(&ckpt)
                .arg("--epochs").arg(epochs.to_string())
                .arg("--batch-size").arg(batch.to_string())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(format!("[train] could not start: {e}"));
                    return;
                }
            };
            // stderr on its own thread: the trainer writes failures there, and
            // reading the two in sequence loses the error when stdout blocks.
            if let Some(err) = child.stderr.take() {
                let etx = tx.clone();
                std::thread::spawn(move || {
                    for l in BufReader::new(err).lines().map_while(Result::ok) {
                        let _ = etx.send(l);
                    }
                });
            }
            if let Some(out) = child.stdout.take() {
                for l in BufReader::new(out).lines().map_while(Result::ok) {
                    if tx.send(l).is_err() {
                        // The page is gone; stop reading rather than filling a
                        // channel nobody drains.
                        break;
                    }
                }
            }
            let _ = child.wait();
            let _ = tx.send("[train] process exited".into());
        });
    }

    /// The dashboard: the curve, then the log.
    fn train_body(&mut self, ui: &mut egui::Ui) {
        use crate::trainpage::Phase;
        ui.heading("training");
        ui.label(self.train.status_line());

        if let Phase::Preparing { done, total } = self.train.phase {
            if total > 0 {
                ui.add(egui::ProgressBar::new(done as f32 / total as f32)
                    .text(format!("decoding {done}/{total}")));
            }
        }

        // THE LOSS CURVE, drawn from the page's own history.
        if self.train.curve.len() >= 2 {
            let h = 140.0;
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), h), egui::Sense::hover());
            let p = ui.painter_at(rect);
            let losses: Vec<f32> = self.train.curve.iter().map(|c| c.loss).collect();
            let (lo, hi) = losses.iter().fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
            // A flat curve has zero span and would divide by nothing; drawing
            // it down the middle is the honest picture of "not moving".
            let span = if (hi - lo).abs() < f32::EPSILON { 1.0 } else { hi - lo };
            let n = losses.len();
            let pts: Vec<egui::Pos2> = losses.iter().enumerate().map(|(i, v)| {
                let x = rect.left() + rect.width() * (i as f32 / (n - 1) as f32);
                let y = rect.bottom() - rect.height() * ((v - lo) / span);
                egui::pos2(x, y)
            }).collect();
            p.rect_filled(rect, 2.0, egui::Color32::from_black_alpha(40));
            for w in pts.windows(2) {
                p.line_segment([w[0], w[1]], egui::Stroke::new(1.5, egui::Color32::LIGHT_BLUE));
            }
            ui.label(egui::RichText::new(format!("loss {lo:.5} … {hi:.5}")).small());
        }

        ui.separator();
        // THE LOG, newest at the bottom and stuck there.
        //
        // stick_to_bottom, because a log that does not follow makes the reader
        // scroll on every frame during the part they are watching for.
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for line in self.train.log_lines() {
                    ui.label(egui::RichText::new(line).monospace().small());
                }
            });
    }
}


impl App {
    /// Mute, stop, and a level — on every page, because the thing you want to
    /// silence is playing whichever page you are looking at.
    fn transport(&mut self, ui: &mut egui::Ui) {
        let (mut gain, mut stopped) = {
            let s = self.shared.lock().unwrap();
            (s.gain, s.stopped)
        };
        let before = (gain, stopped);

        ui.horizontal(|ui| {
            if ui.button(if stopped { "▶" } else { "■" })
                .on_hover_text(if stopped { "resume playback" } else { "stop playback" })
                .clicked()
            {
                stopped = !stopped;
            }
            // MUTE IS A SEPARATE CONTROL from the level, and it remembers.
            // Dragging the slider to zero to silence something loses the level
            // you liked; a mute toggle gives it back.
            let muted = gain <= 0.0;
            if ui.selectable_label(muted, "mute").clicked() {
                gain = if muted { self.last_gain.max(0.1) } else { 0.0 };
            }
            if gain > 0.0 {
                self.last_gain = gain;
            }
            ui.add(egui::Slider::new(&mut gain, 0.0..=1.0).show_value(false))
                .on_hover_text("output level");
        });

        if (gain, stopped) != before {
            let mut s = self.shared.lock().unwrap();
            s.gain = gain;
            s.stopped = stopped;
        }
    }
}
