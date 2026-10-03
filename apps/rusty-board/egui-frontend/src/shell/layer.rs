//! shell::layer — the keyboard as a `zwlr_layer_shell_v1` surface, no winit.
//!
//! This is the shell that meets all three obligations `platform.rs` lists, the way the GTK
//! build does through gtk4-layer-shell and Maliit does through layer-shell-qt: the surface is
//! anchored to the bottom edge with an exclusive zone (other windows make room), it never
//! takes keyboard focus (`KeyboardInteractivity::None`), and it is mapped on Activate and
//! unmapped on Deactivate — the compositor's decision, delivered through the input-method
//! backend into `Feel`, which asks for it the only way the screen knows how:
//! `ViewportCommand::Visible`. This loop is the second reader of that command; eframe was
//! the first. Phosh, Sway and KWin (5.20+) all speak the protocol.
//!
//! ## What is here, in the order it runs
//!
//! 1. `Connection::connect_to_env` — the PUBLIC socket. `platform::capture_inherited_socket`
//!    has already moved KWin's privileged fd out of the environment, so this cannot land on
//!    the wrong one.
//! 2. sctk globals: compositor, layer shell (absent → [`LayerError::Unsupported`], and the
//!    binary falls back to eframe), seat (pointer + touch), outputs (for scale).
//! 3. One `wl_surface`, one layer surface, initial commit, wait for the first `configure`.
//! 4. EGL through glutin's EGL API from the raw `wl_display` / `wl_surface` pointers — glutin
//!    creates the `wl_egl_window` itself. `glow` over `eglGetProcAddress`, `egui_glow::Painter`
//!    on top. Swap interval 0: with interval 1, `eglSwapBuffers` blocks until the compositor's
//!    frame callback, and an UNMAPPED surface never gets one — a hidden keyboard would hang on
//!    its first hide. Throttling is done by `wl_surface.frame` instead (`frame_pending`).
//! 5. The loop: pump the input-method sink on every wake (cheap, no paint); run a
//!    `Feel::frame` only when it is DUE — input arrived, a configure landed, egui's
//!    `repaint_delay` elapsed, or the sink's wish disagrees with our map state; act on
//!    `Visible`; paint if mapped and the compositor has consumed the previous frame; sleep
//!    until the socket is readable or the next deadline. An idle keyboard paints nothing,
//!    which on a phone is the difference between a keyboard and a heater; a poll timeout of
//!    at most 50 ms bounds the Activate→shown latency.
//!
//! ## Map / unmap
//!
//! The protocol's own recipe (zwlr_layer_surface_v1, "unmapping"): attach a null buffer and
//! commit to hide; to show again, commit WITHOUT a buffer, wait for `configure`, ack, then
//! commit a buffer (which, with EGL, is what `swap_buffers` does). So `visible` is not one
//! flag but two: `mapped` (what we asked for) and `configured` (what the compositor has
//! acknowledged), and painting requires both. Rendering into an unconfigured surface is a
//! protocol error that kills the client.
//!
//! ## Not done here
//!
//! - Fractional scale (`wp_fractional_scale_v1`): integer `set_buffer_scale` from the
//!   compositor's preferred scale. On a 2.5× phone KWin scales the buffer; text is slightly
//!   soft. It is the right next protocol to add, in `scale_factor_changed`.
//! - Keyboard input: none by design (interactivity None).
//! - Cursor shape: the pointer over a keyboard keeps whatever cursor it had.
//!
//! Nothing in this file has been run against a compositor. It compiles; the protocol
//! sequence follows the spec text and sctk's `simple_layer` example; the EGL setup follows
//! glutin's own Wayland path. First run is on a device.

use std::ffi::CStr;
use std::num::NonZeroU32;
use std::os::fd::AsFd;
use std::ptr::NonNull;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glutin::config::{ConfigSurfaceTypes, ConfigTemplateBuilder};
use glutin::api::egl::context::PossiblyCurrentContext;
use glutin::context::{ContextApi, ContextAttributesBuilder, Version};
use glutin::display::GetGlDisplay;
use glutin::prelude::*;
use glutin::surface::{SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry, delegate_seat,
    delegate_touch,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        touch::TouchHandler,
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_pointer, wl_seat, wl_surface, wl_touch},
    Connection, Proxy, QueueHandle,
};

use crate::demo_app::Feel;
use crate::platform::Placement;

/// Logical height of the keyboard strip. Same number the KWin rule and the eframe viewport
/// use, so the three placements agree about how much of the screen a keyboard is.
pub const HEIGHT: u32 = 340;

/// The layer height for a content height in points: whole points, never below a floor a
/// thumb can use (GTK's own minimum is 120) and never absurd. Pure so the rounding and the
/// clamp are testable without a compositor.
/// What the keyboard tells the shell about where it is: the height of the strip it covers at the bottom
/// of the screen (logical px), 0 while it is hidden. One line, in `$XDG_RUNTIME_DIR/rusty-board.geom`,
/// rewritten only when it changes. tideshell reads it so its sheets sit ABOVE the keyboard instead of
/// under it.
pub fn geom_line(mapped: bool, configured: bool, height: u32) -> String {
    format!("{}\n", if mapped && configured { height } else { 0 })
}

fn publish_geom(line: &str) {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = std::path::Path::new(&dir).join("rusty-board.geom");
        let tmp = path.with_extension("geom.tmp");
        // Written aside and renamed, so a reader never sees half a line.
        if std::fs::write(&tmp, line).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

pub fn surface_height(content: f32) -> u32 {
    if !content.is_finite() {
        return HEIGHT;
    }
    content.ceil().clamp(120.0, 1600.0) as u32
}

/// Poll timeout while nothing is animating AND the sink has no descriptor to wait on. Bounds
/// the Activate→shown latency in that case; the GTK backend's SLOW_MS is in the same range.
const IDLE_POLL: Duration = Duration::from_millis(50);

/// Poll timeout when the sink's socket is polled too: nothing needs a timer to be noticed, so
/// this is only a safety net. 20 wakes a second used to be the idle floor, which on a phone is
/// a radio-and-CPU tax paid all day by a keyboard nobody is touching.
const IDLE_POLL_WITH_FD: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum LayerError {
    /// The compositor has no `zwlr_layer_shell_v1` (or no `wl_compositor`, or the socket is
    /// not there). The caller should run the eframe shell instead; the keyboard still works,
    /// it just cannot dock.
    Unsupported(String),
    /// Something failed AFTER the shell was up — GL, a dead connection. Not a reason to try
    /// another shell; surface it.
    Runtime(anyhow::Error),
}

impl std::fmt::Display for LayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LayerError::Unsupported(s) => write!(f, "layer-shell unsupported: {s}"),
            LayerError::Runtime(e) => write!(f, "layer-shell runtime: {e:#}"),
        }
    }
}

