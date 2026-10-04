//! layer — tideshell on real layer-shell surfaces.
//!
//! One Wayland connection owns:
//!
//! * three **edge strips** (top, bottom, right): transparent, input-only
//!   `zwlr_layer_surface_v1`s on the overlay layer. A finger that lands on a
//!   strip stays with it for the whole sequence (the compositor's implicit
//!   touch/pointer grab), so a swipe that leaves the strip keeps reporting,
//!   in strip-local coordinates that go negative or past its size. That is
//!   what lets a 28 px strip recognise a swipe across the whole screen, and
//!   why no compositor gesture patch is needed.
//! * one **overlay**: fullscreen, mapped only while `ShellView::needs_overlay`
//!   says so, rendered with egui through `egui_glow` on EGL, with its input
//!   region set from `ShellView::input_rects` so passive overlays let taps
//!   through to apps.
//!
//! The pointer is treated as one finger (slot -1, left button = contact).
//! That is how this is exercised in a nested cosmic-comp, which turns the X
//! mouse into a Wayland pointer; on the phone the same code gets `wl_touch`.
//!
//! Screen size comes from the strips themselves: the bottom strip is anchored
//! left+right, so its configured width is the output width; the right strip
//! is anchored top+bottom, so its height is the output height.

use glow::HasContext as _;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Write;
use std::os::fd::AsFd;
use std::sync::Arc;

use khronos_egl as egl;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_keyboard::{self, WlKeyboard},
    wl_output::WlOutput,
    wl_callback::WlCallback,
    wl_compositor::WlCompositor,
    wl_pointer::{self, WlPointer},
    wl_region::WlRegion,
    wl_registry,
    wl_seat::{self, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::{self, WlSurface},
    wl_touch::{self, WlTouch},
};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{self, WpFractionalScaleV1},
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols::wp::alpha_modifier::v1::client::{
    wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1, wp_alpha_modifier_v1::WpAlphaModifierV1,
};
use wayland_protocols::ext::background_effect::v1::client::{
    ext_background_effect_manager_v1::{self, ExtBackgroundEffectManagerV1},
    ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::ZwlrOutputModeV1,
};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1,
    zwlr_output_power_v1::{self, Mode as PowerMode, ZwlrOutputPowerV1},
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

pub const BTN_LEFT: u32 = 0x110;

/// What the pointer should look like over a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorWant {
    Shape(Shape),
    /// No cursor image (fat cursor mode: the shell draws its own).
    Hidden,
}

/// egui's requested cursor, as a cursor-shape name. Pure.
pub fn shape_for(icon: egui::CursorIcon) -> Shape {
    use egui::CursorIcon as C;
    match icon {
        C::PointingHand => Shape::Pointer,
        C::Text | C::VerticalText => Shape::Text,
        C::Grab => Shape::Grab,
        C::Grabbing => Shape::Grabbing,
        C::Move | C::AllScroll => Shape::Move,
        C::ResizeHorizontal | C::ResizeEast | C::ResizeWest | C::ResizeColumn => Shape::EwResize,
        C::ResizeVertical | C::ResizeNorth | C::ResizeSouth | C::ResizeRow => Shape::NsResize,
        C::NotAllowed | C::NoDrop => Shape::NotAllowed,
        C::Wait => Shape::Wait,
        C::Progress => Shape::Progress,
        C::Crosshair => Shape::Crosshair,
        _ => Shape::Default,
    }
}
pub const POINTER_SLOT: i32 = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    Top,
    Bottom,
    Right,
    Overlay,
    /// The right-hand bar: its own surface on the Top layer, with an
    /// exclusive zone so maximized windows are laid out beside it.
    Bar,
    /// Carousel peeks: Top-layer side surfaces with exclusive zones, so the
    /// active window is laid out in the centre; a tap cycles.
    PeekLeft,
    PeekRight,
}

impl Role {
    /// Surfaces rendered with egui on their own EGL window.
    pub fn is_side(self) -> bool {
        matches!(self, Role::Bar | Role::PeekLeft | Role::PeekRight)
    }
}

/// Input in surface-local logical coordinates; `Io::to_screen` maps it.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Down { role: Role, slot: i32, x: f32, y: f32, t: u32 },
    Move { role: Role, slot: i32, x: f32, y: f32, t: u32 },
    Up { role: Role, slot: i32, t: u32 },
    /// Pointer motion with no button held (hover, for egui).
    Hover { role: Role, x: f32, y: f32 },
    /// A key on the overlay (the only surface that takes keyboard focus):
    /// keysym per xkbcommon, the UTF-8 it produces (empty for non-text),
    /// and modifiers.
    Key { pressed: bool, keysym: u32, utf8: String, ctrl: bool, alt: bool, shift: bool },
    Cancel,
}

/// Map strip-local coordinates to screen coordinates. Pure; see the module
/// header for why strip-local values outside the strip are expected.
pub fn strip_to_screen(role: Role, x: f32, y: f32, screen: (f32, f32), strip: (f32, f32)) -> (f32, f32) {
    match role {
        Role::Top | Role::Overlay => (x, y),
        Role::Bar | Role::PeekRight => (x + screen.0 - strip.0, y),
        Role::PeekLeft => (x, y),
        Role::Bottom => (x, y + screen.1 - strip.1),
        Role::Right => (x + screen.0 - strip.0, y),
    }
}

struct Surf {
    role: Role,
    wl: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    size: (u32, u32),
    configured: bool,
    /// A configure arrived that the runtime has not acted on yet.
    fresh: bool,
    scale: i32,
    closed: bool,
    /// Fractional scaling (EGL surfaces only): the exact scale from
    /// wp_fractional_scale_v1, and the viewport that maps the
    /// ceil(logical x scale) buffer back onto the logical size.
    frac: Option<WpFractionalScaleV1>,
    frac_scale: Option<f32>,
    viewport: Option<WpViewport>,
    /// Last (buffer scale, viewport destination) sent, so per-frame
    /// prepare() only emits requests when something changed (it was
    /// sending set_destination on every frame, seen in WAYLAND_DEBUG).
    sent: Option<(i32, (u32, u32))>,
    /// The bottom margin last asked of the compositor (the overlay ends this far above the screen's edge).
    margin_bottom: i32,
    /// ext-background-effect: the compositor blurs what is BEHIND this surface over the region set.
    /// Created on first use; `blur_for` is the surface size the region was set for (None = no blur).
    blur: Option<ExtBackgroundEffectSurfaceV1>,
    blur_for: Option<(u32, u32)>,
    /// wp-alpha-modifier: the compositor multiplies the whole surface (and what it composites over) by a
    /// factor, which is how the switcher fades out smoothly; `alpha_last` is the factor last sent.
    alpha: Option<WpAlphaModifierSurfaceV1>,
    alpha_last: Option<u32>,
}

impl Surf {
    /// (render scale, uses viewport). Fractional when the compositor offers
    /// both protocols and has sent a preferred scale; else the integer
    /// wl_surface.preferred_buffer_scale path.
    fn render_scale(&self) -> (f32, bool) {
        match (self.frac_scale, &self.viewport) {
            (Some(f), Some(_)) => (f, true),
            _ => (self.scale as f32, false),
        }
    }
}

