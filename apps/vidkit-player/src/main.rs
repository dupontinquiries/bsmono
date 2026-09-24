//! vidkit-player — the runnable shell over `shared/vidkit`.
//!
//! This exists because the kit had no way to run. Everything was libraries plus
//! one feature-gated `example`, so `cargo run` did nothing, nothing installed,
//! and gitmind's catalogue correctly refused to offer a build button for a
//! `role = "shared-lib"` entry that ships no binary. An app nobody can start is
//! indistinguishable from an app that does not work.
//!
//! ```sh
//! vidkit-player            # built-in test pattern, no file needed
//! vidkit-player clip.mp4   # a local MP4 or WebM
//! ```
//!
//! There is deliberately no decoder yet: nothing in this tree decodes H.264,
//! VP9 or AV1. Opening a real file therefore demuxes it, reports what it found,
//! and shows the test pattern in the video surface. That is an honest partial
//! result — the container, codec, resolution and duration on screen are read
//! from the file — and it beats a black window that cannot say why.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use uikit_egui::{adaptive::Adaptive, skin::Skin, PaletteColors};
use vidkit_core::traits::MediaSource;
use vidkit_egui::{bottom_bar, next_fit, EguiPresenter, Fit, Transport, VideoView};

mod filepick;
#[cfg(feature = "bscale")]
mod analysis;
mod audio_out;
mod gpu_selftest;
mod enhance_plan;
mod decode;
mod export;
mod media_cache;
mod seen;
mod transcribe;
mod io;
mod palette;
mod pattern;
mod playback;

/// What we managed to learn about the file the user named.
struct Opened {
    label: String,
    duration: Option<Duration>,
}

/// Demux a local file far enough to describe it.
///
/// Container is sniffed rather than taken from the extension: a `.mp4` holding
/// WebM is rare and a file with no extension is not, and the magic is the file
/// whereas the name is a guess.
fn open_local(path: &PathBuf) -> Result<Opened, String> {
    // Checked first, and read through a FileSource rather than swallowed.
    //
    // `std::fs::read` loaded the WHOLE FILE into memory to look at sixteen
    // bytes of header. A two-hour lecture is gigabytes and the phone this
    // targets has a few, so the process was killed before a frame appeared and
    // it looked like a crash rather than a mistake.
    let facts = io::check_file(path)?;
    let head = facts.head.clone();

    let is_webm = head.len() >= 4 && head[0..4] == [0x1A, 0x45, 0xDF, 0xA3];
    let is_mp4 = head.len() >= 8
        && matches!(
            &head[4..8],
            b"ftyp" | b"styp" | b"moov" | b"free" | b"skip" | b"mdat"
        );

    // Describe, do not read. This used to walk every packet to print a
    // count -- `next_packet` reads each sample's bytes and rewrites H.264 to
    // Annex-B -- so opening a two-hour lecture read and converted the whole
    // file on the UI thread before the first frame could appear.
    let (kind, profile, dims, dur) = if is_webm {
        let d = vidkit_webm::WebmDemuxer::open(io::FileSource::open(path)?)
            .map_err(|e| format!("webm: {e}"))?;
        ("WebM", d.video_profile(), d.dimensions(), d.duration())
    } else if is_mp4 {
        let d =
            vidkit_mp4::Mp4Demuxer::open(io::FileSource::open(path)?).map_err(|e| format!("mp4: {e}"))?;
        ("MP4", d.video_profile(), d.dimensions(), d.duration())
    } else {
        return Err(format!(
            "{}: not an MP4 or WebM file (first bytes {:02x?})",
            path.display(),
            &head[..head.len().min(8)]
        ));
    };

    let codec = profile
        .map(|p| format!("{} {}-bit", p.codec, p.bit_depth))
        .unwrap_or_else(|| "unknown codec".into());

    Ok(Opened {
        label: format!(
            "{} · {kind} · {codec} · {}x{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            dims.0,
            dims.1
        ),
        duration: dur,
    })
}

struct Player {
    presenter: EguiPresenter,
    view: VideoView,
    transport: Transport,
    colors: PaletteColors,
    skin: Skin,
    started: Instant,
    /// `Err` is shown rather than swallowed: a player that opens nothing and
    /// says nothing is the failure this whole binary exists to avoid.
    opened: Option<Result<Opened, String>>,
    palette: palette::Palette,
    picker: Option<Picker>,
    /// The file currently open, so the picker reopens beside it.
    path: Option<PathBuf>,
    quit: bool,
    /// The GPU upload path, when `VIDKIT_GPU=1`: NV12 planes plus a conversion
    /// shader, 1.5 bytes a pixel against the CPU path's 4.
    gpu: Option<Arc<Mutex<vidkit_egui::glow_present::Nv12Gpu>>>,
    /// Reported, never swallowed. A silent fallback to the CPU path means the
    /// fast one can rot untested and nobody finds out.
    gpu_error: Option<String>,
    /// `cmd_` because `palette` is already the colour palette.
    chrome: vidkit_egui::Chrome,
    settings: vidkit_egui::Settings,
    settings_open: bool,
    /// The most recent text pasted into the window. egui delivers `Event::Paste`
    /// on Ctrl+V, which is how the clipboard is read without a clipboard
    /// dependency and, on Wayland, without a protocol this app has no business
    /// knowing about. The cost is honest: "play from clipboard" works after a
    /// paste, not before one.
    last_paste: Option<String>,
    /// The realtime engine: decoding on its own threads, audio on its own,
    /// frames presented against the audio clock. `None` shows the test pattern.
    engine: Option<playback::Engine>,
    /// Kept alive so the downloaded file outlives the decoder reading it.
    /// Dropping it deletes the file, and ffmpeg is still holding it open.
    downloaded: Option<media_cache::Held>,
    /// Section watching: the panel, the loop that feeds it, and what the
    /// loop has produced so far (recomputed only when its version changes).
    sections: vidkit_egui::SectionsPanel,
    sections_open: bool,
    subtitles: bool,
    worker: Option<transcribe::Worker>,
    seen_version: u64,
    windows: Vec<vidkit_relevance::Window>,
    chapters: Vec<vidkit_relevance::sections::Chapter>,
    hits: Vec<vidkit_relevance::sections::Hit>,
    /// Active watch plan; empty plays everything.
    plan: Vec<vidkit_relevance::Section>,
    export_status: Arc<Mutex<Option<String>>>,
    /// A speed the transport asked for and when it last changed. Applied
    /// once it has been still for SPEED_SETTLE: a speed change restarts both
    /// decoders, and a vertical drag passes through a detent every few
    /// pixels.
    speed_pending: Option<(f64, Instant)>,
    /// The app's own palette, and the shell's laid over it when tideshell
    /// publishes one. `colors` is what is in effect.
    base_colors: PaletteColors,
    ambient: Option<ambient_palette::Watcher>,
    /// Everything watched, loaded when the palette opens and dropped when it
    /// closes; and the moments matching the palette's query.
    seen_index: Option<seen::Index>,
    seen_query: String,
    seen_hits: Vec<seen::SeenHit>,
    /// Picture-in-picture: the video floats, the chapters take the screen.
    pip: vidkit_egui::Pip,
    pip_on: bool,
    /// Enhancement: what analysis learned about the title playing (None =
    /// not analysed, so not enhanced), the plan in force and the output size
    /// it was made for.
    /// GL_MAX_TEXTURE_SIZE, read once at startup; and the decode cap the
    /// running decoder was opened with.
    max_texture: Option<u32>,
    cap_applied: u32,
    /// Repaint intervals while playing, for estimating the refresh rate.
    frame_dts: std::collections::VecDeque<f64>,
    last_frame_at: Option<Instant>,
    enh_facts: Option<enhance_plan::TitleFacts>,
    enh_plan: Option<enhance_plan::Plan>,
    enh_px: (u32, u32),
    analysis_status: Arc<Mutex<String>>,
    analysis_running: Arc<std::sync::atomic::AtomicBool>,
}

/// Probed once: sysfs does not change under a running app.
static GPU_CLASS: std::sync::OnceLock<enhance_plan::GpuClass> = std::sync::OnceLock::new();

const SPEED_SETTLE: Duration = Duration::from_millis(250);

/// File-picker state. Navigation is uikit-egui's PathPicker; the file list and
/// the filter are ours, because `Listing` carries directories only.
struct Picker {
    dir: PathBuf,
    entries: Vec<filepick::Entry>,
    query: String,
    selected: usize,
    show_hidden: bool,
    error: Option<String>,
    just_opened: bool,
}

impl Picker {
    fn at(dir: PathBuf, show_hidden: bool) -> Self {
        let (entries, error) = match filepick::read_dir(&dir, show_hidden) {
            Ok(v) => (v, None),
            // A directory that cannot be read leaves the previous listing
            // empty and SAYS so; silently showing nothing reads as an empty
            // folder, which is a different fact.
            Err(e) => (Vec::new(), Some(e)),
        };
        Self {
            dir,
            entries,
            query: String::new(),
            selected: 0,
            show_hidden,
            error,
            just_opened: true,
        }
    }

    fn visible(&self) -> Vec<filepick::Entry> {
        filepick::filter(&self.entries, &self.query)
    }

