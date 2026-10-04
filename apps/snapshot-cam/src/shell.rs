//! shell — the window's contents: viewfinder, shutter, roll, viewer.
//!
//! In the library rather than the binary so the layout can be run through
//! egui's headless context and asserted on (the layout tests at the bottom),
//! and so the binary is only the eframe host. The core (`lib.rs`) owns what
//! a photo IS and where it may go; `camera` owns the frames; `store` and
//! `handoff` own the disk. This moves what the widgets return into the roll
//! and nothing else.

use crate::camera::{Camera, Facing, Source};
use crate::contacts::Contact;
use crate::preview::{self, Preview};
use crate::{capture_filename, handoff, store, Disposition, Roll, ShareTarget, Shot};
use std::path::PathBuf;
use uikit_egui::touch::{GestureRecognizer, TouchSource};
use uikit_egui::zoom::Zoom;
use crate::{develop, hwctl, lens, pro::Pro};

/// The pro panel and the viewfinder aids: a child module so it can reach
/// the shell's private state without widening it.
mod pro_ui;
/// Triggers, primed shots, brackets, merges, cover calibration.
pub mod capture_flow;

/// How the window is laid out. Phones differ: some want the viewfinder to
/// be the whole screen (controls over the bottom, the roll a tap away),
/// others a smaller picture with the roll under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    /// Full on a phone-sized window, compact on a desktop.
    #[default]
    Auto,
    Full,
    Compact,
}

impl ViewMode {
    pub const ALL: [ViewMode; 3] = [ViewMode::Auto, ViewMode::Full, ViewMode::Compact];
    pub fn label(self) -> &'static str { match self { ViewMode::Auto => "auto", ViewMode::Full => "full screen", ViewMode::Compact => "compact" } }
    pub fn to_conf(self) -> &'static str { match self { ViewMode::Auto => "auto", ViewMode::Full => "full", ViewMode::Compact => "compact" } }
    pub fn from_conf(s: &str) -> ViewMode { match s.trim() { "full" => ViewMode::Full, "compact" => ViewMode::Compact, _ => ViewMode::Auto } }
    /// Is the viewfinder the whole window, for a window of this class?
    pub fn is_full(self, compact_class: bool) -> bool {
        match self { ViewMode::Full => true, ViewMode::Compact => false, ViewMode::Auto => compact_class }
    }
}

/// How often autocrop and auto-WB re-measure. A lens shadow does not move
/// at frame rate; measuring every frame would cost a render per frame for
/// nothing and make the crop twitch.
pub const MEASURE_EVERY: std::time::Duration = std::time::Duration::from_millis(400);
/// The developed preview is rendered no larger than this.
pub const PREVIEW_MAX: u32 = 640;

/// The camera's own controls, when it has any.
#[derive(Default)]
pub struct Hw {
    /// Which device the controls below were read from (`None`: not V4L2).
    pub checked: Option<Option<String>>,
    pub controls: Option<hwctl::Controls>,
    pub error: Option<String>,
}

/// What the viewfinder aids measured on the last developed frame.
pub struct Measured {
    pub analysis: develop::Analysis,
}

/// The roll list's thumbnail edge.
pub const THUMB: u32 = 64;
/// How often the handoff markers and the outbox are looked at.
pub const HANDOFF_POLL: std::time::Duration = std::time::Duration::from_millis(1500);
/// Thumbnails decoded from disk per frame, for a roll from before this
/// launch. One keeps a long roll's first frame under a frame's budget; the
/// rest fill in over the next few frames, which nobody notices.
pub const THUMB_DECODES_PER_FRAME: usize = 1;
/// The settings file: facing and fill, remembered.
pub const SETTINGS: &str = "settings.conf";

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The open photo.
pub struct Viewer {
    /// Index into the roll (newest first).
    pub index: usize,
    /// The file the texture is of, so a change of index reloads.
    loaded: Option<String>,
    texture: Option<egui::TextureHandle>,
    pub confirm_delete: bool,
    /// Horizontal drag accumulated this gesture.
    drag_x: f32,
}

impl Viewer {
    pub fn open(index: usize) -> Viewer {
        Viewer { index, loaded: None, texture: None, confirm_delete: false, drag_x: 0.0 }
    }
}

/// What the viewer asks the shell to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerAction { Close, Prev, Next, Delete(usize), Share(usize, ShareTarget) }

/// A drag shorter than this (in points) is a tap, not a page.
pub const SWIPE_MIN_PX: f32 = 48.0;

/// Does a horizontal drag page? +1 next (dragged left), −1 previous.
pub fn swipe_commits(drag_x: f32, width: f32) -> i32 {
    let threshold = SWIPE_MIN_PX.max(width * 0.15);
    if drag_x <= -threshold { 1 } else if drag_x >= threshold { -1 } else { 0 }
}

/// The self-timer choices, in seconds. Three for a selfie at arm's length,
/// ten for propping the phone up and walking into the frame.
pub const TIMERS: [u8; 3] = [0, 3, 10];

/// A running self-timer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Countdown {
    pub started: std::time::Instant,
    pub secs: u8,
}

impl Countdown {
    /// Seconds still to go, rounded up so the display goes 3, 2, 1 and never
    /// shows 0 before the shutter.
    pub fn remaining(&self, now: std::time::Instant) -> u8 {
        let gone = now.saturating_duration_since(self.started).as_secs_f32();
        (self.secs as f32 - gone).ceil().max(0.0) as u8
    }

    pub fn done(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.started).as_secs_f32() >= self.secs as f32
    }
}

/// The app.
pub struct Cam {
    pub roll: Roll,
    pub camera: Camera,
    pub preview: Preview,
    /// The live frame on the GPU, replaced when a newer frame arrives.
    frame: Option<egui::TextureHandle>,
    /// Thumbnails for the roll list, by file name.
    thumbs: std::collections::HashMap<String, egui::TextureHandle>,
    pub contacts: Vec<Contact>,
    /// The picked contact's sc-ID.
    pub contact: Option<String>,
    pub capture_dir: PathBuf,
    pub outbox: PathBuf,
    pub library_root: PathBuf,
    pub note: String,
    colors: uikit_egui::PaletteColors,
    /// The shutter flash: when it was pressed, for the white blink.
    flashed: Option<std::time::Instant>,
    last_poll: std::time::Instant,
    /// The roll changed since it was last written.
    dirty: bool,
    pub viewer: Option<Viewer>,
    /// Facing/fill/timer changed since the settings file was written.
    settings_dirty: bool,
    /// The self-timer setting, seconds; 0 is off.
    pub timer_secs: u8,
    /// A self-timer running now.
    pub countdown: Option<Countdown>,
    /// Digital zoom: the window of the frame the viewfinder shows and the
    /// shutter saves. Pinch, ctrl+wheel, trackpad pinch or +/−/0.
    pub zoom: Zoom,
    /// The kit's touch bridge, polled every frame so its live-contact map
    /// stays true even on frames that draw the viewer.
    touch: TouchSource,
    gestures: GestureRecognizer,
    /// Tap to focus: the search, where it was tapped on screen, and the lens it drives (found on the
    /// first tap, for the camera that is live).
    pub focus: crate::tapfocus::TapFocus,
    focus_screen: Option<egui::Pos2>,
    /// Point-and-shoot focus: the multi-region search, when to run it, and the camera the lens was resolved for.
    pub autofocus: crate::autofocus::AutoRun,
    governor: crate::autofocus::Governor,
    af_for: Option<String>,
    manual_focus_applied: Option<f32>,
    focus_lens: Option<(String, Box<dyn crate::tapfocus::Lens>, crate::tapfocus::Actuator)>,
    /// Where the picture was drawn last frame — what a pinch is measured
    /// against, and what the tests aim at.
    pub picture_rect: egui::Rect,
    /// DSLR settings: lens, crop, EV, WB, overlays.
    pub pro: Pro,
    /// The pro panel is open.
    pub pro_open: bool,
    /// The frame texture holds a `develop` render (already cropped and
    /// zoomed) rather than the raw frame.
    developed: bool,
    /// Zebra/peaking, same size as the developed frame.
    overlay: Option<egui::TextureHandle>,
    pub measured: Option<Measured>,
    last_measure: std::time::Instant,
    pub hw: Hw,
    /// The preview develop worker, started the first time it is needed.
    developer: Option<crate::developer::Developer>,
    /// Triggers, primed shots, brackets and merges.
    pub flow: capture_flow::Flow,
    pub layout: ViewMode,
    /// Full-screen layout: the roll is a page of its own.
    pub roll_open: bool,
    /// Height of everything under the viewfinder last frame, so the
    /// full-screen viewfinder can take the rest.
    bottom_h: f32,
    /// Where last frame's controls began (screen y): taps at or below it are the buttons', not the picture's.
    controls_top: f32,
}

impl Cam {
    /// Bring everything up: the roll, the contacts, the camera.
    pub fn open(ctx: &egui::Context) -> Cam {
        let capture_dir = store::capture_dir();
        let roll = store::load(&capture_dir);
        let contacts = crate::contacts::load();
        let (facing, full_bleed, timer_secs) = load_settings();
        let camera = Camera::open(facing);
        let mut cam = Self::with_parts(ctx, roll, contacts, camera, capture_dir,
                                       snap_handoff::default_dir(), handoff::photosync_root());
        cam.preview.full_bleed = full_bleed;
        cam.timer_secs = timer_secs;
        cam.pro = Pro::with_lenses_from(&lens::lens_dir());
        if let Some(text) = config_kit::load(store::APP, SETTINGS) {
            let ini = config_kit::Ini::parse(&text);
            cam.pro.apply_conf(|k| ini.top(k).map(String::from));
            capture_flow::apply_conf(&mut cam.flow, |k| ini.top(k).map(String::from));
            cam.layout = ViewMode::from_conf(ini.top("layout").unwrap_or("auto"));
        }
        if let Some(c) = std::fs::read_to_string(crate::cover::path()).ok().and_then(|t| crate::cover::CoverProfile::parse(&t)) {
            cam.pro.cover = c;
        }
        cam.apply_triggers();
        cam.note = match cam.pro.load_errors.first() {
            Some(e) => format!("lens profile skipped — {e}"),
            None => format!("{} · {} photo(s) on the roll", cam.camera.probe.summary(), cam.roll.len()),
        };
        cam
    }

