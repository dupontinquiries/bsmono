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

use crate::arrange::cmd::Command;
use crate::arrange::model::Timeline;
use crate::arrange::render::{Library, Sample, Transport};
use crate::arrangepage::{ArrangeIo, ArrangeOut, ArrangeView};
use crate::config::{Config, Playback};
use crate::device_prefs::DevicePrefs;
use audio_kit::BounceFormat;
use crate::dsp::{GlState, Mel};
use crate::model::{load_generator, Generator};
use crate::stream::{Conductor, LatentWalk, Streamer};
use crate::{audio_io, player};

/// What the generator thread is doing, written by it once per window and
/// read by the control column. The GUI's own `cfg` is what the NEXT training
/// run will build; this is what is playing now, which can differ once a
/// checkpoint has been loaded from elsewhere.
#[derive(Clone, Default)]
struct PlaybackInfo {
    /// Stream (continuing windows) or loop (crossfaded beats).
    stream: bool,
    stems: usize,
    ctx_frames: usize,
    beat_phase: bool,
    time_frames: usize,
    stride_frames: usize,
    overlap_frames: usize,
    hop: usize,
    sr: u32,
    /// Stream: tempo the conductor is advancing at, and where in the beat
    /// the next window starts.
    bpm: f32,
    phase: f32,
    /// Stream: audio queued ahead of the playhead, in seconds.
    lead_secs: f32,
    /// Stream: windows rendered since the last (re)start.
    windows: u64,
    /// The epoch recorded beside the loaded checkpoint, when there is one.
    epoch: Option<usize>,
    /// Stream: the next window continues the previous one (false right
    /// after a restart, when it starts from silence).
    continuing: bool,
}

impl PlaybackInfo {
    fn from_cfg(cfg: &Config, stream: bool) -> Self {
        Self {
            stream,
            stems: cfg.stems,
            ctx_frames: cfg.ctx_frames,
            beat_phase: cfg.beat_phase,
            time_frames: cfg.time_frames,
            stride_frames: cfg.window_stride(),
            overlap_frames: cfg.overlap_frames,
            hop: cfg.hop,
            sr: cfg.sr,
            bpm: cfg.bpm,
            ..Default::default()
        }
    }
    fn secs(&self, frames: usize) -> f32 {
        (frames * self.hop) as f32 / self.sr.max(1) as f32
    }
    /// One line saying what the listener will hear next.
    fn headline(&self) -> String {
        if !self.stream {
            return format!(
                "loop — one {:.0} ms beat, crossfaded into the next as the latent moves",
                self.secs(self.time_frames) * 1000.0
            );
        }
        let stems = if self.stems >= 2 { "drums + instruments" } else { "one mixed stem" };
        let how = if self.ctx_frames > 0 {
            if self.continuing { "continuing the previous window" } else { "starting from silence" }
        } else {
            "independent windows, phase-continued"
        };
        format!(
            "stream — {stems}, {how}; {:.0} ms windows every {:.0} ms",
            self.secs(self.time_frames) * 1000.0,
            self.secs(self.stride_frames) * 1000.0
        )
    }
}

/// Knobs the control column turns on the stream. Read by the stream loop
/// before each window; ignored by the loop player, which has none of them.
#[derive(Clone)]
struct StreamControls {
    bpm: f32,
    /// Conductor glide per window, (0, 1].
    smoothing: f32,
    /// Windows of audio to keep queued ahead of the playhead.
    lead_windows: usize,
    /// Griffin-Lim iterations per window at playback.
    gl_iters: usize,
    /// One-shot: forget the previous window and the beat grid, drop the
    /// queue, start over.
    restart: bool,
}

/// The arrangement as the audio thread sees it. The GUI owns the editor
/// and publishes a copy of the timeline after every command; the thread
/// renders blocks from that copy and reports where it is.
struct ArrShared {
    timeline: Timeline,
    /// Behind an Arc so the GUI can take a reference each frame without
    /// copying every decoded file; an import replaces it (copy-on-write).
    library: Arc<Library>,
    /// GUI -> thread.
    playing: bool,
    seek: Option<f64>,
    capture: bool,
    /// thread -> GUI: the beat the speakers are at.
    pos: f64,
    notice: Option<String>,
    /// GUI -> thread: start a take at this beat / stop the running one.
    record_toggle: Option<f64>,
    /// thread -> GUI: seconds recorded so far, while a take runs.
    recording_secs: Option<f32>,
    /// thread -> GUI: a finished take to place (source, start beat,
    /// length in beats, the tempo it was recorded at). The GUI applies it
    /// through the editor so it is undoable like everything else.
    pending_place: Option<(crate::arrange::model::SourceRef, f64, f64, f64)>,
}

struct Shared {
    target_z: Vec<f32>,
    current_z: Vec<f32>,
    gen_ms: f32,
    scope: Vec<f32>,
    save_request: Option<PathBuf>,
    weights_request: Option<PathBuf>,
    loaded_label: String,
    device_label: String,
    playback: PlaybackInfo,
    stream_ctl: StreamControls,
    arr: ArrShared,
    /// The device's bounce rung, for saved beats written on this thread.
    bounce_format: BounceFormat,
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
        playback: PlaybackInfo::from_cfg(&cfg, cfg.playback == Playback::Stream || cfg.conditional()),
        stream_ctl: StreamControls {
            bpm: cfg.bpm,
            smoothing: 0.1,
            lead_windows: 2,
            gl_iters: cfg.griffin_iters_rt,
            restart: false,
        },
        arr: ArrShared {
            timeline: Timeline::default(),
            library: Arc::new(Library::default()),
            playing: false,
            seek: None,
            capture: false,
            pos: 0.0,
            notice: None,
            record_toggle: None,
            recording_secs: None,
            pending_place: None,
        },
        bounce_format: DevicePrefs::load().bounce_format,
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
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([720.0, 500.0]),
        ..Default::default()
    };
    // NOTE: no wgpu device-limits clamp here. beatspace renders through glow
    // (Cargo.toml: `default-features = false, features = ["glow", ...]`,
    // landed in 33b812ad after snapshot-cam's on-device crash — eframe 0.34
    // defaults to wgpu, whose stock device_descriptor asks for
    // wgpu::Limits::default() [max_color_attachments: 8] on every adapter,
    // which fails outright on hardware that allows fewer). glow never
    // requests device limits at all, so there is nothing here to clamp —
    // that fix commit correctly removed this crate's wgpu path from
    // Cargo.toml but missed that this exact block, written for the OLD
    // wgpu-backed build, was still sitting in this file referencing
    // `eframe::egui_wgpu`/`eframe::wgpu`, which eframe only re-exports
    // behind its `wgpu`/`wgpu_no_default_features` features — neither of
    // which this crate enables anymore. `cargo check` never caught it at
    // the time because CLAUDE.md §3 (since corrected) listed ALSA as
    // missing from this sandbox, so the `app` feature — the one gui.rs
    // lives behind — could not be compiled here at all; the graph-only
    // check that commit DID run cannot see inside a function body.
    let res = eframe::run_native("beatspace", opts, Box::new(move |_cc| Ok(Box::new(app))));

    stop.store(true, Ordering::Relaxed);
    let _ = gen_handle.join();
    res.map_err(|e| anyhow!("eframe: {e}"))?;
    Ok(())
}

// ----------------------------------------------------------------------------
// generator / audio thread
// ----------------------------------------------------------------------------

/// Samples per arrangement block. ~93 ms at 22050: short enough that an
/// edit is heard promptly, long enough that the lock is not taken often.
const ARR_BLOCK: usize = 2048;

