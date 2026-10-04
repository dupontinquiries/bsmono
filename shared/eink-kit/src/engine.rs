//! The headless core ([`Engine`]) and the loop around it ([`run`]).
//!
//! `Engine::frame` is one full turn: egui → tessellate → raster into the
//! display buffer → damage → policy → commit. It takes time as an argument
//! and a `&mut dyn Present`, so tests drive it with the in-memory backend
//! and a fake clock. `run` only adds sleeping, input draining and backend
//! selection; there is no logic in it worth testing separately.

use crate::config::Config;
use crate::damage::coalesce;
use crate::motion::{take_hints, Heat, Hint, BUDGET_KEY};
use crate::raster::{DirtyTiles, TILE};
use crate::format::PixelFormat;
use crate::geom::IRect;
use crate::policy::{Policy, Profile, Update};
use crate::present::{Caps, Input, InputHub, Present};
use crate::raster::{render, Target, Textures};
use crate::touch::{TouchEvent, TouchToEgui};
use egui::{Context, Event, RawInput, Rect, ViewportId, ViewportInfo};
use std::time::{Duration, Instant};

/// What an app implements: egui 0.34's Ui-first shape, i.e.
/// `eframe::App::ui` minus the frame. The monorepo has apps in both 0.34
/// shapes; `update(&Context)`-style ones wrap in [`FromCtx`].
pub trait EinkApp {
    fn ui(&mut self, ui: &mut egui::Ui);
}

impl<F: FnMut(&mut egui::Ui)> EinkApp for F {
    fn ui(&mut self, ui: &mut egui::Ui) {
        self(ui)
    }
}

/// Adapter for apps written against `update(&mut self, ctx: &Context, ..)`.
pub struct FromCtx<F: FnMut(&Context)>(pub F);

impl<F: FnMut(&Context)> EinkApp for FromCtx<F> {
    fn ui(&mut self, ui: &mut egui::Ui) {
        (self.0)(ui.ctx())
    }
}

/// Engine state the app may read (via `ctx.data`) for diagnostics.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    pub backend: &'static str,
    pub profile: &'static str,
    pub last_raw: Option<(i32, i32)>,
    pub last_touch: Option<(f32, f32)>,
    pub frames: u64,
    pub last_raster_ms: f32,
    pub last_dirty_tiles: usize,
    pub full_refresh_requested: bool,
    /// Tiles currently classified as motion.
    pub motion_tiles: usize,
    /// Seconds between motion refreshes.
    pub motion_interval: f32,
    /// The panel shows colour (for shells that re-apply `style::apply`).
    pub color_panel: bool,
}

pub struct FrameReport {
    pub updates: Vec<Update>,
    pub dirty_tiles: usize,
    pub raster: Duration,
}

pub struct Engine {
    pub ctx: Context,
    textures: Textures,
    pub policy: Policy,
    touch: TouchToEgui,
    caps: Caps,
    ppp: f32,
    threads: usize,
    events: Vec<Event>,
    first: bool,
    repaint_at: Option<Duration>,
    last_input: Option<Duration>,
    min_frame: Duration,
    last_frame: Duration,
    trace: bool,
    diag: Diagnostics,
    heat: Heat,
    /// Hinted tiles from the last frame the app drew (hints are per-frame).
    hint_motion: Vec<bool>,
    hint_static: Vec<bool>,
    dither_enabled: bool,
    was_moving: bool,
    dithered: bool,
}

