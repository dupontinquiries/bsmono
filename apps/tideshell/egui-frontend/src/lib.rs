//! tideshell-egui — paint and input over `tideshell-core`.
//!
//! [`ShellView::ui`] draws every shell surface into one `Ui` and returns the
//! engine's Actions for the caller to execute (see [`effects`]). Raw input is
//! routed before any widget sees it:
//!
//! * a touch that starts in an edge zone belongs to the edge recognizer for
//!   its whole sequence (what a layer-shell edge strip gets on the phone);
//! * a touch that starts on the trackpad pad belongs to the trackpad;
//! * everything else is left to ordinary egui widgets (tiles, cards, bar).
//!
//! In production each surface becomes its own layer surface; in the simulator
//! they share one window. The painting code does not care which.

pub mod effects;
#[cfg(feature = "wayland")]
pub mod stall;
pub mod wash;
pub mod wayland;
#[cfg(feature = "ei")]
pub mod ei;
#[cfg(feature = "layer")]
pub mod layer;

/// A key from the compositor (X keysym + the UTF-8 it produced) as egui
/// events: a Key event for keys egui names, plus a Text event for printable
/// input (not with Ctrl/Alt held, not control characters). Pure.
pub fn key_events(pressed: bool, keysym: u32, utf8: &str, ctrl: bool, alt: bool, shift: bool, repeat: bool) -> Vec<egui::Event> {
    // X11 keysym values (xkbcommon's keysyms.rs), spelled out so this stays
    // testable without the layer feature.
    const BACKSPACE: u32 = 0xff08;
    const TAB: u32 = 0xff09;
    const RETURN: u32 = 0xff0d;
    const ESCAPE: u32 = 0xff1b;
    const HOME: u32 = 0xff50;
    const LEFT: u32 = 0xff51;
    const UP: u32 = 0xff52;
    const RIGHT: u32 = 0xff53;
    const DOWN: u32 = 0xff54;
    const END: u32 = 0xff57;
    const DELETE: u32 = 0xffff;
    const KP_ENTER: u32 = 0xff8d;
    let modifiers = egui::Modifiers { alt, ctrl, shift, mac_cmd: false, command: ctrl };
    let key = match keysym {
        BACKSPACE => Some(egui::Key::Backspace),
        TAB => Some(egui::Key::Tab),
        RETURN | KP_ENTER => Some(egui::Key::Enter),
        ESCAPE => Some(egui::Key::Escape),
        HOME => Some(egui::Key::Home),
        END => Some(egui::Key::End),
        LEFT => Some(egui::Key::ArrowLeft),
        UP => Some(egui::Key::ArrowUp),
        RIGHT => Some(egui::Key::ArrowRight),
        DOWN => Some(egui::Key::ArrowDown),
        DELETE => Some(egui::Key::Delete),
        0x20 => Some(egui::Key::Space),
        // Letters and digits: from the keysym, which Latin keysyms share with
        // Unicode, so a RELEASE (no text) still names its key; utf8 alone
        // lost every release and egui kept the key "down" (found by a test).
        _ => match char::from_u32(keysym) {
            Some(c) if c.is_ascii_alphanumeric() => egui::Key::from_name(&c.to_ascii_uppercase().to_string()),
            _ => None,
        },
    };
    let mut out = Vec::new();
    if let Some(k) = key {
        out.push(egui::Event::Key { key: k, physical_key: None, pressed, repeat, modifiers });
    }
    if pressed && !ctrl && !alt && !utf8.is_empty() && !utf8.chars().any(|c| c.is_control()) {
        out.push(egui::Event::Text(utf8.to_string()));
    }
    out
}
#[cfg(feature = "sensors")]
pub mod sensors;
/// cpupower's scaling governor, over sysfs. Pure std, no feature gate needed.
pub mod appinfo_io;
pub mod quiet_osd;
pub mod cpu_governor;
/// Hardware keys -- power and volume -- from evdev, with the logind power-key
/// inhibitor. (Supersedes this branch's `power_key`, which handled power only;
/// screen power likewise lives in the layer runtime now, replacing the
/// separate-connection `output_power`.)
#[cfg(feature = "sensors")]
pub mod keys;
#[cfg(feature = "sensors")]
pub mod powerdoctor;
pub mod ctl;
pub mod blur;
pub mod camera;
pub mod damage;
pub mod devices;
pub mod fit;

use egui::{Align2, Color32, CornerRadius, FontId, Id, Pos2, Rect, Sense, Stroke, Vec2};
use std::collections::HashMap;
use tideshell_core::bar::{self, BarWidget};
use tideshell_core::control_center::{QuickKind, SliderKind, Stage};
use tideshell_core::prefs::{Layout, ShellPrefs};
use tideshell_core::switcher::Style;
use tideshell_core::{Action, Shell, TouchEvent};
use uikit_egui::PaletteColors;

/// An app the bar can show: a hatch-core desktop entry, reduced.
#[derive(Debug, Clone, PartialEq)]
pub struct AppTile {
    pub id: String,
    pub name: String,
}

/// Per-frame facts the view shows but does not own.
pub struct FrameInfo<'a> {
    /// hatch favorites (or top-ranked entries), for bar app widgets.
    pub apps: &'a [AppTile],
    /// Local time; the clock's style and fuzziness come from the prefs.
    pub time: tideshell_core::clock::LocalTime,
    pub battery: Option<f32>,
    pub charging: bool,
    /// Why the trackpad cannot work right now (EI failed), shown on the pad.
    pub trackpad_note: Option<&'a str>,
    /// Seconds since the shell started, for the breathing.
    pub t_secs: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Edge,
    Pad,
}

/// How long a finger rests on a quick tile before it counts as a long press.
const TILE_HOLD_S: f64 = 0.55;

#[derive(Default)]
struct SettingsSheet {
    open: bool,
    draft: Option<ShellPrefs>,
    pick: usize,
    notes: Vec<String>,
}

#[derive(Default)]
pub struct ShellView {
    /// When anyone last interacted (ms on the frame clock): a touch, the
    /// cover appearing, a wake. The breathing settles after a while without
    /// one (season::breath_envelope), so an untouched screen stops repainting.
    last_activity_ms: u64,
    /// Inside sleeping hours (copied from the shell each frame): dim and still.
    night: bool,
    pub show_trace: bool,
    /// Whether the mouse may drive the trackpad pad. Off by default: when the
    /// pad's output is the real pointer (EI), a mouse on the pad moves the
    /// pointer it is being read from, and every EI move becomes more drag —
    /// a feedback loop, seen live in nested cosmic-comp. Only the
    /// simulator's fake cursor turns this on.
    pub pointer_pad: bool,
    /// The bar and the carousel peeks are drawn on their own surfaces
    /// (`bar_ui`, `peek_ui`), not in the overlay. The layer runtime sets
    /// this; the simulator keeps it off and draws them in-window.
    pub bar_surface: bool,
    owners: Vec<(i32, Owner)>,
    touch_seen: bool,
    mouse_down: bool,
    card_drag: f32,
    /// Cover drag in progress: (slot, start y).
    cover_drag: Option<(i32, f32)>,
    /// The palette's search field has been given focus for this opening.
    palette_focused: bool,
    /// The camera pane's latest frame as a texture, with its size.
    camera_tex: Option<(egui::TextureHandle, usize, usize)>,
    /// How the sensor is mounted relative to the screen. A property of the
    /// hardware, so it is set once rather than guessed per frame.
    pub camera_rot: crate::fit::Quarter,
    /// Flip the preview horizontally (front cameras).
    pub camera_mirror: bool,
    /// The colour wash: a coarse grid of cell colours, drawn as a lattice of
    /// gradient quads instead of a stretched texture. 60 bytes a window
    /// rather than a 5 kB upload.
    glass: Option<(Vec<[u8; 3]>, usize, usize)>,
    /// A/B state for the lock-screen night wash: when the cover was last drawn and how many sessions so far.
    ab: std::cell::Cell<(Option<std::time::Instant>, u32)>,
    /// Grain, uploaded once. Per-frame noise would put back exactly the data
    /// movement this avoids, and would shimmer.
    grain: Option<egui::TextureHandle>,
    /// Whether a full-screen fill is cheap here. False on llvmpipe, where
    /// asking for 90 frames a second of a slow fade is pure heat.
    pub accelerated: bool,
    bar_drag: f32,
    sheet: SettingsSheet,
    /// Press feedback per widget: a spring on its scale (0.94 pressed, 1.0
    /// released with a jiggle). Ticked every frame; settled ones are dropped.
    press: HashMap<Id, tideshell_core::anim::Spring>,
    last_frame_ms: Option<u64>,
    /// Decides when the living palette is written for other apps: a visible
    /// change, at most twice a second, a light/dark flip at once.
    palette_pub: ambient_palette::Publisher,
    /// This frame's dt for the springs (set at the top of `ui`).
    frame_dt: f32,
    /// Where each palette result was last drawn, by key (see `palette_sheet`).
    palette_anim: std::collections::HashMap<String, Rect>,
    /// The quick tile a finger is holding and since when (egui time), and the tile whose hold
    /// already fired, so the release that follows is not also a tap.
    /// Live CPU / memory / age of the windows the carousel is showing (appinfo_io.rs).
    appinfo: crate::appinfo_io::Poller,
    /// True black for an OLED panel (uikit's `oled_enabled`), read once: the ground of every surface
    /// tideshell draws and publishes goes to (0, 0, 0) instead of a lit dark grey.
    oled: Option<bool>,
    /// The compositor blurs behind the overlay (set by the runtime each frame): the switcher's backdrop is
    /// then a light veil over the blurred apps instead of an almost opaque sheet that hides them.
    pub backdrop_blur: bool,
    tile_hold: Option<(Id, f64)>,
    tile_fired: Option<Id>,
    /// The switcher landed (reveal reached 1): kick the cards once.
    switcher_landed: bool,
    /// The mood's spring parameters for this frame.
    spring_omega: f32,
    spring_zeta: f32,
}

const MOUSE_SLOT: i32 = -1;

fn cr(r: u8) -> CornerRadius {
    CornerRadius::same(r)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgba_unmultiplied(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()), l(a.a(), b.a()))
}

/// Heights, in points, of the parts of a carousel peek strip.
const PEEK_INITIAL_H: f32 = 52.0;
const PEEK_CHEVRON_H: f32 = 44.0;
const PEEK_STATS_H: f32 = 48.0;
/// A turned title shorter than this is not worth drawing.
const PEEK_MIN_TITLE: f32 = 40.0;
/// Stats are only shown when the title still keeps at least this much.
const PEEK_TITLE_KEEP: f32 = 80.0;

/// How a peek strip of height `h` is shared out. The strip is narrow and, in landscape, short, so the parts
/// are given room in priority order and nothing is drawn where it would overlap another: the initial and the
/// chevron always, then the title, and the live numbers only if the title keeps `PEEK_TITLE_KEEP` after them.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PeekLayout {
    /// Height left for the (turned) title.
    title_px: f32,
    stats: bool,
}

fn peek_layout(h: f32, has_stats: bool) -> PeekLayout {
    let fixed = PEEK_INITIAL_H + PEEK_CHEVRON_H;
    let with_stats = h - fixed - PEEK_STATS_H;
    if has_stats && with_stats >= PEEK_TITLE_KEEP {
        PeekLayout { title_px: with_stats, stats: true }
    } else {
        PeekLayout { title_px: (h - fixed).max(0.0), stats: false }
    }
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Stable, distinct colour per app id, for letter tiles and fake thumbnails.
pub fn app_color(app_id: &str) -> Color32 {
    let h = app_id.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    let hue = (h % 360) as f32 / 360.0;
    egui::ecolor::Hsva::new(hue, 0.45, 0.62, 1.0).into()
}

fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into())
}

impl ShellView {
    /// Geometry of the trackpad pad in screen-local coordinates.
    pub fn pad_rect(screen: Vec2, bar_w: f32) -> Rect {
        let h = (screen.y * 0.38).max(180.0);
        Rect::from_min_max(Pos2::new(12.0, screen.y - h - 20.0), Pos2::new(screen.x - bar_w - 12.0, screen.y - 20.0))
    }

    fn bar_width(shell: &Shell) -> f32 {
        match shell.prefs.layout {
            Layout::RightBar if shell.bar_expanded => shell.prefs.bar_width * 2.6,
            Layout::RightBar => shell.prefs.bar_width,
            Layout::PullDown => 0.0,
        }
    }

    /// Width the bar reserves (its exclusive zone). The simulator lays its
    /// fake windows out beside it, as maximized windows would be.
    pub fn reserved_right(shell: &Shell) -> f32 {
        match shell.prefs.layout {
            Layout::RightBar => shell.prefs.bar_width,
            Layout::PullDown => 0.0,
        }
    }

    /// Whether the fullscreen overlay surface has anything to show. When
    /// false the layer runtime unmaps it: a fullscreen transparent layer that
    /// stays mapped costs compositing and can block direct scanout.
    pub fn needs_overlay(&self, shell: &Shell) -> bool {
        shell.drawer.is_visible()
            || shell.switcher.is_visible()
            || shell.hold.is_some()
            || shell.palette.is_visible()
            || shell.swipe_up > 0.0
            || shell.peek_dx.abs() > 1.0
            || shell.modes.trackpad
            || (shell.carousel.is_active() && !self.bar_surface)
            || (shell.prefs.layout == Layout::RightBar && !self.bar_surface)
            || self.sheet.open
            || self.show_trace
            || shell.lock.is_locked()
            // A reduced white point is drawn as a veil on this layer, so the
            // overlay has to stay mapped even when nothing else wants it —
            // unmapping it is what would make the screen jump back to the
            // panel's floor.
            || tideshell_core::dimming::split(shell.drawer.brightness, shell.prefs.digital_dim).is_dimming()
            || shell.sleep.phase != tideshell_core::sleep::Phase::Awake
    }

    /// Where the overlay must take input, in screen-local logical px.
    /// `None` = the whole screen (a modal surface is open: drawer, switcher,
    /// settings sheet). `Some(rects)` = only these; taps elsewhere fall
    /// through to the apps underneath (trackpad pad, bar, or nothing at all
    /// for passive previews and the carousel dots).
    pub fn input_rects(&self, shell: &Shell) -> Option<Vec<Rect>> {
        if shell.drawer.is_visible() || shell.switcher.open || self.sheet.open || shell.hold.is_some() || shell.palette.open || shell.lock.is_locked() || shell.sleep.captures_input() {
            return None;
        }
        let size = Vec2::new(shell.screen.0, shell.screen.1);
        let mut v = Vec::new();
        if shell.modes.trackpad {
            v.push(Self::pad_rect(size, Self::bar_width(shell)));
        }
        if shell.prefs.layout == Layout::RightBar && !self.bar_surface {
            let bw = Self::bar_width(shell);
            v.push(Rect::from_min_size(Pos2::new(size.x - bw, 0.0), Vec2::new(bw, size.y)));
        }
        if shell.carousel.is_active() && !self.bar_surface {
            let pw = shell.peek_reserve();
            v.push(Rect::from_min_size(Pos2::ZERO, Vec2::new(pw, size.y)));
            v.push(Rect::from_min_size(Pos2::new(size.x - Self::reserved_right(shell) - pw, 0.0), Vec2::new(pw, size.y)));
        }
        Some(v)
    }

