//! The Wayland side: feedback and hints for a surface someone else owns.
//!
//! eframe/winit own the `wl_display` and the window's `wl_surface`; this
//! opens a second libwayland *connection object* over the same C display
//! (`from_foreign_display`), on its own event queue, so nothing here touches
//! winit's queue or state. Requests made here are ordered with winit's and
//! EGL's on the one socket, so a feedback request made during `update()`
//! lands before that frame's `eglSwapBuffers` commit.
//!
//! Protocol errors kill the whole connection -- the app's, not just ours --
//! so everything here is capability-gated and constructed once:
//! - `wp_presentation` (stable; weston, KWin, Mutter, wlroots): feedback.
//! - `wp_content_type_v1` (staging; KWin, wlroots-based): says "video".
//! - `wp_commit_timing_v1` (staging; newer KWin/Mutter/wlroots): a target
//!   presentation time per commit -- on a VRR output the frame is shown at
//!   its own time instead of the next fixed vblank. A surface may have only
//!   ONE timer, and Mesa's Vulkan WSI takes one for Vulkan swapchains, so
//!   this is opt-in (the GL path does not use one, but a driver change would
//!   turn our request into a fatal error).

use std::ffi::c_void;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_surface::WlSurface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::commit_timing::v1::client::{wp_commit_timer_v1::WpCommitTimerV1, wp_commit_timing_manager_v1::WpCommitTimingManagerV1};
use wayland_protocols::wp::content_type::v1::client::{
    wp_content_type_manager_v1::WpContentTypeManagerV1,
    wp_content_type_v1::{self, WpContentTypeV1},
};
use wayland_protocols::wp::presentation_time::client::{
    wp_presentation::{self, WpPresentation},
    wp_presentation_feedback::{self, Kind, WpPresentationFeedback},
};

use crate::cadence::{Outcome, Presented};

/// What the compositor offers, as bound.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Caps {
    pub presentation: bool,
    pub content_type: bool,
    pub commit_timing: bool,
    /// The presentation clock (`clockid_t`), from `wp_presentation`.
    pub clock_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    None,
    Photo,
    Video,
    Game,
}

#[derive(Default)]
struct State {
    clock_id: Option<u32>,
    outcomes: Vec<Outcome>,
}

pub struct SurfaceLink {
    conn: Connection,
    queue: EventQueue<State>,
    state: State,
    surface: WlSurface,
    presentation: Option<WpPresentation>,
    content: Option<WpContentTypeV1>,
    timer: Option<WpCommitTimerV1>,
    timing_manager: Option<WpCommitTimingManagerV1>,
    pending: usize,
    /// The connection is someone else's (winit's): never read its socket.
    foreign: bool,
}

impl SurfaceLink {
    /// Over a display and surface owned by someone else (raw-window-handle's
    /// Wayland handles).
    ///
    /// # Safety
    /// Both pointers must be the live `wl_display` / `wl_surface` of this
    /// process, and must outlive the link.
    pub unsafe fn from_raw(display: *mut c_void, surface: *mut c_void) -> Result<Self, String> {
        let backend = wayland_backend::client::Backend::from_foreign_display(display.cast());
        let conn = Connection::from_backend(backend);
        let id = wayland_backend::client::ObjectId::from_ptr(WlSurface::interface(), surface.cast()).map_err(|e| format!("wl_surface: {e:?}"))?;
        let surface = WlSurface::from_id(&conn, id).map_err(|e| format!("wl_surface: {e:?}"))?;
        let mut link = Self::new(conn, surface)?;
        link.foreign = true;
        Ok(link)
    }