/// The arrangement's share of a generator loop iteration: capture the
/// window that is playing when asked, and while the arrangement is
/// playing, render its blocks into the player instead of generating.
///
/// Returns true when the arrangement has the audio and the caller should
/// skip its own generation this iteration. `render_hq` gives the caller's
/// current window at full quality for a capture; `stream_player` says
/// whether the caller's player is in stream mode (a loop-mode player is
/// put back when the arrangement stops).
struct ArrService {
    transport: Transport,
    was_playing: bool,
    pushed: bool,
}

impl ArrService {
    fn new() -> Self {
        Self { transport: Transport::default(), was_playing: false, pushed: false }
    }

    fn tick(
        &mut self,
        cfg: &Config,
        shared: &Arc<Mutex<Shared>>,
        audio: &Arc<Mutex<player::Player>>,
        stream_player: bool,
        render_hq: impl FnOnce() -> Option<(Vec<f32>, Option<Vec<f32>>, f32)>,
    ) -> bool {
        // Capture, whether or not the arrangement is playing.
        let capture = {
            let mut s = shared.lock().unwrap();
            std::mem::take(&mut s.arr.capture)
        };
        if capture {
            match render_hq() {
                Some((audio_hq, planes, bpm)) => {
                    let mut s = shared.lock().unwrap();
                    let n = s.arr.library.beats.len() + 1;
                    Arc::make_mut(&mut s.arr.library).add_beat(Sample { name: format!("beat {n}"), audio: audio_hq, sr: cfg.sr, planes, bpm: Some(bpm) });
                    s.arr.notice = Some(format!("captured beat {n}"));
                }
                None => shared.lock().unwrap().arr.notice = Some("nothing playing to capture".into()),
            }
        }

        let (playing, seek) = {
            let mut s = shared.lock().unwrap();
            (s.arr.playing, s.arr.seek.take())
        };
        if let Some(b) = seek {
            self.transport.seek(b);
            if playing {
                audio.lock().unwrap().clear_stream();
                self.pushed = false;
            }
        }
        if !playing {
            if self.was_playing {
                // Hand the device back to the generator.
                let mut p = audio.lock().unwrap();
                if stream_player { p.clear_stream() } else { p.stop_stream() }
                self.was_playing = false;
                self.pushed = false;
            }
            return false;
        }
        if !self.was_playing {
            audio.lock().unwrap().clear_stream();
            self.was_playing = true;
            self.pushed = false;
        }
        let queued = audio.lock().unwrap().queued_src_samples();
        let low_water = 3 * ARR_BLOCK;
        if queued < low_water {
            let mut block = vec![0.0f32; ARR_BLOCK];
            let started = {
                let s = shared.lock().unwrap();
                self.transport.render_next(&s.arr.timeline, &s.arr.library, cfg.sr, &mut block)
            };
            let (queued, scope) = {
                let mut p = audio.lock().unwrap();
                p.push_window(&block, if self.pushed { ARR_BLOCK } else { 0 });
                (p.queued_src_samples(), p.scope_snapshot())
            };
            self.pushed = true;
            let mut s = shared.lock().unwrap();
            let lag_beats = queued as f64 / cfg.sr as f64 * s.arr.timeline.bpm / 60.0;
            s.arr.pos = (started - lag_beats).max(0.0);
            s.scope = scope;
        } else {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        true
    }
}

fn gen_loop(cfg: Config, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) -> Result<()> {
    // A conditional renderer cannot be looped — every window needs the one
    // before it — so the stream loop is the only sensible playback for it,
    // whatever the config says; and a legacy model streams fine.
    if cfg.playback == Playback::Stream || cfg.conditional() {
        return stream_loop(cfg, shared, stop);
    }
    let dev = Device::cuda_if_available(0).unwrap_or(Device::Cpu);
    {
        let mut s = shared.lock().unwrap();
        s.device_label = format!("{dev:?}");
    }

    let mut g = Generator::new(&cfg, &dev)?;
    // What `g` is currently shaped for. Tracked rather than re-derived,
    // because after a rebuild the config no longer describes it.
    let mut cur_frames = cfg.time_frames;
    let mut cur_latent = cfg.latent;
    // try to load the newest checkpoint
    let latest = cfg.ckpt_dir.join("gen_latest.safetensors");
    if latest.exists() {
        if let Err(e) = load_generator(&mut g, &latest, &dev) {
            eprintln!("[gui] failed to load {latest:?}: {e}");
        } else {
            let mut s = shared.lock().unwrap();
            s.loaded_label = latest.display().to_string();
            s.playback.epoch = crate::train::read_resume_epoch_in(&cfg.ckpt_dir);
        }
    }

    let mel = Mel::new(&cfg);
    let mut gl = GlState::new(); // persistent phase for warm-started Griffin-Lim

    // audio out (keep the stream alive for the whole thread)
    let (_stream, audio) = player::start(cfg.sr)?;

    let latent = cfg.latent;
    let mut current_z = vec![0.0f32; latent];
    let mut arr = ArrService::new();
    // The last window rendered, for a capture.
    let mut last: Option<(Vec<f32>, usize)> = None;

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
            // ADOPT THE CHECKPOINT'S OWN GEOMETRY. `g` was built from the
            // running config, so a checkpoint trained at a different
            // `time_frames` could not load into it at all — the failure
            // was `shape mismatch in set, lhs: [8192, 128], rhs: [81920,
            // 128]`, printed once per frame, naming neither the setting
            // nor the fix. 8192 is 64 frames (the config default), 81920
            // is 640 (what that checkpoint was trained with).
            //
            // The file states its own shape — `g.fc.w` is
            // `[g_filters * h0 * w0, latent]` — so the generator is
            // REBUILT to match rather than the person being asked for a
            // hint. A hint field would be a second place to be wrong
            // about something the file already knows.
            match candle_core::safetensors::load(&path, &dev)
                .map_err(|e| e.to_string())
                .and_then(|m| {
                    match crate::model::checkpoint_shape(&m, cfg.g_filters, cfg.n_mels) {
                        Some(sh) if sh.time_frames != cur_frames || sh.latent != cur_latent => {
                            let mut c2 = cfg.clone();
                            c2.time_frames = sh.time_frames;
                            c2.latent = sh.latent;
                            let mut ng = Generator::new(&c2, &dev).map_err(|e| e.to_string())?;
                            ng.load_state(&m).map_err(|e| e.to_string())?;
                            eprintln!(
                                "[gui] checkpoint is {} frames / latent {} — rebuilt to match \
                                 (was {cur_frames}/{cur_latent})",
                                sh.time_frames, sh.latent
                            );
                            Ok((ng, sh.time_frames, sh.latent))
                        }
                        // Same geometry, or a file that does not decompose:
                        // load into the generator we have and let candle
                        // report it if the shapes really disagree.
                        _ => {
                            let mut ng = Generator::new(&cfg, &dev).map_err(|e| e.to_string())?;
                            ng.load_state(&m).map_err(|e| e.to_string())?;
                            Ok((ng, cur_frames, cur_latent))
                        }
                    }
                }) {
                Ok((ng, frames, lat)) => {
                    g = ng;
                    cur_frames = frames;
                    cur_latent = lat;
                    let mut s = shared.lock().unwrap();
                    s.loaded_label = format!("{} ({frames}f)", path.display());
                    s.playback.time_frames = frames;
                    s.playback.epoch = path.parent().and_then(crate::train::read_resume_epoch_in);
                }
                Err(e) => eprintln!("[gui] load weights {path:?}: {e}"),
            }
        }