impl Engine {
    pub fn new(cfg: &Config, caps: Caps, backend: &'static str) -> Engine {
        let profile = cfg.profile.unwrap_or(if caps.color_panel { Profile::FastColor } else { Profile::FastMono });
        let mut policy = Policy::new(profile, IRect::from_size(caps.width, caps.height), caps.per_update_waveform);
        policy.settle = cfg.settle;
        policy.full_every_pages = cfg.full_every_pages;
        let motion_color = cfg.motion_color;
        let hz = if motion_color { cfg.motion_color_hz } else { cfg.motion_hz }.max(0.2);
        policy.motion = crate::policy::MotionCfg { interval: Duration::from_secs_f32(1.0 / hz), color: motion_color };
        let (cols, rows) = ((caps.width as usize).div_ceil(TILE), (caps.height as usize).div_ceil(TILE));
        // Dither only mono motion, and never in the quality profile (whose
        // motion lane uses a quality waveform that can show greys).
        let dither_enabled = cfg.dither && !motion_color && profile != Profile::Quality;
        let ctx = Context::default();
        let feather = cfg.feather.unwrap_or(profile.feathering());
        ctx.tessellation_options_mut(|o| o.feathering = feather);
        // No animation: every animated frame is a panel refresh.
        let color_panel = caps.color_panel;
        let eink_theme = cfg.eink_theme;
        ctx.global_style_mut(|s| {
            if eink_theme {
                crate::style::apply(s, color_panel);
            }
            s.animation_time = 0.0;
        });
        ctx.options_mut(|o| o.input_options.max_click_duration = 0.8);
        eprintln!(
            "eink: backend={backend} {}x{} {:?} profile={} feather={feather} scale={} threads={} per_update_waveform={} motion={}Hz {} dither={}",
            caps.width, caps.height, caps.format, profile.as_str(), cfg.scale, cfg.threads, caps.per_update_waveform,
            if cfg.motion_color { cfg.motion_color_hz } else { cfg.motion_hz },
            if cfg.motion_color { "colour" } else { "mono" },
            cfg.dither && !cfg.motion_color && cfg.profile != Some(Profile::Quality)
        );
        Engine {
            ctx,
            textures: Textures::default(),
            policy,
            touch: TouchToEgui::default(),
            caps,
            ppp: cfg.scale,
            threads: cfg.threads,
            events: Vec::new(),
            first: true,
            repaint_at: Some(Duration::ZERO),
            last_input: None,
            min_frame: cfg.min_frame,
            last_frame: Duration::ZERO,
            trace: cfg.trace,
            diag: Diagnostics { backend, profile: profile.as_str(), motion_interval: 1.0 / hz, color_panel: caps.color_panel, ..Default::default() },
            heat: Heat::new(cols, rows),
            hint_motion: vec![false; cols * rows],
            hint_static: vec![false; cols * rows],
            dither_enabled,
            was_moving: false,
            dithered: false,
        }
    }

    fn interacting(&self, now: Duration) -> bool {
        self.touch.any_down() || self.last_input.is_some_and(|t| now.saturating_sub(t) < self.policy.settle)
    }

    pub fn push_input(&mut self, now: Duration, input: &Input) {
        match input {
            Input::Touch(t) => self.push_touch(now, t),
            Input::Raw { x, y } => self.diag.last_raw = Some((*x, *y)),
            Input::Closed(_) | Input::Wake => {}
        }
    }

    pub fn push_touch(&mut self, now: Duration, t: &TouchEvent) {
        if self.trace {
            eprintln!("eink: t={:>7.3}s touch {:?} id={} ({:.0},{:.0})", now.as_secs_f32(), t.phase, t.id, t.x, t.y);
        }
        self.touch.translate(t, self.ppp, now, &mut self.events);
        self.diag.last_touch = Some((t.x, t.y));
        self.last_input = Some(now);
        self.policy.on_input(now);
    }

    /// The backend's buffer was replaced: redraw every tile on the next frame, now.
    pub fn force_redraw(&mut self, now: Duration) {
        self.first = true;
        self.repaint_at = Some(now);
    }

    /// Whether any touch contact is considered down (tests, diagnostics).
    pub fn touch_debug_any_down(&self) -> bool {
        self.touch.any_down()
    }

    pub fn wants_frame(&self, now: Duration) -> bool {
        !self.events.is_empty() || self.repaint_at.is_some_and(|t| now >= t)
    }

