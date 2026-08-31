//! frame_pump — what happens between a PipeWire buffer and a drawn frame.
//!
//! [`crate::camera_select`] picks the node and [`crate::camera`] knows the
//! geometry. Neither owns a frame, which is why the viewfinder still draws a
//! placeholder — and frame ownership is exactly where a preview becomes laggy.
//!
//! Modelled on GNOME Snapshot's arrangement, which is the reference for this on
//! mobile Linux: the portal hands back a **PipeWire fd**, and everything after
//! it is a pull from a stream that keeps producing whether or not anyone is
//! looking.
//!
//! ## Latest-frame-wins, because a camera preview is not a video file
//!
//! A queue is the wrong structure here. If the compositor stalls for 200 ms at
//! 30 fps, a queue hands you six stale frames to work through before you reach
//! the current one — so the preview runs **behind reality and stays behind**,
//! and the lag is permanent rather than transient.
//!
//! A single slot that the producer overwrites means a stall costs you dropped
//! frames and **nothing else**: the next paint shows now, not the backlog. The
//! dropped count is kept because a preview silently discarding 80% of its
//! frames is a diagnostic, not a normal state.
//!
//! ## The producer must never block on the consumer
//!
//! PipeWire's callback runs on its own thread and has to return the buffer
//! promptly. A pump that waited for the UI would stall the *camera*, which on a
//! shared ISP can disturb other clients — and on the FP5 the ISP is shared, as
//! `camera_select`'s capture-group rule already records.
//!
//! ## Upload only what will be drawn
//!
//! A texture upload is the expensive step, so it happens once per **paint**,
//! not once per frame. At 30 fps into a 60 Hz repaint that is neutral; at 30 fps
//! into a background window it is the difference between idle and a warm phone.

/// A frame as it arrives, before any conversion.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    /// Monotonic capture time, for staleness and for measuring end-to-end lag.
    pub captured_ms: u64,
    /// Sequence from the producer, so drops can be counted rather than guessed.
    pub seq: u64,
    /// The pixels, RGBA8, `width * height * 4` bytes. An `Arc` because the
    /// frame travels by value through `HostEvent::CameraFrame` and is cloned
    /// into the slot — at 30 fps a deep copy per hop is the difference
    /// between an idle phone and a warm one. Empty is legal and means a
    /// metadata-only frame (the pre-capture tests use it); the texture
    /// upload checks the length rather than trusting it.
    pub rgba: std::sync::Arc<[u8]>,
}

impl RawFrame {
    /// Do the pixels actually describe the claimed geometry? A torn frame —
    /// a reader that got EOF mid-frame, a stride mismatch — must be refused
    /// at the upload, not panicked on inside egui.
    pub fn pixels_plausible(&self) -> bool {
        self.rgba.len() == (self.width as usize) * (self.height as usize) * 4
    }
}

/// A frame slot: one deep, latest wins.
///
/// Not a queue. See the module docs — a queue converts a transient stall into a
/// permanent delay.
#[derive(Clone, Debug, Default)]
pub struct FrameSlot {
    current: Option<RawFrame>,
    /// Frames overwritten before anyone drew them.
    pub dropped: u64,
    /// Frames actually taken by a paint.
    pub delivered: u64,
    /// Highest sequence seen, for gap detection.
    last_seq: u64,
    /// Gaps the PRODUCER left — distinct from frames we dropped ourselves.
    pub producer_gaps: u64,
}

impl FrameSlot {
    /// Called from the capture thread. Never blocks, never fails.
    pub fn offer(&mut self, f: RawFrame) {
        if self.last_seq != 0 && f.seq > self.last_seq + 1 {
            // The camera or the compositor skipped. Counted separately from our
            // own drops, because the two have different causes and the fix for
            // one does nothing for the other.
            self.producer_gaps += f.seq - self.last_seq - 1;
        }
        if f.seq > self.last_seq { self.last_seq = f.seq; }
        if self.current.is_some() { self.dropped += 1; }
        self.current = Some(f);
    }