    /// Width the bar's surface should have right now (expanded or not), and
    /// the exclusive zone it reserves (always the collapsed width).
    pub fn bar_geometry(shell: &Shell) -> Option<(u32, i32)> {
        (shell.prefs.layout == Layout::RightBar).then(|| (Self::bar_width(shell).round() as u32, shell.prefs.bar_width.round() as i32))
    }

    /// The scale to draw a widget at, given whether it is pressed now: a
    /// quick squeeze in, a jiggle back. `id` keys the spring.
    fn press_scale(&mut self, id: Id, pressed: bool, dt_ms: f32) -> f32 {
        // Side surfaces draw before the overlay sets the mood: default then.
        let (omega, zeta) = if self.spring_omega > 0.0 { (self.spring_omega, self.spring_zeta) } else { (28.0, 0.4) };
        let s = self.press.entry(id).or_insert_with(|| tideshell_core::anim::Spring::new(1.0, omega, zeta));
        s.omega = omega;
        s.zeta = zeta;
        s.set_target(if pressed { 0.94 } else { 1.0 });
        s.tick(dt_ms);
        s.value
    }

    /// Long press on a quick tile. True once, when the finger has been down on `resp` for
    /// `TILE_HOLD_S`; the release after it is swallowed by `tile_tap`.
    fn tile_held(&mut self, ui: &egui::Ui, resp: &egui::Response) -> bool {
        let now = ui.input(|i| i.time);
        if resp.is_pointer_button_down_on() {
            match self.tile_hold {
                Some((id, t0)) if id == resp.id => {
                    if self.tile_fired != Some(id) && now - t0 >= TILE_HOLD_S {
                        self.tile_fired = Some(id);
                        return true;
                    }
                }
                _ => self.tile_hold = Some((resp.id, now)),
            }
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(80));
        } else {
            if matches!(self.tile_hold, Some((id, _)) if id == resp.id) {
                self.tile_hold = None;
            }
            if self.tile_fired == Some(resp.id) && !resp.clicked() {
                self.tile_fired = None; // dragged off: nothing left to swallow
            }
        }
        false
    }

    /// A tap on a quick tile, unless that press already became a long press.
    fn tile_tap(&mut self, resp: &egui::Response) -> bool {
        if !resp.clicked() {
            return false;
        }
        if self.tile_fired == Some(resp.id) {
            self.tile_fired = None;
            return false;
        }
        true
    }

    /// Draw `r` scaled about its centre for the press feedback of `id`.
    fn pressed_rect(&mut self, ui: &egui::Ui, id: Id, r: Rect, dt_ms: f32) -> Rect {
        let pressed = ui.input(|i| i.pointer.primary_down()) && ui.rect_contains_pointer(r);
        let s = self.press_scale(id, pressed, dt_ms);
        Rect::from_center_size(r.center(), r.size() * s)
    }

    /// Any press spring still moving (keeps the frame loop awake).
    pub fn is_jiggling(&self) -> bool {
        self.press.values().any(|s| !s.is_settled())
    }

    /// Apply a mood: the accent replaces the theme's blue (softened toward
    /// the surface for accent_soft), warmth tints the surfaces toward amber
    /// or a cool slate, brightness scales them. Sunlight contrast wins: the
    /// tint fades out as contrast rises, so nothing muddies the high-contrast look.
    fn living_palette(pal: &PaletteColors, m: &tideshell_core::season::Mood, accent: [u8; 3], contrast: f32) -> PaletteColors {
        let acc = Color32::from_rgb(accent[0], accent[1], accent[2]);
        let tint_amount = (1.0 - contrast) * 0.14 * m.warmth.abs();
        let tint = if m.warmth >= 0.0 { Color32::from_rgb(255, 180, 110) } else { Color32::from_rgb(150, 175, 215) };
        let surface = |c: Color32| {
            let t = mix(c, tint, tint_amount);
            let b = m.brightness;
            Color32::from_rgba_unmultiplied((t.r() as f32 * b) as u8, (t.g() as f32 * b) as u8, (t.b() as f32 * b) as u8, t.a())
        };
        PaletteColors {
            bg_base: surface(pal.bg_base),
            bg_card: surface(pal.bg_card),
            bg_raised: surface(pal.bg_raised),
            bg_hover: surface(pal.bg_hover),
            accent: mix(acc, pal.accent, contrast * 0.5),
            accent_soft: mix(mix(acc, pal.bg_raised, 0.45), pal.accent_soft, contrast * 0.5),
            ..*pal
        }
    }

    fn blend_palettes(a: &PaletteColors, b: &PaletteColors, t: f32) -> PaletteColors {
        let m = |x: Color32, y: Color32| mix(x, y, t);
        PaletteColors {
            bg_base: m(a.bg_base, b.bg_base),
            bg_card: m(a.bg_card, b.bg_card),
            bg_raised: m(a.bg_raised, b.bg_raised),
            bg_hover: m(a.bg_hover, b.bg_hover),
            text: m(a.text, b.text),
            text_weak: m(a.text_weak, b.text_weak),
            accent: m(a.accent, b.accent),
            accent_soft: m(a.accent_soft, b.accent_soft),
            border: m(a.border, b.border),
            ..*a
        }
    }

    /// The camera pane's newest frame (RGBA), uploaded as a texture.
    pub fn set_camera_frame(&mut self, ctx: &egui::Context, w: usize, h: usize, rgba: &[u8]) {
        let img = egui::ColorImage::from_rgba_unmultiplied([w, h], rgba);
        match &mut self.camera_tex {
            Some((t, tw, th)) if *tw == w && *th == h => t.set(img, egui::TextureOptions::LINEAR),
            _ => self.camera_tex = Some((ctx.load_texture("ts-camera", img, egui::TextureOptions::LINEAR), w, h)),
        }
    }

    pub fn clear_camera_frame(&mut self) {
        self.camera_tex = None;
        self.glass = None;
    }

    /// The wash for this window, from camera::glass.
    pub fn set_glass(&mut self, cells: Vec<[u8; 3]>, cols: usize, rows: usize) {
        self.glass = Some((cells, cols, rows));
    }

    /// The grain texture, made once on first use.
    fn grain(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        if let Some(t) = &self.grain {
            return t.clone();
        }
        const N: usize = 64;
        let a = crate::wash::noise_tile(N, 0x7715, 255);
        let px: Vec<egui::Color32> = a.iter().map(|v| egui::Color32::from_white_alpha(*v)).collect();
        let img = egui::ColorImage { size: [N, N], pixels: px, source_size: egui::Vec2::new(N as f32, N as f32) };
        let t = ctx.load_texture("ts-grain", img, egui::TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Nearest, wrap_mode: egui::TextureWrapMode::Repeat, mipmap_mode: None });
        self.grain = Some(t.clone());
        t
    }

    /// Draw one carousel peek filling this `Ui` (its own layer surface).
    pub fn peek_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell, side: tideshell_core::carousel::Side, pal: &PaletteColors) -> Vec<Action> {
        // One poll per frame for everything the carousel shows (cheap: it re-reads /proc at most
        // every two seconds, and clears itself when the carousel is closed).
        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
        let shown = [shell.carousel.peek(tideshell_core::carousel::Side::Left), shell.carousel.peek(tideshell_core::carousel::Side::Right), shell.carousel.current()];
        let wanted: Vec<(u64, String)> = shown
            .iter()
            .flatten()
            .filter_map(|id| shell.windows().iter().find(|w| w.id == *id).map(|w| (*id, w.app_id.clone())))
            .collect();
        self.appinfo.poll(now_ms, &wanted);
        let area = ui.max_rect();
        self.peek(ui, shell, side, pal, area)
    }

    /// A peek strip: the neighbour window's tile (app colour, initial,
    /// title down the strip) and a chevron. A tap anywhere cycles to it.
    fn peek(&mut self, ui: &mut egui::Ui, shell: &mut Shell, side: tideshell_core::carousel::Side, pal: &PaletteColors, r: Rect) -> Vec<Action> {
        use tideshell_core::carousel::Side;
        let p = ui.painter_at(r);
        p.rect_filled(r, cr(0), pal.bg_card);
        let Some(id) = shell.carousel.peek(side) else { return vec![] };
        let (title, app) = shell
            .windows()
            .iter()
            .find(|w| w.id == id)
            .map(|w| (w.title.clone(), w.app_id.clone()))
            .unwrap_or_default();
        let dt = self.frame_dt;
        let inset = self.pressed_rect(ui, Id::new(("ts-peek-j", side == Side::Left)), r.shrink2(Vec2::new(4.0, 40.0)), dt);
        p.rect_filled(inset, cr(10), with_alpha(app_color(&app), 0.9));
        p.text(inset.center_top() + Vec2::new(0.0, PEEK_INITIAL_H / 2.0), Align2::CENTER_CENTER, initial(&title), FontId::proportional(20.0), Color32::WHITE);
        // The strip's height is shared out in priority order (see `peek_layout`): initial, chevron, then the
        // title turned like a book spine, then the live numbers only if there is room left.
        let info = self.appinfo.info(id);
        let lay = peek_layout(inset.height(), info.is_some());
        if lay.title_px >= PEEK_MIN_TITLE {
            let font = FontId::proportional(13.0);
            let ink = with_alpha(Color32::WHITE, 0.9);
            // Shorten with an ellipsis until the turned text fits the room it was given.
            let mut text = title.clone();
            let mut galley = p.layout_no_wrap(text.clone(), font.clone(), ink);
            while galley.size().x > lay.title_px && text.chars().count() > 1 {
                text.pop();
                text = text.trim_end().to_string();
                galley = p.layout_no_wrap(format!("{text}\u{2026}"), font.clone(), ink);
            }
            // Rotated a quarter turn anticlockwise about its anchor: it reads upward, and its rows extend to
            // the right of the anchor, so the anchor is the bottom-left of the box it will fill.
            let anchor = Pos2::new(inset.center().x - galley.size().y / 2.0, inset.top() + PEEK_INITIAL_H + lay.title_px.min(galley.size().x));
            p.add(egui::epaint::TextShape::new(anchor, galley, ink).with_angle(-std::f32::consts::FRAC_PI_2));
        }
        // The app's live numbers, stacked above the chevron: CPU, memory, age.
        if let (Some(info), true) = (info, lay.stats) {
            let base = inset.center_bottom() - Vec2::new(0.0, PEEK_CHEVRON_H + PEEK_STATS_H - 7.0);
            for (i, text) in [tideshell_core::appinfo::fmt_cpu(info.cpu_pct), tideshell_core::appinfo::fmt_mem(info.rss_mb), tideshell_core::appinfo::fmt_age(info.uptime_s)].into_iter().enumerate() {
                p.text(base + Vec2::new(0.0, 14.0 * i as f32), Align2::CENTER_CENTER, text, FontId::monospace(10.0), with_alpha(Color32::WHITE, 0.85));
            }
        }
        let c = inset.center_bottom() - Vec2::new(0.0, 22.0);
        let d = if side == Side::Left { -1.0 } else { 1.0 };
        let s = Stroke::new(2.5, Color32::WHITE);
        p.line_segment([c + Vec2::new(-6.0 * d, -9.0), c + Vec2::new(6.0 * d, 0.0)], s);
        p.line_segment([c + Vec2::new(6.0 * d, 0.0), c + Vec2::new(-6.0 * d, 9.0)], s);
        let resp = ui.interact(r, Id::new(("ts-peek", side == Side::Left)), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.drag_stopped() {
            // Up / down along the strip steps the stack; mostly-sideways or tiny drags are not swipes.
            let d = resp.drag_delta();
            let total = ui.input(|i| i.pointer.press_origin()).zip(ui.input(|i| i.pointer.latest_pos())).map(|(a, b)| b - a).unwrap_or(d);
            if total.y.abs() > 48.0 && total.y.abs() > 1.5 * total.x.abs() {
                return shell.peek_swipe(total.y < 0.0);
            }
        }
        if resp.clicked() {
            return shell.peek_tap(side);
        }
        vec![]
    }

    /// Draw the bar filling this `Ui` (its own layer surface).
    pub fn bar_ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, info: &FrameInfo) -> Vec<Action> {
        let area = ui.max_rect();
        self.bar(ui, shell, pal, info, area.min, area.size())
    }

    /// The volume pad. The fill's width is the level; a horizontal line marks
    /// the voicing; the accent tints with it (cool = soft, warm = cutting).
    fn volume_pad(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, clip: Rect, r: Rect) -> Vec<Action> {
        let mut out = vec![];
        let pc = ui.painter_at(clip);
        let v = shell.drawer.volume;
        let tone = shell.prefs.audio_tone;
        pc.rect_filled(r, cr(16), pal.bg_raised);
        let tint = if tone >= 0.0 { pal.warning } else { pal.accent_soft };
        let fill = with_alpha(mix(pal.accent, tint, tone.abs() * 0.6), 0.45);
        pc.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width() * v, r.height())), cr(16), fill);
        // Voicing line: top = +1, middle = 0, bottom = -1, kept below the label.
        let (top, bottom) = (r.top() + 30.0, r.bottom() - 10.0);
        let mid = (top + bottom) / 2.0;
        let ly = mid - tone * (bottom - top) / 2.0;
        pc.line_segment([Pos2::new(r.left() + 10.0, ly), Pos2::new(r.right() - 10.0, ly)], Stroke::new(2.0, with_alpha(pal.text, 0.7)));
        pc.line_segment([Pos2::new(r.left() + 10.0, mid), Pos2::new(r.right() - 10.0, mid)], Stroke::new(1.0, with_alpha(pal.text_weak, 0.35)));
        let label = if tone > 0.15 { "cut through" } else if tone < -0.15 { "soft" } else { "flat" };
        pc.text(r.left_top() + Vec2::new(16.0, 14.0), Align2::LEFT_CENTER, format!("Volume  {:.0}%  ·  {label}", v * 100.0), FontId::proportional(13.0), pal.text);
        pc.text(r.right_top() + Vec2::new(-14.0, 14.0), Align2::RIGHT_CENTER, "loud place", FontId::proportional(10.0), pal.text_weak);
        pc.text(r.right_bottom() + Vec2::new(-14.0, -12.0), Align2::RIGHT_CENTER, "quiet room", FontId::proportional(10.0), pal.text_weak);
        let resp = ui.interact(clip, Id::new("ts-volume-pad"), Sense::click_and_drag());
        if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
            out.extend(shell.set_slider(SliderKind::Volume, ((pos.x - r.left()) / r.width()).clamp(0.0, 1.0)));
            let t = -((pos.y - mid) / ((bottom - top) / 2.0)).clamp(-1.0, 1.0);
            out.extend(shell.set_voicing(t));
        }
        out
    }

    /// The dark look pushed to its contrast limit: black ground, white text.
    fn dark_high_contrast_palette(pal: &PaletteColors) -> PaletteColors {
        PaletteColors {
            bg_base: Color32::BLACK,
            bg_card: Color32::from_gray(8),
            bg_raised: Color32::from_gray(40),
            bg_hover: Color32::from_gray(60),
            text: Color32::WHITE,
            text_weak: Color32::from_gray(200),
            accent: Color32::from_rgb(120, 190, 255),
            accent_soft: Color32::from_rgb(90, 140, 210),
            border: Color32::from_gray(140),
            ..*pal
        }
    }

    /// The light look at its softest (just past the flip): light grey ground,
    /// dark grey text, so the switch is not a jolt.
    fn light_soft_palette(pal: &PaletteColors) -> PaletteColors {
        PaletteColors {
            bg_base: Color32::from_gray(215),
            bg_card: Color32::from_gray(222),
            bg_raised: Color32::from_gray(200),
            bg_hover: Color32::from_gray(185),
            text: Color32::from_gray(35),
            text_weak: Color32::from_gray(80),
            accent: Color32::from_rgb(20, 90, 190),
            accent_soft: Color32::from_rgb(120, 160, 220),
            border: Color32::from_gray(130),
            ..*pal
        }
    }

    /// Sunlight look: opaque white surfaces, black text, a saturated accent.
    /// Translucent dark chrome is what disappears first outdoors on a panel
    /// that is not sunlight-readable; contrast is the lever the shell has.
    pub fn sunlight_palette(pal: &PaletteColors) -> PaletteColors {
        PaletteColors {
            bg_base: Color32::WHITE,
            bg_card: Color32::WHITE,
            bg_raised: Color32::from_gray(232),
            bg_hover: Color32::from_gray(215),
            text: Color32::BLACK,
            text_weak: Color32::from_gray(60),
            accent: Color32::from_rgb(0, 90, 200),
            accent_soft: Color32::from_rgb(150, 190, 240),
            border: Color32::from_gray(120),
            ..pal.clone()
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, info: &FrameInfo) -> Vec<Action> {
        // Anything moving on its own counts as someone being around: a wake
        // fade, the cover being dragged, a drawer settling.
        self.night = shell.night_window;
        if shell.sleep.black > 0.0 || !shell.lock.reveal.is_settled() || shell.drawer.is_animating() {
            self.last_activity_ms = (info.t_secs * 1000.0) as u64;
        }
        let now_ms = (ui.input(|i| i.time) * 1000.0) as u64;
        let dt_ms = self.last_frame_ms.map(|t| now_ms.saturating_sub(t) as f32).unwrap_or(16.0).min(100.0);
        self.last_frame_ms = Some(now_ms);
        self.frame_dt = dt_ms;
        self.press.retain(|_, s| !s.is_settled() || s.target < 1.0);
        // The grain is made once, here, because `cover` has only a painter.
        if self.grain.is_none() && (self.glass.is_some() || tideshell_core::dimming::split(shell.drawer.brightness, shell.prefs.digital_dim).is_dimming()) {
            let _ = self.grain(ui.ctx());
        }
        // Contrast rises with the light and never dips: below the polarity
        // flip the dark look gains contrast (darker ground, whiter text);
        // at the flip (the apps' hysteretic one) it becomes a soft light
        // look that then gains contrast toward white/black. (Blending dark
        // straight into light met in a muddy middle where text and ground
        // had the same luminance: seen in the simulator at 5 000 lux.)
        let contrast = shell.brightness_band().contrast;
        let blended;
        let pal = if contrast > 0.0 {
            blended = if shell.apps_light() {
                Self::blend_palettes(&Self::light_soft_palette(pal), &Self::sunlight_palette(pal), ((contrast - 0.3) / 0.7).clamp(0.0, 1.0))
            } else {
                Self::blend_palettes(pal, &Self::dark_high_contrast_palette(pal), (contrast / 0.6).clamp(0.0, 1.0))
            };
            &blended
        } else {
            pal
        };
        // The living design: the scene's accent (never blue at night), its
        // warmth on the surfaces, its brightness, and its springs.
        let mood = shell.mood();
        let living = Self::living_palette(pal, &mood, shell.accent(), contrast);
        let oled = *self.oled.get_or_insert_with(uikit_egui::theme_bridge::oled_enabled);
        let living = if oled { uikit_egui::theme_bridge::oled_black(&living) } else { living };
        // Share it: apps reading $XDG_RUNTIME_DIR/tideshell.palette dress to
        // match (deploy/AMBIENCE-API.md). The publisher throttles; this frame
        // usually writes nothing.
        if let Some(line) = self.palette_pub.offer(uikit_egui::theme_bridge::ambient::to_ambient(&living, shell.apps_light()), now_ms) {
            if let Some(path) = ambient_palette::runtime_path() {
                let _ = ambient_palette::write_atomic(&path, &line);
            }
        }
        let pal = &living;
        self.spring_omega = mood.omega;
        self.spring_zeta = mood.zeta;
        let area = ui.max_rect();
        let origin = area.min;
        let size = area.size();
        if (shell.screen.0 - size.x).abs() > 0.5 || (shell.screen.1 - size.y).abs() > 0.5 {
            shell.set_screen((size.x, size.y));
        }
        let now = (ui.input(|i| i.time) * 1000.0) as u64;
        let mut out = shell.tick(now);

        let p = ui.painter_at(area);
        let to = |x: f32, y: f32| origin + Vec2::new(x, y);

        // Bottom layer first: bar, then overlays above it.
        if shell.prefs.layout == Layout::RightBar && !self.bar_surface {
            out.extend(self.bar(ui, shell, pal, info, origin, size));
        }
        if shell.carousel.is_active() && !self.bar_surface {
            use tideshell_core::carousel::Side;
            let pw = shell.peek_reserve();
            let left = Rect::from_min_size(origin, Vec2::new(pw, size.y));
            let right = Rect::from_min_size(origin + Vec2::new(size.x - Self::reserved_right(shell) - pw, 0.0), Vec2::new(pw, size.y));
            out.extend(self.peek(ui, shell, Side::Left, pal, left));
            out.extend(self.peek(ui, shell, Side::Right, pal, right));
        }
        if shell.modes.trackpad {
            let r = Self::pad_rect(size, Self::bar_width(shell)).translate(origin.to_vec2());
            p.rect_filled(r, cr(22), with_alpha(pal.bg_raised, 0.92));
            p.text(r.center_top() + Vec2::new(0.0, 18.0), Align2::CENTER_CENTER, "Trackpad", FontId::proportional(13.0), pal.text_weak);
            if let Some(note) = info.trackpad_note {
                p.text(r.center(), Align2::CENTER_CENTER, note, FontId::proportional(12.0), with_alpha(pal.warning, 0.9));
            }
            let n = shell.trackpad.finger_count();
            if n > 0 {
                p.text(r.center(), Align2::CENTER_CENTER, format!("{n} finger{}", if n > 1 { "s" } else { "" }), FontId::proportional(15.0), pal.accent);
            }
        }
        self.previews(&p, shell, pal, origin, size);
        if shell.switcher.is_visible() {
            out.extend(self.switcher(ui, shell, pal, origin, size));
        }
        if shell.drawer.is_visible() {
            out.extend(self.drawer(ui, shell, pal, origin, size));
        }
        if shell.carousel.is_active() {
            let (i, n) = shell.carousel.position();
            let w = n as f32 * 14.0;
            for k in 0..n {
                let c = to(size.x / 2.0 - w / 2.0 + k as f32 * 14.0 + 7.0, 12.0);
                p.circle_filled(c, if k == i { 4.5 } else { 3.0 }, if k == i { pal.accent } else { pal.text_weak });
            }
        }
        if shell.palette.is_visible() {
            out.extend(self.palette_sheet(ui, shell, pal, origin, size));
        }
        if shell.settings_requested {
            shell.settings_requested = false;
            self.open_settings(shell);
        }
        // Home indicator: the affordance for every bottom-edge gesture. It
        // breathes: a slow widening and brightening at the mood's pace.
        let br = tideshell_core::season::breath(&mood, info.t_secs) * self.breath_envelope(info);
        let hw = 110.0 + shell.swipe_up * 40.0 + br * 22.0;
        let hy = size.y - 9.0 - shell.swipe_up * 30.0;
        p.rect_filled(Rect::from_center_size(to(size.x / 2.0 + shell.peek_dx * 0.3, hy), Vec2::new(hw, 5.0)), cr(3), with_alpha(pal.text, 0.6 + br * 0.35));

        if shell.lock.is_locked() {
            self.cover(&p, shell, info, origin, size);
        }
        // Screen fade to black (sleep), above everything.
        if shell.sleep.black > 0.0 {
            p.rect_filled(Rect::from_min_size(origin, size), cr(0), Color32::from_black_alpha((shell.sleep.black * 255.0) as u8));
        }
        // Bedside pulse: a dim, warm clock on black (OLED: black pixels are
        // off), moved every minute so no digit edge sits on the same pixels.
        if shell.night.is_pulsing() {
            let lt = info.time;
            let m = lt.hour as u16 * 60 + lt.minute as u16;
            let (dx, dy) = tideshell_core::nightclock::shift(m);
            let a = (tideshell_core::nightclock::NIGHT_ALPHA * 255.0) as u8;
            p.text(
                origin + Vec2::new(size.x / 2.0 + dx, size.y * 0.42 + dy),
                Align2::CENTER_CENTER,
                format!("{:02}:{:02}", lt.hour, lt.minute),
                FontId::proportional(64.0),
                Color32::from_rgba_unmultiplied(255, 176, 120, a),
            );
        }
        if let Some(prefs) = self.settings_sheet(ui.ctx(), shell) {
            out.extend(shell.set_prefs(prefs));
        }
        if self.show_trace {
            self.trace(&p, shell, pal, origin);
        }
        // Ask for the next frame at an interval rather than immediately.
        // The finger's own motion is never slowed — the hand can compare it
        // against the thing it is touching — but a fade or a breath can be,
        // and on a software rasterizer filling this many pixels that is the
        // difference between a warm phone and a cool one.
        use tideshell_core::paintrate::{interval_ms, Motion};
        let tracking = !shell.recognizer.is_idle() || shell.switcher.is_streaking() || self.is_jiggling();
        let transition = shell.drawer.is_animating()
            || shell.switcher.is_animating()
            || shell.fat.as_ref().is_some_and(|f| f.is_animating())
            || shell.sleep.is_animating()
            || (shell.palette.is_visible() && !shell.palette.reveal.is_settled())
            || (shell.lock.is_locked() && !shell.lock.reveal.is_settled());
        let motion = if tracking {
            Motion::Tracking
        } else if transition {
            Motion::Transition
        } else {
            Motion::Still
        };
        if let Some(ms) = interval_ms(motion, self.accelerated) {
            if ms == 0 {
                ui.ctx().request_repaint();
            } else {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(ms));
            }
        }
        // The reduced white point, last so it veils everything drawn above.
        // A black rectangle over the whole surface: the cheapest possible
        // way to scale every pixel toward black, and the only one available
        // to a client that does not own the compositor's colour pipeline.
        let dim = tideshell_core::dimming::split(shell.drawer.brightness, shell.prefs.digital_dim);
        if dim.is_dimming() {
            let a = (dim.veil_alpha() * 255.0).round().clamp(0.0, 255.0) as u8;
            ui.painter().rect_filled(ui.max_rect(), 0.0, egui::Color32::from_black_alpha(a));
        }
        // Grain LAST, after the veil. A veil multiplies everything toward
        // black, so dither applied before it is dither that has been dimmed
        // away — and the banding it exists to hide is worst exactly when the
        // veil is heaviest, because the darkest greys are where 8-bit steps
        // are widest relative to what the eye can see. The amount therefore
        // rises with the dimming rather than staying fixed.
        if let Some(grain) = self.grain.clone() {
            let r = ui.max_rect();
            let (tx, ty) = ((r.width() / 64.0).max(1.0), (r.height() / 64.0).max(1.0));
            let amount = 14.0 + 26.0 * dim.veil_alpha();
            ui.painter().image(grain.id(), r, Rect::from_min_max(Pos2::ZERO, Pos2::new(tx, ty)), Color32::from_white_alpha(amount as u8));
        }
        out
    }

    // ---- input ---------------------------------------------------------------

    fn owner(&self, slot: i32) -> Option<Owner> {
        self.owners.iter().find(|(s, _)| *s == slot).map(|(_, o)| *o)
    }

    /// Route raw input BEFORE egui sees it. A sequence that starts in an
    /// edge zone belongs to the recognizer, one that starts on the trackpad
    /// pad to the trackpad; their events (and the pointer events synthesized
    /// from an owned touch) are removed from `events`, so no widget under an
    /// edge swipe reacts to it. Everything else is left for egui.
    ///
    /// Routing used to happen inside the egui frame, after egui had already
    /// seen the events: in the right-bar layout a right-edge swipe also
    /// dragged the bar's brightness slider (seen live).
    ///
    /// Call once per frame with that frame's events: the simulator from
    /// eframe's `raw_input_hook`, the layer runtime before `run_ui`.
    pub fn route_raw(&mut self, shell: &mut Shell, events: &mut Vec<egui::Event>, origin: Pos2, now: u64) -> Vec<Action> {
        if !events.is_empty() {
            self.last_activity_ms = now;
        }
        if shell.is_covered() {
            // Pocket / face: nothing reaches anything (the sleep model still
            // sees touches, so the wake gate is its decision).
            let mut sink = std::mem::take(events);
            if shell.sleep.captures_input() {
                return self.route_covered(shell, &mut sink, origin, now);
            }
            return vec![];
        }
        if shell.sleep.captures_input() || shell.lock.is_locked() {
            return self.route_covered(shell, events, origin, now);
        }
        let mut out = vec![];
        let size = Vec2::new(shell.screen.0, shell.screen.1);
        let mut keep = Vec::with_capacity(events.len());
        for ev in events.drain(..) {
            let owned_touch_down = self.owners.iter().any(|(s, _)| *s != MOUSE_SLOT);
            let (slot, phase, pos) = match &ev {
                egui::Event::Touch { id, phase, pos, .. } => {
                    self.touch_seen = true;
                    (id.0 as i32, *phase, *pos)
                }
                // Once real touches exist, pointer events are the copies
                // synthesized from the first touch: drop them while a touch
                // is owned, pass them through otherwise.
                egui::Event::PointerButton { .. } | egui::Event::PointerMoved(_) if self.touch_seen => {
                    if !owned_touch_down {
                        keep.push(ev);
                    }
                    continue;
                }
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, .. } => {
                    self.mouse_down = *pressed;
                    (MOUSE_SLOT, if *pressed { egui::TouchPhase::Start } else { egui::TouchPhase::End }, *pos)
                }
                egui::Event::PointerMoved(pos) if self.mouse_down => (MOUSE_SLOT, egui::TouchPhase::Move, *pos),
                _ => {
                    keep.push(ev);
                    continue;
                }
            };
            let l = pos - origin;
            let (x, y) = (l.x, l.y);
            let te = match phase {
                egui::TouchPhase::Start => TouchEvent::Down { slot, x, y, t: now },
                egui::TouchPhase::Move => TouchEvent::Move { slot, x, y, t: now },
                egui::TouchPhase::End => TouchEvent::Up { slot, t: now },
                egui::TouchPhase::Cancel => TouchEvent::Cancel,
            };
            if phase == egui::TouchPhase::Start {
                let pad = Self::pad_rect(size, Self::bar_width(shell));
                let modal = shell.drawer.is_visible() || shell.switcher.open || self.sheet.open;
                let may_pad = slot != MOUSE_SLOT || self.pointer_pad;
                let owner = if shell.modes.trackpad && !modal && may_pad && pad.contains(Pos2::new(x, y)) {
                    Some(Owner::Pad)
                } else if shell.recognizer.edge_at(x, y).is_some() && !self.sheet.open && !shell.palette.open {
                    Some(Owner::Edge)
                } else {
                    None
                };
                self.owners.retain(|(s, _)| *s != slot);
                if let Some(o) = owner {
                    self.owners.push((slot, o));
                }
            }
            let owner = self.owner(slot);
            match (owner, te) {
                (Some(Owner::Edge), e) => out.extend(shell.on_edge_touch(e)),
                (Some(Owner::Pad), e) => out.extend(shell.on_trackpad_touch(e)),
                (None, TouchEvent::Cancel) => {
                    out.extend(shell.on_edge_touch(TouchEvent::Cancel));
                    out.extend(shell.on_trackpad_touch(TouchEvent::Cancel));
                }
                _ => {}
            }
            if owner.is_none() {
                keep.push(ev);
            }
            if matches!(phase, egui::TouchPhase::End | egui::TouchPhase::Cancel) {
                self.owners.retain(|(s, _)| *s != slot);
            }
        }
        *events = keep;
        out
    }

    /// Covered or asleep: every event is swallowed (nothing reaches egui or
    /// the apps). Asleep, touches go to the triple-tap detector; covered, a
    /// vertical drag moves the cover with the finger.
    fn route_covered(&mut self, shell: &mut Shell, events: &mut Vec<egui::Event>, origin: Pos2, now: u64) -> Vec<Action> {
        let mut out = vec![];
        let h = shell.screen.1.max(1.0);
        for ev in events.drain(..) {
            let (slot, phase, pos) = match ev {
                egui::Event::Touch { id, phase, pos, .. } => {
                    self.touch_seen = true;
                    (id.0 as i32, phase, pos)
                }
                _ if self.touch_seen => continue,
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, .. } => {
                    self.mouse_down = pressed;
                    (MOUSE_SLOT, if pressed { egui::TouchPhase::Start } else { egui::TouchPhase::End }, pos)
                }
                egui::Event::PointerMoved(pos) if self.mouse_down => (MOUSE_SLOT, egui::TouchPhase::Move, pos),
                _ => continue,
            };
            let l = pos - origin;
            if shell.sleep.captures_input() {
                let te = match phase {
                    egui::TouchPhase::Start => TouchEvent::Down { slot, x: l.x, y: l.y, t: now },
                    egui::TouchPhase::Move => TouchEvent::Move { slot, x: l.x, y: l.y, t: now },
                    egui::TouchPhase::End => TouchEvent::Up { slot, t: now },
                    egui::TouchPhase::Cancel => TouchEvent::Cancel,
                };
                out.extend(shell.on_sleep_touch(te));
                continue;
            }
            let te = match phase {
                egui::TouchPhase::Start => TouchEvent::Down { slot, x: l.x, y: l.y, t: now },
                egui::TouchPhase::Move => TouchEvent::Move { slot, x: l.x, y: l.y, t: now },
                egui::TouchPhase::End => TouchEvent::Up { slot, t: now },
                egui::TouchPhase::Cancel => TouchEvent::Cancel,
            };
            out.extend(shell.on_cover_touch(te));
            match phase {
                egui::TouchPhase::Start => self.cover_drag = Some((slot, l.y)),
                egui::TouchPhase::Move => {
                    if let Some((s, y0)) = self.cover_drag {
                        if s == slot {
                            // Full reveal after 55% of the screen height.
                            shell.lock.drag_reveal(((y0 - l.y) / (h * 0.55)).clamp(0.0, 1.0), now);
                        }
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    if self.cover_drag.is_some_and(|(s, _)| s == slot) {
                        self.cover_drag = None;
                        shell.lock.release_reveal();
                    }
                }
            }
        }
        out
    }

    /// Which lock-screen session this is, counting up whenever the cover reappears after being away for 2 s or more.
    /// Logs each new session once so a journal read says which variant was on screen when.
    fn ab_session(&self, mode: &str) -> u32 {
        let now = std::time::Instant::now();
        let (last, n) = self.ab.get();
        let n = match last {
            Some(t) if now.duration_since(t) < std::time::Duration::from_secs(2) => n,
            _ => {
                eprintln!("lock-night A/B: session {} uses {} (mode {mode})", n + 1, if (n + 1) % 2 == 0 { "auto night curve" } else { "old fixed black" });
                n + 1
            }
        };
        self.ab.set((Some(now), n));
        n
    }

    fn cover(&self, p: &egui::Painter, shell: &Shell, info: &FrameInfo, origin: Pos2, size: Vec2) {
        let prog = shell.lock.reveal.progress;
        let alpha = (1.0 - prog * 0.8).clamp(0.0, 1.0);
        let top = origin + Vec2::new(0.0, -prog * size.y);
        // Background: the camera when it is running (the pane), else a gradient.
        // The wash: twenty cells drawn as a lattice of gradient quads. The
        // GPU's vertex interpolation is the upscaler, so nothing is
        // resampled and nothing large is uploaded. Preferred over the
        // texture path because this is what the lock screen actually wants —
        // colour from the room, not a picture of it.
        if let Some((cells, cols, rows)) = self.glass.clone() {
            let r = Rect::from_min_size(top, size);
            let corners = crate::wash::lattice(&cells, cols, rows);
            let auto = crate::wash::nightness(shell.ambient.smoothed_lux());
            // None = the old fixed black point (24); Some(n) = the night curve at nightness n
            let night: Option<f32> = match shell.prefs.lock_night.as_str() {
                "off" => None,
                "force" => Some(1.0),
                "ab" => if self.ab_session(shell.prefs.lock_night.as_str()) % 2 == 0 { Some(auto) } else { None },
                _ => Some(auto),
            };
            let mut mesh = egui::Mesh::default();
            for gy in 0..=rows {
                for gx in 0..=cols {
                    let c = corners[gy * (cols + 1) + gx];
                    let c = match (self.oled == Some(true), night) { (true, Some(n)) => crate::wash::night_wash(c, n), (true, None) => crate::wash::black_point(c, 24), _ => c };
                    let p = Pos2::new(r.left() + r.width() * gx as f32 / cols as f32, r.top() + r.height() * gy as f32 / rows as f32);
                    mesh.vertices.push(egui::epaint::Vertex {
                        pos: p,
                        uv: egui::epaint::WHITE_UV,
                        color: Color32::from_rgba_unmultiplied(c[0], c[1], c[2], (alpha * 255.0) as u8),
                    });
                }
            }
            mesh.indices = crate::wash::indices(cols, rows);
            p.add(egui::Shape::mesh(mesh));
            // Grain, to break the banding a gradient this smooth shows on a
            // dim screen. One tiled texture, drawn at low alpha.
            p.rect_filled(r, cr(0), Color32::from_black_alpha((alpha * 90.0) as u8));
            // Depth orbs: the tracked corners, sized and lit by parallax depth
            // (orbs.rs). Soft discs, never a picture; three rings each for blur.
            let short = r.width().min(r.height());
            for o in &shell.orbs.orbs {
                let c = Pos2::new(r.left() + o.x * r.width(), r.top() + o.y * r.height());
                let rad = o.radius(short);
                let lift = |v: u8| (v as f32 * 0.5 + 127.0) as u8; // toward white: glows, not holes
                let col = |a: f32| Color32::from_rgba_unmultiplied(lift(o.color[0]), lift(o.color[1]), lift(o.color[2]), (a * o.alpha * alpha * 255.0).clamp(0.0, 255.0) as u8);
                p.circle_filled(c, rad * 1.8, col(0.10));
                p.circle_filled(c, rad * 1.25, col(0.18));
                p.circle_filled(c, rad, col(0.32 + 0.3 * o.depth));
            }
        } else if let Some((tex, fw, fh)) = &self.camera_tex {
            let r = Rect::from_min_size(top, size);
            // Not `p.image` with a full 0..1 uv: that stretches the frame to
            // the pane on BOTH axes independently, so a landscape sensor in
            // a portrait pane comes out squashed and, with the sensor a
            // quarter turn from the screen, sideways as well. A mesh lets
            // each destination corner read a different source corner, which
            // both turns the image and crops it to one scale factor, so
            // real-world proportions survive.
            let mut uv = crate::fit::corner_uvs((*fw as f32, *fh as f32), (size.x, size.y), self.camera_rot);
            if self.camera_mirror {
                // Swap the left and right columns of the quad: a mirror is a
                // reordering, not a second sampling pass.
                uv.swap(0, 1);
                uv.swap(3, 2);
            }
            let tint = Color32::from_white_alpha((alpha * 255.0) as u8);
            let mut mesh = egui::Mesh::with_texture(tex.id());
            for (i, corner) in [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()].iter().enumerate() {
                mesh.vertices.push(egui::epaint::Vertex { pos: *corner, uv: Pos2::new(uv[i][0], uv[i][1]), color: tint });
            }
            mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            p.add(egui::Shape::mesh(mesh));
            // Darken for legibility, keep the picture.
            p.rect_filled(r, cr(0), Color32::from_black_alpha((alpha * 90.0) as u8));
            // World anchors: rings where the world says they are.
            let c = top + Vec2::new(size.x / 2.0, size.y / 2.0);
            for a in &shell.world.anchors {
                if let Some((x, y)) = shell.world.project(a.dir) {
                    let at = c + Vec2::new(x, y);
                    if r.contains(at) {
                        p.circle_stroke(at, 14.0, Stroke::new(2.0, Color32::from_white_alpha((alpha * 220.0) as u8)));
                        p.circle_filled(at, 3.0, Color32::from_white_alpha((alpha * 220.0) as u8));
                        p.text(at + Vec2::new(0.0, 24.0), Align2::CENTER_CENTER, &a.label, FontId::proportional(12.0), Color32::from_white_alpha((alpha * 220.0) as u8));
                    }
                }
            }
            // Live indicator: the camera is on.
            p.circle_filled(top + Vec2::new(size.x - 22.0, 22.0), 5.0, Color32::from_rgba_unmultiplied(230, 60, 50, (alpha * 255.0) as u8));
            p.text(top + Vec2::new(size.x - 34.0, 22.0), Align2::RIGHT_CENTER, format!("{:.0}°", shell.world.yaw_deg()), FontId::proportional(12.0), Color32::from_white_alpha((alpha * 200.0) as u8));
        }
        let bands = if self.camera_tex.is_some() { 0 } else { 32 };
        let br = tideshell_core::season::breath(&shell.mood(), info.t_secs) * self.breath_envelope(info);
        let acc = shell.accent();
        for i in 0..bands {
            let t = i as f32 / (bands - 1) as f32;
            let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
            // The gradient runs from a dark of the scene's accent to a
            // lighter one, and breathes: the light end lifts with the breath.
            let dark = [acc[0] as f32 * 0.12, acc[1] as f32 * 0.12, acc[2] as f32 * 0.14];
            let light = [acc[0] as f32 * (0.28 + br * 0.08), acc[1] as f32 * (0.28 + br * 0.08), acc[2] as f32 * (0.32 + br * 0.08)];
            let c = Color32::from_rgba_unmultiplied(lerp(dark[0], light[0]), lerp(dark[1], light[1]), lerp(dark[2], light[2]), (alpha * 255.0) as u8);
            let band_h = size.y / bands as f32;
            p.rect_filled(Rect::from_min_size(top + Vec2::new(0.0, i as f32 * band_h), Vec2::new(size.x, band_h + 1.0)), cr(0), c);
        }
        let white = |a: f32| Color32::from_white_alpha((a * alpha * 255.0) as u8);
        self.clock_face(p, shell, info, top + Vec2::new(size.x / 2.0, size.y * 0.24), alpha);
        // Hint and chevron travel with the cover (the first version added the
        // drag back in and left them pinned over the revealed app).
        let c = top + Vec2::new(size.x / 2.0, size.y - 90.0);
        let chev = Stroke::new(2.5, white(0.8));
        p.line_segment([c + Vec2::new(-14.0, -22.0), c + Vec2::new(0.0, -34.0)], chev);
        p.line_segment([c + Vec2::new(0.0, -34.0), c + Vec2::new(14.0, -22.0)], chev);
        p.text(c, Align2::CENTER_CENTER, "Swipe up to unlock", FontId::proportional(15.0), white(0.75));
    }

    /// The cover clock at `c`: digital (exact digits, or words when fuzzy) or
    /// analog (hands snapped to the precision; weekday mode shows the day
    /// inside the face; words underneath when fuzzy).
    fn breath_envelope(&self, info: &FrameInfo) -> f32 {
        if self.night {
            return 0.0; // sleeping hours: nothing moves on its own
        }
        let idle_s = ((info.t_secs * 1000.0) as u64).saturating_sub(self.last_activity_ms) as f32 / 1000.0;
        tideshell_core::season::breath_envelope(idle_s)
    }

    /// The cover clock drawn anamorphically for a phone lying flat
    /// (clock3d.rs): polygons from the core, painted as one mesh.
    fn clock_3d(&self, p: &egui::Painter, shell: &Shell, info: &FrameInfo, c: Pos2, alpha: f32) {
        use tideshell_core::clock3d::{build, Eye, Scene};
        let screen = p.clip_rect().size();
        // A seated viewer about three phone-heights away, 40° above the table.
        let eye = Eye::seated((screen.x, screen.y), 40.0, screen.y * 3.0);
        let scene = Scene {
            style: shell.prefs.clock_3d_style,
            amount: shell.clock_3d_amount(),
            t: info.t_secs,
            screen: (screen.x, screen.y),
            anchor: (c.x, c.y + 40.0),
            height: 84.0,
        };
        let text = format!("{:02}:{:02}", info.time.hour, info.time.minute);
        let mut mesh = egui::Mesh::default();
        for q in build(&text, &scene, &eye) {
            let a = (q.alpha * alpha * 255.0).clamp(0.0, 255.0) as u8;
            let color = if q.shadow {
                Color32::from_black_alpha(a)
            } else {
                let v = (q.shade * 255.0) as u8;
                Color32::from_rgba_unmultiplied(v, v, v, a)
            };
            let base = mesh.vertices.len() as u32;
            for (x, y) in q.pts {
                mesh.vertices.push(egui::epaint::Vertex { pos: Pos2::new(x, y), uv: egui::epaint::WHITE_UV, color });
            }
            mesh.indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        p.add(egui::Shape::mesh(mesh));
    }

    fn clock_face(&self, p: &egui::Painter, shell: &Shell, info: &FrameInfo, c: Pos2, alpha: f32) {
        use tideshell_core::clock::{hands, weekday_name, words, ClockStyle, Fuzzy};
        // Dim inside sleeping hours (bsettings), for the eyes and the OLED.
        let alpha = alpha * tideshell_core::nightclock::night_alpha(shell.night_window);
        let white = |a: f32| Color32::from_white_alpha((a * alpha * 255.0) as u8);
        let f = shell.prefs.clock_fuzzy;
        match shell.prefs.clock_style {
            ClockStyle::Digital if f == Fuzzy::Exact && shell.clock_3d_amount() > 0.001 => {
                self.clock_3d(p, shell, info, c, alpha);
            }
            ClockStyle::Digital => {
                let (txt, font) = if f == Fuzzy::Exact { (format!("{:02}:{:02}", info.time.hour, info.time.minute), 72.0) } else { (words(info.time, f), 34.0) };
                p.text(c, Align2::CENTER_CENTER, txt, FontId::proportional(font), white(1.0));
            }
            ClockStyle::Analog => {
                let r = 84.0;
                p.circle_stroke(c, r, Stroke::new(2.0, white(0.85)));
                for i in 0..12 {
                    let a = i as f32 / 12.0 * std::f32::consts::TAU;
                    let (s, co) = (a.sin(), -a.cos());
                    let major = i % 3 == 0;
                    let inner = if major { r - 14.0 } else { r - 8.0 };
                    p.line_segment([c + Vec2::new(s, co) * inner, c + Vec2::new(s, co) * (r - 3.0)], Stroke::new(if major { 3.0 } else { 1.5 }, white(0.8)));
                }
                match hands(info.time, f) {
                    Some((ha, ma)) => {
                        let dir = |a: f32| Vec2::new(a.sin(), -a.cos());
                        p.line_segment([c, c + dir(ha) * r * 0.5], Stroke::new(5.0, white(1.0)));
                        if let Some(ma) = ma {
                            p.line_segment([c, c + dir(ma) * r * 0.8], Stroke::new(3.0, white(1.0)));
                        }
                        p.circle_filled(c, 5.0, white(1.0));
                    }
                    None => {
                        p.text(c, Align2::CENTER_CENTER, weekday_name(info.time.weekday), FontId::proportional(20.0), white(1.0));
                    }
                }
                if f != Fuzzy::Exact && f != Fuzzy::Weekday {
                    p.text(c + Vec2::new(0.0, r + 28.0), Align2::CENTER_CENTER, words(info.time, f), FontId::proportional(18.0), white(0.8));
                }
            }
        }
    }

    /// A small analog face for the bar: ring, four ticks, hour and minute hands (the minute hand is
    /// absent for hour-fuzzy, and weekday mode writes the day's three letters instead).
    fn bar_analog_clock(p: &egui::Painter, c: Pos2, r: f32, t: tideshell_core::clock::LocalTime, f: tideshell_core::clock::Fuzzy, col: Color32) {
        use tideshell_core::clock::{hands, weekday_name};
        let dir = |a: f32| Vec2::new(a.sin(), -a.cos());
        p.circle_stroke(c, r, Stroke::new(1.5, col));
        match hands(t, f) {
            Some((ha, ma)) => {
                for i in 0..4 {
                    let d = dir(i as f32 * std::f32::consts::FRAC_PI_2);
                    p.line_segment([c + d * (r - 4.0), c + d * (r - 1.0)], Stroke::new(1.5, col));
                }
                p.line_segment([c, c + dir(ha) * r * 0.5], Stroke::new(3.0, col));
                if let Some(ma) = ma {
                    p.line_segment([c, c + dir(ma) * r * 0.8], Stroke::new(2.0, col));
                }
                p.circle_filled(c, 2.0, col);
            }
            None => {
                p.text(c, Align2::CENTER_CENTER, &weekday_name(t.weekday)[..3], FontId::proportional((r * 0.6).max(9.0)), col);
            }
        }
    }

    /// The command palette: a sheet rising from the bottom (reveal follows
    /// the engine's DragTransition), the three mode buttons on top, a search
    /// field, then the rows. Hit-testing is the engine's (`palette::Geom`),
    /// so a tap here and a hold-and-slide behave identically.
    fn palette_sheet(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, origin: Pos2, size: Vec2) -> Vec<Action> {
        use tideshell_core::hold::HoldOption;
        use tideshell_core::palette::{Geom, Hit, MODE_ROW_H, PAD, ROW_H, SEARCH_H};
        let mut out = vec![];
        let g = shell.palette.geom((size.x, size.y));
        // Two separate opacities, and they multiply. `reveal` is the sheet
        // arriving, a spring the shell drives; `hold_fade` is a held thumb
        // carrying it back off the top, which follows the finger exactly so
        // that pulling back restores it. Conflating them would make a
        // dismissal that changed its mind animate twice.
        let reveal = shell.palette.reveal.progress * shell.palette.hold_fade.clamp(0.0, 1.0);
        let dy = (1.0 - reveal) * (size.y - g.sheet_top);
        let full = Rect::from_min_size(origin, size);
        let p = ui.painter_at(full);
        p.rect_filled(full, cr(0), Color32::from_black_alpha((reveal * 120.0) as u8));
        // One interact over the whole screen, registered first so the
        // search field (later) wins where they overlap.
        let bg = ui.interact(full, Id::new("ts-palette-bg"), Sense::click());
        if bg.clicked() {
            if let Some(pos) = bg.interact_pointer_pos() {
                out.extend(shell.palette_tap(pos.x - origin.x, pos.y - origin.y));
            }
        }
        let at = |x: f32, y: f32| origin + Vec2::new(x, y + dy);
        let sheet = Rect::from_min_max(at(0.0, g.sheet_top), at(size.x, size.y));
        p.rect_filled(sheet, CornerRadius { nw: 22, ne: 22, sw: 0, se: 0 }, pal.bg_card);
        p.rect_filled(Rect::from_center_size(sheet.center_top() + Vec2::new(0.0, 7.0), Vec2::new(40.0, 4.0)), cr(2), pal.text_weak);
        for (i, m) in shell.palette.modes.iter().enumerate() {
            let (x, y, w, h) = g.mode_rect(i);
            let r = Rect::from_min_size(at(x, y), Vec2::new(w, h));
            let dt = self.frame_dt;
            let r = self.pressed_rect(ui, Id::new(("ts-mode-j", i)), r, dt);
            let on = match m {
                HoldOption::Carousel => shell.carousel.is_active(),
                HoldOption::Keyboard => shell.modes.keyboard,
                HoldOption::Trackpad => shell.modes.trackpad,
            };
            let hot = shell.palette.hover == Some(Hit::Mode(i));
            p.rect_filled(r, cr(14), if hot { pal.accent } else if on { pal.accent_soft } else { pal.bg_raised });
            p.text(r.center() - Vec2::new(0.0, 10.0), Align2::CENTER_CENTER, initial(m.label()), FontId::proportional(22.0), if hot { pal.bg_base } else { pal.text });
            p.text(r.center() + Vec2::new(0.0, 16.0), Align2::CENTER_CENTER, m.label(), FontId::proportional(12.0), if hot { pal.bg_base } else { pal.text_weak });
            if on {
                p.circle_filled(r.right_top() + Vec2::new(-10.0, 10.0), 4.0, pal.success);
            }
        }
        let sr = Rect::from_min_size(at(PAD, g.search_y()), Vec2::new(size.x - PAD * 2.0, SEARCH_H));
        p.rect_filled(sr, cr(12), pal.bg_raised);
        let mut q = shell.palette.query.clone();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr.shrink2(Vec2::new(12.0, 8.0))));
        let te = egui::TextEdit::singleline(&mut q).id_salt("ts-palette-search").hint_text("Search").frame(egui::Frame::NONE).desired_width(f32::INFINITY);
        let resp = child.add(te);
        // The sheet opens short; the field takes focus (and the list opens in full) once the
        // thumb slides into the search bar or taps it.
        if resp.gained_focus() {
            shell.palette.expanded = true;
        }
        if shell.palette.open && shell.palette.expanded && !self.palette_focused {
            self.palette_focused = true;
            resp.request_focus();
        }
        if !shell.palette.open {
            self.palette_focused = false;
        }
        if resp.changed() {
            shell.palette_query(&q);
        }
        // Results are rows on the short sheet and hatch-style tiles on the full one. Each result keeps the
        // rectangle it was last drawn at (by key) and eases to where the layout now wants it, so opening
        // the sheet in full MORPHS the list into the grid instead of cutting to it; a result that was not
        // there before grows in from its target.
        let rows: Vec<(String, String, &'static str)> = shell.palette.rows().map(|r| (r.key.clone(), r.label.clone(), r.hint)).collect();
        let ease = 1.0 - (-self.frame_dt / 1000.0 * 16.0).exp();
        let mut seen: Vec<String> = Vec::new();
        let mut moving = false;
        let row_w = size.x - PAD * 2.0;
        for (i, (key, label, hint)) in rows.iter().enumerate().take(g.rows_visible()) {
            let (tx, ty, tw, th) = g.item_rect(i);
            // Animated in sheet coordinates: the sheet's own slide (`dy`, the origin) is applied when drawing, or
            // the results would trail behind a rising sheet.
            let target = Rect::from_min_size(Pos2::new(tx, ty), Vec2::new(tw, th));
            let (cur, born) = match self.palette_anim.get(key) {
                Some(r) => (Rect::from_min_max(r.min + (target.min - r.min) * ease, r.max + (target.max - r.max) * ease), false),
                None => (Rect::from_center_size(target.center(), target.size() * 0.6), true),
            };
            // fully arrived (or newly born and growing) results are drawn at `cur`
            let cur = if born { Rect::from_min_max(cur.min + (target.min - cur.min) * ease, cur.max + (target.max - cur.max) * ease) } else { cur };
            // Close enough is arrived: snap, so a settled sheet stops asking for frames.
            let near = (cur.min - target.min).length() < 0.6 && (cur.max - target.max).length() < 0.6;
            let cur = if near { target } else { cur };
            moving |= !near;
            self.palette_anim.insert(key.clone(), cur);
            seen.push(key.clone());
            let drawn = cur.translate(origin.to_vec2() + Vec2::new(0.0, dy));
            let grow = ((cur.width() * cur.height()) / (target.width() * target.height())).clamp(0.0, 1.0).sqrt();
            let alpha = if born { grow } else { 1.0 };
            let hot = shell.palette.hover == Some(Hit::Row(i));
            let as_row = cur.width() > row_w * 0.62;
            let ink = if hot { pal.bg_base } else { pal.text };
            let weak = if hot { pal.bg_base } else { pal.text_weak };
            if as_row {
                if hot {
                    p.rect_filled(drawn, cr(10), with_alpha(pal.accent, alpha));
                }
                p.text(drawn.left_center() + Vec2::new(14.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(15.0), with_alpha(ink, alpha));
                p.text(drawn.right_center() - Vec2::new(14.0, 0.0), Align2::RIGHT_CENTER, *hint, FontId::proportional(11.0), with_alpha(weak, alpha));
            } else {
                // A tile: a raised card (accent when the thumb is on it), the app's colour chip with its initial, the
                // name under it, cut with an ellipsis, and the hint small beneath.
                p.rect_filled(drawn, cr(14), with_alpha(if hot { pal.accent } else { pal.bg_raised }, alpha));
                let chip = Rect::from_center_size(drawn.center_top() + Vec2::new(0.0, 28.0), Vec2::splat(36.0));
                p.rect_filled(chip, cr(10), with_alpha(app_color(key), alpha));
                p.text(chip.center(), Align2::CENTER_CENTER, initial(label), FontId::proportional(18.0), with_alpha(Color32::WHITE, 0.9 * alpha));
                let max_chars = ((cur.width() - 10.0) / 7.0).max(4.0) as usize;
                let name: String = if label.chars().count() > max_chars { format!("{}…", label.chars().take(max_chars - 1).collect::<String>()) } else { label.clone() };
                p.text(drawn.center_bottom() - Vec2::new(0.0, 30.0), Align2::CENTER_CENTER, name, FontId::proportional(12.5), with_alpha(ink, alpha));
                p.text(drawn.center_bottom() - Vec2::new(0.0, 13.0), Align2::CENTER_CENTER, *hint, FontId::proportional(10.0), with_alpha(weak, alpha));
            }
        }
        self.palette_anim.retain(|k, _| seen.contains(k));
        if moving {
            ui.ctx().request_repaint();
        }
        if rows.is_empty() {
            let (tx, ty, tw, th) = g.item_rect(0);
            p.text(at(tx + tw / 2.0, ty + th / 2.0), Align2::CENTER_CENTER, "No matches", FontId::proportional(13.0), pal.text_weak);
        }
        let _ = MODE_ROW_H;
        out
    }

    // ---- previews --------------------------------------------------------------

    fn previews(&self, p: &egui::Painter, shell: &Shell, pal: &PaletteColors, origin: Pos2, size: Vec2) {
        let cfg = &shell.recognizer.cfg;
        if shell.swipe_up > 0.0 {
            p.rect_filled(Rect::from_min_size(origin, size), cr(0), Color32::from_black_alpha((shell.swipe_up * 360.0).min(150.0) as u8));
            // Say what lifting now would do: the recognizer's thresholds,
            // made visible, so the gesture can be learned rather than guessed.
            let label = if shell.swipe_up >= cfg.launcher_commit {
                "Release for apps (hatch)"
            } else if shell.swipe_up >= cfg.switcher_commit {
                "Release for switcher - keep going for apps"
            } else {
                "Keep swiping up"
            };
            p.text(origin + Vec2::new(size.x / 2.0, size.y * (1.0 - shell.swipe_up) - 40.0), Align2::CENTER_CENTER, label, FontId::proportional(15.0), pal.text);
        }
        if shell.peek_dx.abs() > 1.0 {
            let label = if shell.carousel.is_active() {
                if shell.peek_dx > 0.0 { "< previous window" } else { "next window >" }
            } else if shell.peek_dx > 0.0 {
                "< previous app"
            } else {
                "more recent app >"
            };
            let reached = shell.peek_dx.abs() >= cfg.quick_switch_px;
            p.text(origin + Vec2::new(size.x / 2.0, size.y - 40.0), Align2::CENTER_CENTER, label, FontId::proportional(14.0), if reached { pal.accent } else { pal.text_weak });
        }
    }

    // ---- drawer ------------------------------------------------------------------

    /// Height of the quick-settings sheet for `rows` rows of tiles: 40 header + rows of 72 (+10 gap)
    /// + 6, brightness 40 (+10), the volume pad 96 (+10), the settings entry 36 (+4 above), 16 foot.
    /// Mirrors the layout in `drawer`, which is what keeps the last control on the sheet.
    pub fn drawer_content_height(rows: usize) -> f32 {
        40.0 + rows as f32 * 82.0 + 6.0 + 50.0 + 106.0 + 4.0 + 36.0 + 16.0
    }

    /// A battery glyph centred at `c`, `w` wide: outline, terminal nub, a fill whose width is the
    /// charge (red at or under 15% when not charging), a bolt when charging, a dash when unknown.
    fn battery_icon(p: &egui::Painter, c: Pos2, w: f32, level: Option<f32>, charging: bool, pal: &PaletteColors) {
        let h = w * 0.5;
        let body = Rect::from_center_size(c - Vec2::new(w * 0.04, 0.0), Vec2::new(w, h));
        p.rect_stroke(body, cr(3), Stroke::new(1.6, pal.text), egui::StrokeKind::Inside);
        p.rect_filled(Rect::from_center_size(Pos2::new(body.right() + w * 0.06, c.y), Vec2::new(w * 0.1, h * 0.4)), cr(1), pal.text);
        match level {
            Some(l) => {
                let l = l.clamp(0.0, 1.0);
                let inner = body.shrink(2.5);
                let fill = Rect::from_min_size(inner.min, Vec2::new(inner.width() * l, inner.height()));
                let col = if l <= 0.15 && !charging { Color32::from_rgb(220, 70, 60) } else { pal.accent };
                p.rect_filled(fill, cr(2), col);
                if charging {
                    let (x, y, s) = (body.center().x, body.center().y, h * 0.34);
                    let bolt = vec![Pos2::new(x + s * 0.2, y - s * 1.3), Pos2::new(x - s * 0.7, y + s * 0.15), Pos2::new(x - s * 0.05, y + s * 0.15), Pos2::new(x - s * 0.2, y + s * 1.3), Pos2::new(x + s * 0.7, y - s * 0.15), Pos2::new(x + s * 0.05, y - s * 0.15)];
                    p.add(egui::Shape::convex_polygon(bolt, pal.bg_base, Stroke::NONE));
                }
            }
            None => {
                p.line_segment([body.center() - Vec2::new(w * 0.15, 0.0), body.center() + Vec2::new(w * 0.15, 0.0)], Stroke::new(2.0, pal.text_weak));
            }
        }
    }

    fn drawer(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, origin: Pos2, size: Vec2) -> Vec<Action> {
        let mut out = vec![];
        // The sheet is as tall as its content (header, tile rows, brightness, the volume pad, the
        // settings entry) so every control is reachable, but never taller than the screen: a wide
        // (landscape) screen packs the tiles into more columns instead of more rows.
        let cols_n = if size.x > 800.0 { 8usize } else { 5usize };
        let n_tiles = shell.drawer.tiles.len();
        let rows_n = n_tiles.div_ceil(cols_n);
        let need = Self::drawer_content_height(rows_n);
        let full_h = need.min((size.y - 16.0).max(120.0));
        let panel_h = shell.drawer.progress * full_h;
        let panel = Rect::from_min_size(origin, Vec2::new(size.x, panel_h));
        // Backdrop below the panel: tap closes.
        let back = Rect::from_min_max(panel.left_bottom(), origin + size);
        let p = ui.painter_at(Rect::from_min_size(origin, size));
        p.rect_filled(back, cr(0), Color32::from_black_alpha((shell.drawer.progress * 140.0) as u8));
        if ui.interact(back, Id::new("ts-drawer-back"), Sense::click()).clicked() {
            shell.dismiss_overlays();
        }
        p.rect_filled(panel, CornerRadius { nw: 0, ne: 0, sw: 22, se: 22 }, with_alpha(pal.bg_card, 0.97));
        let pc = ui.painter_at(panel);
        pc.text(origin + Vec2::new(18.0, 22.0), Align2::LEFT_CENTER, &String::from("Quick settings"), FontId::proportional(13.0), pal.text_weak);

        let pad = 14.0;
        let cols = cols_n as f32;
        let gap = 10.0;
        let tw = (size.x - pad * 2.0 - gap * (cols - 1.0)) / cols;
        let th = 72.0;
        let tiles: Vec<QuickKind> = shell.drawer.tiles.clone();
        let visible = shell.drawer.stage() != Stage::Closed;
        for (i, kind) in tiles.iter().enumerate() {
            let (col, row) = ((i % cols_n) as f32, (i / cols_n) as f32);
            let r = Rect::from_min_size(origin + Vec2::new(pad + col * (tw + gap), 40.0 + row * (th + gap)), Vec2::new(tw, th));
            if r.top() > panel.bottom() {
                continue;
            }
            let on = shell.quick_state(*kind);
            let dt = self.frame_dt;
            let hit = r.intersect(panel);
            let r = self.pressed_rect(ui, Id::new(("ts-tile-j", i)), r, dt);
            pc.rect_filled(r, cr(16), if on { pal.accent } else { pal.bg_raised });
            pc.text(r.center() - Vec2::new(0.0, 8.0), Align2::CENTER_CENTER, initial(kind.label()), FontId::proportional(20.0), if on { pal.bg_base } else { pal.text });
            // CpuGovernor has no off state -- ITS label is which mode is
            // current, not a fixed short name every other tile uses.
            let sub = if *kind == QuickKind::CpuGovernor { shell.drawer.governor.label().to_string() } else { short(kind.label()).to_string() };
            pc.text(r.center_bottom() - Vec2::new(0.0, 13.0), Align2::CENTER_CENTER, sub, FontId::proportional(10.0), if on { pal.bg_base } else { pal.text_weak });
            if visible && hit.height() > 20.0 {
                let resp = ui.interact(hit, Id::new(("ts-tile", i)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                if self.tile_held(ui, &resp) {
                    out.extend(shell.long_press_quick(*kind));
                } else if self.tile_tap(&resp) {
                    out.extend(shell.tap_quick(*kind));
                }
            }
        }
        let rows = rows_n as f32;
        let mut y = 40.0 + rows * (th + gap) + 6.0;
        for kind in [SliderKind::Brightness, SliderKind::Volume] {
            // Volume is a pad: x = level, y = voicing (up: cut through a loud
            // place; middle: flat; down: soft for a quiet room).
            let h = if kind == SliderKind::Volume { 96.0 } else { 40.0 };
            let r = Rect::from_min_size(origin + Vec2::new(pad, y), Vec2::new(size.x - pad * 2.0, h));
            y += h + 10.0;
            if r.top() > panel.bottom() {
                continue;
            }
            if kind == SliderKind::Volume {
                out.extend(self.volume_pad(ui, shell, pal, r.intersect(panel), r));
                continue;
            }
            let relative = kind == SliderKind::Brightness && shell.ambient.enabled;
            let v = if kind == SliderKind::Brightness { shell.brightness_slider_pos() } else { shell.drawer.slider(kind) };
            pc.rect_filled(r, cr(20), pal.bg_raised);
            pc.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width() * v, r.height())), cr(20), with_alpha(pal.accent, 0.45));
            let band = shell.brightness_band();
            if relative {
                // Centre tick = the auto level; the travel spans the band.
                let cx = r.center().x;
                pc.line_segment([Pos2::new(cx, r.top() + 8.0), Pos2::new(cx, r.bottom() - 8.0)], Stroke::new(1.5, with_alpha(pal.text, 0.5)));
                // Absolute 0..100% track under the slider: where the band sits.
                let t = Rect::from_min_size(Pos2::new(r.left() + 14.0, r.bottom() + 3.0), Vec2::new(r.width() - 28.0, 4.0));
                let at = |x: f32| t.left() + t.width() * x.clamp(0.0, 1.0);
                pc.rect_filled(t, cr(2), with_alpha(pal.text_weak, 0.25));
                pc.rect_filled(Rect::from_min_max(Pos2::new(at(band.low), t.top()), Pos2::new(at(band.high), t.bottom())), cr(2), with_alpha(pal.accent, 0.8));
                pc.line_segment([Pos2::new(at(band.center), t.top() - 2.0), Pos2::new(at(band.center), t.bottom() + 2.0)], Stroke::new(1.5, pal.text));
                if let Some(a) = band.applied {
                    pc.circle_filled(Pos2::new(at(a), t.center().y), 3.5, pal.text);
                }
            }
            let label = if relative {
                format!("{} (auto \u{b1}{:.0}%)", kind.label(), (band.high - band.low) * 50.0)
            } else {
                kind.label().to_string()
            };
            pc.text(r.left_center() + Vec2::new(16.0, 0.0), Align2::LEFT_CENTER, label, FontId::proportional(13.0), pal.text);
            let resp = ui.interact(r.intersect(panel), Id::new(("ts-slider", kind.token())), Sense::click_and_drag());
            if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
                out.extend(shell.set_slider(kind, (pos.x - r.left()) / r.width()));
            }
        }
        // Settings entry at the bottom of the full drawer.
        let r = Rect::from_min_size(origin + Vec2::new(pad, y + 4.0), Vec2::new(size.x - pad * 2.0, 36.0));
        if r.bottom() <= panel.bottom() {
            pc.rect_filled(r, cr(18), pal.bg_raised);
            pc.text(r.center(), Align2::CENTER_CENTER, "Shell settings", FontId::proportional(13.0), pal.text);
            if ui.interact(r, Id::new("ts-drawer-settings"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.open_settings(shell);
            }
        }
        // Grab handle.
        pc.rect_filled(Rect::from_center_size(panel.center_bottom() - Vec2::new(0.0, 10.0), Vec2::new(40.0, 4.0)), cr(2), pal.text_weak);
        out
    }

    // ---- right bar ---------------------------------------------------------------

    fn bar(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, info: &FrameInfo, origin: Pos2, size: Vec2) -> Vec<Action> {
        let mut out = vec![];
        let bw = Self::bar_width(shell);
        let expanded = shell.bar_expanded;
        let rect = Rect::from_min_size(origin + Vec2::new(size.x - bw, 0.0), Vec2::new(bw, size.y));
        let p = ui.painter_at(rect);
        p.rect_filled(rect, cr(0), pal.bg_card);
        // Background drag, registered before the widgets so a drag that
        // starts ON a widget (a slider) stays that widget's: left expands,
        // right collapses. This replaces the right-edge swipe-in.
        let bg = ui.interact(rect, Id::new("ts-bar-bg"), Sense::drag());
        if bg.drag_started() {
            self.bar_drag = 0.0;
        }
        if bg.dragged() {
            self.bar_drag += bg.drag_delta().x;
        }
        if bg.drag_stopped() {
            if self.bar_drag < -30.0 {
                shell.set_bar_expanded(true);
            } else if self.bar_drag > 30.0 {
                shell.set_bar_expanded(false);
            }
            self.bar_drag = 0.0;
        }
        let unit = shell.prefs.bar_width.min(60.0);
        let widgets = shell.prefs.bar_widgets.clone();
        let lay = bar::layout(&widgets, size.y - 28.0, unit);
        let cell = |y: f32, h: f32| Rect::from_min_size(rect.min + Vec2::new(4.0, 14.0 + y), Vec2::new(bw - 8.0, h));
        let icon_c = |r: Rect| if expanded { Pos2::new(r.left() + unit / 2.0, r.center().y) } else { r.center() };
        let label = |p: &egui::Painter, r: Rect, s: &str| {
            if expanded {
                p.text(Pos2::new(r.left() + unit + 4.0, r.center().y), Align2::LEFT_CENTER, s, FontId::proportional(12.0), pal.text);
            }
        };
        let app_name = |id: &str| info.apps.iter().find(|a| a.id == id).map(|a| a.name.clone()).unwrap_or_else(|| id.to_string());
        for pl in &lay.placed {
            let r = cell(pl.y, pl.h);
            let wid = Id::new(("ts-bar", pl.index));
            match &widgets[pl.index] {
                BarWidget::Clock => {
                    if shell.prefs.clock_style == tideshell_core::clock::ClockStyle::Analog {
                        // The same style choice as the cover clock; hands follow the same fuzzy snap.
                        let rad = (r.width().min(r.height()) / 2.0 - 3.0).min(unit * 0.5).max(10.0);
                        Self::bar_analog_clock(&p, r.center(), rad, info.time, shell.prefs.clock_fuzzy, pal.text);
                    } else {
                        let txt = tideshell_core::clock::short_text(info.time, shell.prefs.clock_fuzzy);
                        p.text(r.center(), Align2::CENTER_CENTER, txt, FontId::proportional(if expanded { 22.0 } else { 15.0 }), pal.text);
                    }
                }
                BarWidget::Battery => {
                    // A drawn battery (body, terminal, level fill, bolt while charging), not a number:
                    // legible at a glance in a 64 px bar, and it keeps the percentage when expanded.
                    let c = icon_c(r);
                    Self::battery_icon(&p, c, unit * 0.55, info.battery, info.charging, pal);
                    if expanded {
                        let s = info.battery.map(|b| format!("{:.0}%", b * 100.0)).unwrap_or_else(|| "--".into());
                        p.text(Pos2::new(r.left() + unit + 6.0, r.center().y), Align2::LEFT_CENTER, s, FontId::proportional(16.0), pal.text);
                    }
                }
                BarWidget::Toggle(k) => {
                    let on = shell.quick_state(*k);
                    let c = icon_c(r);
                    p.circle_filled(c, unit * 0.36, if on { pal.accent } else { pal.bg_raised });
                    p.text(c, Align2::CENTER_CENTER, initial(k.label()), FontId::proportional(14.0), if on { pal.bg_base } else { pal.text });
                    label(&p, r, k.label());
                    let resp = ui.interact(r, wid, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    if self.tile_held(ui, &resp) {
                        out.extend(shell.long_press_quick(*k));
                    } else if self.tile_tap(&resp) {
                        out.extend(shell.tap_quick(*k));
                    }
                }
                BarWidget::Slider(k) => {
                    let track = Rect::from_center_size(r.center(), Vec2::new(unit * 0.5, r.height() - 12.0));
                    let v = if *k == SliderKind::Brightness { shell.brightness_slider_pos() } else { shell.drawer.slider(*k) };
                    p.rect_filled(track, cr(12), pal.bg_raised);
                    let fill = Rect::from_min_max(Pos2::new(track.left(), track.bottom() - track.height() * v), track.max);
                    p.rect_filled(fill, cr(12), with_alpha(pal.accent, 0.45));
                    p.text(track.center_bottom() - Vec2::new(0.0, 12.0), Align2::CENTER_CENTER, initial(k.label()), FontId::proportional(12.0), pal.text);
                    let resp = ui.interact(track, wid, Sense::click_and_drag());
                    if let Some(pos) = resp.interact_pointer_pos().filter(|_| resp.dragged() || resp.clicked()) {
                        out.extend(shell.set_slider(*k, (track.bottom() - pos.y) / track.height()));
                    }
                }
                BarWidget::App(id) => {
                    let name = app_name(id);
                    if app_button(ui, &p, r, wid, &name, id, icon_c(r), unit, pal) {
                        out.extend(shell.launch(id));
                    }
                    label(&p, r, &name);
                }
                BarWidget::Favorites(n) => {
                    for (k, app) in info.apps.iter().take(*n as usize).enumerate() {
                        let rr = Rect::from_min_size(r.min + Vec2::new(0.0, k as f32 * unit), Vec2::new(r.width(), unit));
                        if app_button(ui, &p, rr, Id::new(("ts-fav", k)), &app.name, &app.id, icon_c(rr), unit, pal) {
                            out.extend(shell.launch(&app.id));
                        }
                        label(&p, rr, &app.name);
                    }
                }
                BarWidget::Running => {
                    let cards: Vec<(u64, String, String)> =
                        shell.switcher.cards().iter().map(|c| (c.id, c.title.clone(), c.app_id.clone())).collect();
                    let fit = ((r.height() / unit) as usize).max(1);
                    for (k, (id, title, app)) in cards.iter().take(fit).enumerate() {
                        let rr = Rect::from_min_size(r.min + Vec2::new(0.0, k as f32 * unit), Vec2::new(r.width(), unit));
                        if app_button(ui, &p, rr, Id::new(("ts-run", *id)), title, app, icon_c(rr), unit, pal) {
                            out.extend(shell.activate(*id));
                        }
                        label(&p, rr, title);
                    }
                }
                BarWidget::Hatch | BarWidget::Switcher | BarWidget::Settings => {
                    let (txt, lbl) = match &widgets[pl.index] {
                        BarWidget::Hatch => ("Apps", "All apps"),
                        BarWidget::Switcher => ("Tasks", "Switcher"),
                        _ => ("Set", "Settings"),
                    };
                    let c = icon_c(r);
                    p.rect_filled(Rect::from_center_size(c, Vec2::splat(unit * 0.72)), cr(12), pal.bg_raised);
                    p.text(c, Align2::CENTER_CENTER, txt, FontId::proportional(11.0), pal.text);
                    label(&p, r, lbl);
                    if ui.interact(r, wid, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        match &widgets[pl.index] {
                            BarWidget::Hatch => out.extend(shell.open_launcher()),
                            BarWidget::Switcher => shell.toggle_switcher(),
                            _ => self.open_settings(shell),
                        }
                    }
                }
                BarWidget::Spacer => {}
            }
        }
        if !lay.overflow.is_empty() {
            p.text(rect.center_bottom() - Vec2::new(0.0, 8.0), Align2::CENTER_CENTER, format!("+{}", lay.overflow.len()), FontId::proportional(11.0), pal.warning);
        }
        out
    }

    // ---- switcher ------------------------------------------------------------------

    fn switcher(&mut self, ui: &mut egui::Ui, shell: &mut Shell, pal: &PaletteColors, origin: Pos2, size: Vec2) -> Vec<Action> {
        let mut out = vec![];
        let bw = Self::reserved_right(shell);
        let full = Rect::from_min_size(origin, Vec2::new(size.x - bw, size.y));
        let p = ui.painter_at(full);
        // Reveal: backdrop fades in, cards rise from below and scale up with
        // the swipe (or the spring after it); input only once open.
        let rv = shell.switcher.reveal.progress.clamp(0.0, 1.0);
        let live = shell.switcher.open;
        // Landing: the first frame at full reveal kicks every card's spring
        // once, a small settle-in bounce; rearmed when the switcher leaves.
        if rv >= 0.999 && live && !self.switcher_landed {
            self.switcher_landed = true;
            let ids: Vec<u64> = shell.switcher.cards().iter().map(|c| c.id).collect();
            for id in ids {
                self.press.entry(Id::new(("ts-card-j", id))).or_insert_with(|| tideshell_core::anim::Spring::jiggle(1.0)).kick(-1.2);
            }
        }
        if rv < 0.5 {
            self.switcher_landed = false;
        }
        p.rect_filled(full, cr(0), with_alpha(pal.bg_base, if self.backdrop_blur { 0.5 } else { 0.8 } * rv));
        let rise = (1.0 - rv) * size.y * 0.35;
        let scale = 0.86 + 0.14 * rv;
        if live && ui.interact(full, Id::new("ts-sw-back"), Sense::click()).clicked() {
            shell.dismiss_overlays();
        }
        // Header actions: multitask-first means "close all" and "all apps"
        // are one tap away, not buried.
        // "Close all" stays up top; "All apps" is at the bottom, centred, where the thumb already is after a
        // swipe up, and rises with the cards.
        let close_r = Rect::from_min_size(origin + Vec2::new(14.0, 14.0), Vec2::new(96.0, 34.0));
        let apps_r = Rect::from_center_size(origin + Vec2::new(full.width() / 2.0, size.y - 38.0 + (1.0 - rv) * 40.0), Vec2::new(176.0, 42.0));
        for (r, t, fs) in [(close_r, "Close all", 13.0), (apps_r, "All apps", 15.0)] {
            p.rect_filled(r, cr((r.height() / 2.0) as u8), with_alpha(pal.bg_raised, rv));
            p.text(r.center(), Align2::CENTER_CENTER, t, FontId::proportional(fs), with_alpha(pal.text, rv));
        }
        if live && ui.interact(close_r, Id::new("ts-sw-closeall"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            out.extend(shell.switcher_close_all());
        }
        if live && ui.interact(apps_r, Id::new("ts-sw-apps"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            out.extend(shell.open_launcher());
        }
        let top = 60.0;
        // leave the bottom strip to the All apps button
        let area = Vec2::new(full.width(), size.y - top - 84.0);
        let rects = shell.switcher.layout(area.x, area.y);
        let cards: Vec<(u64, String, String, bool)> =
            shell.switcher.cards().iter().map(|c| (c.id, c.title.clone(), c.app_id.clone(), c.activated)).collect();
        if cards.is_empty() {
            // Fades with the reveal (and later than the backdrop, rv squared) instead of popping in and out
            // at full strength while the backdrop is still fading; rises a little with the cards.
            p.text(full.center() + Vec2::new(0.0, rise * 0.25), Align2::CENTER_CENTER, "No open windows", FontId::proportional(16.0), with_alpha(pal.text_weak, rv * rv));
        }
        let dragging = shell.switcher.dragging;
        for ((id, title, app, active), r) in cards.iter().zip(rects) {
            let dy = dragging.filter(|(d, _)| d == id).map(|(_, dy)| dy).unwrap_or(0.0);
            let rr = Rect::from_min_size(origin + Vec2::new(r.x, top + r.y + dy + rise), Vec2::new(r.w, r.h));
            let rr = Rect::from_center_size(rr.center(), rr.size() * scale);
            let dt = self.frame_dt;
            let rr = if live { self.pressed_rect(ui, Id::new(("ts-card-j", *id)), rr, dt) } else { rr };
            // Everything on a card fades with the reveal as well as with a swipe-to-close drag: it used to slide
            // down at full strength and vanish at the last frame.
            let fade = (1.0 - (-dy / (shell.switcher.close_px * 1.6)).clamp(0.0, 0.8)) * rv;
            p.rect_filled(rr, cr(18), with_alpha(app_color(app), fade));
            if *active {
                p.rect_filled(Rect::from_min_size(rr.min, Vec2::new(rr.width(), 4.0)), cr(2), with_alpha(pal.accent, rv));
            }
            p.text(rr.center(), Align2::CENTER_CENTER, initial(title), FontId::proportional(34.0), with_alpha(Color32::WHITE, 0.85 * fade));
            p.text(rr.center_bottom() + Vec2::new(0.0, 11.0), Align2::CENTER_CENTER, title, FontId::proportional(12.0), with_alpha(pal.text, rv));
            if !live {
                continue;
            }
            let resp = ui.interact(rr, Id::new(("ts-card", *id)), Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.drag_started() {
                self.card_drag = 0.0;
            }
            if resp.dragged() {
                self.card_drag += resp.drag_delta().y;
                shell.switcher_drag(*id, self.card_drag);
            }
            if resp.drag_stopped() {
                out.extend(shell.switcher_release());
                self.card_drag = 0.0;
            } else if resp.clicked() {
                out.extend(shell.switcher_tap(*id));
            }
        }
        let style = match shell.switcher.style {
            Style::Grid => "grid",
            Style::Deck => "deck",
        };
        p.text(full.center_bottom() - Vec2::new(0.0, 36.0), Align2::CENTER_CENTER, format!("{} windows - {style} - swipe a card up to close", cards.len()), FontId::proportional(11.0), with_alpha(pal.text_weak, rv));
        out
    }

    // ---- settings sheet -------------------------------------------------------------

    pub fn open_settings(&mut self, shell: &mut Shell) {
        shell.drawer.close();
        self.sheet.open = true;
        self.sheet.draft = Some(shell.prefs.clone());
        self.sheet.notes.clear();
    }

    pub fn settings_open(&self) -> bool {
        self.sheet.open
    }

    /// The settings sheet. Returns new prefs when the user applies.
    fn settings_sheet(&mut self, ctx: &egui::Context, shell: &Shell) -> Option<ShellPrefs> {
        if !self.sheet.open {
            return None;
        }
        let mut applied = None;
        let mut close = false;
        let catalog = bar::catalog();
        let sheet = &mut self.sheet;
        let draft = sheet.draft.get_or_insert_with(ShellPrefs::default);
        egui::Window::new("Shell settings")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
              egui::ScrollArea::vertical().id_salt("ts-settings-scroll").max_height((shell.screen.1 - 90.0).max(200.0)).show(ui, |ui| {
                ui.label("Layout");
                ui.radio_value(&mut draft.layout, Layout::PullDown, "Pull-down control center");
                ui.radio_value(&mut draft.layout, Layout::RightBar, "Right-hand bar");
                ui.separator();
                ui.label("Clock");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut draft.clock_style, tideshell_core::clock::ClockStyle::Digital, "Digital");
                    ui.radio_value(&mut draft.clock_style, tideshell_core::clock::ClockStyle::Analog, "Analog");
                });
                ui.horizontal_wrapped(|ui| {
                    for f in tideshell_core::clock::Fuzzy::ALL {
                        ui.radio_value(&mut draft.clock_fuzzy, f, f.label());
                    }
                });
                ui.label("3D clock (digital, exact time)");
                ui.horizontal_wrapped(|ui| {
                    for t in tideshell_core::clock3d::Trigger::ALL {
                        ui.radio_value(&mut draft.clock_3d, t, t.label());
                    }
                });
                ui.add_enabled_ui(draft.clock_3d != tideshell_core::clock3d::Trigger::Off, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for st in tideshell_core::clock3d::Style::ALL {
                            ui.radio_value(&mut draft.clock_3d_style, st, st.label());
                        }
                    });
                });
                ui.separator();
                ui.label("Screen sleep when idle (triple tap or power key wakes)");
                ui.horizontal_wrapped(|ui| {
                    for (secs, label) in [(0, "Never"), (30, "30 s"), (60, "1 min"), (120, "2 min"), (300, "5 min")] {
                        ui.radio_value(&mut draft.sleep_idle_s, secs, label);
                    }
                });
                ui.separator();
                ui.label("Trackpad cursor");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut draft.fat_cursor, false, "Normal");
                    ui.radio_value(&mut draft.fat_cursor, true, "Fat (iPad-style, eased)");
                });
                ui.separator();
                ui.label("Switcher");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut draft.switcher_style, Style::Grid, "Grid");
                    ui.radio_value(&mut draft.switcher_style, Style::Deck, "Deck");
                });
                ui.separator();
                ui.add_enabled_ui(draft.layout == Layout::RightBar, |ui| {
                    ui.label("Bar widgets (top to bottom)");
                    let mut op: Option<(usize, i32)> = None;
                    egui::ScrollArea::vertical().id_salt("ts-bar-edit").max_height(260.0).show(ui, |ui| {
                        for (i, w) in draft.bar_widgets.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.monospace(w.token());
                                if ui.small_button("Up").clicked() {
                                    op = Some((i, -1));
                                }
                                if ui.small_button("Down").clicked() {
                                    op = Some((i, 1));
                                }
                                if ui.small_button("Remove").clicked() {
                                    op = Some((i, 0));
                                }
                            });
                        }
                    });
                    match op {
                        Some((i, 0)) => bar::remove_widget(&mut draft.bar_widgets, i),
                        Some((i, d)) => {
                            let to = (i as i32 + d).clamp(0, draft.bar_widgets.len() as i32 - 1) as usize;
                            bar::move_widget(&mut draft.bar_widgets, i, to);
                        }
                        None => {}
                    }
                    // Same layout call the bar uses, so the warning cannot
                    // disagree with what will actually be drawn.
                    let unit = draft.bar_width.min(60.0);
                    let over = bar::layout(&draft.bar_widgets, shell.screen.1 - 28.0, unit).overflow;
                    if !over.is_empty() {
                        let names: Vec<String> = over.iter().map(|i| draft.bar_widgets[*i].token()).collect();
                        ui.colored_label(ui.visuals().warn_fg_color, format!("Won't fit on this screen: {}", names.join(", ")));
                    }
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("ts-bar-add")
                            .selected_text(catalog.get(sheet.pick).map(|w| w.token()).unwrap_or_default())
                            .show_ui(ui, |ui| {
                                for (i, w) in catalog.iter().enumerate() {
                                    ui.selectable_value(&mut sheet.pick, i, w.token());
                                }
                            });
                        if ui.button("Add").clicked() {
                            if let Some(w) = catalog.get(sheet.pick) {
                                draft.bar_widgets.push(w.clone());
                            }
                        }
                    });
                });
                ui.separator();
                ui.label("Cold mode (gloves: volume keys steer, flicks act)");
                ui.horizontal(|ui| {
                    for (v, label) in [("auto", "Auto"), ("on", "On"), ("off", "Off")] {
                        ui.radio_value(&mut draft.cold, v.to_string(), label);
                    }
                });
                ui.separator();
                ui.label("Rotation");
                ui.checkbox(&mut draft.autorotate, "Auto-rotate");
                ui.checkbox(&mut draft.rotate_flip, "Landscape is mirrored on this panel (flip 90/270)");
                ui.separator();
                ui.label("Home widget (tap the clock on the cover to cycle)");
                ui.horizontal(|ui| {
                    use tideshell_core::prefs::HomeWidget as H;
                    for (w, label) in [(H::Clock, "Clock"), (H::Snapclone, "Snapclone id"), (H::Temperature, "Temperature")] {
                        ui.radio_value(&mut draft.home_widget, w, label);
                    }
                });
                ui.checkbox(&mut draft.living, "Living lock screen (rear camera glass; uses the GPU)");
                ui.horizontal(|ui| {
                    ui.label("Lock-screen night black (OLED):");
                    for (v, label) in [("on", "auto"), ("off", "old"), ("ab", "A/B"), ("force", "force night")] {
                        ui.radio_value(&mut draft.lock_night, v.to_string(), label);
                    }
                });
                ui.separator();
                ui.label("Unlock pattern (power key: . short, - long, 3+ symbols; empty = swipe only)");
                ui.add(egui::TextEdit::singleline(&mut draft.unlock_morse).id(egui::Id::new("ts-morse")).hint_text("..-").desired_width(160.0));
                if !draft.unlock_morse.is_empty() && tideshell_core::morse::parse(&draft.unlock_morse).is_none() {
                    ui.colored_label(ui.visuals().warn_fg_color, "needs 3 or more of '.' and '-'; not saved as typed");
                }
                ui.separator();
                ui.label("Accessibility");
                ui.checkbox(&mut draft.digital_dim, "Dim below the panel's floor");
                ui.label(
                    egui::RichText::new(
                        "Below 5% the pixels dim as well as the backlight, reaching about a tenth of the panel's lowest step. \
                         For dark rooms. Costs some bit depth, so the darkest greys may band.",
                    )
                    .small()
                    .weak(),
                );
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Apply").clicked() {
                        applied = Some(draft.clone());
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
              });
            });
        if close || applied.is_some() {
            sheet.open = false;
        }
        applied
    }

    // ---- trace -------------------------------------------------------------------------

    fn trace(&self, p: &egui::Painter, shell: &Shell, pal: &PaletteColors, origin: Pos2) {
        let lines: Vec<&String> = shell.trace().collect();
        let tail = &lines[lines.len().saturating_sub(16)..];
        // Bottom, above the home indicator: the top is where the drawer and
        // switcher header live, and the trace must not hide what it explains.
        let h = 26.0 + 14.0 * tail.len() as f32;
        let r = Rect::from_min_size(origin + Vec2::new(6.0, shell.screen.1 - h - 28.0), Vec2::new(shell.screen.0 - 12.0, h));
        p.rect_filled(r, cr(8), Color32::from_black_alpha(190));
        let head = format!(
            "edge:{} drawer:{:.2} kb:{} pad:{} carousel:{} streak:{}",
            shell.recognizer.phase_label(),
            shell.drawer.progress,
            shell.modes.keyboard as u8,
            shell.modes.trackpad as u8,
            shell.carousel.is_active() as u8,
            shell.switcher.is_streaking() as u8
        );
        p.text(r.min + Vec2::new(8.0, 12.0), Align2::LEFT_CENTER, head, FontId::monospace(10.5), pal.accent);
        for (i, l) in tail.iter().enumerate() {
            p.text(r.min + Vec2::new(8.0, 28.0 + 14.0 * i as f32), Align2::LEFT_CENTER, l.as_str(), FontId::monospace(10.0), Color32::from_gray(225));
        }
    }
}