    fn move_selection(&mut self, delta: i32) {
        let n = self.visible().len();
        if n == 0 {
            self.selected = 0;
            return;
        }
        let cur = self.selected.min(n - 1) as i32;
        self.selected = (cur + delta).rem_euclid(n as i32) as usize;
    }

    fn reload(&mut self) {
        let d = self.dir.clone();
        let h = self.show_hidden;
        *self = Picker::at(d, h);
    }
}

impl Player {
    fn new(cc: &eframe::CreationContext<'_>, file: Option<PathBuf>) -> Self {
        // Explicit, though `vidkit_egui::install` calls it too. Two reasons:
        // fonts are process-wide state and an app should not depend on a
        // library's side effect for them, and font_install_check.py greps for
        // this call -- an app reaching it only transitively looks identical to
        // switcheroo, which shipped with star and refresh glyphs rendering as
        // boxes. Idempotent, so the double call costs nothing.
        uikit_egui::fonts::install(&cc.egui_ctx);
        // Clean up films left by runs that did not exit cleanly. Drop covers a
        // normal quit; it does not run on SIGKILL, an OOM kill, or a phone's
        // session manager stopping the app -- and each of those leaves a whole
        // video behind.
        let swept = io::sweep_on_start();
        if swept > 0 {
            eprintln!("removed {swept} leftover download(s) from a previous run");
        }

        let colors = vidkit_egui::install(&cc.egui_ctx, snapclone_theme::palette_by_id("dark"));
        // The largest texture the GPU takes: the ceiling on decode size, so a
        // 12K source on a GPU that stops at 8192 is scaled by ffmpeg rather
        // than failing at upload.
        let max_texture = cc.gl.as_ref().map(|gl| {
            use eframe::glow::HasContext;
            // SAFETY: the creation context's GL is current.
            unsafe { gl.get_parameter_i32(eframe::glow::MAX_TEXTURE_SIZE) as u32 }
        });

        // GPU upload by DEFAULT (VIDKIT_GPU=0 opts out): NV12 planes go to the
        // GPU and are converted there, instead of a CPU colour conversion of
        // every frame. It falls back to the CPU path by itself -- here if it
        // cannot initialise, in `produce` if an upload fails -- and says why.
        // Verified pixel-identical to the CPU path on llvmpipe; not yet on the
        // phones' drivers. The glow context is handed over HERE, in
        // CreationContext -- not in `update` -- which stalled the first attempt.
        let (gpu, gpu_error) = if std::env::var("VIDKIT_GPU").as_deref() != Ok("0") {
            match cc.gl.as_ref() {
                Some(gl) => match unsafe { vidkit_egui::glow_present::Nv12Gpu::new(gl) } {
                    Ok(g) => (Some(Arc::new(Mutex::new(g))), None),
                    Err(e) => (None, Some(format!("GPU path unavailable: {e}"))),
                },
                None => (None, Some("no glow context (is the backend wgpu?)".into())),
            }
        } else {
            (None, None)
        };
        let opened = file.as_ref().map(|p| open_local(p));

        // Try to decode it for real. A failure is not fatal: the file still
        // opens, is described, and falls back to the test pattern -- which is
        // what this player did for everything until now.
        // Built here, before `file` is moved into the struct.
        let mut engine = None;
        let mut decode_error = None;
        if let Some(p) = &file {
            match start_file(p.clone()) {
                Ok(e) => engine = Some(e),
                Err(e) => decode_error = Some(format!("no decode: {e}")),
            }
        }

        let status = match &opened {
            Some(Ok(_)) => "NV12 · CPU convert".to_string(),
            Some(Err(_)) => "not playing".to_string(),
            None => "test pattern · NV12 · CPU convert".into(),
        };
        let duration = match &opened {
            Some(Ok(o)) => o.duration,
            _ => Some(Duration::from_secs(212)),
        };

        let mut me = Self {
            presenter: EguiPresenter::new(&cc.egui_ctx),
            view: VideoView::new().with_fit(Fit::Contain),
            transport: Transport {
                duration,
                playing: true,
                status,
                ..Default::default()
            },
            colors,
            skin: Skin::carved(),
            started: Instant::now(),
            opened,
            palette: palette::Palette::default(),
            picker: None,
            path: file,
            quit: false,
            gpu,
            gpu_error: gpu_error.or(decode_error),
            engine,
            downloaded: None,
            chrome: Default::default(),
            settings: Default::default(),
            settings_open: false,
            last_paste: None,
            sections: Default::default(),
            sections_open: false,
            subtitles: false,
            worker: None,
            seen_version: 0,
            windows: Vec::new(),
            chapters: Vec::new(),
            hits: Vec::new(),
            plan: Vec::new(),
            export_status: Arc::new(Mutex::new(None)),
            speed_pending: None,
            base_colors: colors,
            // Follow the shell unless told not to. With no tideshell running
            // the file is absent and this is one failed stat a second.
            ambient: (std::env::var("VIDKIT_AMBIENT").as_deref() != Ok("0"))
                .then(ambient_palette::Watcher::at_runtime_dir)
                .flatten(),
            seen_index: None,
            seen_query: String::new(),
            seen_hits: Vec::new(),
            pip: Default::default(),
            pip_on: false,
            max_texture,
            cap_applied: decode::decode_cap(),
            frame_dts: std::collections::VecDeque::with_capacity(240),
            last_frame_at: None,
            enh_facts: None,
            enh_plan: None,
            enh_px: (0, 0),
            analysis_status: Arc::new(Mutex::new(String::new())),
            analysis_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        // A file named on the command line gets its transcript too.
        if let Some(p) = me.path.clone().filter(|_| me.engine.is_some()) {
            let total = me.opened.as_ref().and_then(|o| o.as_ref().ok()).and_then(|o| o.duration);
            me.start_transcription(seen::url_for_file(&p), None, Some(p.clone()), transcribe::AudioIn { input: p.to_string_lossy().into(), headers: Vec::new() }, total, None);
        }
        me
    }

    /// Feed the presenter from the engine: the frame the clock says is due,
    /// and when to ask again. Never blocks -- decoding and audio run on the
    /// engine's own threads. Without an engine, the test pattern.
    fn produce(&mut self, eframe_frame: &eframe::Frame) {
        let Some(engine) = self.engine.as_mut() else {
            let t = self.started.elapsed().as_secs_f32();
            self.present(eframe_frame, &pattern::bars(t * 0.25));
            self.presenter.set_deadline(Some(Instant::now() + Duration::from_millis(33)));
            return;
        };
        if let Some(e) = engine.take_audio_error() {
            self.gpu_error = Some(e);
        }
        // Volume every tick: one atomic store, read by the audio thread per
        // block. The transport held a volume that nothing ever applied.
        engine.set_volume(self.transport.volume);
        let want = self.transport.speed as f64;
        if (want - engine.speed()).abs() > 1e-3 {
            match self.speed_pending {
                Some((v, since)) if (v - want).abs() < 1e-3 => {
                    if since.elapsed() >= SPEED_SETTLE {
                        self.speed_pending = None;
                        if let Err(e) = engine.set_speed(want) {
                            self.gpu_error = Some(format!("speed change failed: {e}"));
                        }
                    }
                }
                _ => self.speed_pending = Some((want, Instant::now())),
            }
        } else {
            self.speed_pending = None;
        }
        let tick = engine.tick();
        self.transport.position = engine.position();
        // Say what is timing playback. On a device, "wall clock (no audio
        // player)" versus "audio clock (pw-play)" is the first thing to know
        // when sound and picture disagree.
        let status = format!(
            "{} · NV12 {} · {} clock ({})",
            engine.decoder(),
            if self.gpu.is_some() { "GPU" } else { "CPU convert" },
            if engine.clock_is_audio() { "audio" } else { "wall" },
            engine.audio_note
        );
        if self.transport.status != status {
            self.transport.status = status;
        }
        match tick {
            playback::Tick::Show { frame, next } => {
                self.present(eframe_frame, &frame);
                if let Some(e) = self.engine.as_ref() {
                    e.recycle(frame);
                }
                self.presenter.set_deadline(Some(Instant::now() + next));
            }
            playback::Tick::Wait(d) => self.presenter.set_deadline(Some(Instant::now() + d)),
            playback::Tick::Ended => {
                // Dropped rather than left to fall back to the test pattern:
                // a clip that has finished must not look like a broken decoder.
                self.engine = None;
                self.transport.playing = false;
                self.presenter.idle();
            }
        }
    }

    /// Put one frame on screen: GPU upload when available, CPU otherwise. A
    /// failed upload switches to the CPU path for good and says why, instead
    /// of leaving a black window.
    ///
    /// Borrows the frame: the caller hands its planes back to the decoder's
    /// pool afterwards.
    fn present(&mut self, eframe_frame: &eframe::Frame, frame: &vidkit_core::Frame) {
        if let (Some(gpu), Some(gl)) = (&self.gpu, eframe_frame.gl()) {
            let failed = match gpu.lock() {
                // SAFETY: eframe's glow context is current on the UI thread.
                Ok(mut g) => unsafe { g.upload(gl, frame) }.err(),
                Err(_) => Some("GPU state poisoned".to_string()),
            };
            match failed {
                None => return,
                Some(e) => {
                    self.gpu_error = Some(format!("GPU upload failed, using the CPU path: {e}"));
                    self.gpu = None;
                }
            }
        }
        self.presenter.show(frame);
    }
}

impl eframe::App for Player {
    fn ui(&mut self, _ui: &mut egui::Ui, _f: &mut eframe::Frame) {}