    /// Called from the paint. Takes the frame if there is a new one.
    ///
    /// Takes rather than borrows: leaving the frame in place would redraw the
    /// same image next paint and make a stalled camera look like a working one.
    pub fn take(&mut self) -> Option<RawFrame> {
        let f = self.current.take();
        if f.is_some() { self.delivered += 1; }
        f
    }

    /// Is a frame waiting?
    pub fn has_frame(&self) -> bool { self.current.is_some() }

    /// Fraction of offered frames that reached a paint.
    ///
    /// `None` before anything arrives, so an idle camera does not report 0%
    /// and look broken.
    pub fn delivery_rate(&self) -> Option<f32> {
        let total = self.delivered + self.dropped;
        if total == 0 { None } else { Some(self.delivered as f32 / total as f32) }
    }
}

/// How stale a frame may be before the preview should say so.
///
/// Three frames at 30 fps. Below that, staleness is ordinary jitter; above it,
/// the camera has actually stopped and a frozen image is a lie about what the
/// lens is seeing.
pub const STALE_AFTER_MS: u64 = 100;

/// What the viewfinder should show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Display {
    /// Draw the frame.
    Live,
    /// Keep drawing the last frame, but mark it — the stream has paused.
    Stale,
    /// Nothing has arrived yet.
    Waiting,
}

pub fn display_state(last_drawn_ms: Option<u64>, now_ms: u64) -> Display {
    match last_drawn_ms {
        None => Display::Waiting,
        Some(t) if now_ms.saturating_sub(t) > STALE_AFTER_MS => Display::Stale,
        Some(_) => Display::Live,
    }
}

/// Whether a texture upload should happen this paint.
///
/// The expensive step, so it is gated on there being something new. Uploading
/// an unchanged frame costs a full texture write to draw pixels already on
/// screen.
pub fn should_upload(slot: &FrameSlot, have_texture: bool) -> bool {
    slot.has_frame() || !have_texture
}

/// Should the stream be running at all?
///
/// A camera left streaming behind a closed page is the single most expensive
/// thing this app can do on a phone — it holds the ISP, the sensor and the
/// wakeups. Snapshot stops the stream when its page is not visible, and the
/// same rule applies here more sharply.
pub fn should_stream(page_visible: bool, window_focused: bool, screen_on: bool) -> bool {
    // Focus alone is not enough: a visible-but-unfocused preview is still a
    // preview somebody is looking at, e.g. beside a keyboard.
    page_visible && screen_on && (window_focused || page_visible)
}

// ------------------------------------------------------- conversion path ---

/// How a frame's pixels reach the screen.
///
/// **This is the decision that actually determines whether a preview lags**,
/// and GNOME Snapshot picks it as a three-tier ladder at pipeline build time —
/// not per frame, because the answer cannot change once the sink is built.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum ConvertPath {
    /// The sink takes camera-native YUV directly. No conversion at all.
    ///
    /// Snapshot tests this first by intersecting the sink's caps with YUY2
    /// caps: if the sink accepts YUV, the frame goes straight through and the
    /// most expensive step in the pipeline does not exist.
    NativeYuv,
    /// A GPU shader converts YUV to RGB. Costs a pass, not a copy.
    ///
    /// Chosen only when a GL context is actually present — Snapshot checks
    /// `gl-context` is `Some` rather than assuming, because on a phone it may
    /// not be.
    GlConvert,
    /// `videoconvert` on the CPU. Correct everywhere, and the slow path.
    ///
    /// A 1280x720 YUY2 to RGBA conversion is ~3.7 MB written per frame, 110
    /// MB/s at 30 fps, on cores the app also needs. This is the tier where a
    /// preview starts dropping frames.
    CpuConvert,
}