    /// When the loop must wake even without input.
    pub fn next_wake(&self) -> Option<Duration> {
        // While something is moving, also wake when it will have gone quiet,
        // so the end of motion (and its colour settle) is noticed even if
        // nothing else ever wakes the loop.
        let motion_end = if self.was_moving { self.heat.quiet_deadline() } else { None };
        // A contact that never reports its release must still be let go.
        [self.repaint_at, self.policy.next_deadline(), motion_end, self.touch.next_expiry()].into_iter().flatten().min()
    }

    /// Which tiles count as motion right now.
    ///
    /// Measured motion is ignored while a finger is down: dragging a slider
    /// changes tiles fast, but that is interaction and belongs on the
    /// unthrottled fast path. Hinted Video/Animation regions are motion as
    /// soon as they change (no rate warm-up) and stop being motion when they
    /// go quiet, so a PAUSED video settles to colour even while the app keeps
    /// declaring the hint. Static hints veto.
    fn motion_mask(&self, now: Duration) -> Vec<bool> {
        let touching = self.touch.any_down();
        // A hinted region is judged as a whole: a video's dithered pixels can
        // sit still in one tile for a while even though the picture is
        // playing, and that tile must not drop out of the region.
        let hinted_live = (0..self.hint_motion.len()).any(|i| self.hint_motion[i] && self.heat.recently_changed(i, now));
        // Measured motion is dilated by one tile: a moving object enters its
        // neighbours next, and without this its leading edge is drawn by the
        // slow colour path for a few frames (a coloured sliver on a dithered
        // object). Hinted regions are exact and are not dilated.
        let (cols, rows) = (self.heat.cols, self.heat.rows);
        // Only unhinted measured motion grows: a declared video is exact, and
        // dilating it dithered its neighbours (a label above a video got
        // dithered during playback, then needed its own clean-up on pause).
        let measured = |c: usize, r: usize| {
            let i = r * cols + c;
            !touching && self.heat.is_moving(i) && !self.hint_motion[i]
        };
        (0..self.hint_motion.len())
            .map(|i| {
                let (c, r) = (i % cols, i / cols);
                let near = (r.saturating_sub(1)..=(r + 1).min(rows - 1))
                    .any(|rr| (c.saturating_sub(1)..=(c + 1).min(cols - 1)).any(|cc| measured(cc, rr)));
                !self.hint_static[i] && ((self.hint_motion[i] && hinted_live) || near)
            })
            .collect()
    }

    fn apply_hints(&mut self, hints: &[(egui::Rect, Hint)]) {
        let (cols, rows) = (self.heat.cols, self.heat.rows);
        self.hint_motion.iter_mut().for_each(|b| *b = false);
        self.hint_static.iter_mut().for_each(|b| *b = false);
        for (r, h) in hints {
            let px = |v: f32| (v * self.ppp) as i64;
            let (c0, c1) = ((px(r.min.x).max(0) as usize) / TILE, (px(r.max.x).max(1) as usize - 1) / TILE);
            let (r0, r1) = ((px(r.min.y).max(0) as usize) / TILE, (px(r.max.y).max(1) as usize - 1) / TILE);
            for row in r0..=r1.min(rows - 1) {
                for col in c0..=c1.min(cols - 1) {
                    match h {
                        Hint::Static => self.hint_static[row * cols + col] = true,
                        _ => self.hint_motion[row * cols + col] = true,
                    }
                }
            }
        }
    }

    /// Motion stopped since the last check: settle it.
    fn check_motion_end(&mut self, now: Duration) {
        let moving = self.motion_mask(now).iter().any(|m| *m);
        if self.was_moving && !moving {
            // Dithered pixels are re-rendered clean by the next frame, which
            // yields quality damage on its own; only undithered (colour)
            // motion needs an explicit settle of the region.
            self.policy.motion_ended(now, !self.dithered);
            if self.dithered {
                self.repaint_at = Some(now);
            }
            self.dithered = false;
            if self.trace {
                eprintln!("eink: t={:>7.3}s motion ended", now.as_secs_f32());
            }
        }
        self.was_moving = moving;
    }