/// Nudge a logical width (at most 4 points either way, the smaller nudge first, narrower on a tie) to
/// one whose PHYSICAL width at `scale` is a whole number of pixels. A surface is drawn into a buffer of
/// `ceil(width x scale)` pixels and mapped onto `width` points by the compositor: when `width x scale`
/// is not an integer (41 pt at 2.25 is 92.25 px) the buffer is a fraction of a pixel off the screen
/// area it fills and the whole surface is resampled, which is what made the carousel peeks' small
/// text soft. Integer scales and already-aligned widths come back unchanged.
pub fn snap_logical(width: u32, scale: f32) -> u32 {
    let aligned = |w: u32| {
        let px = w as f32 * scale;
        (px - px.round()).abs() < 0.003
    };
    if width == 0 || scale <= 0.0 || aligned(width) {
        return width;
    }
    for d in 1..=4u32 {
        for w in [width.saturating_sub(d), width + d] {
            if w > 0 && aligned(w) {
                return w;
            }
        }
    }
    width
}

/// Buffer pixels for a logical size at a (possibly fractional) scale. The
/// protocol's rule: round up, so the viewport never stretches a short buffer.
pub fn buffer_px(logical: (u32, u32), scale: f32) -> (i32, i32) {
    ((logical.0 as f32 * scale).ceil() as i32, (logical.1 as f32 * scale).ceil() as i32)
}

/// egui's screen, in points, for a buffer of `size_px` at `scale`.
///
/// The buffer is ceil(logical x scale) pixels; telling egui the screen is
/// `logical` points leaves it painting only logical x scale pixels, a
/// fractional amount, so the last column and row (right and bottom) were
/// cleared-to-transparent or half-covered: a ~1 px strip where whatever is
/// underneath showed through (seen on the FP5 lock screen at a fractional
/// scale). Sizing the screen from the buffer covers every pixel; the
/// viewport maps it back onto the logical size. At an integer scale the two
/// are identical.
pub fn screen_points(size_px: (i32, i32), scale: f32) -> (f32, f32) {
    (size_px.0 as f32 / scale, size_px.1 as f32 / scale)
}

/// Where and at what scale to render an egui surface this frame.
pub struct EguiTarget {
    pub surface: WlSurface,
    pub logical: (u32, u32),
    /// egui pixels-per-point; fractional when `viewport` is true.
    pub scale: f32,
    pub viewport: bool,
    int_scale: i32,
}

impl EguiTarget {
    pub fn size_px(&self) -> (i32, i32) {
        buffer_px(self.logical, self.scale)
    }

    /// egui's screen size in points: the whole BUFFER, not the logical size.
    /// See [`screen_points`].
    pub fn screen_points(&self) -> (f32, f32) {
        screen_points(self.size_px(), self.scale)
    }

    /// Per-frame surface state for this scale path, sent only on change.
    /// Viewport path: buffer scale 1 and a viewport destination of the
    /// logical size. Integer path: buffer scale n. Takes effect with the
    /// frame's commit (eglSwapBuffers).
    pub fn prepare(&self, io: &mut Io) {
        let want = (if self.viewport { 1 } else { self.int_scale }, if self.viewport { self.logical } else { (0, 0) });
        let Some(s) = io.st.surfs.iter_mut().find(|s| s.wl == self.surface) else { return };
        if s.sent == Some(want) {
            return;
        }
        s.sent = Some(want);
        self.surface.set_buffer_scale(want.0);
        if let (true, Some(v)) = (self.viewport, &s.viewport) {
            v.set_destination(self.logical.0 as i32, self.logical.1 as i32);
        }
    }
}

pub struct State {
    surfs: Vec<Surf>,
    input: Vec<Input>,
    pointer: Option<WlPointer>,
    touch: Option<WlTouch>,
    ptr_role: Option<Role>,
    ptr_pos: (f32, f32),
    ptr_down: bool,
    touch_roles: HashMap<i32, (Role, (f32, f32))>,
    keyboard: Option<WlKeyboard>,
    xkb_ctx: xkbcommon::xkb::Context,
    xkb: Option<xkbcommon::xkb::State>,
    /// (rate per second, delay ms) from wl_keyboard.repeat_info.
    pub repeat: (i32, i32),
    /// zwlr_output_power_v1 per output, and the last mode each reported.
    powers: Vec<ZwlrOutputPowerV1>,
    /// Per output, by index: the last mode the COMPOSITOR reported (Some(on)).
    /// These power objects live for the connection, so the compositor reports
    /// every change -- including its own lock-screen and idle blanking, which
    /// tideshell did not ask for and otherwise would not know about.
    power_modes: Vec<Option<bool>>,
    pub power_status: String,
    /// The compositor said it can blur behind surfaces (ext-background-effect `capabilities`).
    blur_caps: bool,
    /// ext-idle-notify: an edge the loop has not consumed yet (true = the user
    /// went idle for the configured time, false = input resumed).
    idle_edge: Option<bool>,
    /// wlr-output-management: the heads the compositor listed, the serial
    /// of the latest `done`, and the result of our last configuration.
    out_mgr: Option<ZwlrOutputManagerV1>,
    heads: Vec<Head>,
    out_serial: Option<u32>,
    pub config_status: String,
    cursor_mgr: Option<WpCursorShapeManagerV1>,
    shape_device: Option<WpCursorShapeDeviceV1>,
    /// Serial of the last pointer enter: set_shape / set_cursor must quote it.
    enter_serial: Option<u32>,
    overlay_cursor: CursorWant,
    applied: Option<(Role, CursorWant)>,
}

impl State {
    /// Apply the wanted cursor for the surface the pointer is over. Strips
    /// get the default arrow; the overlay whatever the view asked for.
    fn apply_cursor(&mut self) {
        let (Some(role), Some(serial), Some(pointer)) = (self.ptr_role, self.enter_serial, self.pointer.clone()) else { return };
        let want = if role == Role::Overlay { self.overlay_cursor } else { CursorWant::Shape(Shape::Default) };
        if self.applied == Some((role, want)) {
            return;
        }
        match (want, &self.shape_device) {
            // A null cursor surface hides the pointer over our surface.
            (CursorWant::Hidden, _) => pointer.set_cursor(serial, None, 0, 0),
            (CursorWant::Shape(s), Some(dev)) => dev.set_shape(serial, s),
            // No cursor-shape manager: leave the compositor's choice alone
            // rather than hide it (the old, invisible-cursor behaviour).
            (CursorWant::Shape(_), None) => return,
        }
        self.applied = Some((role, want));
    }
}

impl State {
    fn role_of(&self, s: &WlSurface) -> Option<Role> {
        self.surfs.iter().find(|x| &x.wl == s).map(|x| x.role)
    }
    fn surf_mut(&mut self, l: &ZwlrLayerSurfaceV1) -> Option<&mut Surf> {
        self.surfs.iter_mut().find(|x| &x.layer == l)
    }
}

pub struct Io {
    conn: Connection,
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    st: State,
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
    _seat: Option<WlSeat>,
    frac_mgr: Option<WpFractionalScaleManagerV1>,
    viewporter: Option<WpViewporter>,
    blur_mgr: Option<ExtBackgroundEffectManagerV1>,
    alpha_mgr: Option<WpAlphaModifierV1>,
    pub strip_px: u32,
    /// Tint the strips so they can be seen (`TIDESHELL_SHOW_STRIPS=1`).
    pub show_strips: bool,
    right_enabled: bool,
    /// (surface width, exclusive zone) per side surface (bar, peeks).
    side_sizes: HashMap<Role, (u32, i32)>,
    _outputs: Vec<WlOutput>,
    /// The overlay currently asks for exclusive keyboard focus.
    overlay_keyboard: bool,
    /// ext-idle-notify: the notifier, the live notification and its timeout (0 = none).
    idle_notifier: Option<ExtIdleNotifierV1>,
    idle_note: Option<ExtIdleNotificationV1>,
    idle_ms: u32,
}