impl ConvertPath {
    /// Pick the cheapest path the hardware actually supports.
    ///
    /// Ordered, and the order is the whole function: never guess GL when no
    /// context exists, because the fallback that catches that guess is the CPU
    /// path — and by then the pipeline is built.
    pub fn choose(sink_takes_yuv: bool, gl_context: bool) -> ConvertPath {
        if sink_takes_yuv { ConvertPath::NativeYuv }
        else if gl_context { ConvertPath::GlConvert }
        else { ConvertPath::CpuConvert }
    }

    /// Does this path cost CPU per frame?
    pub fn is_cpu_bound(self) -> bool { self == ConvertPath::CpuConvert }

    /// Rough bytes the CPU must write per frame at this size.
    ///
    /// Zero for the two GPU paths — the point of the ladder is that this number
    /// is what makes a preview lag.
    pub fn cpu_bytes_per_frame(self, w: u32, h: u32) -> u64 {
        match self {
            ConvertPath::CpuConvert => w as u64 * h as u64 * 4,
            _ => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ConvertPath::NativeYuv => "YUV direct (no conversion)",
            ConvertPath::GlConvert => "GPU conversion",
            ConvertPath::CpuConvert => "CPU conversion (slow path)",
        }
    }
}

/// Should the preview drop to a lower resolution?
///
/// Only on the CPU path, and only above a size where the per-frame write starts
/// competing with the app. On the GPU paths resolution is nearly free, so
/// dropping it there would cost image quality for nothing.
pub fn should_downscale(path: ConvertPath, w: u32, h: u32) -> bool {
    path.is_cpu_bound() && path.cpu_bytes_per_frame(w, h) > 2_000_000
}