    pub fn frame(&mut self, now: Duration, app: &mut dyn EinkApp, present: &mut dyn Present) -> std::io::Result<FrameReport> {
        if present.take_reset() {
            self.first = true; // a new buffer holds nothing we drew
        }
        let size = egui::vec2(self.caps.width as f32 / self.ppp, self.caps.height as f32 / self.ppp);
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(now.as_secs_f64()),
            events: std::mem::take(&mut self.events),
            focused: true,
            max_texture_side: Some(4096),
            ..Default::default()
        };
        raw.viewports.insert(ViewportId::ROOT, ViewportInfo { native_pixels_per_point: Some(self.ppp), ..Default::default() });
        self.diag.frames += 1;
        self.ctx.data_mut(|d| {
            d.insert_temp(egui::Id::NULL, self.diag.clone());
            d.insert_temp(egui::Id::new(BUDGET_KEY), self.policy.motion.interval.as_secs_f32());
        });
        let full = self.ctx.run_ui(raw, |ui| app.ui(ui));
        let hints = take_hints(&self.ctx);
        self.apply_hints(&hints);
        if self.ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new(FULL_REFRESH_KEY))).unwrap_or(false) {
            self.policy.request_full();
        }
        if self.ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new(PAGE_TURN_KEY))).unwrap_or(false) {
            self.policy.page_turn();
        }

        self.textures.apply_set(&full.textures_delta);
        let prims = self.ctx.tessellate(full.shapes, full.pixels_per_point);
        if let Some((px, py)) = probe_pixel() {
            debug_probe(&prims, full.pixels_per_point, px, py);
        }
        let t0 = Instant::now();
        let motion = self.motion_mask(now);
        let dither: Option<Vec<bool>> = self.dither_enabled.then(|| motion.clone());
        if dither.as_ref().is_some_and(|d| d.iter().any(|b| *b)) {
            self.dithered = true;
        }
        let (w, h, stride, format) = (self.caps.width, self.caps.height, self.caps.stride, self.caps.format);
        let dirty = render(
            &prims,
            full.pixels_per_point,
            &self.textures,
            Target { buf: present.buffer(), stride, width: w, height: h, format },
            [255, 255, 255, 255],
            self.threads,
            self.first,
            dither.as_deref(),
        );
        let raster = t0.elapsed();
        self.textures.apply_free(&full.textures_delta);
        self.first = false;

        // Split damage into the motion lane and the normal lane.
        let split = |keep: bool| DirtyTiles {
            dirty: dirty.dirty.iter().zip(&motion).map(|(d, m)| *d && (*m == keep)).collect(),
            ..dirty.clone()
        };
        let (moving_d, still_d) = (split(true), split(false));
        self.heat.observe(&dirty, now);
        let interacting = self.interacting(now);
        let mut updates = self.policy.on_damage(now, &coalesce(&still_d, 8), interacting);
        updates.extend(self.policy.on_motion_damage(now, &coalesce(&moving_d, 4)));
        self.diag.motion_tiles = motion.iter().filter(|m| **m).count();
        self.trace_updates(now, &updates);
        present.commit(&updates)?;
        if present.take_reset() {
            // The backend reconnected during this commit: redraw all, now.
            self.first = true;
            self.repaint_at = Some(now);
        }

        self.diag.last_raster_ms = raster.as_secs_f32() * 1000.0;
        self.diag.last_dirty_tiles = dirty.count();
        self.last_frame = now;
        let delay = full.viewport_output.get(&ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
        self.repaint_at = if delay == Duration::MAX { None } else { Some(now + delay.max(self.min_frame)) };
        // Frame pacing: while something is moving, do not render faster than
        // the motion lane can commit. A video decoder asking for 24 fps gets
        // ~6 frames rendered per second, and can read `motion_interval` to
        // stop decoding the rest.
        if self.diag.motion_tiles > 0 {
            if let Some(t) = self.repaint_at.as_mut() {
                // Snap to the first motion slot at or after the requested
                // time, so every rendered frame is one the panel will show.
                let (slot, step) = (self.policy.next_motion_at(), self.policy.motion.interval);
                if *t > slot && !step.is_zero() {
                    let k = ((*t - slot).as_secs_f64() / step.as_secs_f64()).ceil() as u32;
                    *t = slot + step * k;
                } else {
                    *t = slot.max(*t);
                }
            }
        }
        self.check_motion_end(now);
        Ok(FrameReport { updates, dirty_tiles: dirty.count(), raster })
    }

    /// Timers only: emits the policy's settle refresh when it is due.
    pub fn tick(&mut self, now: Duration, present: &mut dyn Present) -> std::io::Result<Vec<Update>> {
        // Release silent contacts (a resting palm; a release AppLoad never sent).
        if self.touch.expire(now, &mut self.events) && self.trace {
            eprintln!("eink: t={:>7.3}s released silent touch contact(s)", now.as_secs_f32());
        }
        self.heat.decay(now);
        self.check_motion_end(now);
        let interacting = self.touch.any_down();
        let u = self.policy.on_tick(now, interacting);
        self.trace_updates(now, &u);
        present.commit(&u)?;
        Ok(u)
    }

    fn trace_updates(&self, now: Duration, updates: &[Update]) {
        if self.trace {
            for u in updates {
                eprintln!("eink: t={:>7.3}s {:?} {:?} color={} [{}] raster={:.1}ms",
                    now.as_secs_f32(), u.waveform, u.rect, u.color, u.reason, self.diag.last_raster_ms);
            }
        }
    }
}