        {
            let (gain, stopped) = {
                let s = shared.lock().unwrap();
                (s.gain, s.stopped)
            };
            let mut p = audio.lock().unwrap();
            p.set_gain(gain);
            p.set_stopped(stopped);
        }
        let last_ref = last.clone();
        let (mel_ref, iters, bpm) = (&mel, cfg.griffin_iters, cfg.bpm);
        if arr.tick(&cfg, &shared, &audio, false, || {
            let (flat, t) = last_ref?;
            let hq = mel_ref.mel_to_wav(&flat, t, iters);
            Some((hq, Some(flat), bpm))
        }) {
            continue;
        }

        // exponential smoothing toward target (the "thumb glides" feel)
        for i in 0..latent {
            current_z[i] = 0.9 * current_z[i] + 0.1 * target[i];
        }

        let t0 = Instant::now();
        let (flat, _n_mels, t) = g.generate(&current_z, &dev)?;
        last = Some((flat.clone(), t));
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
            let rung = shared.lock().unwrap().bounce_format;
            let target = audio_io::unique_path(&audio_io::with_format_ext(&path, rung));
            match audio_io::save_audio(&target, &hq, cfg.sr, rung) {
                Ok(written) => println!("[gui] saved {} ({} iters)", written.display(), cfg.griffin_iters),
                Err(e) => eprintln!("[gui] save {path:?}: {e}"),
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

/// The stream: windows that continue each other, paced by the audio queue.
///
/// Where `gen_loop` renders a beat from the latent and crossfades into it
/// from the top, this asks the conductor for a step, the streamer for the
/// window after the last one, and the player to overlap-add it; then it
/// waits until the queue is short enough to need another. Generation is
/// driven by how far ahead of the playhead it is, not by a timer, so a slow
/// render eats into the lead rather than into the beat grid.
///
/// The conductor is `LatentWalk` — the same 0.9/0.1 glide, with a beat
/// clock — behind the `Conductor` trait. A learned one replaces it here and
/// nowhere else.
fn stream_loop(cfg: Config, shared: Arc<Mutex<Shared>>, stop: Arc<AtomicBool>) -> Result<()> {
    let dev = Device::cuda_if_available(0).unwrap_or(Device::Cpu);
    {
        let mut s = shared.lock().unwrap();
        s.device_label = format!("{dev:?} · stream");
    }

    let mut g = Generator::new(&cfg, &dev)?;
    let latest = cfg.ckpt_dir.join("gen_latest.safetensors");
    if latest.exists() {
        if let Err(e) = load_generator(&mut g, &latest, &dev) {
            eprintln!("[gui] failed to load {latest:?}: {e}");
        } else {
            let mut s = shared.lock().unwrap();
            s.loaded_label = latest.display().to_string();
            s.playback.epoch = crate::train::read_resume_epoch_in(&cfg.ckpt_dir);
        }
    }
    let mut streamer = Streamer::new(&cfg, g, dev.clone());
    let mut walk = LatentWalk::new(&cfg);
    let mut windows: u64 = 0;
    let mut arr = ArrService::new();
    let mut take: Option<crate::stream::Take> = None;

    let (_stream, audio) = player::start(cfg.sr)?;

    // Lead: how many windows of audio to keep queued. Less and a slow
    // render is a dropout; more and a pad move takes that much longer to
    // be heard. Set from the control column.
    let advance = cfg.window_stride() * cfg.hop;

    while !stop.load(Ordering::Relaxed) {
        let (target, weights_req, save_req, ctl) = {
            let mut s = shared.lock().unwrap();
            let ctl = s.stream_ctl.clone();
            s.stream_ctl.restart = false;
            (s.target_z.clone(), s.weights_request.take(), s.save_request.take(), ctl)
        };
        let low_water = ctl.lead_windows.max(1) * advance;
        walk.set_bpm(&cfg, ctl.bpm);
        walk.set_smoothing(ctl.smoothing);
        if ctl.restart {
            streamer.reset();
            walk.reset_phase();
            audio.lock().unwrap().clear_stream();
            windows = 0;
        }

        if let Some(path) = weights_req {
            // The config.json next to the checkpoint is the authority on the
            // conditional geometry — stems, context, phase planes — which
            // `g.fc.w` alone cannot say. A checkpoint without one is loaded
            // into the running config's shape, and a mismatch is reported by
            // name rather than rebuilt around.
            let sibling = path.parent().map(|d| d.join("config.json"));
            let build_cfg = match sibling.filter(|p| p.exists()).and_then(|p| Config::load_json(&p).ok()) {
                Some(saved) => match cfg.shape_compatible_with(&saved) {
                    Ok(()) => cfg.clone(),
                    Err(why) => {
                        eprintln!(
                            "[gui] {} was trained with a different model shape ({why}); \
restart the GUI with --ckpt-dir pointing at its directory",
                            path.display()
                        );
                        continue;
                    }
                },
                None => cfg.clone(),
            };
            match Generator::new(&build_cfg, &dev).and_then(|mut ng| load_generator(&mut ng, &path, &dev).map(|_| ng)) {
                Ok(ng) => {
                    streamer.set_generator(ng);
                    let mut s = shared.lock().unwrap();
                    s.loaded_label = path.display().to_string();
                    s.playback.epoch = path.parent().and_then(crate::train::read_resume_epoch_in);
                }
                Err(e) => eprintln!("[gui] load weights {path:?}: {e}"),
            }
        }

        let (gain, stopped) = {
            let s = shared.lock().unwrap();
            (s.gain, s.stopped)
        };
        let queued = {
            let mut p = audio.lock().unwrap();
            p.set_gain(gain);
            p.set_stopped(stopped);
            p.queued_src_samples()
        };
        {
            let (st, iters) = (&streamer, cfg.griffin_iters);
            let bpm = ctl.bpm;
            if arr.tick(&cfg, &shared, &audio, true, || {
                let hq = st.render_last_hq(iters)?;
                Some((hq, st.last_planes().map(|p| p.to_vec()), bpm))
            }) {
                continue;
            }
        }
        if queued >= low_water && save_req.is_none() {
            // Nothing to render; keep the lead reading live while waiting.
            shared.lock().unwrap().playback.lead_secs = queued as f32 / cfg.sr as f32;
            std::thread::sleep(std::time::Duration::from_millis(5));
            continue;
        }

        let continuing = streamer.last_planes().is_some();
        let t0 = Instant::now();
        let window = {
            let step = walk.step(&target);
            streamer.next_window(&step, ctl.gl_iters.max(1))?
        };
        let gen_ms = t0.elapsed().as_secs_f32() * 1000.0;
        windows += 1;
        let queued = {
            let mut p = audio.lock().unwrap();
            p.push_window(&window.audio, window.advance);
            p.queued_src_samples()
        };
        // Recording: the same windows the player got, overlap-added into a
        // take. Toggled from the arrange page; a finished take goes into
        // the library and is handed back to be placed.
        let toggle = shared.lock().unwrap().arr.record_toggle.take();
        if let Some(start_beat) = toggle {
            match take.take() {
                Some(t) => finish_take(t, &cfg, &shared),
                None => {
                    take = Some(crate::stream::Take::start(start_beat, ctl.bpm, cfg.sr));
                    shared.lock().unwrap().arr.notice = Some(format!("recording from beat {start_beat:.2}"));
                }
            }
        }
        if let Some(t) = take.as_mut() {
            t.push(&window);
            shared.lock().unwrap().arr.recording_secs = Some(t.secs());
        }

        if let Some(path) = save_req {
            match streamer.render_last_hq(cfg.griffin_iters) {
                Some(hq) => {
                    let rung = shared.lock().unwrap().bounce_format;
                    let target = audio_io::unique_path(&audio_io::with_format_ext(&path, rung));
                    match audio_io::save_audio(&target, &hq, cfg.sr, rung) {
                        Ok(written) => println!("[gui] saved {} ({} iters)", written.display(), cfg.griffin_iters),
                        Err(e) => eprintln!("[gui] save {path:?}: {e}"),
                    }
                }
                None => eprintln!("[gui] nothing rendered yet to save"),
            }
        }

        let scope = audio.lock().unwrap().scope_snapshot();
        {
            let mut s = shared.lock().unwrap();
            s.current_z = walk.current().to_vec();
            s.gen_ms = gen_ms;
            s.scope = scope;
            let p = &mut s.playback;
            p.stream = true;
            p.bpm = ctl.bpm;
            p.phase = walk.beat_phase();
            p.lead_secs = queued as f32 / cfg.sr as f32;
            p.windows = windows;
            p.continuing = continuing;
        }
    }
    Ok(())
}

/// Add a finished take to the library and queue its placement.
fn finish_take(t: crate::stream::Take, cfg: &Config, shared: &Arc<Mutex<Shared>>) {
    let (start_beat, bpm) = (t.start_beat, t.bpm());
    let mut s = shared.lock().unwrap();
    s.arr.recording_secs = None;
    match t.finish() {
        Some((audio, beats)) => {
            let n = s.arr.library.beats.len() + 1;
            let secs = audio.len() as f32 / cfg.sr as f32;
            // The take knows its tempo, so it is placed locked to it and
            // follows the arrangement's tempo from then on.
            let r = Arc::make_mut(&mut s.arr.library).add_beat(Sample {
                name: format!("take {n}"),
                audio,
                sr: cfg.sr,
                planes: None,
                bpm: Some(bpm),
            });
            s.arr.pending_place = Some((r, start_beat, beats, bpm as f64));
            s.arr.notice = Some(format!("take {n}: {secs:.1} s, placed at beat {start_beat:.2}"));
        }
        None => s.arr.notice = Some("the take was empty".into()),
    }
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
    Arrange,
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
    /// The shared kit picker, rather than rfd. A native dialog is somebody
    /// else's full-screen UI with its own touch targets, and on postmarketOS
    /// it may simply not be there.
    picker: uikit_egui::path_picker::PathPicker,
    picker_open: bool,
    train_batch: String,
    /// Sampler duration in mel frames — the width of one training crop and the
    /// length of one generated beat. Had no CLI flag until recently and no GUI
    /// field at all, so it was reachable only by hand-editing config.json.
    train_time_frames: String,
    /// Continue from the checkpoint in ckpt_dir rather than starting over.
    /// Defaults ON: the GUI's own stop button writes a checkpoint, so the
    /// overwhelmingly common next action is to carry on from it, and starting
    /// fresh by accident silently discards a training run.
    train_resume: bool,
    latent: usize,
    shared: Arc<Mutex<Shared>>,
    z_a: Vec<f32>,
    z_b: Vec<f32>,
    walk_t: Option<f32>,
    walk_speed: f32,
    ckpt_text: String,
    save_text: String,
    status: String,
    /// The arrangement page's editor, tools and view state.
    arrange: ArrangeView,
    /// This device's preferences (the bounce rung), from the sidecar.
    device: DevicePrefs,
    /// Whether the model shape in `cfg` (what the next run builds) matches
    /// the checkpoint directory's config.json. Refreshed on the cache timer
    /// rather than per frame; `None` when there is no checkpoint to compare.
    shape_note: Option<Result<(), String>>,
}

const RANGE: f32 = 3.0; // pad maps to [-RANGE, RANGE]

impl App {
    fn new(cfg: Config, shared: Arc<Mutex<Shared>>) -> Self {
        let project_path = crate::arrange::project::default_dir(&cfg.ckpt_dir).display().to_string();
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
            train_time_frames: cfg.time_frames.to_string(),
            train_resume: true,
            picker: Default::default(),
            picker_open: false,
            train_batch,
            cfg,
            latent,
            shared,
            z_a: vec![0.0; latent],
            z_b: vec![0.0; latent],
            walk_t: None,
            walk_speed: 0.01,
            ckpt_text,
            save_text: "beat".into(),
            status: "drag on the pad to morph".into(),
            shape_note: None,
            arrange: {
                let mut a = ArrangeView::default();
                a.project_path = project_path;
                a
            },
            device: DevicePrefs::load(),
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

/// The per-frame snapshot the control column reads, taken once under the
/// shared lock in `ui()`.
///
/// The body used to sit inline and close over these as locals; lifting it
/// into a method meant they had to travel. Passed as one borrowed struct
/// rather than five arguments — five positional floats and strings is the
/// shape that gets silently reordered later.
struct Snapshot<'a> {
    current_z: &'a [f32],
    target_snapshot: &'a [f32],
    gen_ms: f32,
    loaded: &'a str,
    device: &'a str,
    playback: &'a PlaybackInfo,
}

impl App {
    /// The control column, lifted out of its panel builder so `two_pane`
    /// can put it beside the pad or inside a disclosure above it. Body
    /// unchanged from the fixed 250pt panel it used to live in.
    fn controls_body(&mut self, ui: &mut egui::Ui, snap: &Snapshot<'_>) {
        let (current_z, target_snapshot, gen_ms, loaded, device) =
            (snap.current_z, snap.target_snapshot, snap.gen_ms, snap.loaded, snap.device);
                ui.add_space(8.0);
                ui.heading("beatspace");
                ui.label("latent-space beat morpher");
                ui.horizontal(|ui| {
                    let was = self.page;
                    ui.selectable_value(&mut self.page, Page::Morph, "morph");
                    ui.selectable_value(&mut self.page, Page::Arrange, "arrange");
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
                if self.page == Page::Arrange {
                    self.arrange_column(ui);
                    return;
                }

                if ui.button("🎲  Randomize all dims").clicked() {
                    self.randomize();
                    self.status = "randomized latent".into();
                }
                ui.horizontal(|ui| {
                    if ui.button("Set A").clicked() {
                        self.z_a = target_snapshot.to_vec();
                        self.status = "captured A".into();
                    }
                    if ui.button("Set B").clicked() {
                        self.z_b = target_snapshot.to_vec();
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
                self.playback_section(ui, snap.playback);

                ui.separator();
                ui.label("Generator weights:");
                ui.add(egui::TextEdit::singleline(&mut self.ckpt_text).id_salt("gui-self_ckpt_text").desired_width(230.0));
                ui.horizontal(|ui| {
                    if ui.button("Load weights").clicked() {
                        let p = PathBuf::from(self.ckpt_text.trim());
                        if p.exists() {
                            self.shared.lock().unwrap().weights_request = Some(p);
                            self.status = "loading weights…".into();
                        } else {
                            self.status = "weights path not found".into();
                        }
                    }
                    // The common case while a run is going in the other
                    // page: hear the newest checkpoint without retyping
                    // its path.
                    if ui.button("↻ latest")
                        .on_hover_text("reload gen_latest.safetensors from the checkpoint directory")
                        .clicked()
                    {
                        let p = self.cfg.ckpt_dir.join("gen_latest.safetensors");
                        if p.exists() {
                            self.ckpt_text = p.display().to_string();
                            self.shared.lock().unwrap().weights_request = Some(p);
                            self.status = "reloading latest…".into();
                        } else {
                            self.status = "no gen_latest.safetensors yet".into();
                        }
                    }
                });
                ui.label(egui::RichText::new(format!("loaded: {loaded}")).small().weak());

                ui.separator();
                ui.label("Save current beat:");
                ui.add(egui::TextEdit::singleline(&mut self.save_text).id_salt("gui-self_save_text").desired_width(230.0));
                let rung = audio_io::effective_format(self.device.bounce_format);
                if ui.button(format!("💾  Save {}", rung.ext()))
                    .on_hover_text("at this device's bounce rung (arrange page); never overwrites")
                    .clicked()
                {
                    let p = PathBuf::from(self.save_text.trim());
                    self.shared.lock().unwrap().save_request = Some(p);
                    self.status = format!("saving {}…", rung.ext());
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
    }

    /// The page under the controls: the training dashboard, or the pad.
    fn content_body(&mut self, ui: &mut egui::Ui, snap: &Snapshot<'_>) {
        let current_z = snap.current_z;
        if self.page == Page::Train {
            self.train_body(ui);
            return;
        }
        if self.page == Page::Arrange {
            self.arrange_body(ui);
            return;
        }
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
    }
}

impl App {
    /// The arrangement page's canvas. The library and transport are read
    /// under the shared lock once, the view runs, and what it asked for
    /// is written back.
    fn arrange_body(&mut self, ui: &mut egui::Ui) {
        self.arrange_take_pending();
        let (library, playhead, playing, recording, can_record) = {
            let s = self.shared.lock().unwrap();
            (s.arr.library.clone(), s.arr.pos, s.arr.playing, s.arr.recording_secs, s.playback.stream)
        };
        let io = ArrangeIo { library: &library, playhead, playing, recording, can_record, bounce_format: self.device.bounce_format };
        let out = self.arrange.ui(ui, &io);
        self.arrange_apply(out);
    }

    /// A take the stream thread finished: place it through the editor so
    /// it is undoable, then publish.
    fn arrange_take_pending(&mut self) {
        let pending = self.shared.lock().unwrap().arr.pending_place.take();
        if let Some((source, start, beats, bpm)) = pending {
            // The beats track, or wherever the first track is.
            if let Some(track) = self.arrange.timeline().tracks.first().map(|t| t.id) {
                // Exact: a take starts where recording started, not on the
                // nearest grid line. Locked to the tempo it was recorded at.
                let placed = self.arrange.editor.apply(Command::Place { track, source, start, source_beats: beats, exact: true, lock: Some(bpm) });
                match placed {
                    Ok(_) => {}
                    Err(e) => self.status = format!("the take could not be placed at {start:.2}: {e:?} — it is in the library"),
                }
                let mut s = self.shared.lock().unwrap();
                s.arr.timeline = self.arrange.timeline().clone();
            }
        }
    }

    /// The arrangement's controls column: the library, imports, exports,
    /// and the selected clip.
    fn arrange_column(&mut self, ui: &mut egui::Ui) {
        let (library, playhead, playing, notice, recording, can_record) = {
            let mut s = self.shared.lock().unwrap();
            (s.arr.library.clone(), s.arr.pos, s.arr.playing, s.arr.notice.take(), s.arr.recording_secs, s.playback.stream)
        };
        if let Some(n) = notice {
            self.status = n;
        }
        let io = ArrangeIo { library: &library, playhead, playing, recording, can_record, bounce_format: self.device.bounce_format };
        let mut out = ArrangeOut::default();
        self.arrange.library_ui(ui, &io, &mut out);
        self.arrange_apply(out);
        ui.add_space(6.0);
        ui.label(egui::RichText::new(&self.status).italics());
    }

    /// Carry out what the arrangement view asked for.
    fn arrange_apply(&mut self, out: ArrangeOut) {
        if let Some(st) = out.status {
            self.status = st;
        }
        let mut s = self.shared.lock().unwrap();
        if out.changed {
            s.arr.timeline = self.arrange.timeline().clone();
        }
        if let Some(b) = out.seek {
            s.arr.seek = Some(b);
            s.arr.pos = b;
        }
        if out.toggle_play {
            s.arr.playing = !s.arr.playing;
            // The arrangement takes the device; the generator's own
            // transport must not also be stopped or the block queue is
            // silent.
            if s.arr.playing {
                s.stopped = false;
            }
            self.status = if s.arr.playing { "arrangement playing".into() } else { "arrangement stopped".into() };
        }
        if out.capture_beat {
            s.arr.capture = true;
            self.status = "capturing…".into();
        }
        if let Some(start) = out.toggle_record {
            if s.arr.playing {
                self.status = "stop the arrangement first: a take records the live stream".into();
            } else {
                s.arr.record_toggle = Some(start);
                s.stopped = false;
            }
        }
        drop(s);
        if let Some(path) = out.import_file {
            self.import_audio(path);
        }
        if let Some(path) = out.export_wav {
            self.export_arrangement(path);
        }
        if let Some(dir) = out.save_project {
            self.save_project(dir);
        }
        if let Some(dir) = out.load_project {
            self.load_project(dir);
        }
        if let Some(f) = out.set_bounce_format {
            self.set_bounce_format(f);
        }
    }

    /// Move the fidelity slider and keep it on this device: the sidecar in
    /// the user's config directory, never the project or the checkpoint,
    /// because storage constraints are per device.
    fn set_bounce_format(&mut self, f: BounceFormat) {
        self.device.bounce_format = f;
        // The audio thread writes saved beats with it too.
        self.shared.lock().unwrap().bounce_format = f;
        match self.device.save() {
            Ok(p) => self.status = format!("{} (kept in {})", f.label(), p.display()),
            Err(e) => self.status = format!("{} for this session; could not write the device prefs: {e}", f.label()),
        }
    }

    fn save_project(&mut self, dir: String) {
        let dir = if dir.is_empty() { crate::arrange::project::default_dir(&self.cfg.ckpt_dir) } else { PathBuf::from(dir) };
        let library = self.shared.lock().unwrap().arr.library.clone();
        match crate::arrange::project::save(&dir, self.arrange.timeline(), &library, self.device.bounce_format) {
            Ok(written) => self.status = format!("saved project to {} ({} sources as {})", dir.display(), library.beats.len() + library.files.len(), written.label()),
            Err(e) => self.status = format!("save project: {e:#}"),
        }
    }

    fn load_project(&mut self, dir: String) {
        let dir = if dir.is_empty() { crate::arrange::project::default_dir(&self.cfg.ckpt_dir) } else { PathBuf::from(dir) };
        match crate::arrange::project::load(&dir, self.cfg.sr) {
            Ok((timeline, library)) => {
                let n = (library.beats.len(), library.files.len());
                self.arrange.replace(timeline);
                let mut s = self.shared.lock().unwrap();
                s.arr.playing = false;
                s.arr.seek = Some(0.0);
                s.arr.pos = 0.0;
                s.arr.library = Arc::new(library);
                s.arr.timeline = self.arrange.timeline().clone();
                self.status = format!("loaded {} ({} beats, {} files)", dir.display(), n.0, n.1);
            }
            Err(e) => self.status = format!("load project: {e:#}"),
        }
    }

    /// Decode a file on a worker and add it to the library when done.
    fn import_audio(&mut self, path: String) {
        let sr = self.cfg.sr;
        let shared = self.shared.clone();
        self.status = format!("importing {path}…");
        std::thread::spawn(move || {
            let p = PathBuf::from(&path);
            let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(path.clone());
            match audio_io::decode_to_mono(&p, sr) {
                Ok(audio) if !audio.is_empty() => {
                    let mut s = shared.lock().unwrap();
                    Arc::make_mut(&mut s.arr.library).add_file(Sample { name: name.clone(), audio, sr, planes: None, bpm: None });
                    s.arr.notice = Some(format!("imported {name}"));
                }
                Ok(_) => shared.lock().unwrap().arr.notice = Some(format!("{name} decoded to nothing")),
                Err(e) => shared.lock().unwrap().arr.notice = Some(format!("import {name}: {e}")),
            }
        });
    }

    /// Render the whole arrangement offline and write it.
    fn export_arrangement(&mut self, path: String) {
        let (timeline, library) = {
            let s = self.shared.lock().unwrap();
            (self.arrange.timeline().clone(), s.arr.library.clone())
        };
        let sr = self.cfg.sr;
        let out = crate::arrange::render::render_all(&timeline, &library, sr);
        let n = out.len();
        if n == 0 {
            self.status = "nothing to export".into();
            return;
        }
        // Never overwrite a render the user asked for by hand.
        let target = audio_io::unique_path(&audio_io::with_format_ext(&PathBuf::from(&path), self.device.bounce_format));
        match audio_io::save_audio(&target, &out, sr, self.device.bounce_format) {
            Ok(written) => self.status = format!("exported {} ({:.1} s)", written.display(), n as f64 / sr as f64),
            Err(e) => self.status = format!("export {path}: {e}"),
        }
    }

    /// What will be played back, and the knobs on it.
    ///
    /// The headline is one sentence the listener can act on — "stream,
    /// drums + instruments, continuing" — and the details under it are the
    /// numbers behind that sentence. The controls are only the ones the
    /// stream has; the loop player has none, and the section says so
    /// rather than showing sliders that do nothing.
    fn playback_section(&mut self, ui: &mut egui::Ui, p: &PlaybackInfo) {
        egui::CollapsingHeader::new("playback")
            .default_open(true)
            .show(ui, |ui| {
                ui.label(egui::RichText::new(p.headline()).strong());

                let ms = |frames: usize| p.secs(frames) * 1000.0;
                let mut lines = vec![format!(
                    "window {} frames ({:.0} ms) · stride {} ({:.0} ms) · overlap {} ({:.0} ms)",
                    p.time_frames, ms(p.time_frames),
                    p.stride_frames, ms(p.stride_frames),
                    p.overlap_frames, ms(p.overlap_frames),
                )];
                lines.push(format!(
                    "model: {} · context {} · beat phase {}",
                    if p.stems >= 2 { "2 stems (harmonic/percussive)" } else { "1 stem (mixed)" },
                    if p.ctx_frames > 0 { format!("{} frames", p.ctx_frames) } else { "none".into() },
                    if p.beat_phase { "on" } else { "off" },
                ));
                if let Some(e) = p.epoch {
                    lines.push(format!("checkpoint epoch {e}"));
                }
                if p.stream {
                    lines.push(format!(
                        "lead {:.2} s queued · {} windows since start · beat phase {:.2} at {:.0} BPM",
                        p.lead_secs, p.windows, p.phase, p.bpm
                    ));
                }
                for l in lines {
                    ui.label(egui::RichText::new(l).small().weak());
                }

                if !p.stream {
                    ui.label(egui::RichText::new(
                        "loop playback has no tempo or lead: train with the dynamic preset \
(train page → model) to get a stream").small().italics());
                    return;
                }

                // The controls. Each writes to Shared; the stream loop reads
                // them before the next window, so a change is heard within
                // one lead's worth of audio.
                let (mut ctl, changed_before) = {
                    let s = self.shared.lock().unwrap();
                    (s.stream_ctl.clone(), s.stream_ctl.clone())
                };
                let beat_on = p.beat_phase;
                ui.add_enabled(beat_on,
                    egui::Slider::new(&mut ctl.bpm, 60.0..=200.0).text("BPM"))
                    .on_hover_text(if beat_on {
                        "tempo the conductor advances the beat phase at; the renderer was trained \
to put its beats where this says"
                    } else {
                        "this model has no beat phase input, so tempo has nothing to drive"
                    });
                ui.add(egui::Slider::new(&mut ctl.smoothing, 0.01..=1.0).logarithmic(true).text("glide"))
                    .on_hover_text("fraction of the way to the pad position each window moves: \
0.1 is the original glide, 1.0 jumps");
                ui.add(egui::Slider::new(&mut ctl.lead_windows, 1..=6).text("lead (windows)"))
                    .on_hover_text("audio kept queued ahead of the playhead. More survives a slow \
render; less makes the pad respond sooner");
                ui.add(egui::Slider::new(&mut ctl.gl_iters, 1..=16).text("phase iters"))
                    .on_hover_text("Griffin-Lim iterations per window: fewer is faster and rougher");
                ui.horizontal(|ui| {
                    if ui.button("↺ restart track")
                        .on_hover_text("forget the previous window and the beat grid, drop the \
queue, and start again from silence at the current pad position")
                        .clicked()
                    {
                        ctl.restart = true;
                        self.status = "restarting the track".into();
                    }
                    if ui.button("reset knobs").clicked() {
                        ctl.bpm = self.cfg.bpm;
                        ctl.smoothing = 0.1;
                        ctl.lead_windows = 2;
                        ctl.gl_iters = self.cfg.griffin_iters_rt;
                    }
                });
                let changed = ctl.bpm != changed_before.bpm
                    || ctl.smoothing != changed_before.smoothing
                    || ctl.lead_windows != changed_before.lead_windows
                    || ctl.gl_iters != changed_before.gl_iters
                    || ctl.restart;
                if changed {
                    self.shared.lock().unwrap().stream_ctl = ctl;
                }
            });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // FONTS ONLY. beatspace paints with nine hand-picked `Color32`
        // literals and sets no Visuals, so `apply_palette` would recolour it
        // — a separate change from giving it its glyphs. Idempotent, so
        // per-frame costs nothing. See font_install_check.py.
        uikit_egui::fonts::install(ui.ctx());
        ui.ctx().request_repaint(); // keep animating

        // snapshot shared state
        let (current_z, target_snapshot, gen_ms, scope, loaded, device, playback) = {
            let s = self.shared.lock().unwrap();
            (
                s.current_z.clone(),
                s.target_z.clone(),
                s.gen_ms,
                s.scope.clone(),
                s.loaded_label.clone(),
                s.device_label.clone(),
                s.playback.clone(),
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

        let snap = Snapshot {
            current_z: &current_z, target_snapshot: &target_snapshot,
            gen_ms, loaded: &loaded, device: &device, playback: &playback,
        };
        // The scope keeps its own bottom panel and is declared FIRST, so it
        // reserves its strip before the two panes divide what is left.
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

        // SIDE BY SIDE OR STACKED. A fixed 250pt control column beside the
        // latent pad is right on a laptop and most of a 400pt phone. Making
        // it a fraction was tried here first: the pad became usable and the
        // column's own controls started clipping, so it was reverted.
        // `two_pane` changes the ARRANGEMENT instead — stacked with the
        // controls collapsed once neither pane can have its minimum, which
        // gives the pad the whole width on a phone.
        uikit_egui::two_pane::two_pane_ui(
            ui, "beatspace", 250.0, 200.0, 240.0, "controls", self,
            |ui, s| s.controls_body(ui, &snap),
            |ui, s| s.content_body(ui, &snap),
        );
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
                egui::TextEdit::singleline(&mut dir).id_salt("gui-dir").desired_width(150.0)).changed()
            {
                self.cfg.data_dir = dir.clone().into();
            }
            if ui.add_enabled(!running, egui::Button::new("📁")).clicked() {
                // The KIT picker, in-window. rfd was here and it PANICKED on
                // this hardware:
                //
                //   pick_folder -> ashpd -> zbus -> tokio::spawn_blocking
                //   "there is no reactor running, must be called from the
                //    context of a Tokio 1.x runtime"
                //
                // rfd was built with features = ["xdg-portal", "tokio"], which
                // makes zbus use tokio's reactor — and pollster::block_on
                // inside an egui frame is not a tokio context. The blocking
                // comment above it described a stall that never happened
                // because the call could not complete at all.
                //
                // Worse, zbus features are ADDITIVE across the graph: turning
                // tokio on for rfd turned it on for accesskit too, and eframe's
                // Linux accessibility thread panicked at startup with the same
                // message, before anything was clicked. One dependency's
                // feature flag broke a subsystem that never asked for it.
                self.picker_open = !self.picker_open;
                if self.picker_open {
                    let start = self.cfg.data_dir.to_string_lossy().to_string();
                    let start = if start.is_empty() { "~".to_string() } else { start };
                    self.picker.reset_to(&start, uikit_egui::path_picker::list_local);
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

        // THE LIBRARY PATH: typed or walked. Both, because a path pasted from
        // a terminal is faster when you have one and unusable when you do not,
        // and this app's own data dir is 124 folders deep in a music library.
        ui.label(egui::RichText::new("library").small());
        ui.horizontal(|ui| {
            let mut path = self.cfg.data_dir.to_string_lossy().to_string();
            if ui.add(egui::TextEdit::singleline(&mut path).id_salt("gui-path")).changed() {
                self.cfg.data_dir = path.into();
            }
            if ui.button(if self.picker_open { "hide" } else { "browse" }).clicked() {
                self.picker_open = !self.picker_open;
                if self.picker_open {
                    let start = self.cfg.data_dir.to_string_lossy().to_string();
                    let start = if start.is_empty() { "~".to_string() } else { start };
                    self.picker.reset_to(&start, uikit_egui::path_picker::list_local);
                }
            }
        });
        if self.picker_open {
            use uikit_egui::path_picker::{list_local, picker_ui, Action};
            let colors = uikit_egui::palette_colors("midnight");
            // The picker returns an ACTION and does no I/O of its own, so the
            // read_dir happens here where it is obviously synchronous. Local
            // listing is sub-millisecond; if this ever browses a network path
            // it has to move off the frame, and the seam is already in place.
            if let Some(a) = picker_ui(ui, &mut self.picker, &colors, 8) {
                match a {
                    Action::Enter(p) => self.picker.open(&p, list_local),
                    Action::Up => self.picker.up(list_local),
                    Action::Choose(p) => {
                        self.cfg.data_dir = p.into();
                        self.picker_open = false;
                        self.status = "library set".into();
                    }
                    Action::ToggleHidden => {
                        self.picker.show_hidden = !self.picker.show_hidden;
                        if let Some(cur) = self.picker.current().map(str::to_string) {
                            self.picker.open(&cur, list_local);
                        }
                    }
                }
            }
            ui.separator();
        }

        ui.horizontal(|ui| {
            ui.label("epochs");
            ui.add_enabled(!running,
                egui::TextEdit::singleline(&mut self.train_epochs).id_salt("gui-self_train_epochs").desired_width(70.0));
        });
        ui.horizontal(|ui| {
            ui.label("batch");
            ui.add_enabled(!running,
                egui::TextEdit::singleline(&mut self.train_batch).id_salt("gui-self_train_batch").desired_width(70.0));
        });
        ui.horizontal(|ui| {
            ui.label("frames");
            ui.add(egui::TextEdit::singleline(&mut self.train_time_frames).id_salt("gui-self_train_time_frames").desired_width(70.0))
                .on_hover_text(
                    "Sampler duration, in mel frames: the width of one training crop and so the \
length of one generated beat. 64 frames is about 0.74 s at the default hop and rate; 128 doubles \
it. Must be a multiple of 16 — the trainer refuses other values up front rather than building a \
model with a broken seed grid.\n\nChanging this changes the MODEL SHAPE, so it cannot be combined \
with resume against an existing checkpoint.");
            ui.checkbox(&mut self.train_resume, "resume")
                .on_hover_text(
                    "Continue from gen_latest in the checkpoint directory, at the epoch it stopped \
at. Off starts over and overwrites.\n\nWeights resume exactly; the optimizer's momentum does not \
(candle keeps it private), so expect roughly a thousand steps of re-warming.");
        });

        self.model_options(ui, running);

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
        if self.train.no_information() {
            // Said before the "flat" line and in a colour, because at these
            // values flat means dead and the two readings prescribe opposite
            // actions — wait, or stop.
            ui.colored_label(egui::Color32::from_rgb(220, 120, 60),
                "DEAD — d_loss at 2ln2, g_loss at ln2: the discriminator emits one logit for real and fake. No gradient means anything from here.");
        } else if self.train.stalled() {
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
            self.refresh_shape_note();
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

    /// The model shape the next run builds: the dynamic-track preset and
    /// its parts. Written straight into `self.cfg`, which is what
    /// `start_training` passes on, so what the page shows is what the
    /// trainer gets.
    fn model_options(&mut self, ui: &mut egui::Ui, running: bool) {
        ui.separator();
        ui.label(egui::RichText::new("model").small());

        let is_dynamic = self.cfg.stems == 2 && self.cfg.ctx_frames > 0
            && self.cfg.beat_phase && self.cfg.seq_windows >= 2;
        let mut dynamic = is_dynamic;
        if ui.add_enabled(!running, egui::Checkbox::new(&mut dynamic, "dynamic tracks"))
            .on_hover_text(
                "Train a renderer that CONTINUES its own output instead of emitting \
independent loops: two mel stems (drums / instruments, split at cache time), a context of \
the previous window, a beat-phase input, and a second discriminator scoring adjacent windows \
across their join. Playback streams instead of looping.\n\nChanges the MODEL SHAPE and the \
cache format, so it cannot resume a legacy checkpoint and rebuilds the mel cache. About \
three to four times the cost of a legacy step.")
            .changed()
        {
            if dynamic {
                let d = Config::dynamic();
                self.cfg.stems = d.stems;
                self.cfg.ctx_frames = d.ctx_frames;
                self.cfg.beat_phase = d.beat_phase;
                self.cfg.seq_windows = d.seq_windows;
                self.cfg.overlap_frames = d.overlap_frames;
                self.cfg.rollout = d.rollout;
                self.cfg.playback = Playback::Stream;
            } else {
                let d = Config::default();
                self.cfg.stems = d.stems;
                self.cfg.ctx_frames = d.ctx_frames;
                self.cfg.beat_phase = d.beat_phase;
                self.cfg.seq_windows = d.seq_windows;
                self.cfg.playback = Playback::Loop;
            }
            self.refresh_shape_note();
        }

        // The parts, for anyone who wants one without the others. Each is
        // a flag on the trainer; the preset is just all of them at once.
        egui::CollapsingHeader::new("parts")
            .id_salt("gui-model-parts")
            .default_open(false)
            .show(ui, |ui| {
                let before = (self.cfg.stems, self.cfg.ctx_frames, self.cfg.beat_phase,
                              self.cfg.seq_windows, self.cfg.overlap_frames, self.cfg.rollout);
                ui.horizontal(|ui| {
                    ui.label("stems");
                    ui.add_enabled_ui(!running, |ui| {
                        ui.selectable_value(&mut self.cfg.stems, 1, "1 mixed");
                        ui.selectable_value(&mut self.cfg.stems, 2, "2 harm/perc");
                    });
                }).response.on_hover_text(
                    "Two splits the spectrogram into harmonic and percussive planes (median \
filtering, no model) so the generator emits drums and instruments as separate channels that \
share one trunk.");
                ui.add_enabled(!running,
                    egui::Slider::new(&mut self.cfg.ctx_frames, 0..=self.cfg.time_frames.min(64)).text("context frames"))
                    .on_hover_text("Frames of the previous window the renderer sees. 0 is the \
unconditional generator; anything else makes every window a continuation.");
                ui.add_enabled(!running, egui::Checkbox::new(&mut self.cfg.beat_phase, "beat phase input"))
                    .on_hover_text("Feed the renderer where in the beat each column falls, \
estimated per clip at cache time. Playback then has a tempo control.");
                ui.add_enabled(!running,
                    egui::Slider::new(&mut self.cfg.seq_windows, 1..=6).text("windows per sample"))
                    .on_hover_text("Consecutive windows per training sample. 2 or more turns on \
the sequence discriminator; a conditional renderer needs at least 2.");
                ui.add_enabled(!running,
                    egui::Slider::new(&mut self.cfg.overlap_frames, 2..=self.cfg.time_frames.saturating_sub(1).max(2))
                        .text("overlap frames"))
                    .on_hover_text("Frames adjacent windows share, in training and playback: the \
crossfade region.");
                ui.add_enabled(!running, egui::Checkbox::new(&mut self.cfg.rollout, "free-running rollout"))
                    .on_hover_text("Also score the renderer continuing ITSELF for a few windows, \
which is what playback asks of it. Off is faster and drifts.");
                let after = (self.cfg.stems, self.cfg.ctx_frames, self.cfg.beat_phase,
                             self.cfg.seq_windows, self.cfg.overlap_frames, self.cfg.rollout);
                if before != after {
                    // Playback mode follows the shape: a conditional model
                    // cannot be looped.
                    self.cfg.playback = if self.cfg.conditional() { Playback::Stream } else { self.cfg.playback };
                    self.refresh_shape_note();
                }
            });

        // What "start" will do, in one line, and whether the checkpoint
        // directory agrees. Said before the button rather than by the
        // trainer refusing after it.
        let c = &self.cfg;
        let mut what = vec![format!("{} stem{}", c.stems, if c.stems == 1 { "" } else { "s" })];
        if c.ctx_frames > 0 { what.push(format!("ctx {}", c.ctx_frames)); }
        if c.beat_phase { what.push("beat phase".into()); }
        if c.seq_windows >= 2 {
            what.push(format!("{}-window sequences{}", c.seq_windows, if c.rollout { " + rollout" } else { "" }));
        }
        what.push(format!("plays as {}", if c.playback == Playback::Stream || c.conditional() { "stream" } else { "loop" }));
        ui.label(egui::RichText::new(format!("will train: {}", what.join(" · "))).small());
        if c.conditional() && c.seq_windows < 2 {
            ui.colored_label(egui::Color32::from_rgb(220, 120, 60),
                "a conditional renderer needs windows per sample ≥ 2 — the trainer will refuse");
        }
        if let Err(why) = &c.validate() {
            ui.colored_label(egui::Color32::from_rgb(220, 120, 60), format!("invalid: {why}"));
        }
        match &self.shape_note {
            Some(Err(why)) if self.train_resume => {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 60),
                    format!("differs from the checkpoint ({why}) — resume will be refused; \
untick resume or use another checkpoint dir"));
            }
            Some(Err(_)) => {
                ui.label(egui::RichText::new(
                    "shape differs from the checkpoint in this directory: a fresh run overwrites it").small());
            }
            _ => {}
        }
    }

    /// Compare `cfg`'s shape with the checkpoint directory's config.json.
    fn refresh_shape_note(&mut self) {
        let saved = self.cfg.ckpt_dir.join("config.json");
        self.shape_note = if saved.exists() {
            Config::load_json(&saved).ok().map(|prev| self.cfg.shape_compatible_with(&prev))
        } else {
            None
        };
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
        let time_frames: usize = self.train_time_frames.trim().parse().unwrap_or(self.cfg.time_frames);
        let resume = self.train_resume;
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
                // Sampler duration and resume, which this launcher never
                // passed. The trainer validates --time-frames before it
                // allocates and refuses a resume whose shape differs from the
                // checkpoint's, so a bad combination reports a named reason in
                // the log pane rather than failing inside a tensor assignment.
                .arg("--time-frames").arg(time_frames.to_string())
                // The dynamic-track shape, from the config this GUI is
                // playing, so the trainer builds what the stream loop will
                // load. Without these a run launched from here would train
                // the legacy model into a directory whose config.json says
                // otherwise.
                .arg("--stems").arg(cfg.stems.to_string())
                .arg("--ctx-frames").arg(cfg.ctx_frames.to_string())
                .arg("--ctx-dim").arg(cfg.ctx_dim.to_string())
                .arg("--beat-phase").arg(cfg.beat_phase.to_string())
                .arg("--seq-windows").arg(cfg.seq_windows.to_string())
                .arg("--overlap-frames").arg(cfg.overlap_frames.to_string())
                .args(if cfg.rollout { &[][..] } else { &["--no-rollout"][..] })
                .args(if resume { &["--resume"][..] } else { &[][..] })
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

        // THE GAUGES, above the curve. "When do I come back" is the question a
        // long run has to answer on sight, so the finish TIME is shown beside
        // the duration — "11h 20m left" still needs arithmetic at 2am.
        ui.horizontal(|ui| {
            let g = |ui: &mut egui::Ui, label: &str, value: String| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(value).strong().size(18.0));
                    ui.label(egui::RichText::new(label).weak().small());
                });
            };
            match self.train.latest() {
                Some(p) => {
                    g(ui, "epoch", format!("{}/{}", p.epoch, self.train.epochs_target));
                    g(ui, "g_loss", format!("{:.4}", p.g_loss));
                    // Marked when it collapses rather than merely shown: a
                    // discriminator at zero has won and the generator has
                    // stopped learning, and the number alone does not say so.
                    let d = format!("{:.4}", p.d_loss);
                    ui.vertical(|ui| {
                        let txt = egui::RichText::new(d).strong().size(18.0);
                        ui.label(if p.d_loss < 0.01 { txt.color(egui::Color32::from_rgb(220, 120, 60)) } else { txt });
                        ui.label(egui::RichText::new(if p.d_loss < 0.01 { "d_loss ⚠" } else { "d_loss" })
                            .weak().small());
                    });
                }
                None => { g(ui, "epoch", "—".into()); }
            }
            match self.train.rate() {
                Some(r) if r > 0.0 => {
                    // PER MINUTE, not per second. At 97 s/epoch the per-second
                    // figure is 0.01 — a number that rounds to nothing and
                    // conveys nothing. 0.6/min is a rate a person can hold.
                    g(ui, "epochs/min", format!("{:.2}", r * 60.0));
                    g(ui, "s/epoch", format!("{:.0}", 1.0 / r));
                }
                _ => { g(ui, "epochs/min", "—".into()); }
            }
            match self.train.eta_secs() {
                Some(e) => {
                    g(ui, "remaining", crate::trainpage::human_secs(e).replace(" left", ""));
                    g(ui, "finishes", finish_clock(e));
                }
                None => { g(ui, "remaining", "—".into()); }
            }
        });
        ui.separator();

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
            // The GENERATOR's curve — the model being kept. Plotting the
            // discriminator's would show a run collapsing as a line going
            // pleasingly down.
            let losses: Vec<f32> = self.train.curve.iter().map(|c| c.g_loss).collect();
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
                p.line_segment([w[0], w[1]], egui::Stroke::new(1.5_f32, egui::Color32::LIGHT_BLUE));
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


/// A wall-clock finish time, `HH:MM`, with the day if it is not today.
///
/// "11h 20m left" is a duration and the question is a TIME — at 2am the
/// subtraction is exactly the thing nobody wants to do. Computed from the
/// system clock with no date library: seconds since the epoch, shifted, and
/// reduced to hours and minutes.
fn finish_clock(eta_secs: f64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let then = now + eta_secs.max(0.0) as u64;
    // Local time needs the offset, which std does not give. Rather than guess
    // UTC and print a wrong clock time, the day count is relative and the
    // hours are UTC-labelled — an honest "+1d 14:20 UTC" beats a confident
    // wrong "02:20".
    let days = (then / 86400) as i64 - (now / 86400) as i64;
    let (h, m) = ((then % 86400) / 3600, (then % 3600) / 60);
    if days > 0 { format!("+{days}d {h:02}:{m:02}Z") } else { format!("{h:02}:{m:02}Z") }
}
