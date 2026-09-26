//! A video subsurface fed the decoder's dma-bufs: the compositor shows the
//! frame itself (and can scan it out on an overlay plane), the player's GL
//! pass never touches it.
//!
//! - `wl_subsurface` under the window's surface, desynchronised (frames
//!   commit on their own clock, not the UI's), with an empty input region
//!   (clicks go to the window). Stacked BELOW the window by default, which
//!   then paints a transparent hole where the video is -- the UI stays on
//!   top, as it was.
//! - `zwp_linux_dmabuf_v1` wl_buffers, created asynchronously (`create`,
//!   not `create_immed`: a refused import is a `failed` event we can
//!   survive, not a protocol error that kills the app's connection), one
//!   per decoder buffer, reused as the decoder cycles its pool.
//! - `wp_viewporter` scales to the video rectangle.
//! - `wp_color_representation_v1`, when offered, says the matrix/range.
//!
//! Lifetime: a frame given to [`VideoPlane::show`] is held until the
//! compositor RELEASES its wl_buffer -- not merely until the next frame:
//! an overlay plane can still be scanning it out.

use std::os::fd::{AsFd, AsRawFd};

use vidkit_core::dmabuf::{plane_imports, DRM_FORMAT_NV12};
use vidkit_core::frame::{ColorInfo, Frame, FrameData, Matrix, Range};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_region::WlRegion, wl_subcompositor::WlSubcompositor, wl_subsurface::WlSubsurface, wl_surface::WlSurface};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::color_representation::v1::client::{
    wp_color_representation_manager_v1::{self as crm, WpColorRepresentationManagerV1},
    wp_color_representation_surface_v1::{self as crs, WpColorRepresentationSurfaceV1},
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self as params, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::{self as dmabuf, ZwpLinuxDmabufV1},
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};

/// A decoder buffer's identity: (device, inode, first offset, size).
type Key = (u64, u64, u32, (u32, u32));

enum Buf {
    Pending,
    Ready(WlBuffer),
    Failed,
}

#[derive(Default)]
struct State {
    formats: Vec<(u32, u64)>,
    /// (matrix, range) pairs the compositor accepts, protocol values.
    coefficients: Vec<(u32, u32)>,
    created: Vec<(Key, Option<WlBuffer>)>,
    released: Vec<wayland_client::backend::ObjectId>,
}

pub struct VideoPlane {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    foreign: bool,
    surface: WlSurface,
    subsurface: WlSubsurface,
    viewport: WpViewport,
    dmabuf: ZwpLinuxDmabufV1,
    color: Option<WpColorRepresentationSurfaceV1>,
    color_set: Option<ColorInfo>,
    buffers: Vec<(Key, Buf)>,
    in_flight: Vec<(wayland_client::backend::ObjectId, Frame)>,
    rect: Option<(i32, i32, i32, i32)>,
    pub shown: u64,
}

fn key_of(frame: &Frame) -> Option<Key> {
    let FrameData::DmaBuf(img) = &frame.data else { return None };
    let p = img.planes.first()?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local.
    (unsafe { libc::fstat(p.fd.as_raw_fd(), &mut st) } == 0).then(|| (st.st_dev as u64, st.st_ino as u64, p.offset, (frame.width, frame.height)))
}

/// Protocol values for our colour description. Pure.
pub fn coefficients_of(c: &ColorInfo) -> (u32, u32) {
    let m = match c.matrix {
        Matrix::Bt601 => crs::Coefficients::Bt601 as u32,
        Matrix::Bt709 => crs::Coefficients::Bt709 as u32,
        Matrix::Bt2020Ncl => crs::Coefficients::Bt2020 as u32,
    };
    let r = match c.range {
        Range::Limited => crs::Range::Limited as u32,
        Range::Full => crs::Range::Full as u32,
    };
    (m, r)
}