const FULL_REFRESH_KEY: &str = "eink.full_refresh";
/// Stable key: an app sets `true` here (egui temp memory) in the frame its
/// page changes. Written as a plain `bool` so apps need no eink-kit
/// dependency; inert on a desktop.
pub const PAGE_TURN_KEY: &str = "eink.page_turn";

/// Announce that this frame shows a new page (see `Policy::page_turn`).
pub fn page_turn(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(PAGE_TURN_KEY), true));
}

/// `EINK_PROBE_PIXEL=x,y`: print every triangle covering that pixel's
/// centre, in paint order, each frame. For "why is that pixel that colour"
/// on the device, where there is no debugger.
fn probe_pixel() -> Option<(f32, f32)> {
    let v = std::env::var("EINK_PROBE_PIXEL").ok()?;
    let (x, y) = v.split_once(',')?;
    Some((x.trim().parse::<f32>().ok()? + 0.5, y.trim().parse::<f32>().ok()? + 0.5))
}

fn debug_probe(prims: &[egui::epaint::ClippedPrimitive], ppp: f32, cx: f32, cy: f32) {
    for (i, p) in prims.iter().enumerate() {
        let egui::epaint::Primitive::Mesh(m) = &p.primitive else { continue };
        let clip = p.clip_rect;
        if !(clip.min.x * ppp <= cx && cx < clip.max.x * ppp && clip.min.y * ppp <= cy && cy < clip.max.y * ppp) {
            continue;
        }
        for (ti, t) in m.indices.chunks_exact(3).enumerate() {
            let q: Vec<_> = t.iter().map(|&k| &m.vertices[k as usize]).collect();
            let e = |a: &egui::epaint::Vertex, b: &egui::epaint::Vertex| {
                (b.pos.x - a.pos.x) * ppp * (cy - a.pos.y * ppp) - (b.pos.y - a.pos.y) * ppp * (cx - a.pos.x * ppp)
            };
            let (w0, w1, w2) = (e(q[1], q[2]), e(q[2], q[0]), e(q[0], q[1]));
            if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                eprintln!("eink probe ({cx},{cy}): prim {i} {:?} tri {ti}: {:?}", m.texture_id,
                    q.iter().map(|v| ((v.pos.x * ppp, v.pos.y * ppp), v.uv, v.color)).collect::<Vec<_>>());
            }
        }
    }
}