impl Io {
    pub fn connect(strip_px: u32, show_strips: bool) -> Result<Io, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("wayland: {e}"))?;
        let (globals, queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("registry: {e}"))?;
        let qh = queue.handle();
        // v6 for wl_surface.preferred_buffer_scale; older compositors still work at scale 1.
        let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).map_err(|_| "no wl_compositor")?;
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).map_err(|_| "no wl_shm")?;
        let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).map_err(|_| "no zwlr_layer_shell_v1")?;
        let seat: Option<WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
        let frac_mgr: Option<WpFractionalScaleManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
        // Panel off/on without suspending: one power object per output.
        let power_mgr: Option<ZwlrOutputPowerManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
        let out_mgr: Option<ZwlrOutputManagerV1> = globals.bind(&qh, 1..=4, ()).ok();
        let outputs: Vec<WlOutput> = globals.contents().with_list(|list| {
            list.iter().filter(|g| g.interface == "wl_output").map(|g| globals.registry().bind::<WlOutput, _, _>(g.name, g.version.min(4), &qh, ())).collect()
        });
        let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
        // The compositor's own backdrop blur, for the switcher and palette sheets.
        let blur_mgr: Option<ExtBackgroundEffectManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
        let alpha_mgr: Option<WpAlphaModifierV1> = globals.bind(&qh, 1..=1, ()).ok();
        // Idle, as the compositor measures it: v1's get_idle_notification HONOURS idle
        // inhibitors, so a playing video (vidkit's wakelock) keeps the screen up.
        let idle_notifier: Option<ExtIdleNotifierV1> = globals.bind(&qh, 1..=1, ()).ok();
        let st = State {
            surfs: Vec::new(),
            input: Vec::new(),
            pointer: None,
            touch: None,
            ptr_role: None,
            ptr_pos: (0.0, 0.0),
            ptr_down: false,
            touch_roles: HashMap::new(),
            keyboard: None,
            xkb_ctx: xkbcommon::xkb::Context::new(xkbcommon::xkb::CONTEXT_NO_FLAGS),
            xkb: None,
            repeat: (25, 400),
            powers: Vec::new(),
            power_modes: Vec::new(),
            power_status: String::new(),
            blur_caps: false,
            idle_edge: None,
            out_mgr: None,
            heads: Vec::new(),
            out_serial: None,
            config_status: String::new(),
            cursor_mgr: globals.bind(&qh, 1..=1, ()).ok(),
            shape_device: None,
            enter_serial: None,
            overlay_cursor: CursorWant::Shape(Shape::Default),
            applied: None,
        };
        let mut io = Io { conn, queue, qh, st, compositor, shm, layer_shell, _seat: seat, frac_mgr, viewporter, blur_mgr, alpha_mgr, strip_px, show_strips, right_enabled: false, side_sizes: HashMap::new(), _outputs: outputs, overlay_keyboard: false, idle_notifier, idle_note: None, idle_ms: 0 };
        io.st.out_mgr = out_mgr;
        if let Some(m) = &power_mgr {
            io.st.powers = io._outputs.iter().enumerate().map(|(i, o)| m.get_output_power(o, &io.qh, i)).collect();
            io.st.power_modes = vec![None; io.st.powers.len()];
            io.st.power_status = format!("output power: {} output(s)", io.st.powers.len());
        } else {
            io.st.power_status = "output power: zwlr_output_power_manager_v1 not offered".into();
        }
        for r in [Role::Top, Role::Bottom, Role::Right] {
            io.create(r);
        }
        io.flush();
        Ok(io)
    }

    pub fn display_ptr(&self) -> *mut c_void {
        self.conn.backend().display_ptr() as *mut c_void
    }

    fn create(&mut self, role: Role) {
        let wl = self.compositor.create_surface(&self.qh, ());
        // Strips and overlay sit above everything (including fullscreen apps);
        // the bar is ordinary panel furniture on the Top layer.
        let layer_kind = if role.is_side() { zwlr_layer_shell_v1::Layer::Top } else { zwlr_layer_shell_v1::Layer::Overlay };
        let layer = self.layer_shell.get_layer_surface(&wl, None, layer_kind, "tideshell".into(), &self.qh, ());
        let px = self.strip_px;
        let (anchor, size) = match role {
            Role::Top => (Anchor::Top | Anchor::Left | Anchor::Right, (0, px)),
            Role::Bottom => (Anchor::Bottom | Anchor::Left | Anchor::Right, (0, px)),
            Role::Right => (Anchor::Top | Anchor::Bottom | Anchor::Right, (if self.right_enabled { px } else { 1 }, 0)),
            Role::Overlay => (Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right, (0, 0)),
            Role::Bar | Role::PeekRight => (Anchor::Top | Anchor::Bottom | Anchor::Right, (self.side_size(role).0.max(1), 0)),
            Role::PeekLeft => (Anchor::Top | Anchor::Bottom | Anchor::Left, (self.side_size(role).0.max(1), 0)),
        };
        layer.set_anchor(anchor);
        layer.set_size(size.0, size.1);
        // -1: ignore other surfaces' exclusive zones, so strips sit on the
        // true screen edge even under a panel, and the overlay is fullscreen.
        if role.is_side() {
            // The bar reserves its collapsed width only: expanding overlaps
            // windows (a drawer), it does not reflow them on every expand.
            layer.set_exclusive_zone(self.side_size(role).1);
        } else {
            layer.set_exclusive_zone(-1);
        }
        layer.set_keyboard_interactivity(if role == Role::Overlay && self.overlay_keyboard { KeyboardInteractivity::Exclusive } else { KeyboardInteractivity::None });
        // EGL surfaces get fractional scaling; strips are transparent, so
        // their scale is irrelevant.
        let (frac, viewport) = if matches!(role, Role::Overlay | Role::Bar) {
            match (&self.frac_mgr, &self.viewporter) {
                (Some(m), Some(v)) => (Some(m.get_fractional_scale(&wl, &self.qh, ())), Some(v.get_viewport(&wl, &self.qh, ()))),
                _ => (None, None),
            }
        } else {
            (None, None)
        };
        wl.commit();
        self.st.surfs.push(Surf { role, wl, layer, size: (0, 0), configured: false, fresh: false, scale: 1, closed: false, frac, frac_scale: None, viewport, sent: None, margin_bottom: 0, blur: None, blur_for: None, alpha: None, alpha_last: None });
        if role == Role::Right && !self.right_enabled {
            self.set_input_region(Role::Right, Some(&[]));
        }
    }

    fn surf(&self, role: Role) -> Option<&Surf> {
        self.st.surfs.iter().find(|s| s.role == role && !s.closed)
    }

    /// Right-edge swipe-in is only live in the right-bar layout; otherwise the
    /// strip shrinks to 1 px with no input (it still reports screen height).
    pub fn set_right_enabled(&mut self, on: bool) {
        if on == self.right_enabled {
            return;
        }
        self.right_enabled = on;
        if let Some(s) = self.surf(Role::Right) {
            s.layer.set_size(if on { self.strip_px } else { 1 }, 0);
            s.wl.commit();
        }
        self.set_input_region(Role::Right, if on { None } else { Some(&[]) });
    }

    /// `None` = whole surface; `Some(&[])` = no input at all.
    pub fn set_input_region(&self, role: Role, rects: Option<&[(i32, i32, i32, i32)]>) {
        let Some(s) = self.surf(role) else { return };
        match rects {
            None => s.wl.set_input_region(None),
            Some(rs) => {
                let region = self.compositor.create_region(&self.qh, ());
                for (x, y, w, h) in rs {
                    region.add(*x, *y, *w, *h);
                }
                s.wl.set_input_region(Some(&region));
                region.destroy();
            }
        }
        s.wl.commit();
    }

    pub fn screen(&self) -> Option<(f32, f32)> {
        let w = self.surf(Role::Bottom).filter(|s| s.configured)?.size.0;
        let h = self.surf(Role::Right).filter(|s| s.configured)?.size.1;
        (w > 0 && h > 0).then_some((w as f32, h as f32))
    }

    pub fn to_screen(&self, role: Role, x: f32, y: f32) -> Option<(f32, f32)> {
        let screen = self.screen()?;
        let strip = self.surf(role).map(|s| (s.size.0 as f32, s.size.1 as f32))?;
        Some(strip_to_screen(role, x, y, screen, strip))
    }

    pub fn ensure_overlay(&mut self, on: bool) {
        let have = self.surf(Role::Overlay).is_some();
        if on && !have {
            self.create(Role::Overlay);
        } else if !on && have {
            if let Some(i) = self.st.surfs.iter().position(|s| s.role == Role::Overlay) {
                let s = self.st.surfs.remove(i);
                if let Some(f) = &s.frac {
                    f.destroy();
                }
                if let Some(v) = &s.viewport {
                    v.destroy();
                }
                if let Some(b) = &s.blur {
                    b.destroy();
                }
                if let Some(x) = &s.alpha {
                    x.destroy();
                }
                s.layer.destroy();
                s.wl.destroy();
            }
        }
    }

    /// Whether the compositor blurs behind surfaces on request. False until its capabilities event has
    /// arrived, and on a compositor without the protocol.
    pub fn blur_supported(&self) -> bool {
        self.blur_mgr.is_some() && self.st.blur_caps
    }

    /// Ask the compositor to blur what is behind `role`'s surface (all of it), or stop. Cheap to call every
    /// loop: it acts only when the wish or the surface size changed.
    ///
    /// End `role`'s surface `px` above the bottom of the screen (0 = to the edge). The on-screen keyboard is another
    /// overlay-layer surface; whichever of the two was created later is drawn on top, so after Tideshell restarts
    /// its scrim covers the keyboard. Leaving the strip the keyboard occupies out of the overlay makes the order
    /// not matter, and the sheet then simply ends where the keyboard begins.
    pub fn set_bottom_margin(&mut self, role: Role, px: i32) {
        let Some(i) = self.st.surfs.iter().position(|s| s.role == role) else { return };
        let px = px.max(0);
        if self.st.surfs[i].margin_bottom == px {
            return;
        }
        self.st.surfs[i].margin_bottom = px;
        self.st.surfs[i].layer.set_margin(0, 0, px, 0);
        self.st.surfs[i].wl.commit();
    }

    /// `keep_right` px along the right edge are left unblurred: the bar lives there on its own surface and
    /// stays in use under the switcher, so it must not look like part of the blurred backdrop.
    pub fn set_blur(&mut self, role: Role, on: bool, keep_right: u32) {
        let Some(mgr) = self.blur_mgr.clone().filter(|_| self.st.blur_caps) else { return };
        let Some(i) = self.st.surfs.iter().position(|s| s.role == role) else { return };
        let size = self.st.surfs[i].size;
        let size = (size.0.saturating_sub(keep_right), size.1);
        let want = on.then_some(size).filter(|s| s.0 > 0 && s.1 > 0);
        if self.st.surfs[i].blur_for == want {
            return;
        }
        if self.st.surfs[i].blur.is_none() {
            let obj = mgr.get_background_effect(&self.st.surfs[i].wl, &self.qh, ());
            self.st.surfs[i].blur = Some(obj);
        }
        let wl = self.st.surfs[i].wl.clone();
        let obj = self.st.surfs[i].blur.clone().expect("just created");
        match want {
            Some((w, h)) => {
                let region = self.compositor.create_region(&self.qh, ());
                region.add(0, 0, w as i32, h as i32);
                obj.set_blur_region(Some(&region));
                region.destroy();
            }
            None => obj.set_blur_region(None),
        }
        // Double-buffered state: applied at the next commit; a frame may not be due for a while.
        wl.commit();
        self.st.surfs[i].blur_for = want;
    }

    /// Fade a whole surface by `a` (0..1) in the compositor. Cheap every frame: only a changed factor is sent.
    /// The state applies at the surface's next commit, which the frame loop makes on every swap.
    pub fn set_surface_alpha(&mut self, role: Role, a: f32) {
        let Some(mgr) = self.alpha_mgr.clone() else { return };
        let Some(i) = self.st.surfs.iter().position(|s| s.role == role) else { return };
        let factor = (a.clamp(0.0, 1.0) as f64 * u32::MAX as f64) as u32;
        if self.st.surfs[i].alpha_last == Some(factor) {
            return;
        }
        if self.st.surfs[i].alpha.is_none() {
            let obj = mgr.get_surface(&self.st.surfs[i].wl, &self.qh, ());
            self.st.surfs[i].alpha = Some(obj);
        }
        if let Some(obj) = &self.st.surfs[i].alpha {
            obj.set_multiplier(factor);
        }
        self.st.surfs[i].alpha_last = Some(factor);
    }

    fn side_size(&self, role: Role) -> (u32, i32) {
        self.side_sizes.get(&role).copied().unwrap_or((0, 0))
    }

    /// Map/resize or unmap a side surface (bar or peek). `width` is the
    /// drawn width, `reserve` the exclusive zone.
    /// The output's scale as far as this runtime has heard it (any surface will do: they share the
    /// output), or 1.0 before the first surface has been told.
    pub fn scale_hint(&self) -> f32 {
        self.st.surfs.iter().find(|s| s.frac_scale.is_some() || s.configured).map_or(1.0, |s| s.render_scale().0)
    }

    /// `snap_logical` at the output's scale.
    pub fn snap_width(&self, w: u32) -> u32 {
        snap_logical(w, self.scale_hint())
    }

    pub fn ensure_side(&mut self, role: Role, on: bool, width: u32, reserve: i32) {
        let have = self.surf(role).is_some();
        if !on {
            if let Some(i) = self.st.surfs.iter().position(|s| s.role == role) {
                let s = self.st.surfs.remove(i);
                if let Some(f) = &s.frac {
                    f.destroy();
                }
                if let Some(v) = &s.viewport {
                    v.destroy();
                }
                s.layer.destroy();
                s.wl.destroy();
            }
            return;
        }
        let changed = self.side_size(role) != (width, reserve);
        self.side_sizes.insert(role, (width, reserve));
        if !have {
            self.create(role);
        } else if changed {
            if let Some(s) = self.surf(role) {
                s.layer.set_size(width.max(1), 0);
                s.layer.set_exclusive_zone(reserve);
                s.wl.commit();
            }
        }
    }

    /// A side surface (bar, peek), once configured.
    pub fn side(&self, role: Role) -> Option<EguiTarget> {
        self.egui_target(role)
    }

    fn egui_target(&self, role: Role) -> Option<EguiTarget> {
        let s = self.surf(role).filter(|s| s.configured && s.size.0 > 0)?;
        let (scale, viewport) = s.render_scale();
        Some(EguiTarget { surface: s.wl.clone(), logical: s.size, scale, viewport, int_scale: s.scale })
    }

    /// The overlay's surface, logical size and buffer scale, once configured.
    pub fn overlay(&self) -> Option<EguiTarget> {
        self.egui_target(Role::Overlay)
    }

    pub fn flush(&self) {
        let _ = self.conn.flush();
    }



    /// Rotate every enabled head to `deg` (0/90/180/270) through
    /// wlr-output-management; other properties keep their current values.
    /// The result arrives later as `config_status`.
    pub fn set_transform(&mut self, deg: u32) -> Result<String, String> {
        use wayland_client::protocol::wl_output::Transform;
        let mgr = self.st.out_mgr.clone().ok_or("zwlr_output_manager_v1 not offered")?;
        let serial = self.st.out_serial.ok_or("no output state yet (no `done` from the compositor)")?;
        let t = match deg % 360 {
            0 => Transform::Normal,
            90 => Transform::_90,
            180 => Transform::_180,
            _ => Transform::_270,
        };
        let cfg = mgr.create_configuration(serial, &self.qh, ());
        let mut names = Vec::new();
        for h in self.st.heads.iter().filter(|h| h.enabled && !h.finished) {
            let ch = cfg.enable_head(&h.proxy, &self.qh, ());
            ch.set_transform(t);
            names.push(h.name.clone());
        }
        if names.is_empty() {
            cfg.destroy();
            return Err("no enabled heads".into());
        }
        cfg.apply();
        self.st.config_status = "output config: applied, awaiting result".into();
        self.flush();
        Ok(format!("output config: {} -> {:?} sent", names.join(","), t))
    }

    pub fn config_status(&self) -> String {
        self.st.config_status.clone()
    }

    /// Borrowed, for the loop's "has this changed" check. The owning version
    /// allocated twice per iteration — once to compare and once to store —
    /// on a path that runs whether anything changed or not.
    pub fn config_status_ref(&self) -> &str {
        &self.st.config_status
    }

    pub fn power_status_ref(&self) -> &str {
        &self.st.power_status
    }

    pub fn st_repeat(&self) -> (i32, i32) {
        self.st.repeat
    }

    pub fn power_status(&self) -> String {
        self.st.power_status.clone()
    }

    /// Resize the edge strips (cold mode grows them). Takes effect at the
    /// strips' next configure.
    pub fn set_strip_px(&mut self, px: u32) {
        if self.strip_px == px {
            return;
        }
        self.strip_px = px;
        for s in self.st.surfs.iter().filter(|s| matches!(s.role, Role::Top | Role::Bottom | Role::Right)) {
            match s.role {
                Role::Right => s.layer.set_size(px, 0),
                _ => s.layer.set_size(0, px),
            }
            s.wl.commit();
        }
    }

    /// Whether the display is on, as the COMPOSITOR last reported it (see
    /// `combine`); `None` until every output has reported. The backlight
    /// writer follows this, so it stops writing when cosmic blanks the screen.
    pub fn display_on(&self) -> Option<bool> {
        combine(&self.st.power_modes)
    }

    /// Ask to be told when the user has been idle for `ms` (0 = stop asking).
    /// Cheap to call every iteration: only a change recreates the notification.
    /// Returns a trace line when something changed.
    pub fn set_idle_timeout(&mut self, ms: u32) -> Option<String> {
        if ms == self.idle_ms {
            return None;
        }
        self.idle_ms = ms;
        if let Some(n) = self.idle_note.take() {
            n.destroy();
        }
        self.st.idle_edge = None;
        let (Some(notifier), Some(seat)) = (self.idle_notifier.as_ref(), self._seat.as_ref()) else {
            return Some("idle: ext_idle_notifier_v1 not offered; screen sleep only on the power key".into());
        };
        if ms == 0 {
            self.flush();
            return Some("idle: screen sleep on idle off".into());
        }
        self.idle_note = Some(notifier.get_idle_notification(ms, seat, &self.qh, ()));
        self.flush();
        Some(format!("idle: screen sleeps after {} s idle", ms / 1000))
    }

    /// The idle edge since the last call, if any: `Some(true)` idled, `Some(false)` resumed.
    pub fn take_idle_edge(&mut self) -> Option<bool> {
        self.st.idle_edge.take()
    }

    /// Panel on or off (all outputs). Returns what happened, for the trace.
    pub fn set_display_power(&mut self, on: bool) -> String {
        if self.st.powers.is_empty() {
            return self.st.power_status.clone();
        }
        for p in &self.st.powers {
            p.set_mode(if on { PowerMode::On } else { PowerMode::Off });
        }
        self.flush();
        format!("output power: set {} on {} output(s)", if on { "on" } else { "off" }, self.st.powers.len())
    }

    /// Whether the overlay takes keyboard focus exclusively (palette or
    /// settings open). Off, keys go to the app underneath as usual.
    pub fn set_overlay_keyboard(&mut self, exclusive: bool) {
        if self.overlay_keyboard == exclusive {
            return;
        }
        self.overlay_keyboard = exclusive;
        if let Some(s) = self.surf(Role::Overlay) {
            s.layer.set_keyboard_interactivity(if exclusive { KeyboardInteractivity::Exclusive } else { KeyboardInteractivity::None });
            s.wl.commit();
        }
    }

    /// The cursor over the overlay (egui's icon, or hidden in fat mode).
    pub fn set_overlay_cursor(&mut self, want: CursorWant) {
        self.st.overlay_cursor = want;
        self.st.apply_cursor();
    }

    /// Wait up to `timeout_ms` for events, dispatch them, act on configures,
    /// and return the input gathered.
    pub fn dispatch(&mut self, timeout_ms: i32) -> Result<Vec<Input>, String> {
        self.dispatch_with(timeout_ms, &[])
    }

    /// The same, also waking on `extra`.
    ///
    /// The loop sleeps up to its timeout on the Wayland connection, so
    /// anything arriving on another descriptor waits for it — the power key
    /// on the control socket, a volume press from evdev. At the idle timeout
    /// that is a quarter of a second between pressing a key and the screen
    /// reacting, which reads as a phone thinking about whether to obey.
    /// Waiting on all of them costs nothing: it is the same poll with more
    /// entries.
    pub fn dispatch_with(&mut self, timeout_ms: i32, extra: &[std::os::fd::BorrowedFd<'_>]) -> Result<Vec<Input>, String> {
        self.flush();
        self.queue.dispatch_pending(&mut self.st).map_err(|e| e.to_string())?;
        if let Some(guard) = self.queue.prepare_read() {
            let fd = guard.connection_fd();
            let mut fds: Vec<rustix::event::PollFd> = Vec::with_capacity(1 + extra.len());
            fds.push(rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN));
            for e in extra {
                fds.push(rustix::event::PollFd::new(e, rustix::event::PollFlags::IN));
            }
            let ts = rustix::time::Timespec { tv_sec: (timeout_ms / 1000) as i64, tv_nsec: ((timeout_ms % 1000) as i64) * 1_000_000 };
            let ready = rustix::event::poll(&mut fds, Some(&ts)).unwrap_or(0);
            // Only the Wayland guard may be read from; a wake on the extra
            // descriptor just ends the sleep so the caller can poll it.
            let wayland_ready = ready > 0 && fds[0].revents().contains(rustix::event::PollFlags::IN);
            if wayland_ready {
                match guard.read() {
                    Ok(_) => {}
                    Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e.to_string()),
                }
            }
        }
        self.queue.dispatch_pending(&mut self.st).map_err(|e| e.to_string())?;
        // Strips get a buffer once they know their size; the overlay's
        // buffers come from EGL, driven by the caller.
        let fresh: Vec<(Role, (u32, u32))> = self
            .st
            .surfs
            .iter_mut()
            .filter(|s| s.fresh)
            .map(|s| {
                s.fresh = false;
                (s.role, s.size)
            })
            .collect();
        for (role, size) in fresh {
            if !(role == Role::Overlay || role.is_side()) {
                self.attach_strip_buffer(role, size)?;
            }
        }
        self.flush();
        Ok(std::mem::take(&mut self.st.input))
    }

    fn attach_strip_buffer(&mut self, role: Role, (w, h): (u32, u32)) -> Result<(), String> {
        if w == 0 || h == 0 {
            return Ok(());
        }
        let stride = w * 4;
        let len = (stride * h) as usize;
        let fd = rustix::fs::memfd_create("tideshell-strip", rustix::fs::MemfdFlags::CLOEXEC).map_err(|e| e.to_string())?;
        let mut file = std::fs::File::from(fd);
        // ARGB8888, premultiplied. Transparent, or a faint tint for debugging.
        let px: [u8; 4] = if self.show_strips { [0x30, 0x00, 0x30, 0x30] } else { [0; 4] };
        let mut data = Vec::with_capacity(len);
        for _ in 0..(w * h) {
            data.extend_from_slice(&px);
        }
        file.write_all(&data).map_err(|e| e.to_string())?;
        let pool = self.shm.create_pool(file.as_fd(), len as i32, &self.qh, ());
        let buf = pool.create_buffer(0, w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888, &self.qh, ());
        pool.destroy();
        if let Some(s) = self.surf(role) {
            s.wl.attach(Some(&buf), 0, 0);
            s.wl.damage_buffer(0, 0, w as i32, h as i32);
            s.wl.commit();
        }
        Ok(())
    }
}