fn short(label: &str) -> String {
    label.chars().take(9).collect()
}

#[allow(clippy::too_many_arguments)]
fn app_button(ui: &mut egui::Ui, p: &egui::Painter, r: Rect, id: Id, name: &str, app_id: &str, c: Pos2, unit: f32, _pal: &PaletteColors) -> bool {
    p.circle_filled(c, unit * 0.36, app_color(app_id));
    p.text(c, Align2::CENTER_CENTER, initial(name), FontId::proportional(14.0), Color32::WHITE);
    ui.interact(r, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_peek_strip_never_asks_two_parts_for_the_same_pixels() {
        for h10 in 800..14000 {
            let h = h10 as f32 / 10.0;
            for has in [false, true] {
                let l = peek_layout(h, has);
                let used = PEEK_INITIAL_H + PEEK_CHEVRON_H + l.title_px + if l.stats { PEEK_STATS_H } else { 0.0 };
                if h >= PEEK_INITIAL_H + PEEK_CHEVRON_H {
                    assert!(used <= h + 0.001, "h={h} has={has}: {l:?} uses {used}");
                }
                assert!(l.title_px >= 0.0);
                assert!(!l.stats || has, "stats only when there are numbers");
                assert!(!l.stats || l.title_px >= PEEK_TITLE_KEEP, "stats never squeeze the title below its keep: h={h}");
            }
        }
        // the two real cases from the phone: tall portrait strip, short landscape strip
        let portrait = peek_layout(1060.0, true);
        assert!(portrait.stats && portrait.title_px > 800.0, "{portrait:?}");
        let landscape = peek_layout(220.0, true);
        assert!(!landscape.stats, "no room for numbers beside a title on a 220 pt strip: {landscape:?}");
        assert!(landscape.title_px >= PEEK_MIN_TITLE, "but the title still shows: {landscape:?}");
        assert!(peek_layout(80.0, true).title_px < PEEK_MIN_TITLE, "a stub of a strip shows only the initial and the chevron");
    }

    #[test]
    fn the_whole_quick_settings_sheet_fits_the_short_landscape_screen() {
        // 1224x2700 at 2.25x: 544 x 1200 logical. Landscape is 1200 x 544.
        let n = QuickKind::ALL.len();
        let rows = |cols: usize| n.div_ceil(cols);
        let portrait = ShellView::drawer_content_height(rows(5));
        let landscape = ShellView::drawer_content_height(rows(8));
        assert!(portrait <= 1200.0 - 16.0, "portrait sheet {portrait}");
        assert!(landscape <= 544.0 - 16.0, "landscape sheet {landscape} must fit a 544 high screen with its settings entry");
        // The old cap (66% of the height) is what hid the settings entry in landscape.
        assert!(landscape > 544.0 * 0.66, "the test is only meaningful if the old 66% cap cut the sheet short ({landscape} vs {})", 544.0 * 0.66);
    }

    use super::*;

    #[test]
    fn app_color_is_stable_and_spreads() {
        assert_eq!(app_color("org.gnome.Console"), app_color("org.gnome.Console"));
        let distinct: std::collections::HashSet<_> =
            ["a", "b", "firefox", "org.gnome.Console", "cosmic-term"].iter().map(|s| app_color(s)).collect();
        assert!(distinct.len() >= 4);
    }

    #[test]
    fn pad_sits_bottom_and_clear_of_bar() {
        let r = ShellView::pad_rect(Vec2::new(408.0, 900.0), 64.0);
        assert!(r.right() <= 408.0 - 64.0 && r.bottom() < 900.0 && r.height() >= 180.0);
    }

    #[test]
    fn overlay_is_unmapped_when_idle_and_input_follows_what_is_shown() {
        use tideshell_core::prefs::ShellPrefs;
        let view = ShellView::default();
        let mut sh = Shell::new(ShellPrefs::default(), (408.0, 900.0));
        assert!(!view.needs_overlay(&sh), "idle pull-down shell needs no overlay");
        assert_eq!(view.input_rects(&sh), Some(vec![]), "and takes no input");
        sh.toggle_mode(tideshell_core::hold::HoldOption::Trackpad);
        assert!(view.needs_overlay(&sh));
        let r = view.input_rects(&sh).unwrap();
        assert_eq!(r.len(), 1, "only the pad takes input");
        assert!(r[0].contains(Pos2::new(200.0, 800.0)) && !r[0].contains(Pos2::new(200.0, 100.0)));
        sh.open_switcher();
        assert_eq!(view.input_rects(&sh), None, "switcher is modal: whole screen");
    }

    #[test]
    fn bar_on_its_own_surface_frees_the_overlay() {
        use tideshell_core::prefs::{Layout, ShellPrefs};
        let mut view = ShellView::default();
        view.bar_surface = true;
        let mut p = ShellPrefs::default();
        p.layout = Layout::RightBar;
        let mut sh = Shell::new(p, (408.0, 900.0));
        assert!(!view.needs_overlay(&sh), "idle right-bar shell: bar surface only, no overlay");
        assert_eq!(view.input_rects(&sh), Some(vec![]));
        assert_eq!(ShellView::bar_geometry(&sh), Some((64, 64)));
        sh.set_bar_expanded(true);
        let (w, reserve) = ShellView::bar_geometry(&sh).unwrap();
        assert!(w > 64 && reserve == 64, "expanding widens the surface, not the reservation: {w} {reserve}");
    }

    #[test]
    fn right_bar_keeps_overlay_mapped_with_bar_input_only() {
        use tideshell_core::prefs::{Layout, ShellPrefs};
        let view = ShellView::default();
        let mut p = ShellPrefs::default();
        p.layout = Layout::RightBar;
        let sh = Shell::new(p, (408.0, 900.0));
        assert!(view.needs_overlay(&sh));
        let r = view.input_rects(&sh).unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].contains(Pos2::new(400.0, 450.0)) && !r[0].contains(Pos2::new(100.0, 450.0)));
    }

    /// Drive ShellView through a real egui frame with a mouse press + drag
    /// on the pad; return the trackpad pointer actions it produced.
    fn mouse_drag_on_pad(pointer_pad: bool) -> Vec<Action> {
        use tideshell_core::prefs::ShellPrefs;
        let mut sh = Shell::new(ShellPrefs::default(), (408.0, 900.0));
        sh.toggle_mode(tideshell_core::hold::HoldOption::Trackpad);
        let mut view = ShellView::default();
        view.pointer_pad = pointer_pad;
        let pal = uikit_egui::palette_colors("night");
        let info = FrameInfo { apps: &[], time: tideshell_core::clock::LocalTime { hour: 12, minute: 0, weekday: 1 }, battery: None, charging: false, trackpad_note: None, t_secs: 0.0 };
        let ctx = egui::Context::default();
        let mut out = Vec::new();
        let frames = [
            vec![egui::Event::PointerMoved(Pos2::new(200.0, 700.0)),
                 egui::Event::PointerButton { pos: Pos2::new(200.0, 700.0), button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() }],
            vec![egui::Event::PointerMoved(Pos2::new(240.0, 700.0))],
            vec![egui::Event::PointerMoved(Pos2::new(280.0, 700.0))],
        ];
        for (i, mut events) in frames.into_iter().enumerate() {
            out.extend(view.route_raw(&mut sh, &mut events, Pos2::ZERO, 1000 + 16 * i as u64));
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(408.0, 900.0))),
                time: Some(1.0 + i as f64 * 0.016),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(raw, |ui| out.extend(view.ui(ui, &mut sh, &pal, &info)));
        }
        out.into_iter().filter(|a| matches!(a, Action::Pointer(_))).collect()
    }

    #[test]
    fn mouse_cannot_drive_the_pad_unless_allowed() {
        // Allowed (simulator fake cursor): the drag reaches the trackpad.
        let allowed = mouse_drag_on_pad(true);
        assert!(!allowed.is_empty(), "scan must find pad motion when allowed");
        // Default (real shell, EI output): no pointer output from the mouse.
        assert!(mouse_drag_on_pad(false).is_empty(), "mouse on pad must not feed EI");
    }

    fn touch(id: u64, phase: egui::TouchPhase, x: f32, y: f32) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(0), id: egui::TouchId(id), phase, pos: Pos2::new(x, y), force: None }
    }
    fn press(x: f32, y: f32, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos: Pos2::new(x, y), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    #[test]
    fn edge_owned_sequence_never_reaches_egui_other_touches_do() {
        use tideshell_core::prefs::ShellPrefs;
        let mut sh = Shell::new(ShellPrefs::default(), (408.0, 900.0));
        let mut view = ShellView::default();
        // Bottom-edge swipe: Touch + the pointer copies a touchscreen emits.
        let mut ev = vec![touch(1, egui::TouchPhase::Start, 200.0, 895.0), egui::Event::PointerMoved(Pos2::new(200.0, 895.0)), press(200.0, 895.0, true)];
        view.route_raw(&mut sh, &mut ev, Pos2::ZERO, 0);
        assert!(ev.is_empty(), "owned touch + its pointer copies removed: {ev:?}");
        let mut ev = vec![touch(1, egui::TouchPhase::Move, 200.0, 700.0), egui::Event::PointerMoved(Pos2::new(200.0, 700.0))];
        view.route_raw(&mut sh, &mut ev, Pos2::ZERO, 50);
        assert!(ev.is_empty());
        assert!(sh.swipe_up > 0.0, "and the recognizer got it");
        let mut ev = vec![touch(1, egui::TouchPhase::End, 200.0, 700.0)];
        view.route_raw(&mut sh, &mut ev, Pos2::ZERO, 60);
        assert!(ev.is_empty());
        // A touch in the middle of the screen is not the shell's: untouched.
        let mid = vec![touch(2, egui::TouchPhase::Start, 200.0, 400.0), egui::Event::PointerMoved(Pos2::new(200.0, 400.0)), press(200.0, 400.0, true)];
        let mut ev = mid.clone();
        view.route_raw(&mut sh, &mut ev, Pos2::ZERO, 100);
        assert_eq!(ev, mid, "non-owned events pass through unchanged");
    }

    #[test]
    fn mouse_edge_drag_is_removed_too() {
        use tideshell_core::prefs::ShellPrefs;
        let mut sh = Shell::new(ShellPrefs::default(), (408.0, 900.0));
        let mut view = ShellView::default();
        let mut ev = vec![egui::Event::PointerMoved(Pos2::new(200.0, 4.0)), press(200.0, 4.0, true), egui::Event::PointerMoved(Pos2::new(200.0, 300.0)), press(200.0, 300.0, false)];
        view.route_raw(&mut sh, &mut ev, Pos2::ZERO, 0);
        // Only the hover before the press survives; press/drag/release are the recognizer's.
        assert_eq!(ev, vec![egui::Event::PointerMoved(Pos2::new(200.0, 4.0))]);
    }

    #[test]
    fn keys_become_egui_keys_and_text() {
        let ev = key_events(true, 0x61, "a", false, false, false, false);
        assert!(matches!(ev[0], egui::Event::Key { key: egui::Key::A, pressed: true, .. }));
        assert_eq!(ev[1], egui::Event::Text("a".into()));
        assert_eq!(key_events(false, 0x61, "", false, false, false, false).len(), 1, "release: key only, no text");
        let bs = key_events(true, 0xff08, "\u{8}", false, false, false, false);
        assert_eq!(bs.len(), 1, "backspace is a key, its control char is not text");
        assert!(matches!(bs[0], egui::Event::Key { key: egui::Key::Backspace, .. }));
        let ctrl = key_events(true, 0x61, "a", true, false, false, false);
        assert_eq!(ctrl.len(), 1, "ctrl+a: key only, no text");
        assert!(matches!(ctrl[0], egui::Event::Key { modifiers, .. } if modifiers.ctrl && modifiers.command));
        let e = key_events(true, 0xe9, "\u{e9}", false, false, false, false);
        assert_eq!(e, vec![egui::Event::Text("\u{e9}".into())], "non-ASCII text has no egui key but is text");
    }

    #[test]
    fn initial_skips_punctuation() {
        assert_eq!(initial("-cosmic"), "C");
        assert_eq!(initial(""), "?");
    }
}