/// Read the engine's diagnostics from inside an app.
pub fn diagnostics(ctx: &Context) -> Diagnostics {
    ctx.data(|d| d.get_temp::<Diagnostics>(egui::Id::NULL)).unwrap_or_default()
}

/// Ask for a full-screen refresh (ghost clean-up) from inside an app.
pub fn request_full_refresh(ctx: &Context) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(FULL_REFRESH_KEY), true));
}

/// Pick and connect a backend per `cfg.backend`.
pub fn open_backend(cfg: &Config, hub: &InputHub) -> std::io::Result<Box<dyn Present>> {
    use crate::present::mem::MemPresent;
    let want = cfg.backend.as_str();
    if want == "mem" {
        let (w, h) = cfg.mem_size;
        let p = MemPresent::new(w, h, PixelFormat::Rgb888, true, true);
        return Ok(Box::new(match &cfg.dump {
            Some(d) => p.with_dump(d.into())?,
            None => p,
        }));
    }
    #[cfg(target_os = "linux")]
    {
        open_tablet_backend(cfg, hub, want)
    }
    // Off Linux there is no tablet to drive. Say so, rather than the backend
    // silently not existing.
    #[cfg(not(target_os = "linux"))]
    {
        let _ = hub;
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, format!(
            "EINK_BACKEND={want:?}: the tablet backends (qtfb, blight) need Linux -- a reMarkable. \
             On this platform only `mem` is available.")))
    }
}

/// The reMarkable backends: qtfb under AppLoad, else Blight.
#[cfg(target_os = "linux")]
fn open_tablet_backend(cfg: &Config, hub: &InputHub, want: &str) -> std::io::Result<Box<dyn Present>> {
    use crate::present::{blight, qtfb};
    let err = |m: String| std::io::Error::new(std::io::ErrorKind::NotFound, m);
    let (model, how) = crate::device::detect();
    eprintln!("eink: model {model:?} ({how})");
    let color = model.color();
    let fast_name = cfg.fast_waveform.clone().unwrap_or_else(|| if color { "animate".into() } else { "fast".into() });
    let qtfb_key = std::env::var("QTFB_KEY").ok().and_then(|k| k.parse::<i32>().ok());
    if want == "qtfb" || (want == "auto" && qtfb_key.is_some()) {
        let key = qtfb_key.ok_or_else(|| err("EINK_BACKEND=qtfb but no QTFB_KEY: launch from AppLoad".into()))?;
        let mode = qtfb::Mode::parse(&fast_name).ok_or_else(|| err(format!("EINK_FAST_WAVEFORM={fast_name:?} unknown")))?;
        let profile = cfg.profile.unwrap_or(if color { Profile::FastColor } else { Profile::FastMono });
        let mode = if profile == Profile::Quality { qtfb::Mode::Ui } else { mode };
        eprintln!("eink: qtfb window {key}");
        // Where touch and pen come from (EINK_INPUT, see Config::input): AppLoad forwards them at ~7 reports/s on the
        // Paper Pro though the digitizer reports at ~60 Hz, so `evdev` reads the digitizers directly instead.
        let (qtfb_route, direct) = match cfg.input.as_str() {
            "evdev" => (crate::present::Route::DropTouch, Some(crate::present::Route::Pass)),
            "compare" => (crate::present::Route::Pass, Some(crate::present::Route::LogTouch)),
            _ => (crate::present::Route::Pass, None),
        };
        eprintln!("eink: input = {} (AppLoad's forwarded touch is {})", cfg.input, if qtfb_route == crate::present::Route::DropTouch { "ignored" } else { "used" });
        let present = qtfb::QtfbPresent::connect(key, model, mode, hub.sender_with(qtfb_route))?;
        if let Some(route) = direct {
            let caps = present.caps();
            crate::present::evdev::start_direct(cfg.touch, cfg.pen, caps.width, caps.height, hub.sender_with(route));
        }
        return Ok(Box::new(present));
    }
    if want == "blight" || want == "auto" {
        let fast = blight::fast_waveform(&fast_name).ok_or_else(|| err(format!("EINK_FAST_WAVEFORM={fast_name:?} unknown")))?;
        eprintln!("eink: blight, fast waveform = {fast_name}");
        return match blight::BlightPresent::connect(model, fast, cfg.touch, cfg.pen, hub.sender()) {
            Ok(p) => Ok(Box::new(p)),
            Err(e) if want == "auto" => Err(err(format!(
                "no display backend: not launched from AppLoad (no QTFB_KEY), and Blight failed: {e}"))),
            Err(e) => Err(e),
        };
    }
    Err(err(format!("EINK_BACKEND={want:?}: want auto|qtfb|blight|mem")))
}