/// End-to-end latency of a frame that was just drawn.
///
/// Measured from capture rather than from arrival, because arrival already
/// excludes the part of the delay we would most want to find.
pub fn latency_ms(f: &RawFrame, now_ms: u64) -> u64 {
    now_ms.saturating_sub(f.captured_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(seq: u64, at: u64) -> RawFrame {
        RawFrame { width: 1280, height: 720, captured_ms: at, seq,
                   rgba: std::sync::Arc::from(&[][..]) }
    }

    #[test]
    fn a_torn_frame_is_refused_at_the_upload_rather_than_panicking_egui() {
        // A reader that hit EOF mid-frame hands over fewer bytes than the
        // geometry claims. ColorImage::from_rgba_unmultiplied asserts on
        // that, so the check has to happen before egui sees the buffer.
        let good = RawFrame { width: 2, height: 2, captured_ms: 0, seq: 1,
                              rgba: std::sync::Arc::from(&[0u8; 16][..]) };
        assert!(good.pixels_plausible());
        let torn = RawFrame { rgba: std::sync::Arc::from(&[0u8; 15][..]), ..good.clone() };
        assert!(!torn.pixels_plausible());
        // Metadata-only frames (the pre-capture tests) are NOT plausible
        // pixels, and must not be: uploading an empty texture is how the
        // viewfinder would go black instead of saying "no camera".
        assert!(!frame(1, 0).pixels_plausible());
    }

    #[test]
    fn a_stall_costs_dropped_frames_and_not_a_permanent_delay() {
        // The whole reason this is a slot and not a queue. A queue would hand
        // the paint six stale frames to work through, so the preview would run
        // BEHIND REALITY AND STAY BEHIND.
        let mut s = FrameSlot::default();
        for i in 1..=6 { s.offer(frame(i, i * 33)); }
        let drawn = s.take().expect("a frame");
        assert_eq!(drawn.seq, 6, "drew a stale frame from a backlog");
        assert_eq!(s.dropped, 5);
        assert!(s.take().is_none(), "a backlog survived the take");
    }

    #[test]
    fn the_producer_never_blocks_on_the_consumer() {
        // PipeWire's callback runs on its own thread and must return the buffer
        // promptly. A pump that waited for the UI would stall the CAMERA, which
        // on the FP5's shared ISP can disturb other clients.
        let mut s = FrameSlot::default();
        for i in 1..=1_000 { s.offer(frame(i, i)); }
        assert!(s.has_frame());
        assert_eq!(s.take().unwrap().seq, 1_000);
    }

    #[test]
    fn taking_a_frame_empties_the_slot() {
        // Leaving it would redraw the same image next paint and make a stalled
        // camera look like a working one.
        let mut s = FrameSlot::default();
        s.offer(frame(1, 0));
        assert!(s.take().is_some());
        assert!(!s.has_frame());
        assert!(s.take().is_none());
    }

    #[test]
    fn our_own_drops_are_counted_separately_from_producer_gaps() {
        // Different causes, and the fix for one does nothing for the other: our
        // drops mean the UI is slow, a producer gap means the camera or
        // compositor skipped.
        let mut s = FrameSlot::default();
        s.offer(frame(1, 0));
        s.offer(frame(5, 132));   // producer skipped 2,3,4
        assert_eq!(s.producer_gaps, 3);
        assert_eq!(s.dropped, 1, "the overwrite is ours");
    }

    #[test]
    fn an_out_of_order_frame_does_not_invent_a_gap() {
        // Reordering happens; counting it as a gap would make the diagnostic
        // report a camera fault that is not there.
        let mut s = FrameSlot::default();
        s.offer(frame(10, 0));
        s.take();
        s.offer(frame(9, 33));
        assert_eq!(s.producer_gaps, 0);
    }

    #[test]
    fn delivery_rate_reports_nothing_before_the_first_frame() {
        // Zero would read as "dropping everything" and look like a fault on a
        // camera that simply has not started.
        let s = FrameSlot::default();
        assert_eq!(s.delivery_rate(), None);
    }

    #[test]
    fn a_healthy_preview_delivers_most_of_what_it_is_offered() {
        // A preview silently discarding 80% of its frames is a diagnostic, not
        // a normal state.
        let mut s = FrameSlot::default();
        for i in 1..=10 { s.offer(frame(i, i * 33)); s.take(); }
        assert_eq!(s.delivery_rate(), Some(1.0));
        assert_eq!(s.dropped, 0);
    }

    #[test]
    fn a_frozen_stream_is_marked_rather_than_silently_frozen() {
        // A frozen image is a LIE about what the lens is seeing, and the user
        // is about to take a photo based on it.
        assert_eq!(display_state(None, 0), Display::Waiting);
        assert_eq!(display_state(Some(0), STALE_AFTER_MS), Display::Live);
        assert_eq!(display_state(Some(0), STALE_AFTER_MS + 1), Display::Stale);
    }

    #[test]
    fn ordinary_jitter_is_not_reported_as_stale() {
        // Three frames at 30 fps. Flagging one dropped frame would leave the
        // marker flickering constantly and train the user to ignore it.
        assert_eq!(display_state(Some(0), 33), Display::Live);
        assert_eq!(display_state(Some(0), 66), Display::Live);
    }

    #[test]
    fn an_unchanged_frame_is_not_re_uploaded() {
        // The texture upload is the expensive step; writing it again to draw
        // pixels already on screen is pure cost.
        let s = FrameSlot::default();
        assert!(!should_upload(&s, true), "re-uploaded an unchanged frame");
        assert!(should_upload(&s, false), "the first paint needs a texture");
    }

    #[test]
    fn a_new_frame_always_uploads() {
        let mut s = FrameSlot::default();
        s.offer(frame(1, 0));
        assert!(should_upload(&s, true));
    }

    #[test]
    fn the_stream_stops_when_the_page_is_not_visible() {
        // A camera left streaming behind a closed page is the most expensive
        // thing this app can do on a phone: it holds the ISP, the sensor and
        // the wakeups.
        assert!(!should_stream(false, true, true));
        assert!(!should_stream(true, true, false), "streamed to a dark screen");
        assert!(should_stream(true, true, true));
    }

    #[test]
    fn a_visible_but_unfocused_preview_keeps_running() {
        // Focus alone is not enough — a preview beside a keyboard is still a
        // preview somebody is looking at, and stopping it makes the viewfinder
        // freeze whenever they type.
        assert!(should_stream(true, false, true));
    }

    #[test]
    fn a_sink_that_takes_yuv_skips_conversion_entirely() {
        // Snapshot tests this FIRST by intersecting the sink's caps with YUY2
        // caps. When it holds, the most expensive step in the pipeline does not
        // exist at all.
        assert_eq!(ConvertPath::choose(true, true), ConvertPath::NativeYuv);
        assert_eq!(ConvertPath::choose(true, false), ConvertPath::NativeYuv,
                   "native YUV does not need GL");
        assert!(!ConvertPath::NativeYuv.is_cpu_bound());
    }

    #[test]
    fn gl_is_only_chosen_when_a_context_actually_exists() {
        // Snapshot checks `gl-context` is Some rather than assuming, because on
        // a phone it may not be. Guessing wrong is caught by the CPU path — but
        // by then the pipeline is built and the preview is already slow.
        assert_eq!(ConvertPath::choose(false, true), ConvertPath::GlConvert);
        assert_eq!(ConvertPath::choose(false, false), ConvertPath::CpuConvert);
    }

    #[test]
    fn only_the_cpu_path_costs_bytes_per_frame() {
        // The point of the ladder: this number is what makes a preview lag.
        assert_eq!(ConvertPath::NativeYuv.cpu_bytes_per_frame(1280, 720), 0);
        assert_eq!(ConvertPath::GlConvert.cpu_bytes_per_frame(1280, 720), 0);
        assert_eq!(ConvertPath::CpuConvert.cpu_bytes_per_frame(1280, 720), 3_686_400);
    }

    #[test]
    fn the_cpu_path_at_720p_is_over_a_hundred_megabytes_a_second() {
        // 3.7 MB per frame at 30 fps, written on cores the app also needs. This
        // is the tier where a preview starts dropping frames, and it is why the
        // ladder exists rather than just using videoconvert everywhere.
        let per_frame = ConvertPath::CpuConvert.cpu_bytes_per_frame(1280, 720);
        assert!(per_frame * 30 > 100_000_000);
    }

    #[test]
    fn downscaling_is_only_offered_on_the_slow_path() {
        // On the GPU paths resolution is nearly free, so dropping it there
        // costs image quality for nothing.
        assert!(should_downscale(ConvertPath::CpuConvert, 1280, 720));
        assert!(!should_downscale(ConvertPath::GlConvert, 1920, 1080));
        assert!(!should_downscale(ConvertPath::NativeYuv, 3840, 2160));
    }

    #[test]
    fn a_small_cpu_frame_does_not_need_downscaling() {
        // Below the threshold the write is not competing with the app, and
        // downscaling would make a working preview blurry for no gain.
        assert!(!should_downscale(ConvertPath::CpuConvert, 320, 240));
    }

    #[test]
    fn the_paths_are_ordered_cheapest_first() {
        // So a diagnostic can say "we are two tiers below the best available"
        // rather than just naming the current one.
        assert!(ConvertPath::NativeYuv < ConvertPath::GlConvert);
        assert!(ConvertPath::GlConvert < ConvertPath::CpuConvert);
    }

    #[test]
    fn every_path_names_itself_for_a_diagnostic() {
        // "The preview is slow" is not actionable; "CPU conversion (slow path)"
        // tells somebody which of three problems they have.
        for p in [ConvertPath::NativeYuv, ConvertPath::GlConvert, ConvertPath::CpuConvert] {
            assert!(!p.label().is_empty(), "{p:?}");
        }
        assert!(ConvertPath::CpuConvert.label().contains("slow"));
    }

    #[test]
    fn latency_is_measured_from_capture_not_arrival() {
        // Measuring from arrival excludes exactly the part of the delay we most
        // want to find.
        let f = frame(1, 1_000);
        assert_eq!(latency_ms(&f, 1_050), 50);
        assert_eq!(latency_ms(&f, 900), 0, "a backwards clock must not underflow");
    }
}