    /// The shell over explicit parts — what the layout tests build.
    pub fn with_parts(ctx: &egui::Context, roll: Roll, contacts: Vec<Contact>, camera: Camera,
                      capture_dir: PathBuf, outbox: PathBuf, library_root: PathBuf) -> Cam {
        Cam {
            roll,
            preview: Preview { facing: preview::Facing::Front, full_bleed: false, sensor_aspect: camera_aspect(&camera) },
            camera,
            frame: None,
            thumbs: Default::default(),
            contact: contacts.first().map(|c| c.sc_id.clone()),
            contacts,
            capture_dir,
            outbox,
            library_root,
            note: String::new(),
            colors: uikit_egui::palette_colors_ctx(ctx, "cloudscape"),
            flashed: None,
            last_poll: std::time::Instant::now(),
            dirty: false,
            viewer: None,
            settings_dirty: false,
            timer_secs: 0,
            countdown: None,
            zoom: Zoom::default(),
            touch: TouchSource::new(),
            gestures: GestureRecognizer::default(),
            focus: crate::tapfocus::TapFocus::default(),
            focus_screen: None,
            autofocus: crate::autofocus::AutoRun::default(),
            governor: crate::autofocus::Governor::default(),
            af_for: None,
            manual_focus_applied: None,
            focus_lens: None,
            picture_rect: egui::Rect::NOTHING,
            pro: Pro::default(),
            pro_open: false,
            developed: false,
            overlay: None,
            measured: None,
            last_measure: std::time::Instant::now() - MEASURE_EVERY,
            hw: Hw::default(),
            developer: None,
            flow: capture_flow::Flow::default(),
            layout: ViewMode::Auto,
            roll_open: false,
            bottom_h: 0.0,
            controls_top: f32::INFINITY,
        }
    }

    /// The shutter was pressed: now, or after the timer.
    pub fn press_shutter(&mut self, ctx: &egui::Context) {
        if self.flow.mode != crate::bracket::Mode::Single && self.timer_secs == 0 {
            self.start_sequence();
        } else if self.timer_secs == 0 {
            self.take_photo(ctx);
        } else if self.countdown.is_none() {
            self.countdown = Some(Countdown { started: std::time::Instant::now(), secs: self.timer_secs });
            self.note = format!("taking the photo in {}s", self.timer_secs);
        }
    }

    /// Advance a running timer; the shutter fires when it ends.
    fn tick_countdown(&mut self, ctx: &egui::Context) {
        let Some(cd) = self.countdown else { return };
        if cd.done(std::time::Instant::now()) {
            self.countdown = None;
            if self.flow.mode != crate::bracket::Mode::Single { self.start_sequence(); } else { self.take_photo(ctx); }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// The shutter: the frame the viewfinder shows, to a JPEG on the roll.
    pub fn take_photo(&mut self, ctx: &egui::Context) {
        let Some(full) = self.camera.snapshot() else {
            self.note = "no frame to take — the camera is not live".into();
            return;
        };
        self.save_frame(ctx, full, None, None);
    }

    /// A frame to a photo on the roll: developed (lens, crop, zoom, cover,
    /// WB — or `gains` instead of the WB/EV/cover gains — EV) or the zoom
    /// window at native pixels. `what` names how it was made, for the note.
    pub fn save_frame(&mut self, ctx: &egui::Context, full: capture_kit::RawFrame, gains: Option<[f32; 3]>, what: Option<String>) {
        // Developed: the same render the viewfinder ran, at native size —
        // lens correction, crop, zoom, desqueeze, WB, EV. Otherwise the zoom
        // window at native pixels: what the viewfinder showed is what the
        // file holds, and nothing is upscaled to pretend otherwise.
        let tone = self.pro.auto_tone.then(|| crate::autotone::measure(&full.rgba, full.width, full.height));
        let scene = crate::fusion::stats(&full.rgba, full.width, full.height).map(|s| crate::fusion::judge(&s).label());
        let developed = self.pro.develops() || gains.is_some() || tone.is_some();
        let f = if developed {
            let mut p = self.pro.params(self.zoom.window(), full.width, full.height, None);
            if let Some(g) = gains { p.gains = g; }
            if let Some(t) = tone {
                // black point under any cover correction already there; white balance only when the
                // gains were not chosen for us (a WB bracket's are); the exposure bias always
                p.flare = p.flare.map(|f| f.max(t.black));
                let g = if gains.is_some() { [2f32.powf(t.ev); 3] } else { t.total_gains() };
                p.gains = [p.gains[0] * g[0], p.gains[1] * g[1], p.gains[2] * g[2]];
            }
            let mut px = develop::render(&full.rgba, full.width, full.height, &p);
            if self.pro.fusion && gains.is_none() { px = crate::fusion::fuse_brackets(&px, p.out_w, p.out_h); }
            capture_kit::RawFrame { width: p.out_w, height: p.out_h, captured_ms: full.captured_ms, seq: full.seq, rgba: px.into() }
        } else if self.zoom.is_zoomed() { full.crop(self.zoom.window()) } else { full };
        let now = now_ms();
        let name = store::free_name(&self.capture_dir, &capture_filename(now, self.camera.shown_facing().label(), "jpg"));
        let path = self.capture_dir.join(&name);
        match store::save_jpeg(&path, f.width, f.height, &f.rgba) {
            Ok(()) => {
                self.roll.push(Shot::new(&name, f.width, f.height, now, self.camera.shown_facing().label()));
                let (tw, th, px) = store::thumbnail(f.width, f.height, &f.rgba, THUMB);
                if tw > 0 {
                    let img = egui::ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &px);
                    self.thumbs.insert(name.clone(), ctx.load_texture(format!("thumb:{name}"), img, egui::TextureOptions::LINEAR));
                }
                self.flashed = Some(std::time::Instant::now());
                self.dirty = true;
                let what = what.map(|w| format!("{w}; ")).unwrap_or_default();
                self.note = if developed {
                    format!("saved {} ({what}{}{}, {}×{})", path.display(), self.pro.readout(None), scene.map(|s| format!(" · scene: {s}")).unwrap_or_default(), f.width, f.height)
                } else if self.zoom.is_zoomed() {
                    format!("saved {} ({what}{} crop, {}×{})", path.display(), self.zoom.label(), f.width, f.height)
                } else if !what.is_empty() {
                    format!("saved {} ({})", path.display(), what.trim_end_matches("; "))
                } else {
                    format!("saved {}", path.display())
                };
            }
            Err(e) => self.note = format!("could not save the photo: {e}"),
        }
    }

    /// A thumbnail for a shot from before this launch, decoded once, at
    /// most `budget` decodes per call.
    fn thumb_for(&mut self, ctx: &egui::Context, file: &str, budget: &mut usize) -> Option<egui::TextureHandle> {
        if let Some(t) = self.thumbs.get(file) { return Some(t.clone()); }
        if *budget == 0 { return None; }
        *budget -= 1;
        let (w, h, px) = store::thumbnail_from_file(&self.capture_dir.join(file), THUMB)?;
        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
        let t = ctx.load_texture(format!("thumb:{file}"), img, egui::TextureOptions::LINEAR);
        self.thumbs.insert(file.to_string(), t.clone());
        Some(t)
    }

    pub fn share(&mut self, i: usize, target: ShareTarget) {
        let Some(s) = self.roll.shots_mut().get_mut(i) else { return };
        let label = target.label();
        match s.share_to(target) {
            Ok(()) => { self.note = format!("queued for {label}"); self.dirty = true; self.last_poll -= HANDOFF_POLL; }
            Err(e) => self.note = e,
        }
    }

    /// Delete a photo: the file to the trash folder beside the roll, the
    /// shot off the roll. Trash rather than unlink, because a shutter
    /// press next to a delete button is how the one photo that mattered
    /// goes; the folder is `$roll/.trash/`.
    pub fn delete(&mut self, i: usize) {
        let Some(s) = self.roll.shots().get(i).cloned() else { return };
        let trash = self.capture_dir.join(".trash");
        let moved = std::fs::create_dir_all(&trash)
            .and_then(|_| std::fs::rename(self.capture_dir.join(&s.file), trash.join(&s.file)));
        match moved {
            Ok(()) => {
                self.roll.remove(i);
                self.thumbs.remove(&s.file);
                self.dirty = true;
                self.note = format!("{} moved to {}", s.file, trash.display());
                if let Some(v) = &mut self.viewer {
                    if self.roll.is_empty() { self.viewer = None; }
                    else { v.index = v.index.min(self.roll.len() - 1); v.loaded = None; v.confirm_delete = false; }
                }
            }
            Err(e) => self.note = format!("could not move {} to the trash: {e}", s.file),
        }
    }

    fn poll_handoffs(&mut self) {
        if self.last_poll.elapsed() < HANDOFF_POLL { return; }
        self.last_poll = std::time::Instant::now();
        let notes = handoff::poll(&mut self.roll, &self.capture_dir, &self.outbox, &self.library_root, now_ms());
        if let Some(n) = notes.last() { self.note = n.clone(); self.dirty = true; }
    }

    fn save_if_dirty(&mut self) {
        if self.dirty {
            self.dirty = false;
            if let Err(e) = store::save(&self.capture_dir, &self.roll) {
                self.note = format!("could not write the roll: {e}");
            }
        }
        if self.settings_dirty {
            self.settings_dirty = false;
            let _ = config_kit::save(store::APP, SETTINGS,
                &format!("facing = {}\nfill = {}\ntimer = {}\nlayout = {}\n{}{}", self.camera.facing.label(), self.preview.full_bleed, self.timer_secs,
                         self.layout.to_conf(), self.pro.conf(), capture_flow::conf(&self.flow)));
        }
    }

    pub fn disposition_text(&self, s: &Shot) -> (String, egui::Color32) {
        let name = |t: &ShareTarget| match t {
            ShareTarget::PhotoSync => "photosync".to_string(),
            ShareTarget::Contact(id) => self.contacts.iter().find(|c| &c.sc_id == id).map(|c| c.label().to_string()).unwrap_or_else(|| id.clone()),
        };
        let (mut text, col) = match s.disposition() {
            Disposition::Local => ("local".to_string(), self.colors.text_weak),
            Disposition::Pending(t) => (format!("sending to {}…", name(t)), self.colors.warning),
            Disposition::Shared(ShareTarget::PhotoSync) => ("backed up".into(), self.colors.success),
            Disposition::Shared(t) => (format!("shared with {}", name(t)), self.colors.accent),
            Disposition::Failed(t, why) => (format!("to {} failed: {why}", name(t)), self.colors.danger),
        };
        // The sticky record, when the latest act is not the send: a photo
        // backed up after going to Bee still says Bee has it.
        let elsewhere: Vec<String> = s.sent_to.iter()
            .filter(|id| !matches!(s.disposition(), Disposition::Pending(ShareTarget::Contact(x)) | Disposition::Shared(ShareTarget::Contact(x)) | Disposition::Failed(ShareTarget::Contact(x), _) if x == *id))
            .map(|id| name(&ShareTarget::Contact(id.clone()))).collect();
        if !elsewhere.is_empty() { text = format!("{text} · shared with {}", elsewhere.join(", ")); }
        (text, col)
    }

    /// One frame: engine first, then the page (or the viewer over it).
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.camera.poll();
        if let Some(f) = self.camera.new_frame() {
            if f.pixels_plausible() { self.upload_frame(&ctx, &f); }
        }
        self.take_developed(&ctx);
        self.tick_capture(&ctx);
        self.refresh_hw();
        self.poll_handoffs();
        self.tick_countdown(&ctx);
        self.save_if_dirty();
        self.preview.facing = match self.camera.shown_facing() { Facing::Front => preview::Facing::Front, Facing::Rear => preview::Facing::Rear };
        let compact = uikit_egui::adaptive::Adaptive::detect(&ctx).class.is_compact();
        // Polled every frame, whichever surface is up: the bridge tracks
        // live contacts from raw events, and a frame it does not see leaves
        // a finger "down" that lifted while the viewer was open.
        let (gestures, _gliding) = self.touch.poll(&ctx, &mut self.gestures);

        if self.viewer.is_some() {
            let acts = self.viewer_ui(ui, compact);
            for a in acts { self.apply_viewer(a); }
        } else if self.roll_open && self.layout.is_full(compact) {
            ui.horizontal(|ui| {
                if ui.button("← camera").clicked() { self.roll_open = false; }
                ui.heading("roll");
            });
            self.roll_ui(ui, &ctx);
        } else {
            self.page_ui(ui, &ctx, compact, &gestures);
        }
        // Wake for frames while live; otherwise only for the handoff poll.
        ctx.request_repaint_after(if self.camera.is_live() && self.viewer.is_none() {
            std::time::Duration::from_millis(33) } else { HANDOFF_POLL });
    }