/// Which layer to sit on. `RUSTY_BOARD_LAYER=overlay` puts the keyboard above fullscreen
/// windows and lock screens; the default `top` is above normal windows and below fullscreen,
/// which is where an OSK belongs unless the lock screen itself needs typing.
fn layer_from_env() -> Layer {
    match std::env::var("RUSTY_BOARD_LAYER").ok().as_deref().map(str::trim) {
        Some("overlay") => Layer::Overlay,
        Some("bottom") => Layer::Bottom,
        _ => Layer::Top,
    }
}

/// Everything the Wayland dispatch handlers write into. The GL objects live outside (in
/// `run`), because they borrow the connection pointers and are created after the first
/// configure; the handlers only need to know sizes and to queue egui events.
struct State {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    pointer: Option<wl_pointer::WlPointer>,
    touch: Option<wl_touch::WlTouch>,
    layer: LayerSurface,
    /// Logical size the compositor last configured. `None` until the first configure.
    size: Option<(u32, u32)>,
    /// The height we ask for: [`HEIGHT`] until the first frame says what the content needs
    /// (`Feel::desired_height`), then that -- see [`surface_height`].
    height: u32,
    /// Integer buffer scale the compositor prefers for our surface (its `wl_output` scale).
    scale: i32,
    /// A `configure` arrived that we have not painted for yet.
    needs_paint: bool,
    /// We asked the compositor to show us and have (since then) received a configure.
    configured: bool,
    /// A `wl_surface.frame` callback is outstanding — don't paint again until it lands.
    frame_pending: bool,
    /// When that callback was requested, to notice one that never lands.
    frame_sent: Option<Instant>,
    /// The input-method socket hung up; stop polling it (see `wait_for_socket`).
    sink_fd_dead: bool,
    /// The compositor closed the layer surface (output gone, shell shutting down).
    closed: bool,
    /// egui input for the next frame.
    events: Vec<egui::Event>,
    /// The first finger down, whose id is the one that also drives the pointer emulation.
    primary_touch: Option<i32>,
    /// Last pointer/primary-finger position in logical points, for pointer-button events.
    last_pos: egui::Pos2,
}

impl State {
    fn logical_size(&self) -> (u32, u32) {
        self.size.unwrap_or((0, self.height))
    }

    fn pixel_size(&self) -> [u32; 2] {
        let (w, h) = self.logical_size();
        let s = self.scale.max(1) as u32;
        [w.max(1) * s, h.max(1) * s]
    }