    /// Over a connection and surface (tests own both).
    pub fn new(conn: Connection, surface: WlSurface) -> Result<Self, String> {
        let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| format!("registry: {e}"))?;
        let qh = queue.handle();
        let presentation = globals.bind::<WpPresentation, _, _>(&qh, 1..=1, ()).ok();
        let timing_manager = globals.bind::<WpCommitTimingManagerV1, _, _>(&qh, 1..=1, ()).ok();
        let content_manager = globals.bind::<WpContentTypeManagerV1, _, _>(&qh, 1..=1, ()).ok();
        let content = content_manager.as_ref().map(|m| m.get_surface_content_type(&surface, &qh, ()));
        let mut state = State::default();
        // The clock_id event follows the bind.
        queue.roundtrip(&mut state).map_err(|e| format!("roundtrip: {e}"))?;
        Ok(Self { conn, queue, state, surface, presentation, content, timer: None, timing_manager, pending: 0, foreign: false })
    }

    pub fn caps(&self) -> Caps {
        Caps {
            presentation: self.presentation.is_some(),
            content_type: self.content.is_some(),
            commit_timing: self.timing_manager.is_some(),
            clock_id: self.state.clock_id,
        }
    }

    /// Say what the surface shows (applies at the next commit).
    pub fn set_content(&self, c: Content) {
        if let Some(ct) = &self.content {
            ct.set_content_type(match c {
                Content::None => wp_content_type_v1::Type::None,
                Content::Photo => wp_content_type_v1::Type::Photo,
                Content::Video => wp_content_type_v1::Type::Video,
                Content::Game => wp_content_type_v1::Type::Game,
            });
        }
    }

    /// Start giving commits target times (see the module note: opt-in).
    /// `false` when the compositor has no commit timing.
    pub fn enable_commit_timing(&mut self) -> bool {
        if self.timer.is_none() {
            if let Some(m) = &self.timing_manager {
                self.timer = Some(m.get_timer(&self.surface, &self.queue.handle(), ()));
            }
        }
        self.timer.is_some()
    }

    /// Now, on the presentation clock, in ns.
    pub fn clock_now_ns(&self) -> Option<u64> {
        let id = self.state.clock_id? as libc::clockid_t;
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime writes the timespec it is given.
        (unsafe { libc::clock_gettime(id, &mut ts) } == 0).then(|| ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64)
    }

    /// Call before the commit that shows a new frame: asks for its
    /// feedback, and (with commit timing on) sets its target time.
    /// `target_ns` is on the presentation clock ([`clock_now_ns`]).
    pub fn before_commit(&mut self, target_ns: Option<u64>) {
        let qh = self.queue.handle();
        if let Some(p) = &self.presentation {
            // Bounded: a surface that stopped committing (hidden, minimised)
            // gets no events, and requests should not pile up behind it.
            if self.pending < 16 {
                p.feedback(&self.surface, &qh, target_ns);
                self.pending += 1;
            }
        }
        if let (Some(t), Some(ns)) = (&self.timer, target_ns) {
            let s = ns / 1_000_000_000;
            t.set_timestamp((s >> 32) as u32, s as u32, (ns % 1_000_000_000) as u32);
        }
    }

    /// Feedback for the next commit of ANOTHER surface of this window (the
    /// video subsurface, which commits on its own).
    pub fn feedback_on(&mut self, surface: &WlSurface, target_ns: Option<u64>) {
        if let (Some(p), true) = (&self.presentation, self.pending < 16) {
            p.feedback(surface, &self.queue.handle(), target_ns);
            self.pending += 1;
        }
    }

    /// The connection (shared with a [`crate::plane::VideoPlane`]) and
    /// whether it is someone else's.
    pub fn connection(&self) -> (&Connection, bool) {
        (&self.conn, self.foreign)
    }

    /// The window's surface.
    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }

    /// Outcomes that arrived since the last call. Never blocks.
    pub fn poll(&mut self) -> Vec<Outcome> {
        let _ = self.conn.flush();
        let _ = self.queue.dispatch_pending(&mut self.state);
        // Our own connection (tests): read the socket. NEVER on winit's:
        // its event loop holds a prepared read while it runs our code, and
        // wl_display_read_events waits for every prepared reader -- the app
        // froze on its first frame (found under weston). winit's own read
        // routes our events to our queue anyway.
        if !self.foreign {
            if let Some(guard) = self.queue.prepare_read() {
                let _ = guard.read();
            }
            let _ = self.queue.dispatch_pending(&mut self.state);
        }
        let out = std::mem::take(&mut self.state.outcomes);
        self.pending = self.pending.saturating_sub(out.len());
        out
    }
}

impl Drop for SurfaceLink {
    fn drop(&mut self) {
        if let Some(t) = self.timer.take() {
            t.destroy();
        }
        if let Some(c) = self.content.take() {
            c.destroy();
        }
        let _ = self.conn.flush();
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WpPresentation, ()> for State {
    fn event(s: &mut Self, _: &WpPresentation, e: wp_presentation::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_presentation::Event::ClockId { clk_id } = e {
            s.clock_id = Some(clk_id);
        }
    }
}

/// Feedback user data: the frame's target time.
impl Dispatch<WpPresentationFeedback, Option<u64>> for State {
    fn event(s: &mut Self, _: &WpPresentationFeedback, e: wp_presentation_feedback::Event, target: &Option<u64>, _: &Connection, _: &QueueHandle<Self>) {
        match e {
            wp_presentation_feedback::Event::Presented { tv_sec_hi, tv_sec_lo, tv_nsec, refresh, seq_hi, seq_lo, flags } => {
                let secs = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                let flags = match flags {
                    WEnum::Value(f) => f,
                    WEnum::Unknown(b) => Kind::from_bits_truncate(b),
                };
                s.outcomes.push(Outcome::Presented(Presented {
                    target_ns: *target,
                    at_ns: secs * 1_000_000_000 + tv_nsec as u64,
                    refresh_ns: refresh,
                    seq: ((seq_hi as u64) << 32) | seq_lo as u64,
                    vsync: flags.contains(Kind::Vsync),
                    zero_copy: flags.contains(Kind::ZeroCopy),
                    hw_clock: flags.contains(Kind::HwClock),
                }));
            }
            wp_presentation_feedback::Event::Discarded => s.outcomes.push(Outcome::Discarded { target_ns: *target }),
            _ => {}
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
ignore_events!(WpContentTypeManagerV1, WpContentTypeV1, WpCommitTimingManagerV1, WpCommitTimerV1);