// ---- dispatch ----------------------------------------------------------------

macro_rules! noop {
    ($t:ty) => {
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    };
}
noop!(WlCompositor);
noop!(WlShm);
noop!(WlShmPool);
noop!(WlRegion);
noop!(ZwlrLayerShellV1);
noop!(WlCallback);
noop!(WpCursorShapeManagerV1);
noop!(ZwlrOutputPowerManagerV1);
noop!(ExtIdleNotifierV1);

impl Dispatch<ExtIdleNotificationV1, ()> for State {
    fn event(st: &mut Self, _: &ExtIdleNotificationV1, e: ext_idle_notification_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            ext_idle_notification_v1::Event::Idled => st.idle_edge = Some(true),
            ext_idle_notification_v1::Event::Resumed => st.idle_edge = Some(false),
            _ => {}
        }
    }
}
noop!(ZwlrOutputModeV1);
noop!(ZwlrOutputConfigurationHeadV1);

/// One zwlr_output_head_v1 as the compositor describes it.
struct Head {
    proxy: ZwlrOutputHeadV1,
    name: String,
    enabled: bool,
    finished: bool,
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn event(st: &mut Self, _: &ZwlrOutputManagerV1, e: zwlr_output_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            zwlr_output_manager_v1::Event::Head { head } => st.heads.push(Head { proxy: head, name: String::new(), enabled: false, finished: false }),
            zwlr_output_manager_v1::Event::Done { serial } => st.out_serial = Some(serial),
            zwlr_output_manager_v1::Event::Finished => st.out_mgr = None,
            _ => {}
        }
    }
    wayland_client::event_created_child!(State, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for State {
    fn event(st: &mut Self, h: &ZwlrOutputHeadV1, e: zwlr_output_head_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some(head) = st.heads.iter_mut().find(|x| &x.proxy == h) else { return };
        match e {
            zwlr_output_head_v1::Event::Name { name } => head.name = name,
            zwlr_output_head_v1::Event::Enabled { enabled } => head.enabled = enabled != 0,
            zwlr_output_head_v1::Event::Finished => head.finished = true,
            _ => {}
        }
    }
    wayland_client::event_created_child!(State, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for State {
    fn event(st: &mut Self, c: &ZwlrOutputConfigurationV1, e: zwlr_output_configuration_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        st.config_status = match e {
            zwlr_output_configuration_v1::Event::Succeeded => "output config: succeeded".into(),
            zwlr_output_configuration_v1::Event::Failed => "output config: FAILED (compositor rejected it)".into(),
            zwlr_output_configuration_v1::Event::Cancelled => "output config: cancelled (output state changed underneath; retry)".into(),
            _ => return,
        };
        c.destroy();
    }
}
noop!(WlOutput);

impl Dispatch<ZwlrOutputPowerV1, usize> for State {
    fn event(st: &mut Self, _: &ZwlrOutputPowerV1, e: zwlr_output_power_v1::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match e {
            zwlr_output_power_v1::Event::Mode { mode } => {
                if let Some(m) = st.power_modes.get_mut(*idx) {
                    *m = Some(matches!(mode, wayland_client::WEnum::Value(PowerMode::On)));
                }
                st.power_status = format!("output power: compositor reports {mode:?} (output {idx})");
            }
            zwlr_output_power_v1::Event::Failed => st.power_status = "output power: compositor reported failure (object dead)".into(),
            _ => {}
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(st: &mut Self, _: &WlKeyboard, e: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        use xkbcommon::xkb;
        match e {
            wl_keyboard::Event::Keymap { format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), fd, size } => {
                eprintln!("wl_keyboard: keymap received ({size} bytes)");
                // SAFETY: the compositor hands us a read-only keymap of `size` bytes.
                let km = unsafe { xkb::Keymap::new_from_fd(&st.xkb_ctx, fd, size as usize, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS) };
                st.xkb = km.ok().flatten().map(|k| xkb::State::new(&k));
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                if let Some(x) = &mut st.xkb {
                    x.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => st.repeat = (rate, delay),
            wl_keyboard::Event::Key { key, state, .. } => {
                // the first few keys after start, so "typed but nothing happened" shows where it stopped
                static SEEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                if SEEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 40 { eprintln!("wl_keyboard: key {key} {state:?} (keymap loaded: {})", st.xkb.is_some()); }
                let Some(x) = &st.xkb else { return };
                let code = xkb::Keycode::new(key + 8);
                let keysym = x.key_get_one_sym(code).raw();
                let pressed = matches!(state, WEnum::Value(wl_keyboard::KeyState::Pressed));
                let utf8 = if pressed { x.key_get_utf8(code) } else { String::new() };
                let m = |n: &str| x.mod_name_is_active(n, xkb::STATE_MODS_EFFECTIVE);
                st.input.push(Input::Key { pressed, keysym, utf8, ctrl: m(xkb::MOD_NAME_CTRL), alt: m(xkb::MOD_NAME_ALT), shift: m(xkb::MOD_NAME_SHIFT) });
            }
            _ => {}
        }
    }
}
noop!(WpFractionalScaleManagerV1);
noop!(ExtBackgroundEffectSurfaceV1);
noop!(WpAlphaModifierV1);
noop!(WpAlphaModifierSurfaceV1);

impl Dispatch<ExtBackgroundEffectManagerV1, ()> for State {
    fn event(st: &mut Self, _: &ExtBackgroundEffectManagerV1, e: ext_background_effect_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_background_effect_manager_v1::Event::Capabilities { flags } = e {
            st.blur_caps = matches!(flags, WEnum::Value(f) if f.contains(ext_background_effect_manager_v1::Capability::Blur));
        }
    }
}
noop!(WpViewporter);
noop!(WpViewport);

impl Dispatch<WpFractionalScaleV1, ()> for State {
    fn event(st: &mut Self, f: &WpFractionalScaleV1, e: wp_fractional_scale_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = e {
            if let Some(s) = st.surfs.iter_mut().find(|s| s.frac.as_ref() == Some(f)) {
                // The protocol's unit is 1/120.
                s.frac_scale = Some(scale as f32 / 120.0);
            }
        }
    }
}
noop!(WpCursorShapeDeviceV1);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(_: &mut Self, b: &WlBuffer, e: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = e {
            b.destroy();
        }
    }
}

impl Dispatch<WlSurface, ()> for State {
    fn event(st: &mut Self, s: &WlSurface, e: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_surface::Event::PreferredBufferScale { factor } = e {
            if let Some(x) = st.surfs.iter_mut().find(|x| &x.wl == s) {
                x.scale = factor.max(1);
            }
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(st: &mut Self, l: &ZwlrLayerSurfaceV1, e: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                l.ack_configure(serial);
                if let Some(s) = st.surf_mut(l) {
                    s.size = (width, height);
                    s.configured = true;
                    s.fresh = true;
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if let Some(s) = st.surf_mut(l) {
                    s.closed = true;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(st: &mut Self, seat: &WlSeat, e: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = e {
            if c.contains(wl_seat::Capability::Pointer) && st.pointer.is_none() {
                let p = seat.get_pointer(qh, ());
                st.shape_device = st.cursor_mgr.as_ref().map(|m| m.get_pointer(&p, qh, ()));
                st.pointer = Some(p);
            }
            eprintln!("wl_seat: capabilities {c:?} (keyboard already held: {})", st.keyboard.is_some());
            if c.contains(wl_seat::Capability::Keyboard) && st.keyboard.is_none() {
                eprintln!("wl_seat: keyboard acquired");
                st.keyboard = Some(seat.get_keyboard(qh, ()));
            }
            if c.contains(wl_seat::Capability::Touch) && st.touch.is_none() {
                st.touch = Some(seat.get_touch(qh, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(st: &mut Self, _: &WlPointer, e: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                st.ptr_role = st.role_of(&surface);
                st.ptr_pos = (surface_x as f32, surface_y as f32);
                // Without this, tideshell set no cursor on its surfaces and
                // the pointer was invisible over them (seen live).
                st.enter_serial = Some(serial);
                st.applied = None;
                st.apply_cursor();
            }
            wl_pointer::Event::Leave { .. } => {
                // While a button is held the compositor keeps the grab, so a
                // leave mid-drag does not happen; if it does, end the contact.
                if st.ptr_down {
                    st.ptr_down = false;
                    st.input.push(Input::Cancel);
                }
                st.ptr_role = None;
            }
            wl_pointer::Event::Motion { time, surface_x, surface_y } => {
                st.ptr_pos = (surface_x as f32, surface_y as f32);
                if let Some(role) = st.ptr_role {
                    let (x, y) = st.ptr_pos;
                    st.input.push(if st.ptr_down {
                        Input::Move { role, slot: POINTER_SLOT, x, y, t: time }
                    } else {
                        Input::Hover { role, x, y }
                    });
                }
            }
            wl_pointer::Event::Button { time, button, state, .. } if button == BTN_LEFT => {
                let Some(role) = st.ptr_role else { return };
                let (x, y) = st.ptr_pos;
                match state {
                    WEnum::Value(wl_pointer::ButtonState::Pressed) => {
                        st.ptr_down = true;
                        st.input.push(Input::Down { role, slot: POINTER_SLOT, x, y, t: time });
                    }
                    WEnum::Value(wl_pointer::ButtonState::Released) => {
                        st.ptr_down = false;
                        st.input.push(Input::Up { role, slot: POINTER_SLOT, t: time });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlTouch, ()> for State {
    fn event(st: &mut Self, _: &WlTouch, e: wl_touch::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            wl_touch::Event::Down { time, surface, id, x, y, .. } => {
                if let Some(role) = st.role_of(&surface) {
                    st.touch_roles.insert(id, (role, (x as f32, y as f32)));
                    st.input.push(Input::Down { role, slot: id, x: x as f32, y: y as f32, t: time });
                }
            }
            wl_touch::Event::Motion { time, id, x, y } => {
                if let Some((role, pos)) = st.touch_roles.get_mut(&id) {
                    *pos = (x as f32, y as f32);
                    let role = *role;
                    st.input.push(Input::Move { role, slot: id, x: x as f32, y: y as f32, t: time });
                }
            }
            wl_touch::Event::Up { time, id, .. } => {
                if let Some((role, _)) = st.touch_roles.remove(&id) {
                    st.input.push(Input::Up { role, slot: id, t: time });
                }
            }
            wl_touch::Event::Cancel => {
                st.touch_roles.clear();
                st.input.push(Input::Cancel);
            }
            _ => {}
        }
    }
}

// ---- EGL + egui_glow -------------------------------------------------------------

/// egui through egui_glow on EGL: one EGL context, one window + painter per
/// surface. Each egui::Context has its own texture ids (font atlas, images),
/// so two surfaces sharing one Painter would mix up textures; hence a
/// painter per role, all on the same GL context.
pub struct Gl {
    /// GL_RENDERER, read once. The only thing anyone asks of it is whether a
    /// full-screen fill is cheap (paintrate::accelerated_renderer).
    pub renderer: String,
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    glow: Option<Arc<glow::Context>>,
    surfs: HashMap<Role, GlSurf>,
}

#[derive(Default)]
struct GlSurf {
    win: Option<(wayland_egl::WlEglSurface, egl::Surface, (i32, i32))>,
    painter: Option<egui_glow::Painter>,
}

impl Gl {
    pub fn new(wl_display: *mut c_void) -> Result<Gl, String> {
        let egl = egl::Instance::new(egl::Static);
        let display = unsafe { egl.get_display(wl_display as egl::NativeDisplayType) }.ok_or("eglGetDisplay failed")?;
        egl.initialize(display).map_err(|e| format!("eglInitialize: {e}"))?;
        egl.bind_api(egl::OPENGL_ES_API).map_err(|e| format!("eglBindAPI: {e}"))?;
        let attrs = [
            egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8, egl::ALPHA_SIZE, 8,
            egl::SURFACE_TYPE, egl::WINDOW_BIT, egl::RENDERABLE_TYPE, egl::OPENGL_ES2_BIT, egl::NONE,
        ];
        let config = egl.choose_first_config(display, &attrs).map_err(|e| e.to_string())?.ok_or("no RGBA8 GLES2 EGL config")?;
        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE])
            .map_err(|e| format!("eglCreateContext: {e}"))?;
        Ok(Gl { renderer: String::new(), egl, display, config, context, glow: None, surfs: HashMap::new() })
    }

    /// Bind `role`'s surface at `size_px` physical pixels, (re)creating its
    /// EGL window if needed, and make it current.
    pub fn bind(&mut self, role: Role, surface: &WlSurface, size_px: (i32, i32)) -> Result<(), String> {
        let gs = self.surfs.entry(role).or_default();
        if let Some((w, _, sz)) = &mut gs.win {
            if *sz != size_px {
                w.resize(size_px.0, size_px.1, 0, 0);
                *sz = size_px;
            }
        }
        if gs.win.is_none() {
            let w = wayland_egl::WlEglSurface::new(surface.id(), size_px.0, size_px.1).map_err(|e| format!("wl_egl_window: {e:?}"))?;
            let es = unsafe { self.egl.create_window_surface(self.display, self.config, w.ptr() as egl::NativeWindowType, None) }
                .map_err(|e| format!("eglCreateWindowSurface: {e}"))?;
            gs.win = Some((w, es, size_px));
        }
        let es = gs.win.as_ref().unwrap().1;
        self.egl.make_current(self.display, Some(es), Some(es), Some(self.context)).map_err(|e| format!("eglMakeCurrent: {e}"))?;
        // Never block in swap: the loop paces itself, and a hidden surface
        // would otherwise stall the whole shell waiting for a frame callback.
        let _ = self.egl.swap_interval(self.display, 0);
        if self.glow.is_none() {
            let egl = &self.egl;
            let gl = unsafe {
                glow::Context::from_loader_function(|name| egl.get_proc_address(name).map_or(std::ptr::null(), |p| p as *const c_void))
            };
            // Read GL_RENDERER here, the one moment a context is current.
            // What downstream wants is only "is a full-screen fill cheap",
            // but the string itself is worth keeping for the trace: a shell
            // that has silently fallen back to llvmpipe should be able to
            // say so rather than just feeling slow.
            self.renderer = unsafe { gl.get_parameter_string(glow::RENDERER) };
            self.glow = Some(Arc::new(gl));
        }
        let gs = self.surfs.get_mut(&role).unwrap();
        if gs.painter.is_none() {
            let gl = self.glow.clone().unwrap();
            gs.painter = Some(egui_glow::Painter::new(gl, "", None, false).map_err(|e| format!("egui_glow: {e}"))?);
        }
        Ok(())
    }

    /// Drop `role`'s EGL window (surface unmapped). Its painter stays, so the
    /// next map does not recompile shaders or re-upload the font atlas.
    pub fn unbind(&mut self, role: Role) {
        if let Some(gs) = self.surfs.get_mut(&role) {
            if let Some((_w, es, _)) = gs.win.take() {
                let _ = self.egl.make_current(self.display, None, None, Some(self.context));
                let _ = self.egl.destroy_surface(self.display, es);
            }
        }
    }

    /// Paint into `role`'s surface; `bind(role, ..)` must have been called
    /// this frame (it makes the surface current).
    pub fn paint(&mut self, role: Role, size_px: (i32, i32), ppp: f32, prims: &[egui::ClippedPrimitive], tex: &egui::TexturesDelta) -> Result<(), String> {
        let gs = self.surfs.get_mut(&role).ok_or("paint before bind")?;
        let painter = gs.painter.as_mut().ok_or("paint before bind")?;
        let sz = [size_px.0 as u32, size_px.1 as u32];
        painter.clear(sz, [0.0, 0.0, 0.0, 0.0]);
        painter.paint_and_update_textures(sz, ppp, prims, tex);
        let es = gs.win.as_ref().ok_or("no window")?.1;
        self.egl.swap_buffers(self.display, es).map_err(|e| format!("eglSwapBuffers: {e}"))
    }
}

/// Whether the display counts as on, from each output's last reported mode:
/// on if ANY output is on, off once EVERY output has said off, unknown until
/// then. Pure.
pub fn combine(modes: &[Option<bool>]) -> Option<bool> {
    if modes.iter().any(|m| *m == Some(true)) {
        Some(true)
    } else if !modes.is_empty() && modes.iter().all(|m| *m == Some(false)) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn side_widths_snap_to_whole_physical_pixels_and_integer_scales_are_untouched() {
        // The phone: 2.25 -> multiples of 4 points are whole pixels (9 px per 4 pt).
        assert_eq!(snap_logical(41, 2.25), 40, "the carousel peek: 92.25 px becomes 90");
        assert_eq!(snap_logical(64, 2.25), 64, "the bar is already aligned: 144 px");
        assert_eq!(snap_logical(166, 2.25), 164, "the expanded bar: 373.5 px becomes 369");
        assert_eq!(snap_logical(42, 2.25), 40, "ties go narrower");
        for w in 1..300u32 {
            assert_eq!(snap_logical(w, 1.0), w, "scale 1 at {w}");
            assert_eq!(snap_logical(w, 2.0), w, "scale 2 at {w}");
            assert_eq!(snap_logical(w, 3.0), w, "scale 3 at {w}");
        }
        for scale in [1.25f32, 1.5, 1.75, 2.25, 2.5, 2.75, 3.25] {
            for w in 8..400u32 {
                let s = snap_logical(w, scale);
                let px = s as f32 * scale;
                assert!((px - px.round()).abs() < 0.003, "{w} at {scale} -> {s} is {px} px");
                assert!((s as i64 - w as i64).abs() <= 4, "{w} at {scale} moved to {s}");
            }
        }
        assert_eq!(snap_logical(0, 2.25), 0);
        assert_eq!(snap_logical(40, 0.0), 40, "a nonsense scale changes nothing");
    }

    use super::*;

    #[test]
    fn the_display_is_on_if_any_output_is_and_off_only_when_all_say_so() {
        assert_eq!(combine(&[Some(true)]), Some(true));
        assert_eq!(combine(&[Some(false)]), Some(false));
        assert_eq!(combine(&[Some(false), None]), None, "not all known yet: no claim");
        assert_eq!(combine(&[Some(false), Some(true)]), Some(true), "an external screen still on");
        assert_eq!(combine(&[]), None);
    }

    #[test]
    fn buffer_rounds_up_at_fractional_scales() {
        assert_eq!(buffer_px((853, 533), 1.5), (1280, 800));
        assert_eq!(buffer_px((432, 960), 2.5), (1080, 2400));
        // 1/120 steps do not divide evenly: round UP, never short.
        assert_eq!(buffer_px((100, 100), 121.0 / 120.0), (101, 101));
        assert_eq!(buffer_px((64, 800), 1.0), (64, 800));
    }

    #[test]
    fn egui_covers_every_buffer_pixel_at_fractional_scales() {
        // Every logical size up to a phone's, at the scales compositors
        // offer (n/120 steps): what egui paints must reach the buffer's
        // last pixel, which `logical` points did not.
        let mut short_before = 0;
        for scale120 in [150u32, 180, 210, 240, 270, 300, 330, 350] {
            let s = scale120 as f32 / 120.0;
            for lw in (300u32..=500).step_by(7) {
                let lh = lw * 2 + 3;
                let px = buffer_px((lw, lh), s);
                let (pw, ph) = screen_points(px, s);
                assert!((pw * s - px.0 as f32).abs() < 0.01 && (ph * s - px.1 as f32).abs() < 0.01, "scale {s} logical {lw}x{lh}");
                if (lw as f32 * s) < px.0 as f32 - 0.01 || (lh as f32 * s) < px.1 as f32 - 0.01 {
                    short_before += 1;
                }
                // Never more than a pixel away from the logical size.
                assert!(pw - lw as f32 <= 1.0 / s + 1e-3 && pw >= lw as f32 - 1e-3);
            }
        }
        assert!(short_before > 0, "the old sizing left edge pixels uncovered in these cases");
        assert_eq!(screen_points((1080, 2400), 3.0), (360.0, 800.0), "integer scale: identical to the logical size");
    }

    #[test]
    fn egui_cursor_icons_map_to_shapes() {
        assert_eq!(shape_for(egui::CursorIcon::PointingHand), Shape::Pointer);
        assert_eq!(shape_for(egui::CursorIcon::Text), Shape::Text);
        assert_eq!(shape_for(egui::CursorIcon::ResizeVertical), Shape::NsResize);
        assert_eq!(shape_for(egui::CursorIcon::Default), Shape::Default);
    }

    #[test]
    fn strip_coordinates_map_to_screen_including_outside_the_strip() {
        let screen = (408.0, 900.0);
        // Bottom strip is 408x28 at y=872. A finger that started on it and
        // moved 300 px up reports y = -300 + start: must land 300 px up the screen.
        assert_eq!(strip_to_screen(Role::Bottom, 100.0, 20.0, screen, (408.0, 28.0)), (100.0, 892.0));
        assert_eq!(strip_to_screen(Role::Bottom, 100.0, -280.0, screen, (408.0, 28.0)), (100.0, 592.0));
        // Right strip 28 wide: x offset by 380; a swipe-in goes negative.
        assert_eq!(strip_to_screen(Role::Right, -60.0, 400.0, screen, (28.0, 900.0)), (320.0, 400.0));
        // Top strip and overlay are already screen-aligned.
        assert_eq!(strip_to_screen(Role::Top, 5.0, 300.0, screen, (408.0, 28.0)), (5.0, 300.0));
        assert_eq!(strip_to_screen(Role::Overlay, 5.0, 300.0, screen, (408.0, 900.0)), (5.0, 300.0));
    }
}