    fn pointer_button(&mut self, pressed: bool) {
        self.events.push(egui::Event::PointerButton {
            pos: self.last_pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
    }
}

/// Everything the layer surface is told about itself, in one place, because it has to be said
/// twice: once after `get_layer_surface`, and again on every re-map — unmapping (a null-buffer
/// commit) returns the surface to its freshly-created state, anchor included. Bottom edge, full
/// width (width 0 = stretch between the two side anchors), a strip HEIGHT tall that other
/// windows must make room for, and NO keyboard focus — the text goes to whoever had it.
fn apply_layer_state(layer: &LayerSurface, height: u32) {
    layer.set_anchor(Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, height);
    layer.set_exclusive_zone(height as i32);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
}

/// Run the keyboard under the layer shell until the compositor closes it. `Err(Unsupported)`
/// means nothing was shown and the caller may run a different shell.
pub fn run(title: &str) -> Result<(), LayerError> {
    let conn = Connection::connect_to_env().map_err(|e| LayerError::Unsupported(format!("connect: {e}")))?;
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|e| LayerError::Unsupported(format!("registry: {e}")))?;
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).map_err(|e| LayerError::Unsupported(format!("wl_compositor: {e}")))?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|e| LayerError::Unsupported(format!("zwlr_layer_shell_v1: {e}")))?;

    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(&qh, surface, layer_from_env(), Some(title.to_string()), None);
    apply_layer_state(&layer, HEIGHT);
    // Initial commit without a buffer: this is what asks for the first configure.
    layer.commit();

    let mut state = State {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        pointer: None,
        touch: None,
        layer,
        size: None,
        height: HEIGHT,
        scale: 1,
        needs_paint: false,
        configured: false,
        frame_pending: false,
        frame_sent: None,
        sink_fd_dead: false,
        closed: false,
        events: Vec::new(),
        primary_touch: None,
        last_pos: egui::Pos2::ZERO,
    };

    // Wait for the first configure so the EGL window can be created at a real size.
    while state.size.is_none() && !state.closed {
        queue.blocking_dispatch(&mut state).map_err(|e| LayerError::Unsupported(format!("first configure: {e}")))?;
    }
    if state.closed {
        return Err(LayerError::Unsupported("layer surface closed before configure".into()));
    }

    let mut gl = Gl::new(&conn, state.layer.wl_surface(), state.pixel_size()).map_err(LayerError::Runtime)?;
    let ctx = egui::Context::default();
    let mut feel = Feel::with_placement(Placement::LayerShell);
    // We are mapped from the first paint; `Feel` decides from the sink whether we should be,
    // and its first frame will say Visible(false) on a Wayland session with nothing focused.
    let mut mapped = true;
    let mut map_requested: Option<Instant> = None;
    let mut last_geom = String::new();
    // Hidden by parking (1 px, no zone), and the transparent parked buffer is still owed.
    let mut park_paint_owed = false;
    let start = Instant::now();
    let mut last_frame = Instant::now();
    // When egui next wants a frame regardless of input (animations, request_repaint_after).
    let mut repaint_at = Instant::now();
    // Texture uploads owed to the painter, accumulated across frames that ran but were not
    // painted. See the note at the paint site.
    let mut pending_textures = egui::TexturesDelta::default();

    tracing::info!("layer-shell shell up: {}x{} @{}x, layer {:?}", state.logical_size().0, state.logical_size().1, state.scale, layer_from_env());

    let mut callback_lost_noted = false;
    // RUSTY_BOARD_LOOP_STATS=1: every 5 s, one stderr line saying how many frames ran and WHY
    // (input, a configure, the sink's show/hide wish, or egui asking), how many were painted and
    // how often the loop woke. Printed on the first wake after each 5 s, so a truly idle keyboard
    // prints nothing at all (it does not wake); a busy one names its cause.
    let mut stats = LoopStats::from_env();
    while !state.closed {
        stats.report_if_due();
        if stats.on {
            stats.gates = format!("mapped={mapped} configured={} frame_pending={}", state.configured, state.frame_pending);
        }
        // ---- 0. the sink first: Activate/Deactivate are seen on every wake, and a change of
        //         wish is itself a reason to run a frame (which is what emits `Visible`).
        let wanted = feel.pump_sink();
        // A FRAME CALLBACK THAT NEVER LANDS stalls painting for good: `frame_pending`
        // stays set and nothing paints again. Treat it as lost after a second, log once
        // per stall, and paint anyway -- a keyboard stuck on its last (or first, empty)
        // frame looked exactly like the empty band reported under COSMIC.
        if state.frame_pending && state.frame_sent.is_some_and(|t| t.elapsed() > FRAME_CALLBACK_GRACE) {
            if !callback_lost_noted {
                tracing::warn!("layer: no frame callback for {:?}; treating it as lost and repainting", FRAME_CALLBACK_GRACE);
                callback_lost_noted = true;
            }
            state.frame_pending = false;
        }
        // `needs_paint` is due ONLY WHEN A PAINT IS POSSIBLE. It is cleared only by
        // painting, so while painting was blocked -- callback outstanding, surface not yet
        // configured -- every iteration counted as due, and the loop ran the whole
        // keyboard UI about 20 times a second, forever, queueing each frame's textures:
        // reported as 4+ GB and an overheating phone.
        // A re-map is sometimes never answered with a configure (seen on cosmic-comp: the
        // surface holds its exclusive zone, paints nothing, and the screen keeps a black
        // band). The commit changed no state the compositor cares about, so change one:
        // one pixel taller, then back. Both commits are real state; the second wins.
        if mapped && !state.configured {
            if let Some(t0) = map_requested {
                if t0.elapsed() >= Duration::from_millis(250) {
                    state.layer.set_size(0, state.height + 1);
                    state.layer.wl_surface().commit();
                    state.layer.set_size(0, state.height);
                    state.layer.wl_surface().commit();
                    map_requested = Some(Instant::now());
                    tracing::debug!("layer: no configure after map, nudged");
                }
            }
        } else {
            map_requested = None;
        }
        let can_paint = mapped && state.configured && !state.frame_pending;
        let due = feel.take_needs_frame()
            || !state.events.is_empty()
            || (state.needs_paint && can_paint)
            || wanted != mapped
            || Instant::now() >= repaint_at;
        stats.wakes += 1;
        if due {
            stats.count(!state.events.is_empty(), state.needs_paint && can_paint, wanted != mapped, Instant::now() >= repaint_at);
        }
        if !due {
            let sink_fd = feel.sink_wake_fd().filter(|_| !state.sink_fd_dead);
            let timeout = deadline_timeout(repaint_at, &state, sink_fd.is_some());
            // Waiting for a configure that may never come: wake to run the nudge, or the loop
            // sleeps until the next input and the board stays unpainted.
            let timeout = if mapped && !state.configured { timeout.min(Duration::from_millis(300)) } else { timeout };
            wait_for_socket(&conn, &mut queue, &mut state, timeout, sink_fd, feel.ctl_fd())?;
            continue;
        }

        // ---- 1. one egui frame.
        // egui lays the board out against this height, and the board's wanted height depends on
        // it: while parked (1 px) or before the show's configure arrives, keep telling it the
        // full height, or it asks for a tiny surface when it comes back.
        let (lw, mut lh) = state.logical_size();
        if !(mapped && state.configured) {
            lh = state.height;
        }
        let ppp = state.scale.max(1) as f32;
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(lw as f32, lh as f32))),
            time: Some(start.elapsed().as_secs_f64()),
            predicted_dt: last_frame.elapsed().as_secs_f32().clamp(1.0 / 240.0, 0.1),
            events: std::mem::take(&mut state.events),
            ..Default::default()
        };
        last_frame = Instant::now();
        ctx.set_pixels_per_point(ppp);
        let out = ctx.run_ui(raw, |ctx| feel.frame(ctx));

        // ---- 2. the screen's one request of the shell.
        let mut want_visible = None;
        let mut repaint_delay = Duration::from_secs(3600);
        if let Some(vp) = out.viewport_output.get(&egui::ViewportId::ROOT) {
            repaint_delay = vp.repaint_delay;
            // egui says "never" as Duration::MAX; `Instant + MAX` panics (overflow). The old
            // unconditional 320 ms repaint request hid this: no frame ever said "never".
            repaint_at = repaint_deadline(Instant::now(), repaint_delay);
            for cmd in &vp.commands {
                if let egui::ViewportCommand::Visible(v) = cmd {
                    want_visible = Some(*v);
                }
            }
        }
        match want_visible {
            Some(false) if mapped => {
                // Hide by PARKING, not unmapping: a null-buffer commit unmaps the surface, and
                // cosmic-comp then never answers the next show with a configure (WAYLAND_DEBUG
                // on the FP5: set_size + commit, no configure, ever; the board reserved its
                // strip and drew nothing). A size change on a mapped surface is always
                // answered, so shrink to one transparent pixel with no exclusive zone, which
                // gives the app below its space back just the same.
                state.layer.set_size(0, 1);
                state.layer.set_exclusive_zone(0);
                state.layer.wl_surface().commit();
                park_paint_owed = true;
                mapped = false;
                state.configured = false;
                state.frame_pending = false;
                tracing::debug!("layer: unmapped");
            }
            Some(true) if !mapped => {
                // Show: commit without a buffer, then wait for configure (handled below).
                //
                // THE WHOLE STATE, not just size and exclusive zone. Unmapping resets the layer
                // surface "to the state it had right after layer_shell.get_layer_surface"
                // (wlr-layer-shell-unstable-v1, "unmapping"), so the anchor is gone too — and
                // a width of 0 without both side anchors is a protocol error, not a default:
                //   zwlr_layer_surface_v1#7: error 1: the layer surface has a width of 0 but
                //   its anchor doesn't include the left and the right screen edge
                // which is how every hide→show cycle on a strict compositor (KDE, sway) killed
                // the shell. Phones that never hide the board never saw it.
                apply_layer_state(&state.layer, state.height);
                state.layer.wl_surface().commit();
                park_paint_owed = false;
                // The parked configure is stale: the resize back to full height gets its own.
                state.configured = false;
                mapped = true;
                map_requested = Some(Instant::now());
                tracing::debug!("layer: map requested");
            }
            _ => {}
        }
        // ---- 2b. the surface is as tall as its content. A fixed 340 left a strip of nothing
        //          under the keys (and reserved it from the app above, via the exclusive
        //          zone). GTK resizes its layer surface to the computed height on every
        //          rebuild; this does the same from what the frame measured. Only on change,
        //          so a steady board costs no requests; the configure that answers it resizes
        //          the next frame's screen_rect.
        if let Some(h) = feel.desired_height().map(surface_height).filter(|_| mapped && state.configured) {
            if h != state.height {
                state.height = h;
                if mapped {
                    state.layer.set_size(0, h);
                    state.layer.set_exclusive_zone(h as i32);
                    state.layer.wl_surface().commit();
                    tracing::debug!(height = h, "layer: resized to content");
                }
            }
        }

        // ---- 2c. tell the shell where the keyboard is (see `geom_line`), when that changed.
        let line = geom_line(mapped, state.configured, state.height);
        if line != last_geom {
            publish_geom(&line);
            last_geom = line;
        }

        // ---- 3. paint if the compositor has said we may. Every frame that ran is painted
        //         (that is what egui's model expects; frames only run when due).
        // A SKIPPED FRAME MUST NOT DROP ITS TEXTURE DELTA. Shapes are cheap to lose — the
        // next frame re-runs the UI and produces them again — but a `TexturesDelta` is an
        // upload instruction egui issues ONCE. The font atlas goes up in the delta of the
        // FIRST frame, and the first frame is exactly the one this loop skips: the initial
        // commit is made without a buffer and painting waits for `configure`.
        //
        // Losing it fails quietly. Later frames tessellate correctly and reference a texture
        // that was never uploaded, so every glyph and every rounded corner samples an empty
        // atlas — the board renders as a black rectangle that still takes taps, because
        // input never went through the painter at all.
        //
        // So: accumulate across skipped frames and apply on the next paint that happens.
        // MERGED, NOT APPENDED -- see `merge_textures`. Appending grew without bound
        // whenever painting stalled.
        merge_textures(&mut pending_textures, out.textures_delta);
        if mapped && state.configured && !state.frame_pending {
            let px = state.pixel_size();
            gl.resize_if_needed(px).map_err(LayerError::Runtime)?;
            state.layer.wl_surface().set_buffer_scale(state.scale.max(1));
            let primitives = ctx.tessellate(out.shapes, ppp);
            gl.paint(px, ppp, &primitives, &pending_textures);
            pending_textures.clear();
            // Throttle to the compositor: no more paints until it says it consumed this one.
            state.layer.wl_surface().frame(&qh, state.layer.wl_surface().clone());
            state.frame_pending = true;
            state.frame_sent = Some(Instant::now());
            callback_lost_noted = false;
            gl.swap().map_err(LayerError::Runtime)?;
            state.needs_paint = false;
            stats.paints += 1;
        }
        // Parked: the compositor answered the 1 px resize, so give it a matching transparent
        // buffer (a buffer of any other size is a surface of any other size).
        if !mapped && park_paint_owed && state.configured {
            let px = state.pixel_size();
            gl.resize_if_needed(px).map_err(LayerError::Runtime)?;
            gl.paint(px, ppp, &[], &egui::TexturesDelta::default());
            gl.swap().map_err(LayerError::Runtime)?;
            park_paint_owed = false;
            tracing::debug!("layer: parked");
        }

        // ---- 4. sleep until the socket is readable or egui's next deadline.
        let sink_fd = feel.sink_wake_fd().filter(|_| !state.sink_fd_dead);
        let timeout = deadline_timeout(repaint_at, &state, sink_fd.is_some());
            // Waiting for a configure that may never come: wake to run the nudge, or the loop
            // sleeps until the next input and the board stays unpainted.
            let timeout = if mapped && !state.configured { timeout.min(Duration::from_millis(300)) } else { timeout };
        wait_for_socket(&conn, &mut queue, &mut state, timeout, sink_fd, feel.ctl_fd())?;
        let _ = repaint_delay;
    }

    gl.destroy();
    tracing::info!("layer-shell shell: surface closed by the compositor, exiting");
    Ok(())
}