impl VideoPlane {
    /// A plane under `parent`, on `conn` (`foreign`: someone else reads the
    /// socket -- see SurfaceLink). `below`: under the parent (it must
    /// leave a transparent hole) or above it.
    pub fn new(conn: &Connection, parent: &WlSurface, foreign: bool, below: bool) -> Result<Self, String> {
        let (globals, mut queue) = registry_queue_init::<State>(conn).map_err(|e| format!("registry: {e}"))?;
        let qh = queue.handle();
        let comp: WlCompositor = globals.bind(&qh, 1..=4, ()).map_err(|_| "no wl_compositor")?;
        let sub: WlSubcompositor = globals.bind(&qh, 1..=1, ()).map_err(|_| "no wl_subcompositor")?;
        let vp: WpViewporter = globals.bind(&qh, 1..=1, ()).map_err(|_| "no wp_viewporter")?;
        // Version 3: formats and modifiers arrive as events on bind (v4
        // moved them to feedback objects and a mapped table).
        let dmabuf: ZwpLinuxDmabufV1 = globals.bind(&qh, 3..=3, ()).map_err(|_| "no zwp_linux_dmabuf_v1 v3")?;
        let crm: Option<WpColorRepresentationManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
        let surface = comp.create_surface(&qh, ());
        let subsurface = sub.get_subsurface(&surface, parent, &qh, ());
        subsurface.set_desync();
        if below {
            subsurface.place_below(parent);
        } else {
            subsurface.place_above(parent);
        }
        let empty: WlRegion = comp.create_region(&qh, ());
        surface.set_input_region(Some(&empty));
        empty.destroy();
        let viewport = vp.get_viewport(&surface, &qh, ());
        let color = crm.as_ref().map(|m| m.get_surface(&surface, &qh, ()));
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(|e| format!("roundtrip: {e}"))?;
        Ok(Self { conn: conn.clone(), queue, state, foreign, surface, subsurface, viewport, dmabuf, color, color_set: None, buffers: Vec::new(), in_flight: Vec::new(), rect: None, shown: 0 })
    }

    /// The video's own surface (for presentation feedback on it).
    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }

    /// Whether the compositor takes this format with this modifier.
    pub fn supports(&self, format: u32, modifier: u64) -> bool {
        self.state.formats.iter().any(|&(f, m)| f == format && (m == modifier || m == vidkit_core::dmabuf::DRM_FORMAT_MOD_INVALID && modifier == vidkit_core::dmabuf::DRM_FORMAT_MOD_LINEAR))
    }

    /// Whether the compositor can be told this matrix and range.
    pub fn can_say(&self, c: &ColorInfo) -> bool {
        self.color.is_some() && self.state.coefficients.contains(&coefficients_of(c))
    }

    /// Frames still held by the compositor.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    fn pump(&mut self) {
        let _ = self.conn.flush();
        let _ = self.queue.dispatch_pending(&mut self.state);
        if !self.foreign {
            if let Some(g) = self.queue.prepare_read() {
                let _ = g.read();
            }
            let _ = self.queue.dispatch_pending(&mut self.state);
        }
        for (k, b) in self.state.created.drain(..) {
            if let Some(e) = self.buffers.iter_mut().find(|e| e.0 == k) {
                e.1 = match b {
                    Some(b) => Buf::Ready(b),
                    None => Buf::Failed,
                };
            }
        }
        for id in self.state.released.drain(..) {
            // Dropping the frame gives the buffer back to its decoder.
            if let Some(i) = self.in_flight.iter().position(|(b, _)| *b == id) {
                self.in_flight.swap_remove(i);
            }
        }
    }

    /// Process events: buffer creations and releases. Call once per UI
    /// frame.
    pub fn poll(&mut self) {
        self.pump();
    }

    /// Show `frame` at `rect` (x, y, w, h in the parent's surface-local
    /// coordinates). `Err` gives the frame back with the reason when it
    /// cannot be shown this way now -- its buffer is still being imported
    /// (show it the other way this once), or the compositor refused it.
    pub fn show(&mut self, frame: Frame, rect: (i32, i32, i32, i32)) -> Result<(), (Frame, &'static str)> {
        self.pump();
        let Some(key) = key_of(&frame) else { return Err((frame, "not a dma-buf")) };
        let buf = match self.buffers.iter().find(|e| e.0 == key).map(|e| &e.1) {
            Some(Buf::Ready(b)) => b.clone(),
            Some(Buf::Pending) => return Err((frame, "buffer import pending")),
            Some(Buf::Failed) => return Err((frame, "compositor refused the buffer")),
            None => {
                if let Err(e) = self.create(&frame, key) {
                    return Err((frame, e));
                }
                return Err((frame, "buffer import pending"));
            }
        };
        if let Some(c) = &self.color {
            if self.color_set != Some(frame.color) && self.state.coefficients.contains(&coefficients_of(&frame.color)) {
                let (m, r) = coefficients_of(&frame.color);
                if let (Ok(m), Ok(r)) = (crs::Coefficients::try_from(m), crs::Range::try_from(r)) {
                    c.set_coefficients_and_range(m, r);
                    self.color_set = Some(frame.color);
                }
            }
        }
        if self.rect != Some(rect) {
            // Position is parent state (applied at the parent's next
            // commit, which a layout change makes anyway); the destination
            // size is ours.
            self.subsurface.set_position(rect.0, rect.1);
            self.viewport.set_destination(rect.2.max(1), rect.3.max(1));
            self.rect = Some(rect);
        }
        self.surface.attach(Some(&buf), 0, 0);
        self.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
        self.surface.commit();
        let _ = self.conn.flush();
        self.in_flight.push((buf.id(), frame));
        self.shown += 1;
        Ok(())
    }

    fn create(&mut self, frame: &Frame, key: Key) -> Result<(), &'static str> {
        let FrameData::DmaBuf(img) = &frame.data else { return Err("not a dma-buf") };
        if img.fourcc != DRM_FORMAT_NV12 && img.fourcc != vidkit_core::dmabuf::V4L2_PIX_FMT_NV12M {
            return Err("not NV12");
        }
        let (_, imps) = plane_imports(img, frame.width, frame.height).map_err(|_| "bad plane layout")?;
        let qh = self.queue.handle();
        let p = self.dmabuf.create_params(&qh, key);
        let m = img.modifier;
        for (i, imp) in imps.iter().enumerate() {
            let fd = img.planes[imp.fd_index].fd.as_fd();
            p.add(fd, i as u32, imp.offset, imp.stride, (m >> 32) as u32, m as u32);
        }
        p.create(frame.width as i32, frame.height as i32, DRM_FORMAT_NV12, params::Flags::empty());
        self.buffers.push((key, Buf::Pending));
        // A pool that changed (new decoder) leaves old entries: bound them.
        if self.buffers.len() > 64 {
            if let (_, Buf::Ready(b)) = self.buffers.remove(0) {
                b.destroy();
            }
        }
        let _ = self.conn.flush();
        Ok(())
    }

    /// Stop showing video on the plane (the GL path takes over). Held
    /// frames go when the compositor releases them.
    pub fn hide(&mut self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
        self.rect = None;
        let _ = self.conn.flush();
    }

    /// Forget the cached buffers (a new decoder: its pool is new).
    pub fn forget_buffers(&mut self) {
        for (_, b) in self.buffers.drain(..) {
            if let Buf::Ready(b) = b {
                b.destroy();
            }
        }
    }
}