    #[allow(deprecated)]
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        if self.quit {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.handle_keys(ctx);

        // Play/pause is toggled from many places (keys, taps, palette); the
        // engine follows the transport here, once, rather than at each site.
        if let Some(e) = self.engine.as_mut() {
            if self.transport.playing && e.is_paused() { e.resume(); }
            if !self.transport.playing && !e.is_paused() { e.pause(); }
        }
        if self.transport.playing {
            self.produce(_f);
        } else {
            // Paused: stop asking for repaints. Without this the deadline from
            // the last played frame stays set, egui is asked to redraw
            // immediately, and a paused player burns the GPU at full rate to
            // show a still image -- which on a phone is most of the battery
            // this app can waste, and looks like nothing at all on a desktop.
            self.presenter.idle();
        }
        self.presenter.schedule_repaint();

        ctx.input(|i| {
            for e in &i.events {
                if let egui::Event::Paste(t) = e {
                    self.last_paste = Some(t.clone());
                }
            }
        });

        // Settings are pushed into the live config every frame rather than on
        // an Apply button: every control is reversible and cheap, and seeing
        // the change while dragging the slider is the point of having one.
        self.chrome.policy = self.settings.chrome_policy();
        *self.view.gestures() = self.settings.gesture_config();
        self.view.fit = self.settings.fit();
        self.transport.rotation = self.settings.rotation;

        let settings_open = self.settings_open;
        // The video frame is the backdrop, so the blur samples the actual
        // picture. `full_uv` because the panel floats over the whole surface
        // rather than a crop of it.
        let backdrop = self.presenter.texture().map(|t| {
            (
                t.id(),
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            )
        });
        vidkit_egui::settings::show_over(
            ctx,
            &mut self.settings_open,
            &mut self.settings,
            backdrop,
        );

        // Any pointer activity wakes the chrome. A modal open holds it up, as
        // does being paused -- both are moments the viewer is using the UI.
        let now = Instant::now();
        // Sampled BEFORE waking. `wake` makes the chrome interactive again in
        // the same frame, so asking afterwards always says "act" and the tap
        // that was meant to reveal also toggles playback -- which is exactly
        // what happened, and only a screenshot showed it: the policy had the
        // rule and the player never asked.
        let tap_only_reveals = self.chrome.tap_reveals();
        let moved = ctx.input(|i| {
            i.pointer.velocity().length() > 0.5 || i.pointer.any_down() || !i.events.is_empty()
        });
        if moved {
            self.chrome.wake(now);
        }
        self.chrome
            .update(now, self.transport.playing, settings_open);
        if let Some(after) = self.chrome.next_repaint() {
            ctx.request_repaint_after(after);
        }
        let chrome_alpha = self.chrome.alpha();

        // NOTE: this player already had its own command palette (src/palette.rs)
        // and file picker before vidkit-egui grew one. The kit module is kept
        // because the voice router shares its `Action` set -- two lists of
        // commands would drift -- but it is NOT wired here, or Ctrl+P opens two
        // palettes and only the first one sees the key. Migrating the app's
        // palette onto the kit's actions is the follow-up; adding a second was
        // the mistake.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma)) {
            self.settings_open = !self.settings_open;
        }

        if !self.chrome.hidden() {
        egui::TopBottomPanel::bottom("transport").show(ctx, |ui| {
            ui.set_opacity(chrome_alpha);
            let adaptive = Adaptive::detect(ui.ctx());
            let act = bottom_bar(
                ui,
                &mut self.transport,
                self.view.fit,
                &self.colors,
                &self.skin,
                &adaptive,
            );
            if act.toggle_play {
                self.transport.playing = !self.transport.playing;
            }
            if let Some(to) = act.seek_to {
                self.seek_to(to);
            }
            if act.rotate {
                // Through settings, not straight onto the transport: two
                // places holding rotation means the popup shows one value and
                // the picture another.
                self.settings.rotation = (self.settings.rotation + 1).rem_euclid(4);
            }
            if act.cycle_fit {
                self.settings.fill = vidkit_egui::FillMode::from_fit(next_fit(self.view.fit));
            }
            if ui.small_button("⚙").on_hover_text("settings (Ctrl+,)").clicked() {
                self.settings_open = !self.settings_open;
            }
            if act.reset_view {
                self.transport.zoom = 1.0;
                self.transport.rotation = 0;
            }
        });
        }

        if !Adaptive::detect(ctx).is_compact() && !self.chrome.hidden() {
            egui::TopBottomPanel::bottom("hints").show(ctx, |ui| {
                ui.set_opacity(chrome_alpha);
                ui.horizontal_wrapped(|ui| {
                    ui.small("drag ↔ scrub (higher = faster)");
                    ui.separator();
                    ui.small("left ↕ volume");
                    ui.separator();
                    ui.small("right ↕ speed");
                    ui.separator();
                    ui.small("right edge ↕ rotate");
                    ui.separator();
                    ui.small("pinch: 5% zoom, far = fit/fill");
                    ui.separator();
                    ui.small("tap: play/pause");
                });
            });
        }

        // What is playing, always visible.
        //
        // This used to live only in the bar's status field, which the adaptive
        // layout drops on anything narrower than a desktop -- so at 960 points
        // the app opened a file, parsed it, and said nothing about it. "What am
        // I playing" is the primary question a player answers; it does not
        // belong in the first field to be dropped for space.
        self.sections_tick(ctx);
        self.follow_shell_colours(ctx);
        self.follow_display(ctx);
        self.measure_refresh();

        let mut want_analysis = false;
        egui::TopBottomPanel::top("what").show(ctx, |ui| match &self.opened {
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::from_rgb(220, 120, 120), e);
            }
            Some(Ok(o)) => {
                ui.horizontal_wrapped(|ui| {
                    ui.small(&o.label);
                    ui.separator();
                    // Said plainly rather than left to be inferred from a
                    // picture that does not match the file's dimensions.
                    ui.small(if self.engine.is_some() {
                        "decoding via ffmpeg"
                    } else {
                        "not decoding — showing the test pattern"
                    });
                    ui.separator();
                    if ui.selectable_label(self.sections_open, "Sections").clicked() {
                        self.sections_open = !self.sections_open;
                    }
                    if ui.selectable_label(self.subtitles, "CC").on_hover_text("subtitles from the transcript").clicked() {
                        self.subtitles = !self.subtitles;
                    }
                    let running = self.analysis_running.load(std::sync::atomic::Ordering::Relaxed);
                    if ui.selectable_label(running, "Analyse").on_hover_text("bscale analysis of watched titles, within a time budget; only analysed titles are enhanced").clicked() && !running {
                        want_analysis = true;
                    }
                    if let Some(p) = &self.enh_plan {
                        ui.small(format!("enhance: {:?}", p.tier)).on_hover_text(p.reason.clone());
                    }
                    if ui.selectable_label(self.pip_on, "PiP").on_hover_text("float the video over the chapters; drag it off the side to hide it").clicked() {
                        self.pip_on = !self.pip_on;
                        self.pip.unhide();
                    }
                });
            }
            None => {
                ui.small("test pattern — pass a .mp4 or .webm to open one");
            }
        });
        if want_analysis {
            self.start_analysis();
        }

        // After the top bar, so the panel sits beside the picture rather than
        // claiming the full window height and pushing its strip under the bar.
        self.sections_ui(ctx);

        if self.pip_on {
            // The chapters are the page; the video floats over it.
            let page = egui::CentralPanel::default().show(ctx, |ui| self.sections_in(ui));
            for a in page.inner {
                self.section_action(a);
            }
            self.pip_ui(ctx, page.response.rect);
            self.palette_ui(ctx);
            self.picker_ui(ctx);
            return;
        }

        let central = egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(gpu) = self.gpu.clone() {
                let surface = ui.available_rect_before_wrap();
                let (w, h) = gpu.lock().map(|g| g.size()).unwrap_or((0, 0));
                ui.painter()
                    .rect_filled(surface, 0.0, egui::Color32::from_rgb(28, 24, 38));
                let r = vidkit_egui::fit_rect(
                    surface,
                    egui::Vec2::new(w as f32, h as f32),
                    vidkit_egui::PixelAspect::SQUARE,
                    self.view.fit,
                );
                if r.width() > 0.0 {
                    let ppp = ui.ctx().pixels_per_point();
                    self.apply_enhance(((r.width() * ppp) as u32, (r.height() * ppp) as u32));
                    ui.painter().add(vidkit_egui::glow_present::callback(r, gpu));
                }
                return;
            }
            let (_r, action) = self.view.show(ui, &self.presenter, &mut self.transport);
            if action.toggle_play && tap_only_reveals {
                // Consumed by bringing the chrome back. Doing both means the
                // viewer asks to see the controls and gets a pause they have to
                // undo.
            } else if action.toggle_play {
                self.transport.playing = !self.transport.playing;
            }
            if let Some(to) = action.seek_to {
                self.seek_to(to);
            }
        });

        self.subtitle_overlay(ctx, central.response.rect);

        // Overlays after the page, so they draw on top of it.
        self.palette_ui(ctx);
        self.picker_ui(ctx);
    }
}