/// When egui next wants a frame: `now + delay`, where a delay too large to represent (egui
/// uses `Duration::MAX` for "no repaint wanted") becomes a day away — the idle cap bounds the
/// actual sleep anyway.
pub fn repaint_deadline(now: Instant, delay: Duration) -> Instant {
    now.checked_add(delay).unwrap_or_else(|| now + Duration::from_secs(86_400))
}

/// Opt-in loop accounting (`RUSTY_BOARD_LOOP_STATS=1`). Costs a branch per wake when off.
#[derive(Default)]
struct LoopStats {
    on: bool,
    since: Option<Instant>,
    wakes: u32,
    paints: u32,
    input: u32,
    configure: u32,
    wish: u32,
    egui: u32,
    /// The paint gates at the last wake: why a mapped surface is not painting.
    gates: String,
}

impl LoopStats {
    fn from_env() -> Self {
        let on = matches!(std::env::var("RUSTY_BOARD_LOOP_STATS").ok().as_deref(), Some("1") | Some("true"));
        LoopStats { on, since: Some(Instant::now()), ..Default::default() }
    }
    /// One frame ran; attribute it to every reason that made it due.
    fn count(&mut self, input: bool, configure: bool, wish: bool, egui: bool) {
        self.input += input as u32;
        self.configure += configure as u32;
        self.wish += wish as u32;
        self.egui += egui as u32;
    }
    fn report_if_due(&mut self) {
        if !self.on || self.since.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
            return;
        }
        let frames = self.input.max(self.configure).max(self.wish).max(self.egui);
        eprintln!(
            "[rusty-board] loop/5s: wakes {} paints {} | frames due to: input {} configure {} show/hide {} egui-repaint {} | {}{}",
            self.wakes, self.paints, self.input, self.configure, self.wish, self.egui, self.gates,
            if frames == 0 { " (idle)" } else { "" },
        );
        *self = LoopStats { on: true, since: Some(Instant::now()), ..Default::default() };
    }
}

