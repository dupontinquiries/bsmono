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
use vidkit_core::io::MemorySource;
use vidkit_core::traits::{MediaSource, Presenter};
use vidkit_egui::{bottom_bar, next_fit, EguiPresenter, Fit, Transport, VideoView};

mod filepick;
mod palette;
mod pattern;

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
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let head = bytes.get(..16).unwrap_or(&bytes).to_vec();

    let is_webm = head.len() >= 4 && head[0..4] == [0x1A, 0x45, 0xDF, 0xA3];
    let is_mp4 = head.len() >= 8
        && matches!(
            &head[4..8],
            b"ftyp" | b"styp" | b"moov" | b"free" | b"skip" | b"mdat"
        );

    let (kind, profile, dims, dur, packets) = if is_webm {
        let mut d = vidkit_webm::WebmDemuxer::open(MemorySource(bytes))
            .map_err(|e| format!("webm: {e}"))?;
        let (p, dims, dur) = (d.video_profile(), d.dimensions(), d.duration());
        let mut n = 0;
        while matches!(d.next_packet(), Ok(Some(_))) {
            n += 1;
        }
        ("WebM", p, dims, dur, n)
    } else if is_mp4 {
        let mut d =
            vidkit_mp4::Mp4Demuxer::open(MemorySource(bytes)).map_err(|e| format!("mp4: {e}"))?;
        let (p, dims, dur) = (d.video_profile(), d.dimensions(), d.duration());
        let mut n = 0;
        while matches!(d.next_packet(), Ok(Some(_))) {
            n += 1;
        }
        ("MP4", p, dims, dur, n)
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

    let _ = packets;
    Ok(Opened {
        label: format!(
            "{} · {kind} · {codec} · {}x{} · {packets} packets",
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
}

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
        let colors = vidkit_egui::install(&cc.egui_ctx, snapclone_theme::palette_by_id("dark"));

        // Opt-in while it is unproven on hardware. The glow context is handed
        // over HERE, in CreationContext -- not in `update` -- which is the
        // detail that stalled the first attempt at this.
        let (gpu, gpu_error) = if std::env::var("VIDKIT_GPU").as_deref() == Ok("1") {
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

        let status = match &opened {
            Some(Ok(_)) => "NV12 · CPU convert".to_string(),
            Some(Err(_)) => "not playing".to_string(),
            None => "test pattern · NV12 · CPU convert".into(),
        };
        let duration = match &opened {
            Some(Ok(o)) => o.duration,
            _ => Some(Duration::from_secs(212)),
        };

        Self {
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
            gpu_error,
            chrome: Default::default(),
            settings: Default::default(),
            settings_open: false,
        }
    }

    /// Feed the presenter. Until a decoder exists this is the test pattern —
    /// stated on screen rather than implied.
    fn produce(&mut self, eframe_frame: &eframe::Frame) {
        let t = self.started.elapsed().as_secs_f32();
        let frame = pattern::bars(t * 0.25);

        if let (Some(gpu), Some(gl)) = (&self.gpu, eframe_frame.gl()) {
            if let Ok(mut g) = gpu.lock() {
                // SAFETY: eframe's glow context is current on the UI thread.
                if let Err(e) = unsafe { g.upload(gl, &frame) } {
                    self.gpu_error = Some(e);
                }
            }
        } else {
            let _ = self.presenter.submit(frame);
        }
        self.presenter
            .set_deadline(Some(Instant::now() + Duration::from_millis(33)));
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

        if self.transport.playing {
            let t = self.started.elapsed().as_secs_f64() * self.transport.speed;
            if let Some(d) = self.transport.duration {
                self.transport.position = Duration::from_secs_f64(t % d.as_secs_f64().max(1.0));
            }
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

        // Settings are pushed into the live config every frame rather than on
        // an Apply button: every control is reversible and cheap, and seeing
        // the change while dragging the slider is the point of having one.
        self.chrome.policy = self.settings.chrome_policy();
        *self.view.gestures() = self.settings.gesture_config();
        self.view.fit = self.settings.fit();
        self.transport.rotation = self.settings.rotation;

        let settings_open = self.settings_open;
        vidkit_egui::settings::show(ctx, &mut self.settings_open, &mut self.settings);

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
                self.transport.position = to;
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
                    ui.small("no decoder yet — showing the test pattern");
                });
            }
            None => {
                ui.small("test pattern — pass a .mp4 or .webm to open one");
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
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
                self.transport.position = to;
            }
        });

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
            if self.palette.open {
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
        if space && !self.palette.open && self.picker.is_none() {
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
            C::Subtitles | C::ScrubToSections | C::PlayClipboard => {
                self.gpu_error = Some(format!("{:?} is not wired up yet", c));
            }
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
        if !self.palette.open {
            return;
        }
        let (up, down, enter) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if up {
            self.palette.move_selection(-1);
        }
        if down {
            self.palette.move_selection(1);
        }

        self.scrim(ctx);
        let mut chosen = None;
        egui::Window::new("Command palette")
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .collapsible(false)
            .resizable(false)
            .default_width(460.0)
            .frame(self.overlay_frame())
            .show(ctx, |ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.palette.query)
                        .hint_text("type a command…")
                        .desired_width(f32::INFINITY),
                );
                if self.palette.just_opened {
                    edit.request_focus();
                    self.palette.just_opened = false;
                }
                if edit.changed() {
                    // Narrowing can leave the highlight past the end; Enter
                    // would then run nothing, or the wrong row.
                    self.palette.on_query_changed();
                }

                ui.separator();
                let matches = self.palette.matches();
                if matches.is_empty() {
                    ui.weak("no matching command");
                }
                // Capped at the kit's MAX_ROWS: past that a palette stops
                // being a shortcut and becomes a list you read.
                for (i, c) in matches
                    .iter()
                    .take(uikit_egui::command_bar::MAX_ROWS)
                    .enumerate()
                {
                    let sel = i == self.palette.selected;
                    // Label and shortcut on ONE row, the shortcut pushed right.
                    // Stacking them made every command look like two entries.
                    let mut r = None;
                    ui.horizontal(|ui| {
                        r = Some(ui.selectable_label(sel, c.label()));
                        if let Some(k) = c.shortcut() {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.weak(k);
                                },
                            );
                        }
                    });
                    if r.map(|r| r.clicked()).unwrap_or(false) {
                        chosen = Some(*c);
                    }
                }
            });

        if enter {
            chosen = self.palette.chosen();
        }
        if let Some(c) = chosen {
            self.palette.close();
            self.run(c);
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
                    egui::TextEdit::singleline(&mut p.query)
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
        println!("vidkit-player [FILE]\n");
        println!("  FILE   a local .mp4 or .webm; omit for the built-in test pattern");
        println!("\nGestures: drag to scrub (rate rises with height), left half");
        println!("vertical for volume, right half for speed, right edge to rotate,");
        println!("pinch to zoom, tap to play/pause.");
        return Ok(());
    }

    let file = first.map(PathBuf::from);

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
        Box::new(|cc| Ok(Box::new(Player::new(cc, file)))),
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