impl Player {
    /// Keyboard entry points.
    ///
    /// Read before the panels so a keystroke that opens an overlay takes
    /// effect on the same frame -- otherwise Ctrl+P feels like it dropped the
    /// first press.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (cmd_p, cmd_o, cmd_q, esc, space) = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::P),
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::Q),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Space),
            )
        });

        if cmd_q {
            self.quit = true;
        }
        if esc {
            // One Escape closes one thing, innermost first: closing both at
            // once loses the palette the picker was opened from.
            if self.picker.is_some() {
                self.picker = None;
            } else {
                self.palette.close();
            }
        }
        if cmd_p {
            if self.palette.is_open() {
                self.palette.close();
            } else {
                self.palette.open();
            }
        }
        if cmd_o {
            self.open_picker();
        }
        // Space only when no overlay has the keyboard, or it types a space
        // into the query instead of toggling playback.
        if space && !self.palette.is_open() && self.picker.is_none() {
            self.transport.playing = !self.transport.playing;
        }
    }

    fn open_picker(&mut self) {
        self.palette.close();
        let dir = filepick::start_dir(self.path.as_deref());
        self.picker = Some(Picker::at(dir, false));
    }

    fn run(&mut self, c: palette::Command) {
        use palette::Command as C;
        match c {
            C::OpenFile => self.open_picker(),
            C::PlayPause => self.transport.playing = !self.transport.playing,
            // Through `settings`, not straight onto the transport: two places
            // holding rotation means the popup shows one value and the picture
            // another.
            C::Rotate => self.settings.rotation = (self.settings.rotation + 1).rem_euclid(4),
            C::CycleFit => {
                self.settings.fill = vidkit_egui::FillMode::from_fit(next_fit(self.view.fit))
            }
            C::ResetView => {
                self.transport.zoom = 1.0;
                self.settings.rotation = 0;
            }
            C::Settings => self.settings_open = true,
            // Unimplemented rows say so rather than doing nothing: an entry
            // that appears to work and does not is worse than one that admits
            // it is unfinished.
            C::PlayClipboard => self.play_clipboard(),
            C::Subtitles => self.subtitles = !self.subtitles,
            C::ScrubToSections => self.sections_open = !self.sections_open,
            // The same 5% steps the gestures and the bar use, so the three
            // controls cannot land on different values.
            C::SpeedUp => {
                self.transport.speed = vidkit_egui::apply_speed_step(self.transport.speed, 1)
            }
            C::SpeedDown => {
                self.transport.speed = vidkit_egui::apply_speed_step(self.transport.speed, -1)
            }
            C::VolumeUp => {
                self.transport.volume = vidkit_egui::apply_volume_step(self.transport.volume, 1)
            }
            C::VolumeDown => {
                self.transport.volume = vidkit_egui::apply_volume_step(self.transport.volume, -1)
            }
            C::Quit => self.quit = true,
        }
    }

    fn load(&mut self, path: PathBuf) {
        let result = open_local(&path);
        self.transport.duration = match &result {
            Ok(o) => o.duration,
            Err(_) => None,
        };
        self.transport.position = Duration::ZERO;
        self.started = Instant::now();
        // Was missing entirely: `open_local` only demuxes far enough to
        // build the metadata label (codec, dimensions, packet count) --
        // it never starts a real, ongoing decoder. The app's own
        // constructor already does this correctly for a file passed on
        // the command line (`decode::FfmpegFrames::open(p, Some(1920))`,
        // right after its own `open_local` call); this runtime re-open
        // path (drag-drop, the file picker) never had the same second
        // step, so nothing ever actually played after the first file --
        // confirmed against the reported "not playing anything from
        // files or clipboard," which routes through this exact function.
        //
        // Now an engine: video and audio on their own threads, the audio clock
        // as master. A failure is REPORTED -- `.ok()` here used to leave a file
        // that would not decode showing nothing and saying nothing.
        self.engine = match start_file(path.clone()) {
            Ok(e) => { self.transport.playing = true; Some(e) }
            Err(e) => { self.gpu_error = Some(format!("cannot play {}: {e}", path.display())); None }
        };
        self.start_transcription(seen::url_for_file(&path), None, Some(path.clone()), transcribe::AudioIn { input: path.to_string_lossy().into(), headers: Vec::new() }, self.transport.duration, None);
        self.path = Some(path);
        self.opened = Some(result);
    }

    /// Dim the page behind an overlay.
    ///
    /// Without this the palette floats over moving colour bars and its rows
    /// are unreadable -- which a screenshot showed immediately and no test
    /// could. uikit-egui's `command_bar` defines SCRIM_PANE for exactly this
    /// and notes that a scrim which only works on one theme is the bug it
    /// exists to stop repeating, so the value comes from there rather than
    /// from a number picked here.
    fn scrim(&self, ctx: &egui::Context) {
        let alpha = (uikit_egui::command_bar::SCRIM_PANE * 255.0) as u8;
        egui::Area::new(egui::Id::new("vidkit-scrim"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                let r = ui.ctx().screen_rect();
                ui.painter()
                    .rect_filled(r, 0.0, egui::Color32::from_black_alpha(alpha));
                ui.allocate_rect(r, egui::Sense::click());
            });
    }

    /// An opaque frame for overlay windows.
    ///
    /// egui's default window fill is translucent; over video that is not a
    /// style choice, it is illegible.
    fn overlay_frame(&self) -> egui::Frame {
        egui::Frame::window(&egui::Style::default())
            .fill(self.colors.bg_card)
            .stroke(egui::Stroke::new(1.0, self.colors.border))
    }

    fn palette_ui(&mut self, ctx: &egui::Context) {
        // The surface is the kit's: placement (turned over on a phone, the usual
        // card on a desk), scrim, keys, focus and one-line rows all come from
        // `command_bar::show`. This supplies the rows and runs the choice.
        // Commands, then the moments in watched videos that match. A leading
        // `?` searches only what was watched, from two letters.
        let query = self.palette.bar.query.clone();
        let seen_only = query.trim_start().starts_with('?');
        let sq = query.trim_start().trim_start_matches('?').trim().to_string();
        if self.palette.is_open() {
            let idx = self.seen_index.get_or_insert_with(|| seen::dir().map(|d| seen::Index::load(&d)).unwrap_or_default());
            if sq != self.seen_query {
                let min = if seen_only { 2 } else { 3 };
                self.seen_hits = if sq.chars().count() >= min { idx.search(&sq, if seen_only { 20 } else { 5 }, 2) } else { Vec::new() };
                self.seen_query = sq;
            }
        } else if self.seen_index.is_some() {
            self.seen_index = None;
            self.seen_query.clear();
            self.seen_hits.clear();
        }
        let matches = if seen_only { Vec::new() } else { self.palette.matches() };
        let mut rows: Vec<uikit_egui::command_bar::Row> = matches
            .iter()
            .map(|c| uikit_egui::command_bar::Row::new(c.label(), c.shortcut().unwrap_or("")))
            .collect();
        rows.extend(self.seen_hits.iter().map(|h| {
            uikit_egui::command_bar::Row::new(
                format!("“{}…” — {}", h.snippet.trim(), h.title),
                vidkit_egui::format_time(h.at),
            )
        }));
        let adaptive = uikit_egui::Adaptive::detect(ctx);
        let out = uikit_egui::command_bar::show(
            ctx,
            "vidkit-palette",
            &mut self.palette.bar,
            &rows,
            "type a command, or ? to search what you watched",
            &self.colors,
            &adaptive,
            |_| {},
        );
        if let Some(i) = out.chosen {
            if let Some(c) = matches.get(i).copied() {
                self.palette.close();
                self.run(c);
            } else if let Some(h) = self.seen_hits.get(i - matches.len()).cloned() {
                self.palette.close();
                self.open_seen(&h.url, h.at);
            }
        }
    }

    fn picker_ui(&mut self, ctx: &egui::Context) {
        let Some(p) = self.picker.as_mut() else {
            return;
        };
        let (up, down, enter) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if up {
            p.move_selection(-1);
        }
        if down {
            p.move_selection(1);
        }

        let mut go_to: Option<PathBuf> = None;
        let mut play: Option<PathBuf> = None;
        let mut close = false;
        let mut toggle_hidden = false;

        let frame = self.overlay_frame();
        self.scrim(ctx);
        let p = self.picker.as_mut().expect("checked above");
        egui::Window::new("Open file")
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .collapsible(false)
            .resizable(false)
            .default_width(560.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui.button("⬆ up").clicked() {
                        if let Some(parent) = p.dir.parent() {
                            go_to = Some(parent.to_path_buf());
                        }
                    }
                    // Elided from the LEFT, as uikit's picker does: the end of
                    // a path is the part that identifies it.
                    ui.label(
                        egui::RichText::new(uikit_egui::path_picker::elide_left(
                            &p.dir.display().to_string(),
                            52,
                        ))
                        .monospace()
                        .small(),
                    );
                    if ui.checkbox(&mut p.show_hidden, "hidden").changed() {
                        toggle_hidden = true;
                    }
                });

                let edit = ui.add(
                    egui::TextEdit::singleline(&mut p.query).id_salt("vidkit-picker-query")
                        .hint_text("filter…")
                        .desired_width(f32::INFINITY),
                );
                if p.just_opened {
                    edit.request_focus();
                    p.just_opened = false;
                }
                if edit.changed() {
                    p.selected = 0;
                }

                ui.separator();
                if let Some(e) = &p.error {
                    ui.colored_label(egui::Color32::from_rgb(220, 120, 120), e);
                }

                let visible = p.visible();
                if visible.is_empty() && p.error.is_none() {
                    // Distinguishes "nothing here" from "nothing matches",
                    // which are different problems with different fixes.
                    ui.weak(if p.query.trim().is_empty() {
                        "no playable files or folders here"
                    } else {
                        "nothing matches that filter"
                    });
                }

                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for (i, e) in visible.iter().enumerate() {
                        let label = if e.is_dir {
                            format!("📁 {}", e.name)
                        } else {
                            format!("🎞 {}", e.name)
                        };
                        let r = ui.selectable_label(i == p.selected, label);
                        if r.clicked() {
                            p.selected = i;
                        }
                        if r.double_clicked() {
                            let path = p.dir.join(&e.name);
                            if e.is_dir {
                                go_to = Some(path);
                            } else {
                                play = Some(path);
                            }
                        }
                    }
                });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("open").clicked() {
                        if let Some(e) = visible.get(p.selected) {
                            let path = p.dir.join(&e.name);
                            if e.is_dir {
                                go_to = Some(path);
                            } else {
                                play = Some(path);
                            }
                        }
                    }
                    if ui.button("cancel").clicked() {
                        close = true;
                    }
                    ui.weak("Enter opens · Esc cancels");
                });
            });

        if enter {
            if let Some(e) = p.visible().get(p.selected).cloned() {
                let path = p.dir.join(&e.name);
                if e.is_dir {
                    go_to = Some(path);
                } else {
                    play = Some(path);
                }
            }
        }
        if toggle_hidden {
            p.reload();
        }
        if let Some(d) = go_to {
            let hidden = p.show_hidden;
            self.picker = Some(Picker::at(d, hidden));
        }
        if let Some(f) = play {
            self.picker = None;
            self.load(f);
        }
        if close {
            self.picker = None;
        }
    }
}

