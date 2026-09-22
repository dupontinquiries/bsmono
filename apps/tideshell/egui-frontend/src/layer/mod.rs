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

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Write;
use std::os::fd::AsFd;
use std::sync::Arc;

use khronos_egl as egl;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
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
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
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
}

/// Input in surface-local logical coordinates; `Io::to_screen` maps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    Down { role: Role, slot: i32, x: f32, y: f32, t: u32 },
    Move { role: Role, slot: i32, x: f32, y: f32, t: u32 },
    Up { role: Role, slot: i32, t: u32 },
    /// Pointer motion with no button held (hover, for egui).
    Hover { role: Role, x: f32, y: f32 },
    Cancel,
}

/// Map strip-local coordinates to screen coordinates. Pure; see the module
/// header for why strip-local values outside the strip are expected.
pub fn strip_to_screen(role: Role, x: f32, y: f32, screen: (f32, f32), strip: (f32, f32)) -> (f32, f32) {
    match role {
        Role::Top | Role::Overlay => (x, y),
        Role::Bar => (x + screen.0 - strip.0, y),
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

/// Buffer pixels for a logical size at a (possibly fractional) scale. The
/// protocol's rule: round up, so the viewport never stretches a short buffer.
pub fn buffer_px(logical: (u32, u32), scale: f32) -> (i32, i32) {
    ((logical.0 as f32 * scale).ceil() as i32, (logical.1 as f32 * scale).ceil() as i32)
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
    pub strip_px: u32,
    /// Tint the strips so they can be seen (`TIDESHELL_SHOW_STRIPS=1`).
    pub show_strips: bool,
    right_enabled: bool,
    /// (surface width, exclusive zone) for the bar.
    bar_size: (u32, i32),
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
        let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
        let st = State {
            surfs: Vec::new(),
            input: Vec::new(),
            pointer: None,
            touch: None,
            ptr_role: None,
            ptr_pos: (0.0, 0.0),
            ptr_down: false,
            touch_roles: HashMap::new(),
            cursor_mgr: globals.bind(&qh, 1..=1, ()).ok(),
            shape_device: None,
            enter_serial: None,
            overlay_cursor: CursorWant::Shape(Shape::Default),
            applied: None,
        };
        let mut io = Io { conn, queue, qh, st, compositor, shm, layer_shell, _seat: seat, frac_mgr, viewporter, strip_px, show_strips, right_enabled: false, bar_size: (0, 0) };
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
        let layer_kind = if role == Role::Bar { zwlr_layer_shell_v1::Layer::Top } else { zwlr_layer_shell_v1::Layer::Overlay };
        let layer = self.layer_shell.get_layer_surface(&wl, None, layer_kind, "tideshell".into(), &self.qh, ());
        let px = self.strip_px;
        let (anchor, size) = match role {
            Role::Top => (Anchor::Top | Anchor::Left | Anchor::Right, (0, px)),
            Role::Bottom => (Anchor::Bottom | Anchor::Left | Anchor::Right, (0, px)),
            Role::Right => (Anchor::Top | Anchor::Bottom | Anchor::Right, (if self.right_enabled { px } else { 1 }, 0)),
            Role::Overlay => (Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right, (0, 0)),
            Role::Bar => (Anchor::Top | Anchor::Bottom | Anchor::Right, (self.bar_size.0.max(1), 0)),
        };
        layer.set_anchor(anchor);
        layer.set_size(size.0, size.1);
        // -1: ignore other surfaces' exclusive zones, so strips sit on the
        // true screen edge even under a panel, and the overlay is fullscreen.
        if role == Role::Bar {
            // Reserve the collapsed width only: expanding overlaps windows
            // (a drawer), it does not reflow them on every expand.
            layer.set_exclusive_zone(self.bar_size.1);
        } else {
            layer.set_exclusive_zone(-1);
        }
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
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
        self.st.surfs.push(Surf { role, wl, layer, size: (0, 0), configured: false, fresh: false, scale: 1, closed: false, frac, frac_scale: None, viewport, sent: None });
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
                s.layer.destroy();
                s.wl.destroy();
            }
        }
    }

    /// Map/resize or unmap the bar. `width` is the drawn width (expanded or
    /// not), `reserve` the exclusive zone (the collapsed width).
    pub fn ensure_bar(&mut self, on: bool, width: u32, reserve: i32) {
        let have = self.surf(Role::Bar).is_some();
        if !on {
            if let Some(i) = self.st.surfs.iter().position(|s| s.role == Role::Bar) {
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
        let changed = self.bar_size != (width, reserve);
        self.bar_size = (width, reserve);
        if !have {
            self.create(Role::Bar);
        } else if changed {
            if let Some(s) = self.surf(Role::Bar) {
                s.layer.set_size(width.max(1), 0);
                s.layer.set_exclusive_zone(reserve);
                s.wl.commit();
            }
        }
    }

    /// The bar's surface, logical size and buffer scale, once configured.
    pub fn bar(&self) -> Option<EguiTarget> {
        self.egui_target(Role::Bar)
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



    /// The cursor over the overlay (egui's icon, or hidden in fat mode).
    pub fn set_overlay_cursor(&mut self, want: CursorWant) {
        self.st.overlay_cursor = want;
        self.st.apply_cursor();
    }

    /// Wait up to `timeout_ms` for events, dispatch them, act on configures,
    /// and return the input gathered.
    pub fn dispatch(&mut self, timeout_ms: i32) -> Result<Vec<Input>, String> {
        self.flush();
        self.queue.dispatch_pending(&mut self.st).map_err(|e| e.to_string())?;
        if let Some(guard) = self.queue.prepare_read() {
            let fd = guard.connection_fd();
            let mut fds = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN)];
            let ts = rustix::time::Timespec { tv_sec: (timeout_ms / 1000) as i64, tv_nsec: ((timeout_ms % 1000) as i64) * 1_000_000 };
            let ready = rustix::event::poll(&mut fds, Some(&ts)).unwrap_or(0);
            if ready > 0 {
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
            if !matches!(role, Role::Overlay | Role::Bar) {
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
noop!(WpFractionalScaleManagerV1);
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
        Ok(Gl { egl, display, config, context, glow: None, surfs: HashMap::new() })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_rounds_up_at_fractional_scales() {
        assert_eq!(buffer_px((853, 533), 1.5), (1280, 800));
        assert_eq!(buffer_px((432, 960), 2.5), (1080, 2400));
        // 1/120 steps do not divide evenly: round UP, never short.
        assert_eq!(buffer_px((100, 100), 121.0 / 120.0), (101, 101));
        assert_eq!(buffer_px((64, 800), 1.0), (64, 800));
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