    fn page_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, compact: bool, gestures: &[uikit_egui::touch::Gesture]) {
        let c = self.colors.clone();
        let full = self.layout.is_full(compact);
        if !full {
            ui.horizontal(|ui| {
                ui.heading("snapshot-cam");
                ui.colored_label(c.text_weak, egui::RichText::new("photos stay on this device until you send them").small());
            });
        }
        // Full screen: the viewfinder takes everything the controls under
        // it did not need last frame.
        let vf_h = full.then(|| (ui.available_height() - self.bottom_h.max(110.0)).max(120.0));

        // THE VIEWFINDER: real frames, mirrored for the front camera, the
        // zoom window of them when zoomed.
        let live = if self.camera.is_live() { self.frame.as_ref() } else { None };
        let has_live = live.is_some();
        let uv = if self.developed { preview::frame_uv(&self.preview, &Zoom::default()) } else { preview::frame_uv(&self.preview, &self.zoom) };
        let (shown, visible) = preview::show(ui, &self.preview, &self.camera.status_line(), live, uv, vf_h);
        self.picture_rect = shown;
        if live.is_some() { self.draw_aids(ui, shown, visible, uv); }
        // DIGITAL ZOOM. A pinch (two fingers, a trackpad, ctrl+wheel) over
        // the picture zooms about the point between the fingers; a
        // one-finger drag pans while zoomed; +/− step and 0 resets, when no
        // text field has the keyboard; the chip on the picture is the
        // one-finger way back to 1×. The gestures are this frame's, from
        // the bridge polled in `ui`.
        if self.zoom_input(ctx, gestures, shown) { ctx.request_repaint(); }
        // TAP TO FOCUS: a tap on the picture (not a pinch, not a drag) focuses there
        for g in gestures {
            if let uikit_egui::touch::Gesture::Tap(p) = g {
                let at = egui::pos2(p.x, p.y);
                if crate::tapfocus::tap_counts([at.x, at.y], [shown.left(), shown.top(), shown.right(), shown.bottom()], self.controls_top) && has_live && self.viewer.is_none() { self.tap_focus(at, shown); }
            }
        }
        self.drive_focus(ctx);
        self.draw_focus_ring(ui, ctx);
        uikit_egui::zoom::chip_ui(ui, &mut self.zoom, visible, ui.id().with("zoom-chip"));
        if let Some(cd) = self.countdown {
            // The count, big, in the middle of the picture — where the eye
            // is while posing.
            let n = cd.remaining(std::time::Instant::now());
            ui.painter().circle_filled(shown.center(), 44.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120));
            ui.painter().text(shown.center(), egui::Align2::CENTER_CENTER, n.to_string(),
                egui::FontId::proportional(56.0), egui::Color32::WHITE);
        }
        if let Some(line) = self.flow.status(ctx.input(|i| i.time) * 1000.0) {
            // Primed / bracketing / merging: said over the picture, where
            // the eye is while holding still.
            let g = ui.painter().layout_no_wrap(line, egui::FontId::proportional(14.0), egui::Color32::WHITE);
            let at = egui::pos2(visible.center().x - g.size().x * 0.5, visible.top() + 40.0);
            ui.painter().rect_filled(egui::Rect::from_min_size(at - egui::vec2(8.0, 4.0), g.size() + egui::vec2(16.0, 8.0)), 6.0,
                                     egui::Color32::from_rgba_unmultiplied(200, 60, 40, 200));
            ui.painter().galley(at, g, egui::Color32::WHITE);
        }
        let bottom_top = ui.cursor().top();
        self.controls_top = bottom_top;
        if let Some(t) = &self.flashed {
            let age = t.elapsed().as_secs_f32();
            if age < 0.25 {
                let a = ((1.0 - age / 0.25) * 200.0) as u8;
                ui.painter().rect_filled(visible, 6.0, egui::Color32::from_rgba_unmultiplied(255, 255, 255, a));
                ctx.request_repaint();
            } else { self.flashed = None; }
        }
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(egui::RichText::new(self.camera.status_line()).small().color(c.text_weak)).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if self.pro_open { "pro ▾" } else { "pro ▸" };
                if ui.selectable_label(self.pro_open, label).on_hover_text("lens, exposure, white balance, aids").clicked() {
                    self.pro_open = !self.pro_open;
                }
            });
        });

        // Controls: facing, fill, the shutter.
        ui.horizontal(|ui| {
            let can_switch = self.camera.can_switch() || self.camera.source == Source::TestPattern;
            if ui.add_enabled(can_switch, egui::Button::new(match self.camera.facing { Facing::Front => "⇄ front", Facing::Rear => "⇄ rear" }))
                .on_hover_text("switch camera").clicked() {
                self.camera.flip();
                self.frame = None;
                // A window into one sensor means nothing on the other.
                self.zoom.reset();
                self.pro.measured_crop = None;
                self.settings_dirty = true;
            }
            if ui.checkbox(&mut self.preview.full_bleed, "fill").changed() { self.settings_dirty = true; }
            // The self-timer: one control that cycles 0 → 3 → 10 seconds.
            let timer_label = if self.timer_secs == 0 { "⏱ off".to_string() } else { format!("⏱ {}s", self.timer_secs) };
            if ui.button(timer_label).on_hover_text("self-timer").clicked() {
                let i = TIMERS.iter().position(|t| *t == self.timer_secs).unwrap_or(0);
                self.timer_secs = TIMERS[(i + 1) % TIMERS.len()];
                self.settings_dirty = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = match (self.countdown, self.flow.mode) {
                    (Some(cd), _) => format!("● {}…", cd.remaining(std::time::Instant::now())),
                    (None, crate::bracket::Mode::Single) => "● take photo".to_string(),
                    (None, m) => format!("● {}", m.label()),
                };
                let shutter = egui::Button::new(egui::RichText::new(label).size(18.0))
                    .min_size(egui::vec2(if compact { 150.0 } else { 180.0 }, 40.0));
                let space = self.camera.is_live() && ui.input(|i| i.key_pressed(egui::Key::Space)) && !ctx.egui_wants_keyboard_input();
                let can = self.camera.is_live() && self.countdown.is_none() && !self.flow.busy();
                if ui.add_enabled(can, shutter).clicked() || (space && can) {
                    self.press_shutter(ctx);
                }
            });
        });

        if self.pro_open {
            if full {
                // Over a full-screen viewfinder the panel scrolls rather
                // than pushing the picture off the top.
                let max = ui.ctx().content_rect().height() * 0.42;
                egui::ScrollArea::vertical().id_salt("pro-scroll").max_height(max).show(ui, |ui| self.pro_panel(ui));
            } else {
                self.pro_panel(ui);
            }
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.colored_label(c.text_weak, "send to:");
            if self.contacts.is_empty() {
                ui.colored_label(c.text_weak, egui::RichText::new("no snapclone contacts on this device").small());
            } else {
                let current = self.contact.clone();
                let label = self.contacts.iter().find(|x| Some(&x.sc_id) == current.as_ref()).map(|x| x.label().to_string()).unwrap_or_else(|| "pick a contact".into());
                egui::ComboBox::from_id_salt("contact").selected_text(label).show_ui(ui, |ui| {
                    for ct in &self.contacts {
                        if ui.selectable_label(current.as_deref() == Some(ct.sc_id.as_str()), ct.label()).clicked() {
                            self.contact = Some(ct.sc_id.clone());
                        }
                    }
                });
            }
            if full {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(format!("roll · {} ›", self.roll.len())).clicked() { self.roll_open = true; }
                });
            } else {
                ui.add(egui::Label::new(egui::RichText::new(format!("library: {}", self.library_root.display())).small().color(c.text_weak)).truncate());
            }
        });
        if !self.note.is_empty() {
            ui.add(egui::Label::new(egui::RichText::new(&self.note).weak().small()).truncate());
        }
        self.bottom_h = ui.cursor().top() - bottom_top + 8.0;
        if full { return; }
        self.roll_ui(ui, ctx);
    }

    /// The roll: counts, then every photo with its disposition and actions.
    fn roll_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let c = self.colors.clone();

        ui.add_space(6.0);
        ui.separator();
        // Counts first: "what have I shared" answered without scrolling.
        ui.colored_label(c.text, format!("{} photo(s) · {} local · {} shared with someone",
            self.roll.len(), self.roll.local_only().len(), self.roll.disclosed().len()));

        let contact = self.contact.clone();
        let contact_label = contact.as_ref().and_then(|id| self.contacts.iter().find(|x| &x.sc_id == id)).map(|x| x.label().to_string());
        let mut acted: Option<(usize, ShareTarget)> = None;
        let mut open: Option<usize> = None;
        let mut budget = THUMB_DECODES_PER_FRAME;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let shots: Vec<Shot> = self.roll.shots().to_vec();
            for (i, s) in shots.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(THUMB as f32, THUMB as f32 * 9.0 / 16.0), egui::Sense::click());
                    match self.thumb_for(ctx, &s.file, &mut budget) {
                        Some(t) => { ui.painter().image(t.id(), rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE); }
                        None => { ui.painter().rect_filled(rect, 3.0, c.bg_raised); }
                    }
                    if resp.clicked() { open = Some(i); }
                    resp.on_hover_text("open");
                    ui.vertical(|ui| {
                        ui.add(egui::Label::new(egui::RichText::new(&s.file).monospace().small().color(c.text)).truncate());
                        // The DISPOSITION is always visible, not on hover: a
                        // photo somebody else can see should say so from
                        // across the room.
                        let (text, col) = self.disposition_text(s);
                        ui.colored_label(col, egui::RichText::new(text).small());
                        ui.horizontal(|ui| {
                            if !matches!(s.disposition(), Disposition::Pending(_)) {
                                if ui.small_button("back up").on_hover_text("copy into the photosync library on this device").clicked() {
                                    acted = Some((i, ShareTarget::PhotoSync));
                                }
                                if let (Some(id), Some(label)) = (&contact, &contact_label) {
                                    if ui.small_button(format!("send to {label}")).on_hover_text("they will see it").clicked() {
                                        acted = Some((i, ShareTarget::Contact(id.clone())));
                                    }
                                }
                            }
                        });
                    });
                });
                ui.add_space(2.0);
            }
            if shots.is_empty() {
                ui.colored_label(c.text_weak, "No photos yet. The shutter (or Space) takes one; pinch, ctrl+wheel or +/− zoom.");
            }
        });
        if let Some((i, target)) = acted { self.share(i, target); }
        if let Some(i) = open { self.viewer = Some(Viewer::open(i)); }
        // More thumbnails to decode next frame: ask for one.
        if budget == 0 { ctx.request_repaint(); }
    }

    /// A new frame to the viewfinder: as it is (the GPU draws the zoom
    /// window), or through `develop` when the pro settings change the
    /// picture or an aid needs to measure it.
    fn upload_frame(&mut self, ctx: &egui::Context, f: &capture_kit::RawFrame) {
        if self.pro.needs_cpu() {
            // To the worker; `take_developed` uploads what comes back.
            let measure = ((self.pro.measuring() || self.pro.wb == develop::WhiteBalance::Auto)
                && self.last_measure.elapsed() >= MEASURE_EVERY).then(|| {
                    self.last_measure = std::time::Instant::now();
                    self.pro.measure_params(f.width, f.height)
                });
            let ppp = ctx.pixels_per_point();
            let fit = if self.picture_rect.is_positive() {
                (((self.picture_rect.width() * ppp) as u32).clamp(160, PREVIEW_MAX), ((self.picture_rect.height() * ppp) as u32 * 2).clamp(160, PREVIEW_MAX))
            } else { (480, 480) };
            let params = self.pro.params(self.zoom.window(), f.width, f.height, Some(fit));
            let o = self.pro.overlays;
            self.developer.get_or_insert_with(crate::developer::Developer::start)
                .submit(crate::developer::Job { frame: f.clone(), params, zebra: o.zebra, peaking: o.peaking, measure });
            return;
        }
        self.developed = false;
        self.overlay = None;
        self.measured = None;
        let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.rgba);
        self.set_frame(ctx, img, f.width, f.height);
    }

    fn set_frame(&mut self, ctx: &egui::Context, img: egui::ColorImage, w: u32, h: u32) {
        match &mut self.frame {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => self.frame = Some(ctx.load_texture("viewfinder", img, egui::TextureOptions::LINEAR)),
        }
        self.preview.sensor_aspect = w as f32 / h.max(1) as f32;
    }

    /// Upload the worker's newest developed frame, fold its measurement in.
    fn take_developed(&mut self, ctx: &egui::Context) {
        let Some(done) = self.developer.as_ref().and_then(|d| d.latest()) else { return };
        if !self.pro.needs_cpu() { return; } // settled back to the raw path meanwhile
        if let Some((small, w, h)) = &done.measured { self.pro.measured(small, *w, *h); }
        let (w, h) = (done.params.out_w, done.params.out_h);
        match &done.analysis.overlay {
            Some(ov) => {
                let oi = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], ov);
                match &mut self.overlay {
                    Some(t) => t.set(oi, egui::TextureOptions::NEAREST),
                    None => self.overlay = Some(ctx.load_texture("viewfinder-aids", oi, egui::TextureOptions::NEAREST)),
                }
            }
            None => self.overlay = None,
        }
        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &done.rgba);
        self.set_frame(ctx, img, w, h);
        self.measured = Some(Measured { analysis: done.analysis });
        self.developed = true;
    }

    /// Re-read the camera's controls when the live device changes.
    fn refresh_hw(&mut self) {
        let dev = self.camera.live_device();
        if self.hw.checked.as_ref() == Some(&dev) { return; }
        self.hw = Hw { checked: Some(dev.clone()), ..Hw::default() };
        if let Some(d) = dev {
            match hwctl::list_with(&hwctl::program(), &d) {
                Ok(c) if c.has_any() => self.hw.controls = Some(c),
                Ok(_) => self.hw.error = Some(format!("{d} offers no exposure, gain, WB or focus controls")),
                Err(e) => self.hw.error = Some(e),
            }
        }
    }

    /// Find the lens motor for the live camera (once per camera). `Err` says why not: no motor, or not known.
    fn ensure_lens(&mut self) -> Result<(), String> {
        let id = self.camera.camera_id();
        if matches!(&self.focus_lens, Some((cached, _, _)) if Some(cached) == id.as_ref()) { return Ok(()); }
        // the camera changed (or this is the first look): forget what belonged to the other one
        self.focus_lens = None;
        self.autofocus.cancel();
        self.focus.cancel();
        self.governor.reset();
        self.manual_focus_applied = None;
        let id = id.ok_or("no focus motor known for this camera source")?;
        let subdevs = capture_kit::focus::list_subdevs();
        let dev = crate::tapfocus::actuator_for(&id, &subdevs).ok_or("this camera has a fixed focus (no lens motor)")?;
        let range = capture_kit::focus::read_range(&hwctl::program(), &dev).map_err(|e| format!("focus: {e}"))?;
        let lens = crate::tapfocus::V4l2Lens::new(hwctl::program(), dev.clone());
        self.focus_lens = Some((id, Box::new(lens), crate::tapfocus::Actuator { device: dev, range }));
        Ok(())
    }

    /// A tap at `at` on the picture drawn in `shown`: it overrides the automatic focus, which then
    /// leaves the lens alone until the scene changes.
    fn tap_focus(&mut self, at: egui::Pos2, shown: egui::Rect) {
        if matches!(self.pro.profile().focus, lens::FocusHint::Manual(_)) {
            self.note = "manual focus is set in the pro panel; choose the camera's focus to tap".into();
            return;
        }
        if let Err(e) = self.ensure_lens() { self.note = e; return; }
        self.autofocus.cancel();
        let window = crate::pro::compose(self.pro.base_window(), self.zoom.window());
        let (roi, tapped) = crate::tapfocus::roi_for_tap(self.preview.picture_frac(at, shown), window);
        if let Some((_, lens, act)) = &mut self.focus_lens {
            self.focus.begin(roi, tapped, act, lens.as_mut(), now_ms());
            self.focus_screen = Some(at);
            self.note = "focusing…".into();
        }
    }

    /// Per frame: a tap's search, else the automatic one, else decide whether to start it.
    fn drive_focus(&mut self, ctx: &egui::Context) {
        let now = now_ms();
        // TAP in progress: its frames, its verdict, then hold the lens until the scene changes
        if self.focus.is_searching() {
            ctx.request_repaint();
            let (Some(f), Some((_, lens, _))) = (self.camera.snapshot(), self.focus_lens.as_mut()) else { return };
            if self.focus.on_frame(&f, now, lens.as_mut()).is_some() {
                if let Some(m) = self.focus.message() { self.note = m; }
                self.governor.hold(&f, now);
            }
            return;
        }
        // AUTOMATIC search in progress
        if self.autofocus.is_searching() {
            ctx.request_repaint();
            let (Some(f), Some((_, lens, _))) = (self.camera.snapshot(), self.focus_lens.as_mut()) else { return };
            if let Some(v) = self.autofocus.on_frame(&f, now, lens.as_mut()) {
                if let crate::autofocus::AutoVerdict::Failed(e) = v { self.note = format!("autofocus: {e}"); }
            }
            return;
        }
        if !self.camera.is_live() { return; }
        // MANUAL focus (a pro lens profile): the lens goes where the profile says, nothing automatic
        if let lens::FocusHint::Manual(frac) = self.pro.profile().focus {
            if self.manual_focus_applied != Some(frac) && self.ensure_lens().is_ok() {
                if let Some((_, lens, act)) = &mut self.focus_lens {
                    let (min, max, _) = act.range;
                    let _ = lens.move_to(min + ((max - min) as f32 * frac.clamp(0.0, 1.0)).round() as i64);
                    self.manual_focus_applied = Some(frac);
                }
            }
            return;
        }
        self.manual_focus_applied = None;
        // POINT AND SHOOT: once the picture is held steady, and again when the scene really changes.
        // SNAPSHOT_CAM_AUTOFOCUS=0 turns it off (the lens then only moves for a tap or a manual profile).
        static AF_ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*AF_ON.get_or_init(|| std::env::var("SNAPSHOT_CAM_AUTOFOCUS").map(|v| v.trim() != "0").unwrap_or(true)) { return; }
        if self.af_for != self.camera.camera_id() {
            self.af_for = self.camera.camera_id();
            self.governor.reset();
            if self.ensure_lens().is_err() { self.focus_lens = None; }
        }
        let (Some(f), Some((_, lens, act))) = (self.camera.snapshot(), self.focus_lens.as_mut()) else { return };
        if let Some(w) = self.governor.feed(&f, now, false) {
            self.autofocus.begin(w, act.range, lens.as_mut(), now);
        }
    }

    /// The square over the tap: white and pulsing while searching, then green / red and fading.
    fn draw_focus_ring(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // the grid cell the automatic search chose: four corner brackets, green, fading
        if let Some((rect, a)) = self.autofocus.shown_region(now_ms()) {
            let window = crate::pro::compose(self.pro.base_window(), self.zoom.window());
            let shown = self.picture_rect;
            let to_screen = |u: f32, v: f32| {
                let (mut x, y) = ((u - window[0]) / (window[2] - window[0]).max(1e-3), (v - window[1]) / (window[3] - window[1]).max(1e-3));
                if self.preview.facing.mirrored() { x = 1.0 - x; }
                egui::pos2(shown.left() + x * shown.width(), shown.top() + y * shown.height())
            };
            let (p0, p1) = (to_screen(rect[0], rect[1]), to_screen(rect[2], rect[3]));
            let r = egui::Rect::from_two_pos(p0, p1).shrink(6.0);
            let col = egui::Color32::from_rgba_unmultiplied(90, 220, 110, (230.0 * a) as u8);
            let st = egui::Stroke::new(2.5, col);
            let l = (r.width().min(r.height()) * 0.18).max(8.0);
            for (c, dx, dy) in [(r.left_top(), 1.0, 1.0), (r.right_top(), -1.0, 1.0), (r.left_bottom(), 1.0, -1.0), (r.right_bottom(), -1.0, -1.0)] {
                ui.painter().line_segment([c, c + egui::vec2(dx * l, 0.0)], st);
                ui.painter().line_segment([c, c + egui::vec2(0.0, dy * l)], st);
            }
            ctx.request_repaint();
        }
        let (Some(at), Some((ring, a))) = (self.focus_screen, self.focus.ring(now_ms())) else { return };
        let (col, pulse) = match ring {
            crate::tapfocus::Ring::Searching => (egui::Color32::WHITE, 0.6 + 0.4 * ((now_ms() % 600) as f32 / 600.0 * std::f32::consts::TAU).sin()),
            crate::tapfocus::Ring::Locked => (egui::Color32::from_rgb(90, 220, 110), 1.0),
            crate::tapfocus::Ring::Refused => (egui::Color32::from_rgb(240, 180, 60), 1.0),
            crate::tapfocus::Ring::Failed => (egui::Color32::from_rgb(230, 80, 70), 1.0),
        };
        let col = egui::Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (255.0 * a * pulse) as u8);
        ui.painter().rect_stroke(egui::Rect::from_center_size(at, egui::vec2(64.0, 64.0)), 6.0, egui::Stroke::new(2.5, col), egui::StrokeKind::Middle);
        ctx.request_repaint();
    }

    /// Fold this frame's zoom input in: the bridge's gestures over the
    /// picture, and the keys. Returns whether the window changed.
    pub fn zoom_input(&mut self, ctx: &egui::Context, gestures: &[uikit_egui::touch::Gesture], picture: egui::Rect) -> bool {
        let preview = self.preview;
        let mut changed = self.zoom.apply(gestures, picture, |p| preview.picture_frac(p, picture));
        if !ctx.egui_wants_keyboard_input() {
            let (plus, minus, zero) = ctx.input(|i| (
                i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals),
                i.key_pressed(egui::Key::Minus),
                i.key_pressed(egui::Key::Num0),
            ));
            if plus { changed |= self.zoom.step_in(); }
            if minus { changed |= self.zoom.step_out(); }
            if zero { changed |= self.zoom.reset(); }
        }
        changed
    }

    /// The open photo, full size, with swipe/arrow paging, its disposition,
    /// and the actions the roll row has plus delete.
    fn viewer_ui(&mut self, ui: &mut egui::Ui, compact: bool) -> Vec<ViewerAction> {
        let c = self.colors.clone();
        let ctx = ui.ctx().clone();
        let mut out = Vec::new();
        let Some(v) = &mut self.viewer else { return vec![ViewerAction::Close] };
        let Some(shot) = self.roll.shots().get(v.index).cloned() else { return vec![ViewerAction::Close] };
        if v.loaded.as_deref() != Some(shot.file.as_str()) {
            v.texture = image::open(self.capture_dir.join(&shot.file)).ok().map(|img| {
                let rgba = img.to_rgba8();
                let ci = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
                ctx.load_texture(format!("full:{}", shot.file), ci, egui::TextureOptions::LINEAR)
            });
            v.loaded = Some(shot.file.clone());
            v.confirm_delete = false;
        }
        let (esc, left, right) = ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
        if esc { out.push(ViewerAction::Close); }
        if left { out.push(ViewerAction::Prev); }
        if right { out.push(ViewerAction::Next); }
        let total = self.roll.len();
        ui.horizontal(|ui| {
            if ui.button("← Back").clicked() { out.push(ViewerAction::Close); }
            ui.add(egui::Label::new(egui::RichText::new(&shot.file).monospace().color(c.text)).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(c.text_weak, format!("{} / {}", v.index + 1, total));
            });
        });
        let avail = ui.available_rect_before_wrap();
        let pic_h = if compact { avail.height() * 0.62 } else { avail.height() * 0.75 };
        let pic = egui::Rect::from_min_size(avail.min, egui::vec2(avail.width(), pic_h));
        let resp = ui.allocate_rect(pic, egui::Sense::click_and_drag());
        ui.painter().rect_filled(pic, 0.0, egui::Color32::from_rgb(8, 8, 10));
        match &v.texture {
            Some(t) => {
                let [w, h] = t.size();
                let scale = (pic.width() / w as f32).min(pic.height() / h as f32);
                let r = egui::Rect::from_center_size(pic.center(), egui::vec2(w as f32 * scale, h as f32 * scale));
                ui.painter().image(t.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
            }
            None => { ui.painter().text(pic.center(), egui::Align2::CENTER_CENTER, "the file could not be read", egui::FontId::proportional(14.0), c.warning); }
        }
        if resp.dragged() { v.drag_x += resp.drag_delta().x; }
        if resp.drag_stopped() {
            match swipe_commits(v.drag_x, pic.width()) { 1 => out.push(ViewerAction::Next), -1 => out.push(ViewerAction::Prev), _ => {} }
            v.drag_x = 0.0;
        }
        let idx = v.index;
        let confirm = v.confirm_delete;
        ui.add_space(6.0);
        let (text, col) = self.disposition_text(&shot);
        ui.colored_label(col, text);
        ui.colored_label(c.text_weak, egui::RichText::new(format!("{}×{} · {} camera", shot.width, shot.height, shot.camera)).small());
        ui.horizontal_wrapped(|ui| {
            if !matches!(shot.disposition(), Disposition::Pending(_)) {
                if ui.button("back up").clicked() { out.push(ViewerAction::Share(idx, ShareTarget::PhotoSync)); }
                if let Some(id) = &self.contact {
                    let label = self.contacts.iter().find(|x| &x.sc_id == id).map(|x| x.label().to_string()).unwrap_or_else(|| id.clone());
                    if ui.button(format!("send to {label}")).clicked() { out.push(ViewerAction::Share(idx, ShareTarget::Contact(id.clone()))); }
                }
            }
            let label = if confirm { "Really delete?" } else { "delete" };
            if ui.button(label).clicked() {
                if confirm { out.push(ViewerAction::Delete(idx)); }
                else if let Some(v) = &mut self.viewer { v.confirm_delete = true; }
            }
        });
        out
    }

    pub fn apply_viewer(&mut self, a: ViewerAction) {
        match a {
            ViewerAction::Close => self.viewer = None,
            ViewerAction::Prev => { if let Some(v) = &mut self.viewer { if v.index > 0 { v.index -= 1; } } }
            ViewerAction::Next => {
                let n = self.roll.len();
                if let Some(v) = &mut self.viewer { if v.index + 1 < n { v.index += 1; } }
            }
            ViewerAction::Delete(i) => self.delete(i),
            ViewerAction::Share(i, t) => self.share(i, t),
        }
    }
}