fn main() -> eframe::Result<()> {
    let mut args = std::env::args().skip(1);
    let first = args.next();

    if matches!(first.as_deref(), Some("-h") | Some("--help")) {
        println!("vidkit-player [FILE | --clipboard]\n");
        println!("  FILE         a local .mp4 or .webm; omit for the built-in test pattern");
        println!("  --clipboard  play what is copied: a file (as a path or file:// URI,");
        println!("               e.g. copied in a file manager) or a video link");
        println!("\nGestures: drag to scrub (rate rises with height), left half");
        println!("vertical for volume, right half for speed, right edge to rotate,");
        println!("pinch to zoom, tap to play/pause.");
        return Ok(());
    }

    if first.as_deref() == Some("--gpu-selftest") {
        let opts = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([200.0, 140.0]),
            renderer: eframe::Renderer::Glow,
            ..Default::default()
        };
        return eframe::run_native("vidkit gpu selftest", opts, Box::new(|cc| Ok(Box::new(gpu_selftest::SelfTest::new(cc)))));
    }

    // `--clipboard`: the palette's "Play from clipboard", at launch. On a phone
    // that is one tap on a launcher shortcut -- copy a video in the file
    // manager or a link in the browser, tap "Play copied video" -- instead of
    // open the app, open the palette, pick the command.
    let from_clipboard = first.as_deref() == Some("--clipboard");
    let file = if from_clipboard { None } else { first.map(PathBuf::from) };

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // An app id, or Wayland shows the binary name and no icon.
            .with_app_id("vidkit-player")
            .with_inner_size([960.0, 640.0]),
        ..Default::default()
    };
    eframe::run_native(
        "vidkit-player",
        opts,
        Box::new(move |cc| {
            let mut p = Player::new(cc, file);
            if from_clipboard {
                p.play_clipboard();
            }
            Ok(Box::new(p))
        }),
    )
}

impl Player {
    /// Apply a palette choice.
    ///
    /// Actions with no implementation yet are recorded in `gpu_error`'s banner
    /// slot rather than silently doing nothing -- a palette entry that appears
    /// to work and does not is worse than one that says it is unfinished.
    fn run_action(&mut self, action: vidkit_egui::Action) {
        use vidkit_egui::Action as A;
        match action {
            A::TogglePlay => self.transport.playing = !self.transport.playing,
            A::SeekBy(d) => {
                let now = self.transport.position.as_secs_f64();
                let to = (now + d as f64).max(0.0);
                self.transport.position = Duration::from_secs_f64(to);
            }
            A::SeekTo(t) => self.transport.position = t,
            A::SetSpeed(v) => self.transport.speed = v,
            A::SetVolume(v) => self.transport.volume = v,
            A::Rotate => self.settings.rotation = (self.settings.rotation + 1).rem_euclid(4),
            A::CycleFit => {
                self.settings.fill =
                    vidkit_egui::FillMode::from_fit(vidkit_egui::next_fit(self.view.fit))
            }
            A::ResetView => {
                self.transport.zoom = 1.0;
                self.settings.rotation = 0;
            }
            A::OpenSettings => self.settings_open = true,
            A::Quit => self.quit = true,
            other => {
                self.gpu_error = Some(format!("{other:?} is not wired up yet"));
            }
        }
    }
}

impl Player {
    /// Play whatever was last pasted in.
    ///
    /// A direct media link is opened straight over HTTP with no extractor: a
    /// spawned Python interpreter and several seconds to learn what the URL
    /// already said is a poor trade, and it means a LAN file works on a machine
    /// with no yt-dlp. A page URL needs the extractor, and that path needs TLS
    /// this build does not have, so it says so rather than failing obscurely.
    fn play_clipboard(&mut self) {
        // The SYSTEM clipboard, then the last in-app paste as a fallback.
        //
        // Reading only egui's `Event::Paste` meant the app never saw the
        // clipboard at all: copying a URL in a browser and clicking "play from
        // clipboard" delivered no event, so the command reported "nothing
        // pasted yet" on a clipboard that plainly had a URL in it. That was a
        // documented limitation and it made the feature useless as anyone
        // would actually use it.
        let Some(text) = io::read_clipboard().or_else(|| self.last_paste.clone()) else {
            self.gpu_error = Some(
                "could not read the clipboard — install wl-paste, xclip or xsel, \
                 or paste into the window with Ctrl+V"
                    .into(),
            );
            return;
        };
        let what = vidkit_ytdlp::classify(&text);
        let url = match &what {
            vidkit_ytdlp::Paste::Video { url, .. } => url.clone(),
            vidkit_ytdlp::Paste::LocalFile(p) => {
                // Was open_local(p) + apply_opened() inline, a second copy
                // of load()'s own sequence -- and the copy that never got
                // the real-decode fix load() needed. Calling load()
                // directly means there is exactly one place that knows
                // how to actually open a local file, not two that can
                // silently drift.
                self.load(p.clone());
                return;
            }
            other => {
                self.gpu_error = Some(format!("{} — not playable yet", other.label()));
                return;
            }
        };

        self.open_url(url);
    }

    /// Open a URL: a direct media link over HTTP, a page through yt-dlp --
    /// from the /tmp copy when one is kept.
    fn open_url(&mut self, url: String) {
        // A page goes through the extractor; a direct link does not, because
        // spawning a Python interpreter to learn what the URL already said is
        // seconds of latency for nothing.
        if !io::is_direct_media(&url) {
            // A page: let yt-dlp fetch and MUX it, then play the file.
            //
            // YouTube serves video and audio as separate streams. Joining them
            // over our own range fetcher would mean a DASH muxer in this repo;
            // yt-dlp already does it well. The cost is stated rather than
            // hidden: this waits for the whole download, so the first frame
            // comes after it rather than during.
            let cap = self.settings.quality.max_long_edge();
            let root = media_cache::root();

            // A copy kept from before: no network, no extractor, no wait.
            if let Some(m) = media_cache::lookup(&root, &url, cap) {
                self.play_kept(media_cache::hold(m), &url, true);
                return;
            }

            // FETCH WHILE PLAYING first. Resolution costs one yt-dlp run; the
            // media itself is then pulled by ffmpeg as it decodes, so the
            // first frame arrives in milliseconds instead of after the whole
            // download. Falls back to download-then-play, because a site whose
            // streams ffmpeg cannot read directly still works that way.
            let stream_err = match self.stream_page(&url, cap) {
                Ok(()) => {
                    self.keep_in_background(url.clone(), cap);
                    return;
                }
                Err(e) => e,
            };

            match media_cache::get_or_fetch(&root, &url, cap, |dir| {
                io::fetch_into(&mut io::ProcessRunner::default(), &url, cap, dir)
            }) {
                Ok((held, hit)) => self.play_kept(held, &url, hit),
                // BOTH reasons, not just the last one. The download error
                // overwrote the streaming error, so a failure that started in
                // resolution was reported as a download problem and sent the
                // reader to the wrong half of the system.
                Err(e) => {
                    self.gpu_error =
                        Some(format!("streaming: {stream_err} / download: {e}"))
                }
            }
            return;
        }
        match io::open_direct_url(&url) {
            Ok(o) => self.accept_opened(o),
            Err(e) => self.gpu_error = Some(e),
        }
    }