/// How long a frame callback may take before it is treated as lost.
const FRAME_CALLBACK_GRACE: Duration = Duration::from_secs(1);

/// Fold a frame's texture delta into the ones still waiting for a paint.
///
/// Skipped frames must not DROP their deltas -- the first frame's font atlas arrives
/// before the surface can paint, and losing it drew a black rectangle (see the loop).
/// But APPENDING them grows without bound if painting stalls. Merging keeps only what
/// still matters:
///   * a newer FULL image for a texture replaces everything queued for it;
///   * freeing a texture drops its queued uploads (the free itself is kept -- freeing a
///     texture the GPU never received is a no-op in egui_glow's painter);
///   * partial updates are kept in order, since each patches the one before.
/// So memory is bounded by the textures alive, however long a stall lasts.
pub fn merge_textures(pending: &mut egui::TexturesDelta, newer: egui::TexturesDelta) {
    for id in &newer.free {
        pending.set.retain(|(t, _)| t != id);
        if !pending.free.contains(id) {
            pending.free.push(*id);
        }
    }
    for (id, delta) in newer.set {
        if delta.pos.is_none() {
            pending.set.retain(|(t, _)| *t != id);
        }
        // A texture re-created after being freed must not be freed again afterwards.
        pending.free.retain(|t| *t != id);
        pending.set.push((id, delta));
    }
}

/// How long the loop may sleep: until egui's deadline, capped so the sink is pumped often
/// enough (IDLE_POLL when its socket cannot be polled, a long safety net when it can) — and
/// never a zero-timeout spin while a frame callback is outstanding (the callback arrives on the
/// socket; poll wakes us for it; the cap is the lost-callback grace so that check still runs).
fn deadline_timeout(repaint_at: Instant, state: &State, sink_polled: bool) -> Duration {
    idle_cap_timeout(repaint_at, Instant::now(), state.frame_pending, state.frame_sent, sink_polled)
}

/// Pure half of [`deadline_timeout`], testable without a compositor.
pub fn idle_cap_timeout(
    repaint_at: Instant,
    now: Instant,
    frame_pending: bool,
    frame_sent: Option<Instant>,
    sink_polled: bool,
) -> Duration {
    let cap = if sink_polled { IDLE_POLL_WITH_FD } else { IDLE_POLL };
    if frame_pending {
        let grace_left = frame_sent
            .map(|t| (t + FRAME_CALLBACK_GRACE).saturating_duration_since(now))
            .unwrap_or(FRAME_CALLBACK_GRACE);
        // A little past the grace so the check at the top of the loop sees it expired.
        (grace_left + Duration::from_millis(5)).min(cap)
    } else {
        repaint_at.saturating_duration_since(now).min(cap)
    }
}

/// Flush, dispatch what is already buffered, sleep on the socket up to `timeout`, read and
/// dispatch again. The prepare_read/poll/read sequence is wayland-client's own recipe for a
/// single-threaded client that owns its loop.
fn wait_for_socket(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    timeout: Duration,
    sink_fd: Option<i32>,
    ctl_fd: Option<i32>,
) -> Result<(), LayerError> {
    conn.flush().map_err(|e| LayerError::Runtime(anyhow::anyhow!("flush: {e}")))?;
    // Anything already buffered is dispatched before sleeping, so an event that arrived
    // during the paint is not delayed by a whole poll timeout.
    queue.dispatch_pending(state).map_err(|e| LayerError::Runtime(anyhow::anyhow!("dispatch: {e}")))?;
    if let Some(guard) = conn.prepare_read() {
        let fd = guard.connection_fd();
        // Our socket, plus the input-method connection's when the sink has one: an
        // Activate/Deactivate on that connection wakes us directly. The sink's socket is
        // only WATCHED here; `Feel::pump_sink` at the top of the loop reads it.
        // A negative fd is ignored by poll(2), so absent ones simply never fire.
        // The ctl socket (ctl.rs) is watched the same way: a command from the
        // shell wakes an idle keyboard at once.
        let mut pfds = [
            libc::pollfd { fd: fd.as_fd().as_raw_fd_compat(), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: sink_fd.unwrap_or(-1), events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: ctl_fd.unwrap_or(-1), events: libc::POLLIN, revents: 0 },
        ];
        let n = 3;
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        // SAFETY: pfds is a valid, initialised array of `n` pollfds for the duration of the call.
        let _ = unsafe { libc::poll(pfds.as_mut_ptr(), n as libc::nfds_t, ms) };
        // A hung-up input-method socket stays "ready" forever; stop watching it (the
        // sink's own pump logs the dead connection) rather than spin on it.
        if sink_fd.is_some() && pfds[1].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 {
            if !state.sink_fd_dead {
                tracing::warn!("layer: input-method socket hung up; falling back to timed polling");
            }
            state.sink_fd_dead = true;
        }
        // POLLHUP/POLLERR on OUR socket: read anyway, so the error surfaces as a Runtime
        // error instead of a zero-timeout loop around a dead connection.
        if pfds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            match guard.read() {
                Ok(_) => {}
                Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(LayerError::Runtime(anyhow::anyhow!("read: {e}"))),
            }
        } else {
            drop(guard);
        }
    } else {
        // Events were already buffered by someone else's read (never, single-threaded — but
        // the API allows it); don't spin.
        std::thread::sleep(timeout.min(Duration::from_millis(4)));
    }
    queue.dispatch_pending(state).map_err(|e| LayerError::Runtime(anyhow::anyhow!("dispatch: {e}")))?;
    Ok(())
}

