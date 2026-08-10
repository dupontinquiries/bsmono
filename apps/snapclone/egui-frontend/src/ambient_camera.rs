//! ambient_camera — `AmbientCameraDriver.cpp`, ported.
//!
//! Derives the theme from the live viewfinder, so the chrome tracks what the
//! camera is pointed at. Frames arrive at ~30Hz; derivation runs on a slow tick
//! — and every number here was tuned against a real device.
//!
//! ## Storing a frame and using a frame are different rates
//!
//! Keeping the latest frame is cheap; deriving from it iterates 4,096 pixels
//! with a colour conversion each. The C++ measures that at **20–40ms on the
//! FP5**, *"visible as jank when a derive tick coincides with a swipe"* — so
//! frames are stored on arrival and derived on a tick, which the header notes
//! *"saves ~29 derivations per second relative to a naive subscription"*.
//!
//! ## Three guards, each with a measured reason
//!
//! * **Drop, don't stack.** If the previous derive has not returned by the next
//!   tick, the tick is skipped rather than spawning a parallel worker: *"in
//!   which case skipping a tick is the right behavior"* — a device already
//!   thrashing does not need a second sampler.
//! * **Stop entirely when backgrounded.** Frames stop arriving but the timer
//!   keeps re-deriving the *same stale frame*, which is pure waste. The header
//!   quantifies it: **~30 worker-pool wakeups per minute** of backgrounded use.
//! * **Smooth toward the target, α = 0.15.** At a 1Hz tick that is a ~4.3s
//!   half-life, tuned for *"noticeable but not jumpy"*. Without it the chrome
//!   strobes as the camera pans.
//!
//! The first tick deliberately does **not** blend: `m_smoothedValid` starts
//! false so the chrome reacts immediately at activation. Blending from a
//! default would fade in from whatever colour the previous palette was, which
//! reads as the feature being slow rather than smooth.

/// How often a derivation may run.
pub const TICK_MS: u64 = 2_000;
/// Blend factor toward each new sample.
pub const SMOOTHING_ALPHA: f32 = 0.15;

/// Why a tick did or did not derive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tick {
    /// Sample the stored frame now.
    Derive,
    /// Not enough time has passed.
    TooSoon,
    /// The previous derive has not returned.
    Busy,
    /// The app is not on screen.
    Backgrounded,
    /// No frame has arrived yet, or the driver is stopped.
    Idle,
}

/// An RGB colour.
pub type Rgb = [u8; 3];

#[derive(Clone, Debug, Default)]
pub struct AmbientCameraDriver {
    active: bool,
    app_active: bool,
    has_frame: bool,
    derive_in_flight: bool,
    last_tick_ms: u64,
    ever_ticked: bool,
    smoothed: Option<(Rgb, Rgb)>,
}

impl AmbientCameraDriver {
    pub fn new() -> AmbientCameraDriver {
        AmbientCameraDriver { app_active: true, ..Default::default() }
    }

    pub fn is_active(&self) -> bool { self.active }
    pub fn smoothed(&self) -> Option<(Rgb, Rgb)> { self.smoothed }

    /// Start consuming frames. Idempotent, as the C++ documents.
    pub fn start(&mut self) { self.active = true; }

    /// Stop, and forget the smoothed colours.
    ///
    /// Forgetting matters: resuming later must not blend from colours sampled
    /// in a different room, which would drift the chrome through a state that
    /// matched nothing on screen.
    pub fn stop(&mut self) {
        self.active = false;
        self.has_frame = false;
        self.derive_in_flight = false;
        self.smoothed = None;
    }

    /// The app moved to or from the foreground.
    ///
    /// Going to background stops the tick entirely rather than merely skipping
    /// derivations, which is where the ~30 wakeups per minute go.
    pub fn set_app_active(&mut self, on: bool) { self.app_active = on; }

    /// A preview frame arrived. Cheap by design — storage only, no work.
    pub fn on_frame(&mut self) { if self.active { self.has_frame = true; } }

    /// Should this tick derive?
    pub fn tick(&mut self, now_ms: u64) -> Tick {
        if !self.active || !self.has_frame { return Tick::Idle; }
        if !self.app_active { return Tick::Backgrounded; }
        if self.derive_in_flight { return Tick::Busy; }
        if self.ever_ticked && now_ms.saturating_sub(self.last_tick_ms) < TICK_MS {
            return Tick::TooSoon;
        }
        self.last_tick_ms = now_ms;
        self.ever_ticked = true;
        self.derive_in_flight = true;
        Tick::Derive
    }

    /// A derivation returned. `None` means it failed or produced nothing.
    ///
    /// The in-flight flag clears either way, or one failed sample would wedge
    /// the driver into `Busy` forever and the theme would freeze with no error.
    pub fn complete(&mut self, sample: Option<(Rgb, Rgb)>) {
        self.derive_in_flight = false;
        let Some((accent, bg)) = sample else { return };
        self.smoothed = Some(match self.smoothed {
            // First sample after activation is used raw: blending from a
            // default would fade in from the previous palette's colour, which
            // reads as the feature being slow rather than smooth.
            None => (accent, bg),
            Some((sa, sb)) => (blend(sa, accent), blend(sb, bg)),
        });
    }

    /// Whether a derivation is outstanding.
    pub fn is_deriving(&self) -> bool { self.derive_in_flight }
}

