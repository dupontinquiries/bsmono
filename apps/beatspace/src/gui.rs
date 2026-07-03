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
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([720.0, 500.0]),
        ..Default::default()
    };
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

        audio.lock().unwrap().push(&wav);

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

struct App {
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
        Self {
            latent,
            shared,
            z_a: vec![0.0; latent],
            z_b: vec![0.0; latent],
            walk_t: None,
            walk_speed: 0.01,
            ckpt_text: cfg
                .ckpt_dir
                .join("gen_latest.safetensors")
                .display()
                .to_string(),
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

        egui::Panel::left("controls")
            .resizable(false)
            .exact_size(250.0)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading("beatspace");
                ui.label("latent-space beat morpher");
                ui.separator();

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
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 220, 160)),
                    ));
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_space(4.0);
            ui.label("latent pad — X→z₀  Y→z₁  scroll→z₂");

            let side = ui.available_width().min(ui.available_height()).max(64.0) - 8.0;
            let (resp, painter) =
                ui.allocate_painter(egui::vec2(side, side), egui::Sense::click_and_drag());
            let rect = resp.rect;

            // background grid
            painter.rect_filled(rect, 6.0, egui::Color32::from_gray(24));
            let grid = egui::Stroke::new(1.0, egui::Color32::from_gray(40));
            for k in 1..6 {
                let fx = rect.left() + rect.width() * k as f32 / 6.0;
                let fy = rect.top() + rect.height() * k as f32 / 6.0;
                painter.line_segment([egui::pos2(fx, rect.top()), egui::pos2(fx, rect.bottom())], grid);
                painter.line_segment([egui::pos2(rect.left(), fy), egui::pos2(rect.right(), fy)], grid);
            }
            // center crosshair
            let axis = egui::Stroke::new(1.0, egui::Color32::from_gray(70));
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
            painter.circle_stroke(pa, 7.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(90, 160, 255)));
            painter.text(pa + egui::vec2(10.0, -10.0), egui::Align2::LEFT_CENTER, "A",
                egui::FontId::proportional(14.0), egui::Color32::from_rgb(90, 160, 255));
            painter.circle_stroke(pb, 7.0, egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 150, 90)));
            painter.text(pb + egui::vec2(10.0, -10.0), egui::Align2::LEFT_CENTER, "B",
                egui::FontId::proportional(14.0), egui::Color32::from_rgb(255, 150, 90));
            // A->B guide line
            painter.line_segment([pa, pb], egui::Stroke::new(1.0, egui::Color32::from_gray(60)));

            // current position glow
            let cur = to_pad(current_z.first().copied().unwrap_or(0.0), current_z.get(1).copied().unwrap_or(0.0));
            for (r, a) in [(18.0, 18u8), (12.0, 40), (7.0, 90)] {
                painter.circle_filled(cur, r, egui::Color32::from_rgba_unmultiplied(120, 230, 170, a));
            }
            painter.circle_filled(cur, 4.0, egui::Color32::from_rgb(140, 255, 190));
        });
    }
}