impl Drop for VideoPlane {
    fn drop(&mut self) {
        self.forget_buffers();
        if let Some(c) = self.color.take() {
            c.destroy();
        }
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
        let _ = self.conn.flush();
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for State {
    fn event(s: &mut Self, _: &ZwpLinuxDmabufV1, e: dmabuf::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            dmabuf::Event::Modifier { format, modifier_hi, modifier_lo } => s.formats.push((format, (modifier_hi as u64) << 32 | modifier_lo as u64)),
            dmabuf::Event::Format { format } => s.formats.push((format, vidkit_core::dmabuf::DRM_FORMAT_MOD_INVALID)),
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, Key> for State {
    fn event(s: &mut Self, p: &ZwpLinuxBufferParamsV1, e: params::Event, key: &Key, _: &Connection, _: &QueueHandle<Self>) {
        match e {
            params::Event::Created { buffer } => s.created.push((*key, Some(buffer))),
            params::Event::Failed => s.created.push((*key, None)),
            _ => {}
        }
        p.destroy();
    }
    event_created_child!(State, ZwpLinuxBufferParamsV1, [
        params::EVT_CREATED_OPCODE => (WlBuffer, ()),
    ]);
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(s: &mut Self, b: &WlBuffer, e: wayland_client::protocol::wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wayland_client::protocol::wl_buffer::Event::Release = e {
            s.released.push(b.id());
        }
    }
}

impl Dispatch<WpColorRepresentationManagerV1, ()> for State {
    fn event(s: &mut Self, _: &WpColorRepresentationManagerV1, e: crm::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let crm::Event::SupportedCoefficientsAndRanges { coefficients, range } = e {
            s.coefficients.push((u32::from(coefficients), u32::from(range)));
        }
    }
}

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(WlCompositor, WlSubcompositor, WlSurface, WlSubsurface, WlRegion, WpViewporter, WpViewport, WpColorRepresentationSurfaceV1);