fn camera_aspect(c: &Camera) -> f32 {
    match c.source {
        Source::TestPattern => crate::camera::PATTERN_WIDTH as f32 / crate::camera::PATTERN_HEIGHT as f32,
        _ => crate::camera::WIDTH as f32 / crate::camera::HEIGHT as f32,
    }
}

/// Facing, fill and timer from the settings file; front, letterboxed and no
/// timer when there is none.
pub fn load_settings() -> (Facing, bool, u8) {
    // SNAPSHOT_CAM_DEFAULT_FACING=rear: a phone's first launch opens the main camera, not the selfie one
    let default = if std::env::var("SNAPSHOT_CAM_DEFAULT_FACING").map(|v| v.trim() == "rear").unwrap_or(false) { Facing::Rear } else { Facing::Front };
    let Some(text) = config_kit::load(store::APP, SETTINGS) else { return (default, false, 0) };
    let ini = config_kit::Ini::parse(&text);
    let get = |k: &str| ini.get("", k).or_else(|| ini.get("general", k)).map(str::trim);
    let facing = if get("facing") == Some("rear") { Facing::Rear } else { Facing::Front };
    let timer = get("timer").and_then(|v| v.parse::<u8>().ok()).filter(|t| TIMERS.contains(t)).unwrap_or(0);
    (facing, get("fill") == Some("true"), timer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use capture_kit::probe::Probe;

    fn shell(dir: &std::path::Path, n: usize) -> Cam {
        let ctx = egui::Context::default();
        let mut roll = Roll::default();
        for i in 0..n {
            let name = format!("cam-2026092{i}-100000-front.jpg");
            let (w, h) = (64u32, 36u32);
            let rgba = vec![90u8; (w * h * 4) as usize];
            store::save_jpeg(&dir.join(&name), w, h, &rgba).unwrap();
            roll.push(Shot::new(&name, w, h, 1_700_000_000_000 + i as u64, "front"));
        }
        let contacts = vec![Contact { sc_id: "sc-2dd464a4".into(), nickname: "Bee".into() }];
        let camera = Camera::unstarted(Probe::default(), Facing::Front);
        let mut cam = Cam::with_parts(&ctx, roll, contacts, camera, dir.to_path_buf(),
                                      dir.join("outbox"), dir.join("Pictures"));
        if let Some(first) = cam.roll.shots_mut().first_mut() {
            first.share_to(ShareTarget::Contact("sc-2dd464a4".into())).unwrap();
        }
        // The compact layout: these tests are about the roll under the
        // picture. The full-screen layout has its own below.
        cam.layout = ViewMode::Compact;
        cam
    }

    fn run(cam: &mut Cam, size: egui::Vec2) -> (uikit_egui::layout_probe::Report, Vec<uikit_egui::layout_probe::Text>) {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(screen);
        let mut last = Vec::new();
        for _ in 0..3 {
            #[allow(deprecated)]
            let out = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| { cam.ui(ui); });
            });
            last = out.shapes;
        }
        (uikit_egui::layout_probe::analyse(screen, &last, size.x < 600.0), uikit_egui::layout_probe::texts(&last))
    }

    #[test]
    fn the_page_lays_out_without_collisions_on_a_phone_and_a_desktop() {
        let d = tempfile::tempdir().unwrap();
        for size in [egui::vec2(360.0, 640.0), egui::vec2(1000.0, 700.0)] {
            let mut cam = shell(d.path(), 4);
            let (r, texts) = run(&mut cam, size);
            assert!(r.overlaps.is_empty(), "{size:?}: {}", r.describe());
            let has = |t: &str| texts.iter().any(|g| g.text.contains(t));
            assert!(has("take photo"), "the shutter is drawn");
            assert!(has("no camera found"), "the status says why there is no picture");
            assert!(has("sending to Bee"), "the disposition names the contact");
            assert!(has("4 photo(s)"), "the counts line");
        }
    }

    #[test]
    fn the_viewer_opens_pages_and_deletes_to_the_trash() {
        let d = tempfile::tempdir().unwrap();
        let mut cam = shell(d.path(), 3);
        cam.viewer = Some(Viewer::open(0));
        let (r, texts) = run(&mut cam, egui::vec2(360.0, 640.0));
        assert!(r.overlaps.is_empty(), "{}", r.describe());
        assert!(texts.iter().any(|g| g.text == "1 / 3"), "the position is shown");
        cam.apply_viewer(ViewerAction::Next);
        assert_eq!(cam.viewer.as_ref().unwrap().index, 1);
        cam.apply_viewer(ViewerAction::Next);
        cam.apply_viewer(ViewerAction::Next);
        assert_eq!(cam.viewer.as_ref().unwrap().index, 2, "no wrap past the end");
        let file = cam.roll.shots()[2].file.clone();
        cam.apply_viewer(ViewerAction::Delete(2));
        assert_eq!(cam.roll.len(), 2);
        assert!(d.path().join(".trash").join(&file).exists(), "deleted means moved to the trash");
        assert!(!d.path().join(&file).exists());
        assert_eq!(cam.viewer.as_ref().unwrap().index, 1, "the viewer stays on the last remaining");
        cam.apply_viewer(ViewerAction::Delete(1));
        cam.apply_viewer(ViewerAction::Delete(0));
        assert!(cam.viewer.is_none(), "nothing left to view");
        assert!(cam.roll.is_empty());
    }

    #[test]
    fn the_countdown_counts_down_and_fires_the_shutter_once() {
        let t0 = std::time::Instant::now();
        let cd = Countdown { started: t0, secs: 3 };
        assert_eq!(cd.remaining(t0), 3);
        assert_eq!(cd.remaining(t0 + std::time::Duration::from_millis(2500)), 1, "rounded up: never a 0 before the shutter");
        assert!(!cd.done(t0 + std::time::Duration::from_millis(2999)));
        assert!(cd.done(t0 + std::time::Duration::from_secs(3)));
        assert_eq!(cd.remaining(t0 + std::time::Duration::from_secs(5)), 0);
        // Pressing the shutter with a timer arms it once; a second press
        // during the count does nothing.
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut cam = shell(d.path(), 0);
        cam.timer_secs = 3;
        cam.press_shutter(&ctx);
        let first = cam.countdown;
        assert!(first.is_some());
        cam.press_shutter(&ctx);
        assert_eq!(cam.countdown, first, "a second press did not restart the timer");
        // With no live camera the shutter refuses when the timer ends —
        // and says so rather than pretending.
        cam.countdown = Some(Countdown { started: t0 - std::time::Duration::from_secs(10), secs: 3 });
        cam.tick_countdown(&ctx);
        assert!(cam.countdown.is_none());
        assert!(cam.note.contains("not live"), "{}", cam.note);
    }

    #[test]
    fn a_swipe_pages_only_past_the_threshold() {
        assert_eq!(swipe_commits(-10.0, 400.0), 0);
        assert_eq!(swipe_commits(-70.0, 400.0), 1);
        assert_eq!(swipe_commits(70.0, 400.0), -1);
    }

    /// Frames of input through ONE context, so the viewfinder texture, the
    /// pointer and the touch bridge carry over. The last frame's shapes.
    fn drive(cam: &mut Cam, ctx: &egui::Context, size: egui::Vec2, frames: Vec<(f64, Vec<egui::Event>)>) -> Vec<egui::epaint::ClippedShape> {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let mut last = Vec::new();
        for (t, events) in frames {
            #[allow(deprecated)]
            let out = ctx.run(egui::RawInput { screen_rect: Some(screen), time: Some(t), events, ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| cam.ui(ui));
            });
            last = out.shapes;
        }
        last
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    #[test]
    fn a_pinch_over_the_picture_zooms_and_the_shutter_saves_the_window_at_native_pixels() {
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let size = egui::vec2(360.0, 640.0);
        let camera = Camera::open_with(Facing::Front, true);
        let mut cam = Cam::with_parts(&ctx, Roll::default(), vec![], camera, d.path().to_path_buf(),
                                      d.path().join("outbox"), d.path().join("Pictures"));
        // The pattern's first frame has to reach the viewfinder.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut t = 0.0;
        while cam.frame.is_none() && std::time::Instant::now() < deadline {
            drive(&mut cam, &ctx, size, vec![(t, vec![])]);
            t += 0.033;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(cam.frame.is_some(), "no frame from the test pattern");
        let pic = cam.picture_rect;
        assert!(pic.width() > 100.0 && pic.height() > 50.0, "{pic:?}");

        // A trackpad pinch / ctrl+wheel (egui's Zoom event) with the pointer
        // over the picture's right third, upper part.
        let at = egui::pos2(pic.left() + pic.width() * 0.75, pic.top() + pic.height() * 0.25);
        let shapes = drive(&mut cam, &ctx, size, vec![
            (t, vec![egui::Event::PointerMoved(at)]),
            (t + 0.016, vec![egui::Event::Zoom(2.0)]),
            (t + 0.032, vec![]),
        ]);
        assert!((cam.zoom.factor - 2.0).abs() < 1e-3, "factor {}", cam.zoom.factor);
        // The front preview is a mirror: the screen's right is the frame's
        // LEFT, so the window sits on the frame's left half, upper part.
        let w = cam.zoom.window();
        assert!((w[0] - 0.125).abs() < 0.01 && (w[1] - 0.125).abs() < 0.01, "window {w:?}");
        let texts = uikit_egui::layout_probe::texts(&shapes);
        assert!(texts.iter().any(|g| g.text == "2.0×"), "the chip says the magnification");

        // The shutter saves THAT window at native pixels: half the pattern. (Raw path: auto tone off.)
        cam.pro.auto_tone = false;
        cam.take_photo(&ctx);
        let shot = cam.roll.shots()[0].clone();
        assert_eq!((shot.width, shot.height), (crate::camera::PATTERN_WIDTH / 2, crate::camera::PATTERN_HEIGHT / 2));
        let img = image::open(d.path().join(&shot.file)).expect("the JPEG opens");
        assert_eq!((img.width(), img.height()), (shot.width, shot.height));
        assert!(cam.note.contains("2.0× crop"), "{}", cam.note);

        // Keys: 0 resets, + steps in, − steps out.
        drive(&mut cam, &ctx, size, vec![(t + 0.1, vec![key(egui::Key::Num0)])]);
        assert!(!cam.zoom.is_zoomed());
        drive(&mut cam, &ctx, size, vec![(t + 0.2, vec![key(egui::Key::Plus)])]);
        assert!((cam.zoom.factor - uikit_egui::zoom::STEP).abs() < 1e-3);
        drive(&mut cam, &ctx, size, vec![(t + 0.3, vec![key(egui::Key::Minus)])]);
        assert!(!cam.zoom.is_zoomed());
        // A pinch over the roll, below the picture, is not the picture's.
        let below = egui::pos2(size.x * 0.5, size.y - 40.0);
        drive(&mut cam, &ctx, size, vec![
            (t + 0.4, vec![egui::Event::PointerMoved(below)]),
            (t + 0.42, vec![egui::Event::Zoom(2.0)]),
        ]);
        assert!(!cam.zoom.is_zoomed(), "a pinch over the list zoomed the picture");
        // At 1× the photo is the whole frame again.
        cam.take_photo(&ctx);
        let shot = cam.roll.shots()[0].clone();
        assert_eq!((shot.width, shot.height), (crate::camera::PATTERN_WIDTH, crate::camera::PATTERN_HEIGHT));
    }

    fn live_pattern(d: &std::path::Path, ctx: &egui::Context, size: egui::Vec2) -> (Cam, f64) {
        let camera = Camera::open_with(Facing::Rear, true);
        let mut cam = Cam::with_parts(ctx, Roll::default(), vec![], camera, d.to_path_buf(), d.join("outbox"), d.join("Pictures"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut t = 0.0;
        while cam.frame.is_none() && std::time::Instant::now() < deadline {
            drive(&mut cam, ctx, size, vec![(t, vec![])]);
            t += 0.033;
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(cam.frame.is_some(), "no frame from the test pattern");
        (cam, t)
    }

    /// Run frames until a NEW camera frame has been uploaded.
    fn next_frame(cam: &mut Cam, ctx: &egui::Context, size: egui::Vec2, t: &mut f64) {
        let seen = cam.camera.drawn_seq;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while cam.camera.drawn_seq == seen && std::time::Instant::now() < deadline {
            *t += 0.033;
            drive(cam, ctx, size, vec![(*t, vec![])]);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn full_screen_gives_the_viewfinder_the_window_and_the_roll_a_page() {
        let d = tempfile::tempdir().unwrap();
        let size = egui::vec2(360.0, 720.0);
        let mut cam = shell(d.path(), 4);
        cam.layout = ViewMode::Auto; // a phone-sized window: auto is full
        cam.camera = Camera::unstarted(capture_kit::probe::Probe::default(), Facing::Front);
        let (r, texts) = run(&mut cam, size);
        assert!(r.overlaps.is_empty(), "{}", r.describe());
        let has = |t: &str| texts.iter().any(|g| g.text.contains(t));
        assert!(has("roll · 4"), "the roll is a button");
        assert!(!has("sending to Bee"), "and not listed under the picture");
        assert!(!has("snapshot-cam"), "no title bar over a full-screen camera");
        assert!(cam.picture_rect.height() > 0.0);
        // The viewfinder's budget is most of the window: the image rect is
        // letterboxed inside it, so measure the band it was given instead —
        // everything above the status row.
        // "no camera found" is drawn twice: in the middle of the picture,
        // and in the status row under it — the lower one.
        let status_y = texts.iter().filter(|g| g.text.contains("no camera found")).map(|g| g.rect.top()).fold(0.0, f32::max);
        assert!(status_y > size.y * 0.55, "viewfinder band ends at {status_y}");
        // The roll page.
        cam.roll_open = true;
        let (r, texts) = run(&mut cam, size);
        assert!(r.overlaps.is_empty(), "{}", r.describe());
        assert!(texts.iter().any(|g| g.text.contains("sending to Bee")));
        assert!(texts.iter().any(|g| g.text.contains("← camera")));
        // A desktop window on auto stays compact.
        let mut wide = shell(d.path(), 2);
        wide.layout = ViewMode::Auto;
        let (_, texts) = run(&mut wide, egui::vec2(1000.0, 700.0));
        assert!(texts.iter().any(|g| g.text.contains("snapshot-cam")), "compact on a desktop");
        // Full forced on a desktop.
        wide.layout = ViewMode::Full;
        let (r, texts) = run(&mut wide, egui::vec2(1000.0, 700.0));
        assert!(r.overlaps.is_empty(), "{}", r.describe());
        assert!(texts.iter().any(|g| g.text.contains("roll · 2")));
    }

    #[test]
    fn the_pro_panel_scrolls_over_a_full_screen_viewfinder_without_collisions() {
        let d = tempfile::tempdir().unwrap();
        let mut cam = shell(d.path(), 2);
        cam.layout = ViewMode::Full;
        cam.pro_open = true;
        let (r, texts) = run(&mut cam, egui::vec2(360.0, 720.0));
        assert!(r.overlaps.is_empty(), "{}", r.describe());
        // The panel scrolls: its first rows are on screen, the rest a
        // scroll away (egui does not emit what is clipped out).
        for want in ["lens", "EV", "zebras"] {
            assert!(texts.iter().any(|g| g.text.contains(want)), "`{want}` not drawn");
        }
    }

    #[test]
    fn the_pro_panel_lays_out_on_a_phone_and_a_desktop() {
        let d = tempfile::tempdir().unwrap();
        for size in [egui::vec2(360.0, 720.0), egui::vec2(1000.0, 760.0)] {
            let mut cam = shell(d.path(), 3);
            cam.pro_open = true;
            cam.pro.overlays = crate::pro::Overlays { histogram: true, zebra: true, peaking: true, grid: true };
            // The hardware rows too, from a captured listing.
            cam.hw.controls = Some(hwctl::Controls { device: "/dev/video0".into(), ctrls: hwctl::parse(
                "auto_exposure 0x009a0901 (menu) : min=0 max=3 default=3 value=1\n\
                 exposure_time_absolute 0x009a0902 (int) : min=3 max=2047 step=1 default=250 value=333\n\
                 gain 0x00980913 (int) : min=0 max=255 step=1 default=0 value=31\n\
                 white_balance_automatic 0x0098090c (bool) : default=1 value=1\n") });
            cam.hw.checked = Some(None);
            cam.pro.select_lens(cam.pro.lenses.iter().position(|l| l.name.contains("Macro")).unwrap());
            let (r, texts) = run(&mut cam, size);
            assert!(r.overlaps.is_empty(), "{size:?}: {}", r.describe());
            let has = |t: &str| texts.iter().any(|g| g.text.contains(t));
            for want in ["lens", "EV", "WB", "zebras", "peaking", "shutter 1/30 s", "gain 31", "take photo", "working distance",
                         "shots", "volume keys", "calibrate", "full screen"] {
                assert!(has(want), "{size:?}: `{want}` not drawn");
            }
        }
    }

    #[test]
    fn a_saved_photo_gets_its_black_point_back_and_the_switch_turns_that_off() {
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let size = egui::vec2(360.0, 720.0);
        let (mut cam, _t) = live_pattern(d.path(), &ctx, size);
        // a veiled, bright frame: every pixel lifted by about 5% linear, a grey ramp
        let (w, h) = (96u32, 64u32);
        let mut px = Vec::new();
        for _y in 0..h {
            for x in 0..w {
                let l = (x as f32 / w as f32) * 0.8 + 0.05;
                let v = crate::develop::to_srgb(l);
                px.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let frame = || capture_kit::RawFrame { width: w, height: h, captured_ms: 0, seq: 1, rgba: px.clone().into() };
        let darkest = |f: &str| image::open(d.path().join(f)).unwrap().to_luma8().as_raw().iter().copied().min().unwrap();
        cam.pro.auto_tone = false;
        cam.save_frame(&ctx, frame(), None, None);
        let raw = cam.roll.shots()[0].clone();
        cam.pro.auto_tone = true;
        cam.save_frame(&ctx, frame(), None, None);
        let toned = cam.roll.shots()[0].clone();
        assert_ne!(raw.file, toned.file);
        let (a, b) = (darkest(&raw.file), darkest(&toned.file));
        assert!(a >= 60, "the veiled frame is saved as it is: darkest {a}");
        assert!(b + 25 < a, "with auto tone its blacks come back: {a} -> {b}");
    }

    #[test]
    fn a_tap_on_the_picture_with_no_lens_motor_says_so_and_does_nothing_else() {
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let size = egui::vec2(360.0, 720.0);
        let (mut cam, mut t) = live_pattern(d.path(), &ctx, size);
        let at = cam.picture_rect.center();
        let tap = |t: f64| vec![
            (t, vec![egui::Event::PointerMoved(at)]),
            (t + 0.016, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }]),
            (t + 0.060, vec![egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() }]),
            (t + 0.120, vec![]),
        ];
        drive(&mut cam, &ctx, size, tap(t));
        t += 0.3;
        drive(&mut cam, &ctx, size, vec![(t, vec![])]);
        assert!(!cam.focus.is_searching(), "no motor: nothing is searching");
        assert!(cam.note.contains("focus"), "it said why: {}", cam.note);
        assert!(cam.zoom.factor == 1.0 && cam.roll.shots().is_empty(), "and the tap did nothing else");
    }

    #[test]
    fn fusion_lifts_a_dim_subject_in_a_saved_photo_and_is_off_by_default() {
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let size = egui::vec2(360.0, 720.0);
        let (mut cam, _t) = live_pattern(d.path(), &ctx, size);
        assert!(!cam.pro.fusion, "off by default");
        // dim textured left half, bright textured right half
        let (w, h) = (96u32, 64u32);
        let mut px = Vec::new();
        for y in 0..h { for x in 0..w {
            let tex = if (x / 2 + y / 2) % 2 == 0 { 1.0 } else { 0.7 };
            let l = if x < w / 2 { 0.03 * tex } else { (0.5f32 * tex).min(1.0) };
            let v = crate::develop::to_srgb(l);
            px.extend_from_slice(&[v, v, v, 255]);
        } }
        let frame = || capture_kit::RawFrame { width: w, height: h, captured_ms: 0, seq: 1, rgba: px.clone().into() };
        let left_mean = |f: &str| {
            let img = image::open(d.path().join(f)).unwrap().to_luma8();
            let mut s = 0u64; let mut n = 0u64;
            for y in 0..h { for x in 4..(w / 2 - 4) { s += img.get_pixel(x, y).0[0] as u64; n += 1; } }
            s as f64 / n as f64
        };
        cam.save_frame(&ctx, frame(), None, None);
        let plain = cam.roll.shots()[0].clone();
        cam.pro.fusion = true;
        cam.save_frame(&ctx, frame(), None, None);
        let fused = cam.roll.shots()[0].clone();
        assert!(left_mean(&fused.file) > left_mean(&plain.file) + 4.0, "the dim half came up: {} -> {}", left_mean(&plain.file), left_mean(&fused.file));
        assert!(cam.note.contains("scene:"), "the note names the scene: {}", cam.note);
        assert!(cam.pro.conf().contains("fusion = true"));
    }

    #[test]
    fn an_anamorphic_lens_with_ev_saves_a_desqueezed_brighter_photo() {
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let size = egui::vec2(360.0, 720.0);
        let (mut cam, mut t) = live_pattern(d.path(), &ctx, size);
        // The plain shot first, for comparison.
        cam.take_photo(&ctx);
        let plain = cam.roll.shots()[0].clone();
        assert_eq!((plain.width, plain.height), (crate::camera::PATTERN_WIDTH, crate::camera::PATTERN_HEIGHT));
        cam.pro.lens = cam.pro.lenses.iter().position(|l| l.name == "Moment Anamorphic 1.33x").unwrap();
        cam.pro.nudge_ev(3);
        cam.pro.overlays.zebra = true;
        cam.pro.overlays.histogram = true;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !(cam.developed && cam.measured.as_ref().is_some_and(|m| m.analysis.overlay.is_some())) && std::time::Instant::now() < deadline {
            next_frame(&mut cam, &ctx, size, &mut t);
        }
        assert!(cam.developed, "the preview went through develop");
        assert!(cam.overlay.is_some(), "the zebra layer was uploaded");
        let a = &cam.measured.as_ref().unwrap().analysis;
        assert!(a.hist[0].iter().sum::<u32>() > 0);
        // The preview is the desqueezed shape.
        assert!((cam.preview.sensor_aspect - 640.0 * 1.33 / 360.0).abs() < 0.05, "{}", cam.preview.sensor_aspect);
        let shapes = drive(&mut cam, &ctx, size, vec![(t + 0.05, vec![])]);
        let texts = uikit_egui::layout_probe::texts(&shapes);
        assert!(texts.iter().any(|g| g.text.contains("EV +1") && g.text.contains("Anamorphic")), "the readout is drawn");
        cam.take_photo(&ctx);
        let shot = cam.roll.shots()[0].clone();
        assert_eq!((shot.width, shot.height), ((640.0f32 * 1.33).round() as u32, 360), "desqueezed at native height");
        let mean = |f: &str| {
            let img = image::open(d.path().join(f)).unwrap().to_luma8();
            img.as_raw().iter().map(|v| *v as u64).sum::<u64>() as f64 / img.as_raw().len() as f64
        };
        assert!(mean(&shot.file) > mean(&plain.file) + 10.0, "+1 EV is brighter: {} vs {}", mean(&shot.file), mean(&plain.file));
        assert!(cam.note.contains("EV +1"), "{}", cam.note);
    }

    #[test]
    fn a_trigger_primes_a_burst_merges_and_a_wb_bracket_writes_three() {
        let d = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let size = egui::vec2(360.0, 720.0);
        let (mut cam, mut t) = live_pattern(d.path(), &ctx, size);
        let until = |cam: &mut Cam, t: &mut f64, n: usize, what: &str| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while cam.roll.len() < n {
                assert!(std::time::Instant::now() < deadline, "{what}: timed out ({})", cam.note);
                *t += 0.033;
                drive(cam, &ctx, size, vec![(*t, vec![])]);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        // A shake primes; nothing is saved until the second is up.
        cam.trigger(&ctx, crate::triggers::Trigger::Shake);
        assert!(cam.flow.primed.is_some());
        let shapes = drive(&mut cam, &ctx, size, vec![(t + 0.01, vec![])]);
        assert!(uikit_egui::layout_probe::texts(&shapes).iter().any(|g| g.text.contains("holding for the sharpest frame")));
        assert_eq!(cam.roll.len(), 0);
        until(&mut cam, &mut t, 1, "primed");
        assert!(cam.note.contains("shake, sharpest of"), "{}", cam.note);
        // Burst merge, through the shutter.
        cam.flow.mode = crate::bracket::Mode::Burst;
        cam.press_shutter(&ctx);
        assert!(cam.flow.sequence.is_some());
        until(&mut cam, &mut t, 2, "burst");
        assert!(cam.note.contains("burst merge of 6"), "{}", cam.note);
        // WB bracket: one frame, three files, three different colours.
        cam.flow.mode = crate::bracket::Mode::WbBracket;
        cam.press_shutter(&ctx);
        until(&mut cam, &mut t, 5, "wb bracket");
        let blue = |f: &str| {
            let img = image::open(d.path().join(f)).unwrap().to_rgb8();
            img.pixels().map(|p| p[2] as f64 - p[0] as f64).sum::<f64>() / (img.width() * img.height()) as f64
        };
        let files: Vec<String> = cam.roll.shots()[..3].iter().map(|s| s.file.clone()).collect();
        let (b0, b2) = (blue(&files[2]), blue(&files[0]));
        assert!(b0 > b2 + 3.0, "the 4500K variant is bluer than the 6500K one: {b0} vs {b2}");
        // HDR without camera controls is refused, and says why.
        cam.flow.mode = crate::bracket::Mode::Hdr;
        cam.press_shutter(&ctx);
        assert!(cam.flow.sequence.is_none() && cam.note.contains("exposure control"), "{}", cam.note);
    }

    #[test]
    fn the_preview_render_fits_a_frame_budget() {
        // 1280×720 in, the preview's largest size out. Debug builds are
        // slow; this bounds the release-irrelevant worst case loosely and
        // prints the number so a regression is visible.
        let src = vec![128u8; 1280 * 720 * 4];
        let mut p = crate::pro::Pro::default();
        p.lens = p.lenses.iter().position(|l| l.name == "Moment Anamorphic 1.33x").unwrap();
        p.profile_mut().distortion = [0.05, 0.0];
        p.profile_mut().vignetting = [-0.3, 0.0, 0.0];
        let params = p.params([0.0, 0.0, 1.0, 1.0], 1280, 720, Some((PREVIEW_MAX, PREVIEW_MAX)));
        let t0 = std::time::Instant::now();
        let out = develop::render(&src, 1280, 720, &params);
        let a = develop::analyse(&out, params.out_w, params.out_h, true, true);
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        eprintln!("preview render+analyse {}×{}: {ms:.1} ms", params.out_w, params.out_h);
        assert!(a.overlay.is_some());
        assert!(ms < 400.0, "{ms} ms");
    }

    #[test]
    fn the_first_frame_decodes_one_old_thumbnail_and_the_rest_follow() {
        // A roll of many photos from before this launch: the first frame
        // must not decode them all.
        let d = tempfile::tempdir().unwrap();
        let mut cam = shell(d.path(), 6);
        run(&mut cam, egui::vec2(360.0, 900.0));
        // Three frames ran in `run`: at most three decodes, and at least one.
        assert!(cam.thumbs.len() >= 1 && cam.thumbs.len() <= 3 * THUMB_DECODES_PER_FRAME, "{}", cam.thumbs.len());
    }
}