/// `AsRawFd` without importing it at every call site; sctk's guard hands back a `BorrowedFd`.
trait RawFdCompat {
    fn as_raw_fd_compat(&self) -> i32;
}
impl RawFdCompat for std::os::fd::BorrowedFd<'_> {
    fn as_raw_fd_compat(&self) -> i32 {
        use std::os::fd::AsRawFd;
        self.as_raw_fd()
    }
}

/// The GL half: EGL display/context/surface via glutin, glow, and the egui painter.
struct Gl {
    display: glutin::api::egl::display::Display,
    context: PossiblyCurrentContext,
    surface: glutin::api::egl::surface::Surface<WindowSurface>,
    painter: egui_glow::Painter,
    size: [u32; 2],
}

impl Gl {
    fn new(conn: &Connection, wl_surface: &wl_surface::WlSurface, size: [u32; 2]) -> anyhow::Result<Gl> {
        use anyhow::{anyhow, Context as _};
        // Raw pointers into libwayland: this needs wayland-backend's `client_system` (the
        // libwayland-backed implementation), which the `layer` feature turns on. The pure-Rust
        // backend has no `wl_proxy*` for EGL to bind to.
        let display_ptr = NonNull::new(conn.backend().display_ptr() as *mut std::ffi::c_void)
            .ok_or_else(|| anyhow!("null wl_display"))?;
        let surface_ptr = NonNull::new(wl_surface.id().as_ptr() as *mut std::ffi::c_void)
            .ok_or_else(|| anyhow!("wl_surface has no proxy pointer (is the pure-Rust wayland backend in use?)"))?;
        let rdh = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display_ptr));
        let rwh = RawWindowHandle::Wayland(WaylandWindowHandle::new(surface_ptr));

        // SAFETY: the display pointer is a live wl_display owned by `conn`, which outlives Gl.
        let display = unsafe { glutin::api::egl::display::Display::new(rdh) }.context("EGL display")?;
        let template = ConfigTemplateBuilder::new()
            .with_alpha_size(8)
            .with_surface_type(ConfigSurfaceTypes::WINDOW)
            .build();
        // SAFETY: display is valid.
        let config = unsafe { display.find_configs(template) }
            .context("EGL configs")?
            .next()
            .ok_or_else(|| anyhow!("no EGL config with an alpha channel and window support"))?;
        let attrs = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(Some(Version::new(2, 0))))
            .build(Some(rwh));
        // SAFETY: config belongs to display.
        let not_current = unsafe { display.create_context(&config, &attrs) }.context("EGL context")?;
        let sattrs = SurfaceAttributesBuilder::<WindowSurface>::new().build(
            rwh,
            NonZeroU32::new(size[0].max(1)).unwrap(),
            NonZeroU32::new(size[1].max(1)).unwrap(),
        );
        // SAFETY: rwh is a live wl_surface proxy that outlives the EGL surface (destroy() runs first).
        let surface = unsafe { display.create_window_surface(&config, &sattrs) }.context("EGL window surface")?;
        let context = not_current.make_current(&surface).context("make current")?;
        // Interval 0 — see the module doc: interval 1 blocks in eglSwapBuffers waiting for a
        // frame callback an unmapped surface never receives.
        surface.set_swap_interval(&context, SwapInterval::DontWait).context("swap interval")?;

        let gl_display = context.display();
        // SAFETY: the loader is queried while the context is current on this thread.
        let glow = unsafe {
            glow::Context::from_loader_function_cstr(|name: &CStr| gl_display.get_proc_address(name) as *const _)
        };
        let painter = egui_glow::Painter::new(Arc::new(glow), "", None, false)
            .map_err(|e| anyhow!("egui_glow painter: {e}"))?;
        Ok(Gl { display, context, surface, painter, size })
    }

    fn resize_if_needed(&mut self, size: [u32; 2]) -> anyhow::Result<()> {
        if size != self.size {
            self.surface.resize(
                &self.context,
                NonZeroU32::new(size[0].max(1)).unwrap(),
                NonZeroU32::new(size[1].max(1)).unwrap(),
            );
            self.size = size;
        }
        Ok(())
    }

    fn paint(&mut self, size: [u32; 2], ppp: f32, primitives: &[egui::ClippedPrimitive], textures: &egui::TexturesDelta) {
        // Transparent clear: the board paints its own background, and anything it leaves
        // clear shows the app underneath — which is how the glass palette reads on a phone.
        self.painter.clear(size, [0.0, 0.0, 0.0, 0.0]);
        self.painter.paint_and_update_textures(size, ppp, primitives, textures);
    }

    fn swap(&self) -> anyhow::Result<()> {
        use anyhow::Context as _;
        self.surface.swap_buffers(&self.context).context("eglSwapBuffers")
    }

    fn destroy(&mut self) {
        self.painter.destroy();
        let _ = &self.display;
    }
}

// ---------------------------------------------------------------- sctk handlers