    /// Adopt a freshly opened URL.
    fn accept_opened(&mut self, o: io::OpenedUrl) {
        self.transport.duration = o.duration;
        self.transport.status = "NV12 · CPU convert".into();
        self.opened = Some(Ok(Opened {
            label: o.label,
            duration: o.duration,
        }));
        self.gpu_error = None;
    }

    /// Push a freshly opened file into the transport.
    fn apply_opened(&mut self) {
        if let Some(Ok(o)) = &self.opened {
            self.transport.duration = o.duration;
        }
    }
}

impl Player {
    /// Resolve a page and start decoding straight from the media URLs.
    fn stream_page(&mut self, url: &str, cap: Option<u32>) -> Result<(), String> {
        use vidkit_ytdlp::{ResolveOptions, Resolver};

        let caps = io::assumed_caps();
        let mut r = Resolver::new(io::ProcessRunner::default(), url, ResolveOptions::default());
        let resolved = r
            .resolve(&caps)
            .map_err(|e| format!("{url}: {e:?}"))?
            .clone();

        let input = decode::StreamingInput {
            video_url: resolved.video_url.clone(),
            audio_url: resolved.audio_url.clone(),
            // Verbatim: the signed URL is bound to them and a different
            // User-Agent gets a 403 that looks like an expired link.
            headers: resolved.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        };

        let engine = playback::Engine::start(stream_opener(input, cap), sink_maker(), Duration::ZERO, playback::refresh_hz())?;
        // The length comes from the same yt-dlp JSON (Resolved::duration).
        self.transport.status = "NV12 · CPU convert · streaming".into();
        self.opened = Some(Ok(Opened {
            label: format!(
                "{} · {}x{} · streaming while it downloads",
                resolved.video_format_id, resolved.width, resolved.height
            ),
            duration: resolved.duration,
        }));
        self.engine = Some(engine);
        // Transcribe from the audio URL (the video URL when muxed). No cache:
        // the signed URL expires and is no key for anything.
        let audio_url = resolved.audio_url.clone().unwrap_or_else(|| resolved.video_url.clone());
        let headers: Vec<(String, String)> = resolved.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let audio = transcribe::AudioIn { input: audio_url, headers: headers.clone() };
        // The page's own captions when it has them: YouTube's cost nothing,
        // whisper costs minutes of CPU per hour of lecture on a phone.
        let factory = resolved.captions.clone().map(|c| transcribe::caption_factory(c, headers));
        self.transport.duration = resolved.duration;
        self.start_transcription(url.to_string(), resolved.title.clone(), None, audio, resolved.duration, factory);
        self.path = None;
        // The previous download, if any, is no longer needed.
        self.downloaded = None;
        self.transport.playing = true;
        self.gpu_error = None;
        Ok(())
    }
}

impl Player {
    /// Move the playhead AND the decoder.
    ///
    /// Setting the transport position alone moved the scrub bar and left the
    /// picture where it was — the gesture looked like it worked and did
    /// nothing, which is worse than a control that is visibly disabled.
    fn seek_to(&mut self, to: Duration) {
        self.transport.position = to;
        if let Some(engine) = self.engine.as_mut() {
            if let Err(e) = engine.seek(to) {
                // A failed seek leaves the decoder where it was rather than
                // killing playback: the film carries on from the old position,
                // which is a better outcome than a black window.
                self.gpu_error = Some(format!("seek failed: {e}"));
            }
        }
    }
}

/// Probe once per source, not once per open. Each seek reopens the decoder,
/// and re-probing spawned ffprobe and re-parsed the container every time --
/// over HTTP, a round trip per scrub.
fn probed(
    cache: &std::sync::OnceLock<decode::Probe>,
    probe: impl FnOnce() -> Result<decode::Probe, String>,
) -> Result<decode::Probe, String> {
    if let Some(p) = cache.get() {
        return Ok(p.clone());
    }
    let p = probe()?;
    let _ = cache.set(p.clone());
    Ok(p)
}

/// The engine's opener for a local file: video capped at 1920 on the long
/// edge, hardware decode where the machine has it, audio at 48 kHz stereo.
fn file_opener(p: PathBuf) -> playback::Opener {
    let cache = std::sync::OnceLock::new();
    Box::new(move |from, speed| {
        let pr = probed(&cache, || decode::probe(&p))?;
        let v = decode::FfmpegFrames::open_probed(
            &decode::VideoInput::file(&p),
            &pr,
            Some(decode::decode_cap()),
            from,
            decode::accel_for(&pr.codec),
        )?;
        let a = decode::AudioStream::open(&p.to_string_lossy(), 48_000, 2, from, speed).ok();
        Ok((v, a))
    })
}

/// The engine's opener for a resolved stream: video and audio from the signed
/// URLs, each with the headers, each fetched by its own process.
fn stream_opener(input: decode::StreamingInput, _format_cap: Option<u32>) -> playback::Opener {
    let cache = std::sync::OnceLock::new();
    Box::new(move |from, speed| {
        let pr = probed(&cache, || decode::probe_input(&input.video_url, &input.headers))?;
        let v = decode::FfmpegFrames::open_probed(
            &decode::VideoInput::url(&input.video_url, &input.headers),
            &pr,
            // The format was already chosen at the preset's height; this is
            // the decode size, which follows the display like a file's.
            Some(decode::decode_cap()),
            from,
            decode::accel_for(&pr.codec),
        )?;
        let a = decode::AudioStream::open_streaming(&input, 48_000, 2, from, speed).ok();
        Ok((v, a))
    })
}

fn sink_maker() -> playback::SinkMaker {
    Box::new(|| decode::open_sink(48_000, 2))
}

fn start_file(p: PathBuf) -> Result<playback::Engine, String> {
    playback::Engine::start(file_opener(p), sink_maker(), Duration::ZERO, playback::refresh_hz())
}

/// `1:30–2:00`, for status lines.
pub(crate) fn fmt_span(a: Duration, b: Duration) -> String {
    format!("{}–{}", vidkit_egui::format_time(a), vidkit_egui::format_time(b))
}

impl Player {
    /// (Re)start the transcription loop for what is now playing. The old
    /// worker, if any, is dropped and stops after its current chunk.
    /// (Re)start the transcription loop for what is now playing, recording
    /// it in the seen store. `url` identifies it (a page URL, or `file://`);
    /// its transcript lives in the seen store, keyed by URL, so reopening --
    /// streamed, re-downloaded or from the /tmp copy -- resumes rather than
    /// transcribing again.
    fn start_transcription(
        &mut self,
        url: String,
        title: Option<String>,
        file: Option<PathBuf>,
        audio: transcribe::AudioIn,
        total: Option<Duration>,
        factory: Option<transcribe::Factory>,
    ) {
        // A local file's transcript is keyed by its size and mtime too; a
        // URL's by the URL alone.
        let key_file = if url.starts_with("file://") { file.as_deref() } else { None };
        let cache = seen::dir()
            .and_then(|d| seen::record(&d, &url, title.as_deref(), key_file))
            .or_else(|| file.as_deref().and_then(transcribe::cache_path));
        let factory = factory.unwrap_or_else(|| transcribe::source_factory(file));
        self.worker = Some(transcribe::Worker::start(factory, audio, total, cache));
        self.reset_sections();
        // Enhancement only for titles analysis has looked at.
        self.enh_facts = seen::dir().and_then(|d| enhance_plan::load_facts(&d, &url));
        self.enh_plan = None;
        self.enh_px = (0, 0);
    }

    fn reset_sections(&mut self) {
        self.seen_version = 0;
        self.windows.clear();
        self.chapters.clear();
        self.hits.clear();
        self.plan.clear();
        self.sections.selected.clear();
    }

    /// Per frame: feed the loop the playhead, pick up new transcript, and
    /// enforce the watch plan.
    fn sections_tick(&mut self, ctx: &egui::Context) {
        let pos = self.transport.position;
        if let Some(w) = &self.worker {
            w.set_playhead(pos);
            let (v, done) = w.with(|s| (s.version, s.coverage.complete()));
            if v != self.seen_version {
                self.seen_version = v;
                // Minute windows: the unit vidkit-relevance scores and cuts.
                self.windows = w.with(|s| s.transcript.windows(Duration::from_secs(60)));
                self.chapters = vidkit_relevance::sections::chapters(
                    &self.windows,
                    &vidkit_relevance::sections::ChapterPolicy::default(),
                );
                let q = self.sections.query.trim();
                if !q.is_empty() {
                    self.hits = vidkit_relevance::sections::search(&self.windows, q, 8);
                }
            }
            // Keep a visible panel current while paused, once a second --
            // not more: nothing in it changes faster than a chunk.
            if self.sections_open && !done {
                ctx.request_repaint_after(Duration::from_secs(1));
            }
        }
        if !self.plan.is_empty() && self.engine.is_some() && self.transport.playing {
            if let Some(to) = vidkit_relevance::next_position(&self.plan, pos) {
                let end = self.plan.last().map(|s| s.end).unwrap_or_default();
                if to >= end {
                    self.transport.playing = false;
                    self.gpu_error = Some("end of the chosen sections".into());
                } else {
                    self.seek_to(to);
                }
            }
        }
        if let Some(msg) = self.export_status.lock().ok().and_then(|mut m| m.take()) {
            self.gpu_error = Some(msg);
        }
    }