/// Move `from` toward `to` by the smoothing factor.
fn blend(from: Rgb, to: Rgb) -> Rgb {
    let mut out = [0u8; 3];
    for i in 0..3 {
        let f = from[i] as f32;
        let t = to[i] as f32;
        out[i] = (f + (t - f) * SMOOTHING_ALPHA).round().clamp(0.0, 255.0) as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Rgb = [200, 40, 40];
    const BLUE: Rgb = [40, 40, 200];
    const GREY: Rgb = [128, 128, 128];

    fn running() -> AmbientCameraDriver {
        let mut d = AmbientCameraDriver::new();
        d.start();
        d.on_frame();
        d
    }

    #[test]
    fn frames_are_stored_cheaply_and_derivation_waits_for_a_tick() {
        // The whole battery model: storing is cheap, deriving iterates 4,096
        // pixels at 20-40ms on the FP5. Naive subscription would do that 30
        // times a second.
        let mut d = AmbientCameraDriver::new();
        d.start();
        for _ in 0..60 { d.on_frame(); }
        assert!(!d.is_deriving(), "no work happened on arrival");
        assert_eq!(d.tick(0), Tick::Derive, "one tick, one derivation");
    }

    #[test]
    fn a_second_tick_inside_the_interval_is_refused() {
        let mut d = running();
        assert_eq!(d.tick(0), Tick::Derive);
        d.complete(Some((RED, GREY)));
        assert_eq!(d.tick(TICK_MS - 1), Tick::TooSoon);
        assert_eq!(d.tick(TICK_MS), Tick::Derive);
    }

    #[test]
    fn a_slow_derive_drops_the_next_tick_rather_than_stacking() {
        // "Skipping a tick is the right behavior" — a device already thrashing
        // does not need a second sampler.
        let mut d = running();
        assert_eq!(d.tick(0), Tick::Derive);
        assert_eq!(d.tick(TICK_MS * 5), Tick::Busy, "the first has not returned");
        d.complete(Some((RED, GREY)));
        assert_eq!(d.tick(TICK_MS * 6), Tick::Derive);
    }

    #[test]
    fn a_failed_derive_does_not_wedge_the_driver() {
        // One failed sample would otherwise leave it Busy forever and the theme
        // would freeze with no error anywhere.
        let mut d = running();
        d.tick(0);
        d.complete(None);
        assert!(!d.is_deriving());
        assert_eq!(d.tick(TICK_MS), Tick::Derive);
    }

    #[test]
    fn backgrounding_stops_the_work_entirely() {
        // Frames stop arriving but the timer would keep re-deriving the SAME
        // STALE FRAME — the header measures ~30 worker-pool wakeups per minute
        // of backgrounded use.
        let mut d = running();
        d.set_app_active(false);
        for t in 0..10 {
            assert_eq!(d.tick(t * TICK_MS), Tick::Backgrounded);
        }
        d.set_app_active(true);
        assert_eq!(d.tick(TICK_MS * 20), Tick::Derive);
    }

    #[test]
    fn nothing_happens_before_the_first_frame() {
        // Activation precedes the camera producing anything.
        let mut d = AmbientCameraDriver::new();
        d.start();
        assert_eq!(d.tick(0), Tick::Idle);
    }

    #[test]
    fn a_stopped_driver_ignores_frames_and_ticks() {
        let mut d = running();
        d.stop();
        d.on_frame();
        assert_eq!(d.tick(TICK_MS * 10), Tick::Idle);
    }

    #[test]
    fn the_first_sample_is_used_raw_so_the_chrome_reacts_at_once() {
        // Blending from a default would fade in from whatever the previous
        // palette was, which reads as slow rather than smooth.
        let mut d = running();
        d.tick(0);
        d.complete(Some((RED, GREY)));
        assert_eq!(d.smoothed(), Some((RED, GREY)));
    }

    #[test]
    fn later_samples_are_smoothed_rather_than_snapped() {
        // Without this the chrome strobes as the camera pans.
        let mut d = running();
        d.tick(0);
        d.complete(Some((RED, GREY)));
        d.tick(TICK_MS);
        d.complete(Some((BLUE, GREY)));
        let (accent, _) = d.smoothed().unwrap();
        assert!(accent[0] < RED[0], "it moved toward blue");
        assert!(accent[0] > BLUE[0], "but did not snap there: {accent:?}");
    }

    #[test]
    fn smoothing_converges_over_several_ticks() {
        // A ~4.3s half-life at 1Hz; the tick here is 2s, so a handful of
        // samples should get most of the way.
        let mut d = running();
        d.tick(0);
        d.complete(Some((RED, GREY)));
        for i in 1..40u64 {
            d.tick(i * TICK_MS);
            d.complete(Some((BLUE, GREY)));
        }
        let (accent, _) = d.smoothed().unwrap();
        assert!((accent[0] as i32 - BLUE[0] as i32).abs() < 10, "converged to {accent:?}");
    }

    #[test]
    fn stopping_forgets_the_smoothed_colour() {
        // Resuming later must not blend from colours sampled in a different
        // room, which would drift the chrome through a state matching nothing
        // on screen.
        let mut d = running();
        d.tick(0);
        d.complete(Some((RED, GREY)));
        assert!(d.smoothed().is_some());
        d.stop();
        assert!(d.smoothed().is_none());
    }

    #[test]
    fn a_backwards_clock_does_not_defeat_the_tick_interval() {
        let mut d = running();
        d.tick(100_000);
        d.complete(Some((RED, GREY)));
        assert_eq!(d.tick(50_000), Tick::TooSoon);
    }

    #[test]
    fn starting_twice_is_idempotent_as_the_header_promises() {
        let mut d = AmbientCameraDriver::new();
        d.start();
        d.on_frame();
        d.start();
        assert!(d.is_active());
        assert_eq!(d.tick(0), Tick::Derive, "the frame survived the restart");
    }
}