impl CompositorHandler for State {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, new_factor: i32) {
        if new_factor != self.scale {
            self.scale = new_factor;
            self.needs_paint = true;
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.frame_pending = false;
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.closed = true;
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface, configure: LayerSurfaceConfigure, _serial: u32) {
        // sctk acks the configure for us. A zero dimension means "you choose": keep the
        // height we asked for and, for width, what we last knew (or the ask).
        let (w, h) = configure.new_size;
        let prev = self.size.unwrap_or((0, self.height));
        let w = if w == 0 { prev.0.max(1) } else { w };
        let h = if h == 0 { self.height } else { h };
        self.size = Some((w, h));
        self.configured = true;
        self.needs_paint = true;
    }
}

impl SeatHandler for State {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
        if capability == Capability::Touch && self.touch.is_none() {
            self.touch = self.seat_state.get_touch(qh, &seat).ok();
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() { p.release(); }
        }
        if capability == Capability::Touch {
            if let Some(t) = self.touch.take() { t.release(); }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for State {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for ev in events {
            if ev.surface != *self.layer.wl_surface() {
                continue;
            }
            let pos = egui::pos2(ev.position.0 as f32, ev.position.1 as f32);
            match ev.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.last_pos = pos;
                    self.events.push(egui::Event::PointerMoved(pos));
                }
                PointerEventKind::Leave { .. } => {
                    self.events.push(egui::Event::PointerGone);
                }
                // BTN_LEFT is 0x110; any button is a press for a keyboard.
                PointerEventKind::Press { .. } => {
                    self.last_pos = pos;
                    self.events.push(egui::Event::PointerMoved(pos));
                    self.pointer_button(true);
                }
                PointerEventKind::Release { .. } => {
                    self.last_pos = pos;
                    self.pointer_button(false);
                }
                PointerEventKind::Axis { .. } => {}
            }
        }
    }
}

impl TouchHandler for State {
    fn down(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _serial: u32, _time: u32, surface: wl_surface::WlSurface, id: i32, position: (f64, f64)) {
        if surface != *self.layer.wl_surface() {
            return;
        }
        let pos = egui::pos2(position.0 as f32, position.1 as f32);
        self.events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(id as u64),
            phase: egui::TouchPhase::Start,
            pos,
            force: None,
        });
        // The first finger is the pointer: egui's widgets (and so every key) are driven by
        // pointer events; Touch events feed multi-touch gestures only. Same emulation
        // egui-winit does.
        if self.primary_touch.is_none() {
            self.primary_touch = Some(id);
            self.last_pos = pos;
            self.events.push(egui::Event::PointerMoved(pos));
            self.pointer_button(true);
        }
    }

    fn up(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _serial: u32, _time: u32, id: i32) {
        self.events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(id as u64),
            phase: egui::TouchPhase::End,
            pos: self.last_pos,
            force: None,
        });
        if self.primary_touch == Some(id) {
            self.primary_touch = None;
            self.pointer_button(false);
            self.events.push(egui::Event::PointerGone);
        }
    }

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _time: u32, id: i32, position: (f64, f64)) {
        let pos = egui::pos2(position.0 as f32, position.1 as f32);
        self.events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(id as u64),
            phase: egui::TouchPhase::Move,
            pos,
            force: None,
        });
        if self.primary_touch == Some(id) {
            self.last_pos = pos;
            self.events.push(egui::Event::PointerMoved(pos));
        }
    }

    fn shape(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _id: i32, _major: f64, _minor: f64) {}
    fn orientation(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch, _id: i32, _orientation: f64) {}

    fn cancel(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_touch::WlTouch) {
        // The compositor took the touch sequence (a gesture, a swipe from the edge). Release
        // whatever egui thinks is pressed so no key stays down.
        if self.primary_touch.take().is_some() {
            self.pointer_button(false);
            self.events.push(egui::Event::PointerGone);
        }
        self.events.push(egui::Event::Touch {
            device_id: egui::TouchDeviceId(0),
            id: egui::TouchId(0),
            phase: egui::TouchPhase::Cancel,
            pos: self.last_pos,
            force: None,
        });
    }
}

delegate_compositor!(State);
delegate_output!(State);
delegate_seat!(State);
delegate_pointer!(State);
delegate_touch!(State);
delegate_layer!(State);
delegate_registry!(State);

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_geom_line_is_the_height_only_while_the_keyboard_is_up() {
        assert_eq!(super::geom_line(true, true, 312), "312\n");
        assert_eq!(super::geom_line(false, true, 312), "0\n", "hidden (parked)");
        assert_eq!(super::geom_line(true, false, 312), "0\n", "not configured yet: it covers nothing");
    }

    /// A frame that runs but is not painted must hand its texture uploads to the next one.
    ///
    /// `TexturesDelta` is an instruction egui issues ONCE. The font atlas goes up in the
    /// first frame's delta, and the first frame is the one this loop skips — the initial
    /// commit is made without a buffer and painting waits for `configure`. Dropping it does
    /// not fail loudly: later frames reference a texture that was never uploaded, every
    /// glyph and rounded corner samples an empty atlas, and the board is a black rectangle
    /// that still takes taps, because input never went through the painter at all.
    #[test]
    fn skipped_frames_hand_their_texture_uploads_forward() {
        use egui::epaint::{image::ImageDelta, ColorImage, TextureId};
        let img = |v: u8| {
            ImageDelta::full(
                ColorImage::new([1, 1], vec![egui::Color32::from_gray(v)]),
                egui::TextureOptions::LINEAR,
            )
        };
        let mut pending = egui::TexturesDelta::default();

        // Frame 1: the font atlas. Not painted — no configure yet.
        let mut first = egui::TexturesDelta::default();
        first.set.push((TextureId::Managed(0), img(1)));
        pending.append(first);
        assert_eq!(pending.set.len(), 1, "the atlas upload was dropped");

        // Frame 2: also skipped, and adds its own.
        let mut second = egui::TexturesDelta::default();
        second.set.push((TextureId::Managed(1), img(2)));
        pending.append(second);
        assert_eq!(pending.set.len(), 2, "a second skipped frame lost an upload");

        // Frame 3 paints: everything owed goes up at once, then the slate is clean.
        assert!(!pending.is_empty());
        pending.clear();
        assert!(pending.is_empty(), "the pending set would be re-uploaded every frame");

        // A free is carried too, or a texture leaks for the process's lifetime.
        let mut freeing = egui::TexturesDelta::default();
        freeing.free.push(TextureId::Managed(1));
        pending.append(freeing);
        assert_eq!(pending.free.len(), 1);
    }
}