    /// The sections panel's contents into `ui`: beside the video normally,
    /// the whole page in picture-in-picture.
    fn sections_in(&mut self, ui: &mut egui::Ui) -> Vec<vidkit_egui::SectionAction> {
        let (status, progress, transcribed, total, mode) = match &self.worker {
            Some(w) => w.with(|s| {
                let rtf = s.realtime_factor.map(|r| format!(" · {r:.2}× realtime")).unwrap_or_default();
                (format!("{}{rtf}", s.status), s.coverage.progress(), s.coverage.spans(), s.coverage.total(), s.coverage.mode)
            }),
            None => ("nothing open".into(), None, Vec::new(), None, vidkit_transcribe::coverage::Mode::Paused),
        };
        let scope = match mode {
            vidkit_transcribe::coverage::Mode::Paused => vidkit_egui::Scope::Paused,
            vidkit_transcribe::coverage::Mode::Ahead(_) => vidkit_egui::Scope::Ahead,
            vidkit_transcribe::coverage::Mode::All => vidkit_egui::Scope::All,
        };
        let model = vidkit_egui::SectionsModel {
            chapters: &self.chapters,
            hits: &self.hits,
            total: total.or(self.transport.duration),
            position: self.transport.position,
            transcribed: &transcribed,
            progress,
            status: &status,
            plan: &self.plan,
            scope,
            can_export: self.path.is_some(),
        };
        self.sections.ui(ui, &model)
    }

    fn sections_ui(&mut self, ctx: &egui::Context) {
        if !self.sections_open || self.pip_on {
            return;
        }
        // A phone in portrait has no room beside the picture: underneath.
        let narrow = ctx.content_rect().width() < 700.0;
        let actions = if narrow {
            // Capped at half the screen: sized to its content, a long chapter
            // list took most of a phone's height and left the picture a strip.
            let cap = ctx.content_rect().height() * 0.5;
            egui::TopBottomPanel::bottom("sections").resizable(true).default_height(280.0_f32.min(cap)).max_height(cap)
                .show(ctx, |ui| self.sections_in(ui)).inner
        } else {
            egui::SidePanel::right("sections").resizable(true).default_width(340.0)
                .show(ctx, |ui| self.sections_in(ui)).inner
        };
        for a in actions {
            self.section_action(a);
        }
    }

    fn section_action(&mut self, a: vidkit_egui::SectionAction) {
        use vidkit_egui::SectionAction as A;
        let total = self
            .worker
            .as_ref()
            .and_then(|w| w.with(|s| s.coverage.total()))
            .or(self.transport.duration)
            .unwrap_or_else(|| self.windows.last().map(|w| w.end).unwrap_or_default());
        match a {
            A::Seek(t) => self.seek_to(t),
            A::Search(q) => self.hits = vidkit_relevance::sections::search(&self.windows, &q, 8),
            A::WatchSpans(spans) => {
                self.plan = vidkit_relevance::plan_keeping(&spans, total);
                self.enter_plan();
            }
            A::WatchMatching(q) => {
                let scores: Vec<vidkit_relevance::Score> = self
                    .windows
                    .iter()
                    .map(|w| vidkit_relevance::Score::Lexical(vidkit_relevance::lexical_score(&q, &w.text)))
                    .collect();
                let plan = vidkit_relevance::plan(&self.windows, &scores, &vidkit_relevance::SkipPolicy::default());
                let skipped = vidkit_relevance::skipped_fraction(&plan);
                if skipped <= 0.0 {
                    // plan() keeps everything both when everything matches and
                    // when nothing does (its safety rule). Either way there is
                    // no plan to follow, and the viewer should hear why.
                    self.plan.clear();
                    self.gpu_error = Some(format!("\"{q}\": no sections to skip — it matched everywhere or nowhere"));
                } else {
                    self.gpu_error = Some(format!("\"{q}\": skipping {:.0}% of the transcribed video", skipped * 100.0));
                    self.plan = plan;
                    self.enter_plan();
                }
            }
            A::ClearPlan => self.plan.clear(),
            A::Export(spans) => {
                let Some(input) = self.path.clone() else { return };
                let out = export::default_output(&input);
                let status = self.export_status.clone();
                self.gpu_error = Some(format!("exporting {} section(s) to {}…", spans.len(), out.display()));
                let _ = std::thread::Builder::new().name("vidkit-export".into()).spawn(move || {
                    let msg = match export::export(&input, &spans, &out) {
                        Ok(()) => format!("exported to {}", out.display()),
                        Err(e) => format!("export failed: {e}"),
                    };
                    if let Ok(mut s) = status.lock() {
                        *s = Some(msg);
                    }
                });
            }
            A::SetScope(scope) => {
                if let Some(w) = &self.worker {
                    w.set_mode(match scope {
                        vidkit_egui::Scope::Paused => vidkit_transcribe::coverage::Mode::Paused,
                        // Five minutes: about ten chunks, a few minutes of
                        // whisper on a phone -- enough to stay ahead of the
                        // viewer without transcribing a lecture they may quit.
                        vidkit_egui::Scope::Ahead => vidkit_transcribe::coverage::Mode::Ahead(Duration::from_secs(300)),
                        vidkit_egui::Scope::All => vidkit_transcribe::coverage::Mode::All,
                    });
                }
            }
        }
    }

    /// Starting a plan from outside a kept section jumps to the first one.
    fn enter_plan(&mut self) {
        let pos = self.transport.position;
        let inside = self.plan.iter().any(|s| s.keep && pos >= s.start && pos < s.end);
        if !inside {
            if let Some(first) = self.plan.iter().find(|s| s.keep && s.end > pos).or_else(|| self.plan.iter().find(|s| s.keep)) {
                let to = first.start;
                self.seek_to(to);
            }
        }
        self.transport.playing = true;
    }

    /// The spoken line, over the bottom of the picture.
    fn subtitle_overlay(&self, ctx: &egui::Context, area: egui::Rect) {
        if !self.subtitles {
            return;
        }
        let Some(text) = self
            .worker
            .as_ref()
            .and_then(|w| w.with(|s| s.transcript.at(self.transport.position).map(|c| c.text.clone())))
        else {
            return;
        };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("vidkit-subtitles")));
        let font = egui::FontId::proportional((area.height() * 0.045).clamp(14.0, 30.0));
        let galley = painter.layout(text, font, egui::Color32::WHITE, area.width() * 0.85);
        let pos = egui::pos2(area.center().x - galley.size().x / 2.0, area.bottom() - galley.size().y - area.height() * 0.08);
        // A backing box, not an outline: legible over white snow and black
        // night alike, which a coloured outline is not.
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand(6.0);
        painter.rect_filled(bg, 4.0, egui::Color32::from_black_alpha(170));
        painter.galley(pos, galley, egui::Color32::WHITE);
    }
}

impl Player {
    /// tideshell's living palette over our own, when it publishes one; our
    /// own back when it stops.
    fn follow_shell_colours(&mut self, ctx: &egui::Context) {
        let Some(w) = self.ambient.as_mut() else { return };
        let now = (ctx.input(|i| i.time) * 1000.0) as u64;
        let was = w.present;
        if let Some(p) = w.poll(now) {
            self.colors = uikit_egui::theme_bridge::ambient::merge(&self.base_colors, &p);
            uikit_egui::theme_bridge::apply_colors(ctx, &self.colors);
        } else if was && !w.present {
            self.colors = self.base_colors;
            uikit_egui::theme_bridge::apply_colors(ctx, &self.colors);
        }
        // An idle window repaints only on input, so a shell change would wait
        // for a touch. Every 2 s while the shell is publishing -- a static
        // frame, and only on a device running tideshell.
        if w.present {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
    }
}

impl Player {
    /// Play a download from the /tmp cache, identified by its page URL.
    fn play_kept(&mut self, held: media_cache::Held, url: &str, hit: bool) {
        let path = held.path().to_path_buf();
        match start_file(path.clone()) {
            Ok(e) => {
                let opened = open_local(&path);
                self.transport.duration = opened.as_ref().ok().and_then(|o| o.duration);
                self.opened = Some(opened);
                self.engine = Some(e);
                // Held (a lock eviction respects) so the file outlives the
                // decoder reading it.
                self.downloaded = Some(held);
                self.transport.playing = true;
                self.gpu_error = hit.then(|| "playing the copy kept in /tmp — no download".to_string());
                self.path = Some(path.clone());
                let total = self.transport.duration;
                self.start_transcription(
                    url.to_string(),
                    None,
                    Some(path.clone()),
                    transcribe::AudioIn { input: path.to_string_lossy().into(), headers: Vec::new() },
                    total,
                    None,
                );
            }
            Err(e) => self.gpu_error = Some(format!("downloaded, but {e}")),
        }
    }