/// Run `app` until the display server goes away.
/// `YYYY-MM-DD HH:MM:SS UTC` from unix seconds (no date crate for one line).
fn utc(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // civil-from-days (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod launch_line_tests {
    #[test]
    fn utc_formats_known_instants() {
        assert_eq!(super::utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(super::utc(1_790_000_000), "2026-09-21 14:13:20 UTC"); // checked against Python
        assert_eq!(super::utc(951_782_400), "2000-02-29 00:00:00 UTC"); // leap day
    }
}

/// `EINK_LOG=<path>`: append stderr there. An app launched from AppLoad's
/// sidebar has no terminal, so without this its startup errors (no display
/// server, wrong model, bad knob) are invisible; gitmind sets it in the
/// manifest to the log `gitmind eink status` reads.
fn redirect_stderr() {
    let Ok(path) = std::env::var("EINK_LOG") else { return };
    let Ok(c) = std::ffi::CString::new(path) else { return };
    unsafe {
        let fd = libc::open(c.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND | libc::O_CLOEXEC, 0o644);
        if fd >= 0 {
            libc::dup2(fd, 2);
            libc::close(fd);
        }
    }
}

pub fn run(mut app: impl EinkApp) -> std::io::Result<()> {
    redirect_stderr();
    // One line per launch that identifies it: when, which process, and the
    // settings as the process actually received them (a log appended over
    // many launches was otherwise impossible to attribute: a manifest edit
    // and a cached launcher environment look identical without this).
    let since_epoch = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let seen: Vec<String> = std::env::vars().filter(|(k, _)| k.starts_with("EINK_") || k == "QTFB_KEY").map(|(k, v)| format!("{k}={v}")).collect();
    eprintln!("eink: ---- launch pid={} unix={since_epoch} ({}) {}", std::process::id(), utc(since_epoch), seen.join(" "));
    let cfg = Config::from_env().map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let hub = InputHub::new()?;
    let mut present = open_backend(&cfg, &hub).inspect_err(|e| eprintln!("eink: {e}"))?;
    let mut engine = Engine::new(&cfg, present.caps(), present.name());
    let t0 = Instant::now();
    loop {
        let now = t0.elapsed();
        if engine.wants_frame(now) {
            engine.frame(now, &mut app, present.as_mut())?;
        }
        engine.tick(t0.elapsed(), present.as_mut())?;
        let timeout = match engine.next_wake() {
            Some(t) => t.saturating_sub(t0.elapsed()).as_millis().min(i32::MAX as u128) as i32,
            None => -1,
        };
        if !engine.wants_frame(t0.elapsed()) {
            hub.wait(timeout);
        }
        let now = t0.elapsed();
        for ev in hub.drain() {
            if let Input::Closed(why) = &ev {
                eprintln!("eink: exiting: {why}");
                return Ok(());
            }
            engine.push_input(now, &ev);
        }
        // Backend housekeeping (qtfb reconnects here if AppLoad dropped us
        // after a mode request): a replaced buffer means redraw everything.
        match present.poll() {
            Ok(true) => engine.force_redraw(now),
            Ok(false) => {}
            Err(e) => {
                eprintln!("eink: exiting: {e}");
                return Ok(());
            }
        }
    }
}