#[cfg(test)]
mod merge_tests {
    use super::merge_textures;
    use egui::epaint::ImageDelta;
    use egui::{Color32, ColorImage, TextureId, TextureOptions, TexturesDelta};

    fn full(id: u64, px: usize) -> (TextureId, ImageDelta) {
        (TextureId::Managed(id), ImageDelta::full(ColorImage::new([px, px], vec![Color32::WHITE; px * px]), TextureOptions::LINEAR))
    }
    fn part(id: u64) -> (TextureId, ImageDelta) {
        (TextureId::Managed(id), ImageDelta::partial([0, 0], ColorImage::new([2, 2], vec![Color32::RED; 4]), TextureOptions::LINEAR))
    }
    fn delta(set: Vec<(TextureId, ImageDelta)>, free: Vec<u64>) -> TexturesDelta {
        TexturesDelta { set, free: free.into_iter().map(TextureId::Managed).collect() }
    }

    /// THE LEAK, bounded: a thousand stalled frames each re-sending one texture in full
    /// used to queue a thousand copies. Merged, one remains.
    #[test]
    fn a_stall_keeps_one_copy_of_a_resent_texture() {
        let mut p = TexturesDelta::default();
        for _ in 0..1000 { merge_textures(&mut p, delta(vec![full(1, 64)], vec![])); }
        assert_eq!(p.set.len(), 1);
    }

    /// A texture created and freed every frame (a load_texture in the UI) must not
    /// accumulate either: each free drops the upload it cancels.
    #[test]
    fn churned_textures_do_not_accumulate() {
        let mut p = TexturesDelta::default();
        for f in 0..1000u64 {
            merge_textures(&mut p, delta(vec![full(100 + f, 32)], if f > 0 { vec![99 + f] } else { vec![] }));
        }
        assert_eq!(p.set.len(), 1, "only the live texture is still queued");
        assert!(p.set[0].0 == TextureId::Managed(1099));
    }

    /// The first frame's font atlas must survive skipped frames -- the black-rectangle
    /// bug this queue exists to prevent -- and partial updates stay after it, in order.
    #[test]
    fn the_first_atlas_and_later_patches_survive() {
        let mut p = TexturesDelta::default();
        merge_textures(&mut p, delta(vec![full(0, 128)], vec![]));
        merge_textures(&mut p, delta(vec![part(0)], vec![]));
        merge_textures(&mut p, delta(vec![], vec![]));
        merge_textures(&mut p, delta(vec![part(0)], vec![]));
        assert_eq!(p.set.len(), 3);
        assert!(p.set[0].1.pos.is_none(), "the full atlas first");
        assert!(p.set[1].1.pos.is_some() && p.set[2].1.pos.is_some());
    }

    /// A texture freed and then re-created while stalled must end up uploaded, not freed.
    #[test]
    fn recreating_a_freed_texture_cancels_its_free() {
        let mut p = TexturesDelta::default();
        merge_textures(&mut p, delta(vec![full(5, 8)], vec![]));
        merge_textures(&mut p, delta(vec![], vec![5]));
        merge_textures(&mut p, delta(vec![full(5, 8)], vec![]));
        assert_eq!(p.set.len(), 1);
        assert!(!p.free.contains(&TextureId::Managed(5)), "not freed after being re-created");
    }
}

#[cfg(test)]
mod height_tests {
    use super::{surface_height, HEIGHT};

    /// Whole points, rounded UP (a half-point short clips the bottom row's border), floored
    /// at GTK's 120 and capped; a NaN from an empty first frame keeps the default.
    #[test]
    fn the_surface_height_follows_the_content_within_bounds() {
        assert_eq!(surface_height(216.2), 217);
        assert_eq!(surface_height(216.0), 216);
        assert_eq!(surface_height(40.0), 120);
        assert_eq!(surface_height(9000.0), 1600);
        assert_eq!(surface_height(f32::NAN), HEIGHT);
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn an_idle_keyboard_with_a_polled_sink_sleeps_long_not_twenty_times_a_second() {
        let now = Instant::now();
        let far = now + Duration::from_secs(3600);
        assert_eq!(idle_cap_timeout(far, now, false, None, true), IDLE_POLL_WITH_FD);
        assert_eq!(idle_cap_timeout(far, now, false, None, false), IDLE_POLL, "no fd: the old timer stays");
    }

    #[test]
    fn egui_never_wanting_a_frame_is_not_an_overflow_panic() {
        let now = Instant::now();
        let at = repaint_deadline(now, Duration::MAX);
        assert!(at > now + Duration::from_secs(3600));
        assert_eq!(idle_cap_timeout(at, now, false, None, true), IDLE_POLL_WITH_FD);
        assert_eq!(repaint_deadline(now, Duration::from_millis(7)), now + Duration::from_millis(7));
    }

    #[test]
    fn deadlines_and_frame_callbacks_still_bound_the_sleep() {
        let now = Instant::now();
        let soon = now + Duration::from_millis(120);
        assert_eq!(idle_cap_timeout(soon, now, false, None, true), Duration::from_millis(120));
        // Callback outstanding: wake just after the grace so a lost callback is noticed.
        let t = idle_cap_timeout(now, now, true, Some(now), true);
        assert!(t > FRAME_CALLBACK_GRACE && t < FRAME_CALLBACK_GRACE + Duration::from_millis(50), "{t:?}");
        // ...but never longer than the no-fd cap when the sink cannot be polled.
        assert_eq!(idle_cap_timeout(now, now, true, Some(now), false), IDLE_POLL);
    }
}