    /// While a page streams, save it to /tmp for next time.
    ///
    /// The cost, stated: the first watch fetches the video twice (ffmpeg's
    /// ranges for playing, yt-dlp's download for keeping), and on a slow
    /// link the two compete. After that it plays from /tmp with no network
    /// at all. VIDKIT_KEEP_STREAMS=0 streams only, as before.
    fn keep_in_background(&mut self, url: String, cap: Option<u32>) {
        if std::env::var("VIDKIT_KEEP_STREAMS").as_deref() == Ok("0") {
            return;
        }
        let status = self.export_status.clone();
        let _ = std::thread::Builder::new().name("vidkit-keep".into()).spawn(move || {
            // Lower priority than playback, like transcription; inherited by
            // yt-dlp and the ffmpeg it runs to mux.
            unsafe {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
            }
            let root = media_cache::root();
            let r = media_cache::get_or_fetch(&root, &url, cap, |dir| io::fetch_into(&mut io::ProcessRunner::default(), &url, cap, dir));
            let msg = match r {
                Ok((held, hit)) => {
                    drop(held);
                    (!hit).then(|| "kept a copy in /tmp — next time plays without the network".to_string())
                }
                Err(e) => Some(format!("could not keep a copy: {e}")),
            };
            if let (Some(m), Ok(mut s)) = (msg, status.lock()) {
                *s = Some(m);
            }
        });
    }

    /// Open a remembered video at a moment.
    fn open_seen(&mut self, url: &str, at: Duration) {
        match url.strip_prefix("file://") {
            Some(p) => self.load(PathBuf::from(p)),
            None => self.open_url(url.to_string()),
        }
        if self.engine.is_some() {
            self.seek_to(at);
        }
    }
}

impl Player {
    /// The floating video. Tapping it expands back to full screen; flinging
    /// it off a side hides it behind a tab while the sound carries on.
    fn pip_ui(&mut self, ctx: &egui::Context, screen: egui::Rect) {
        let size = match &self.gpu {
            Some(g) => g.lock().map(|g| g.size()).map(|(w, h)| egui::Vec2::new(w as f32, h as f32)).unwrap_or_default(),
            None => self.presenter.frame_size(),
        };
        let aspect = if size.y > 0.0 { size.x / size.y } else { 16.0 / 9.0 };
        let (gpu, fit) = (self.gpu.clone(), self.view.fit);
        let tex = self.presenter.texture().map(|t| t.id());
        if std::env::var_os("VIDKIT_PIP_DEBUG").is_some() {
            // Read the pointer, THEN ask the context: calling ctx inside
            // ctx.input deadlocks egui's lock.
            let pressed = ctx.input(|i| i.pointer.interact_pos().filter(|_| i.pointer.any_pressed() || i.pointer.any_released()));
            if let Some(p) = pressed {
                eprintln!("PIPDBG pos={p:?} layer={:?} pip={:?} screen={screen:?}", ctx.layer_id_at(p), self.pip.rect(screen, aspect));
            }
        }
        let (_, action) = self.pip.show(ctx, screen, aspect, |ui, r| {
            let v = vidkit_egui::fit_rect(r, size, vidkit_egui::PixelAspect::SQUARE, fit);
            match (&gpu, tex) {
                (Some(g), _) if v.width() > 0.0 => {
                    ui.painter().add(vidkit_egui::glow_present::callback(v, g.clone()));
                }
                (None, Some(id)) => {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    ui.painter().image(id, v, uv, egui::Color32::WHITE);
                }
                _ => {}
            }
        });
        match action {
            Some(vidkit_egui::PipAction::Expand) => self.pip_on = false,
            Some(vidkit_egui::PipAction::Hid(_)) => {
                self.gpu_error = Some("video hidden — the sound carries on; tap the tab to bring it back".into());
            }
            Some(vidkit_egui::PipAction::Unhide) | None => {}
        }
    }
}

impl Player {
    /// Plan for this title at this output size, and hand it to the GPU.
    /// Recomputed only when the size changes: the plan is pure and cheap,
    /// but not free every frame.
    fn apply_enhance(&mut self, px: (u32, u32)) {
        if px == self.enh_px && (self.enh_plan.is_some() || self.enh_facts.is_none()) {
            return;
        }
        self.enh_px = px;
        let enhance = match &self.enh_facts {
            Some(f) => {
                let class = *GPU_CLASS.get_or_init(|| {
                    enhance_plan::classify_gpu(&enhance_plan::drm_cards(), decode::machine().kind == decode::MachineKind::QualcommVenus)
                });
                let plan = enhance_plan::plan_title(f, &enhance_plan::Device::default_for(class, px.0, px.1));
                let e = plan.enhance;
                self.enh_plan = Some(plan);
                e
            }
            None => {
                self.enh_plan = None;
                vidkit_egui::glow_present::Enhance::default()
            }
        };
        if let Some(g) = &self.gpu {
            if let Ok(mut g) = g.lock() {
                g.enhance = enhance;
            }
        }
    }

    /// Analysis mode: rank watched titles that can be read without the
    /// network, analyse the best ones within the time budget, store facts.
    fn start_analysis(&mut self) {
        #[cfg(not(feature = "bscale"))]
        {
            self.gpu_error = Some("this build has no bscale analysis (feature `bscale`)".into());
        }
        #[cfg(feature = "bscale")]
        {
            use std::sync::atomic::Ordering;
            let Some(dir) = seen::dir() else { return };
            let (status, running, done_msg) = (self.analysis_status.clone(), self.analysis_running.clone(), self.export_status.clone());
            running.store(true, Ordering::Relaxed);
            let cap = self.settings.quality.max_long_edge();
            let screen = (1920u32, 1080u32);
            let budget = Duration::from_secs(std::env::var("VIDKIT_ANALYSIS_S").ok().and_then(|v| v.parse().ok()).unwrap_or(120));
            let _ = std::thread::Builder::new().name("vidkit-analysis".into()).spawn(move || {
                unsafe {
                    libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                }
                let set = |m: String| {
                    if let Ok(mut s) = status.lock() {
                        *s = m;
                    }
                };
                // Candidates: watched, playable offline, not yet analysed.
                let root = media_cache::root();
                let mut cands = Vec::new();
                let mut inputs = std::collections::HashMap::new();
                for m in seen::Index::metas(&dir) {
                    if enhance_plan::load_facts(&dir, &m.url).is_some() {
                        continue;
                    }
                    let input = match m.url.strip_prefix("file://") {
                        Some(p) if std::path::Path::new(p).is_file() => p.to_string(),
                        Some(_) => continue,
                        None => match media_cache::lookup(&root, &m.url, cap) {
                            Some(p) => p.to_string_lossy().into_owned(),
                            None => continue,
                        },
                    };
                    let Ok(pr) = decode::probe(std::path::Path::new(&input)) else { continue };
                    let dur = decode::probe_duration(&input);
                    inputs.insert(m.url.clone(), (input, pr.fps, dur));
                    cands.push(enhance_plan::Candidate { key: m.url.clone(), src_w: pr.width, src_h: pr.height, watched: m.watched, duration: dur });
                }
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                enhance_plan::rank(&mut cands, screen.0, screen.1, now);
                let total = cands.len();
                let (mut spent, mut done) = (Duration::ZERO, 0usize);
                for c in &cands {
                    if !enhance_plan::within_budget(spent, done, budget) {
                        break;
                    }
                    let (input, fps, dur) = &inputs[&c.key];
                    set(format!("analysing {}/{}: {}", done + 1, total, seen::default_title(&c.key)));
                    match analysis::analyse(input, c.src_w, c.src_h, *fps, *dur) {
                        Ok((facts, took)) => {
                            spent += took;
                            let _ = std::fs::write(enhance_plan::facts_path(&dir, &c.key), enhance_plan::facts_to_line(&facts));
                        }
                        Err(e) => set(format!("{}: {e}", seen::default_title(&c.key))),
                    }
                    done += 1;
                }
                let msg = format!("analysis: {done} of {total} candidate titles within {}s; only those are enhanced", budget.as_secs());
                set(msg.clone());
                if let Ok(mut s) = done_msg.lock() {
                    *s = Some(msg);
                }
                running.store(false, Ordering::Relaxed);
            });
        }
    }
}

impl Player {
    /// Decode at the display's size. The monitor is known only once frames
    /// flow, so the first open uses the old 1920; when the real cap differs
    /// the decoder is reopened once, at the current position.
    fn follow_display(&mut self, ctx: &egui::Context) {
        let ppp = ctx.pixels_per_point();
        let display = ctx.input(|i| i.viewport().monitor_size).map(|s| (s.x.max(s.y) * ppp).round() as u32);
        if display.is_none() {
            return;
        }
        let cap = decode::cap_for(display, self.max_texture, self.settings.quality.max_long_edge());
        if cap == self.cap_applied {
            return;
        }
        decode::set_decode_cap(cap);
        self.cap_applied = cap;
        if self.engine.is_some() {
            let at = self.transport.position;
            self.seek_to(at);
        }
    }
}

impl Player {
    /// While playing, egui repaints at the display's rate: the median
    /// interval IS the refresh. Re-estimated every 120 frames; the engine's
    /// sync slack follows it (it was fixed at 60 Hz).
    fn measure_refresh(&mut self) {
        let now = Instant::now();
        let prev = self.last_frame_at.replace(now);
        if !self.transport.playing {
            self.frame_dts.clear();
            return;
        }
        if let Some(p) = prev {
            self.frame_dts.push_back(now.duration_since(p).as_secs_f64());
        }
        if self.frame_dts.len() >= 120 {
            let v: Vec<f64> = self.frame_dts.drain(..).collect();
            if let Some(hz) = playback::estimate_refresh(&v) {
                if hz != playback::refresh_hz() {
                    playback::set_refresh_hz(hz);
                    if let Some(e) = self.engine.as_mut() {
                        e.set_refresh_hz(hz);
                    }
                }
            }
        }
    }
}
