//! posture — `PostureEngine`, `DimOverlay`, `PostureSource`.
//!
//! The fleet's only sensor integration, and it was unported. snapclone reads
//! **proximity** and **motion** — proximity from `iio-sensor-proxy` over the
//! system bus, motion from raw IIO accelerometer counts in sysfs — and uses them
//! for two things: a screensaver that dims when the phone is at your ear, and a
//! flip from push-to-talk to gated talk mode when it gets there.
//!
//! ## The hardware stays behind a source, and that is what makes it shippable
//!
//! The Qt header is blunt about it: **unverified on hardware**. Whether the FP5
//! exposes proximity through iio-sensor-proxy, the exact IIO device path, and
//! the motion threshold all need on-device validation. What makes it safe to
//! ship anyway is the degradation contract, which is ported exactly: **no
//! sensor means far and still**, which is to say the app behaves completely
//! normally. We never blank a screen we cannot justify.
//!
//! That inverts the usual default and it is the right way round. A screensaver
//! that dims when it is *unsure* would black out a phone with no proximity
//! sensor at all.
//!
//! ## Armed, and why the gate is separate from the sensors
//!
//! The engine only acts while ARMED, and arming is the app's decision — the
//! inbox, or a conversation with voice in the last five minutes. Sensors say
//! what the world is doing; arming says whether it matters here. Fusing them
//! would mean a phone in your pocket dimming a page you were about to read.
//!
//! ## Motion resets stillness, it does not wake
//!
//! In `Dim`, movement restarts the stillness timer but the dim HOLDS. Waking on
//! any motion would undim every time the phone shifted in your hand against
//! your ear, which is constantly. Only proximity going far, or a snap arriving,
//! wakes.

/// What the sensors report. `available()` false means the world is far and
/// still — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Posture {
    pub proximity_near: bool,
    pub moving: bool,
}

/// A source of posture readings. The real one talks D-Bus and sysfs; a host
/// without sensors reports unavailable.
pub trait PostureSource {
    fn available(&self) -> bool;
    fn read(&self) -> Posture;
}

/// The stand-in used when there is no sensor, and in tests.
///
/// Reports far + still, which is the "behave normally" state.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSensors;

impl PostureSource for NoSensors {
    fn available(&self) -> bool { false }
    fn read(&self) -> Posture { Posture::default() }
}

/// Timings and depths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Config {
    /// First dim, overlay opacity.
    pub dim_level_1: f32,
    /// Screensaver dim.
    pub dim_level_2: f32,
    /// Normal → Dim fade.
    pub fade_to_dim1_ms: f32,
    /// Still this long in Dim → Screensaver.
    pub stillness_ms: f32,
    /// Dim → Screensaver fade. Fast, because by then nobody is looking.
    pub fade_to_dim2_ms: f32,
    /// Any → Normal. Quick, because someone just asked for the screen.
    pub wake_ms: f32,
    /// Master switch, from settings.
    pub enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            dim_level_1: 0.50,
            dim_level_2: 0.75,
            fade_to_dim1_ms: 30_000.0,
            stillness_ms: 60_000.0,
            fade_to_dim2_ms: 3_000.0,
            wake_ms: 1_000.0,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State { Normal, Dim, Screensaver }

/// What the UI should do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    /// Overlay opacity, 0 clear to 1 black.
    pub dim: f32,
    /// True while the render rate may be capped.
    pub screensaver: bool,
    /// True when talk mode should be GATED rather than push-to-talk.
    pub gated_talk: bool,
}

/// The engine.
#[derive(Debug, Clone)]
pub struct PostureEngine {
    pub cfg: Config,
    state: State,
    armed: bool,
    in_call: bool,
    near: bool,
    moving: bool,
    /// Current overlay opacity, eased toward the state's target.
    dim: f32,
    /// How long we have been still in Dim.
    still_ms: f32,
}

impl Default for PostureEngine {
    fn default() -> Self {
        PostureEngine {
            cfg: Config::default(), state: State::Normal, armed: false, in_call: false,
            near: false, moving: false, dim: 0.0, still_ms: 0.0,
        }
    }
}

impl PostureEngine {
    pub fn state(&self) -> State { self.state }
    pub fn armed(&self) -> bool { self.armed }
    pub fn dim(&self) -> f32 { self.dim }

    /// The gate. Disarming fully resets — a page that stops being voice-active
    /// must not keep a dim it inherited.
    pub fn set_armed(&mut self, armed: bool) {
        if self.armed == armed { return; }
        self.armed = armed;
        if !armed { self.reset(); }
    }

    pub fn set_in_call(&mut self, in_call: bool) { self.in_call = in_call; }

    /// Feed a source. Unavailable hardware reads as far + still, so the app
    /// behaves exactly as it would with no sensors at all.
    pub fn observe(&mut self, source: &dyn PostureSource) {
        let p = if source.available() { source.read() } else { Posture::default() };
        self.set_proximity_near(p.proximity_near);
        self.set_moving(p.moving);
    }

    pub fn set_proximity_near(&mut self, near: bool) {
        if self.near == near { return; }
        self.near = near;
        if !near {
            // Going far always wakes: someone took the phone from their ear.
            self.wake();
        }
    }

    pub fn set_moving(&mut self, moving: bool) {
        self.moving = moving;
        // Motion RESETS stillness rather than waking. Waking on any motion
        // would undim every time the phone shifted against your ear, which is
        // constantly.
        if moving { self.still_ms = 0.0; }
    }

    /// A snap arrived for the current context. Wakes, then the cycle restarts —
    /// a snap arriving while still covered lights up and re-dims.
    pub fn notify_snap_arrived(&mut self) { self.wake(); }

    fn wake(&mut self) {
        self.state = State::Normal;
        self.still_ms = 0.0;
    }

    fn reset(&mut self) {
        self.wake();
        self.dim = 0.0;
        self.in_call = false;
    }

    /// The dim this state is heading toward.
    fn target_dim(&self) -> f32 {
        match self.state {
            State::Normal => 0.0,
            State::Dim => self.cfg.dim_level_1,
            State::Screensaver => self.cfg.dim_level_2,
        }
    }

    /// How long the current fade takes.
    fn fade_ms(&self) -> f32 {
        match self.state {
            // Waking is quick: someone just asked for the screen.
            State::Normal => self.cfg.wake_ms,
            State::Dim => self.cfg.fade_to_dim1_ms,
            State::Screensaver => self.cfg.fade_to_dim2_ms,
        }
    }

    pub fn tick(&mut self, dt_ms: f32) -> Output {
        // Disabled or unarmed means fully normal, and the overlay clears rather
        // than freezing at whatever depth it had reached.
        if !self.cfg.enabled || !self.armed {
            self.state = State::Normal;
            self.dim = 0.0;
            return self.output();
        }

        match self.state {
            State::Normal => {
                if self.near { self.state = State::Dim; self.still_ms = 0.0; }
            }
            State::Dim => {
                if !self.moving {
                    self.still_ms += dt_ms;
                    if self.still_ms >= self.cfg.stillness_ms {
                        self.state = State::Screensaver;
                    }
                }
            }
            State::Screensaver => {}
        }

        let target = self.target_dim();
        let step = dt_ms / self.fade_ms().max(1.0);
        if self.dim < target { self.dim = (self.dim + step).min(target); }
        else if self.dim > target { self.dim = (self.dim - step).max(target); }
        self.output()
    }

    fn output(&self) -> Output {
        Output {
            dim: self.dim,
            screensaver: self.state == State::Screensaver,
            // Decoupled from the dim states on purpose: putting the phone to
            // your ear switches PTT→gated immediately, without waiting for a
            // 30-second fade.
            gated_talk: self.armed && self.near && self.in_call,
        }
    }
}

/// Draw the dim overlay. Nothing when clear, so it costs nothing in the common
/// case and never eats a tap it did not need to.
pub fn dim_overlay_ui(ui: &mut egui::Ui, out: Output, screen: egui::Rect) {
    if out.dim <= 0.001 { return; }
    let a = (out.dim.clamp(0.0, 1.0) * 255.0) as u8;
    ui.painter().rect_filled(screen, 0.0,
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, a));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed_engine() -> PostureEngine {
        let mut e = PostureEngine::default();
        e.set_armed(true);
        e
    }

    struct Sensors { near: bool, moving: bool }
    impl PostureSource for Sensors {
        fn available(&self) -> bool { true }
        fn read(&self) -> Posture {
            Posture { proximity_near: self.near, moving: self.moving }
        }
    }

    #[test]
    fn no_sensors_means_far_and_still_so_the_app_behaves_normally() {
        // THE degradation contract. A screensaver that dimmed when unsure would
        // black out a phone with no proximity sensor at all.
        let mut e = armed_engine();
        e.observe(&NoSensors);
        let out = e.tick(60_000.0);
        assert_eq!(e.state(), State::Normal);
        assert_eq!(out.dim, 0.0);
        assert!(!out.screensaver);
    }

    #[test]
    fn a_broken_sensor_is_treated_as_no_sensor() {
        // available() false means its readings are not consulted, even if it
        // happens to be reporting something.
        struct Lying;
        impl PostureSource for Lying {
            fn available(&self) -> bool { false }
            fn read(&self) -> Posture { Posture { proximity_near: true, moving: false } }
        }
        let mut e = armed_engine();
        e.observe(&Lying);
        e.tick(60_000.0);
        assert_eq!(e.state(), State::Normal);
    }

    #[test]
    fn nothing_happens_at_all_while_unarmed() {
        // Sensors say what the world is doing; arming says whether it matters
        // here. A phone in your pocket must not dim a page you were about to
        // read.
        let mut e = PostureEngine::default();
        e.set_proximity_near(true);
        let out = e.tick(120_000.0);
        assert_eq!(e.state(), State::Normal);
        assert_eq!(out.dim, 0.0);
    }

    #[test]
    fn proximity_near_moves_to_dim_and_the_overlay_fades_in() {
        let mut e = armed_engine();
        e.set_proximity_near(true);
        let out = e.tick(1_000.0);
        assert_eq!(e.state(), State::Dim);
        assert!(out.dim > 0.0 && out.dim < e.cfg.dim_level_1, "part way, got {}", out.dim);
        for _ in 0..40 { e.tick(1_000.0); }
        assert!((e.dim() - e.cfg.dim_level_1).abs() < 0.01);
    }

    #[test]
    fn stillness_in_dim_deepens_to_the_screensaver() {
        let mut e = armed_engine();
        e.set_proximity_near(true);
        e.tick(1_000.0);
        e.tick(e.cfg.stillness_ms);
        assert_eq!(e.state(), State::Screensaver);
        for _ in 0..10 { e.tick(1_000.0); }
        assert!((e.dim() - e.cfg.dim_level_2).abs() < 0.01);
    }

    #[test]
    fn motion_resets_stillness_but_does_not_wake() {
        // Waking on any motion would undim every time the phone shifted against
        // your ear, which is constantly.
        let mut e = armed_engine();
        e.set_proximity_near(true);
        e.tick(1_000.0);
        e.tick(e.cfg.stillness_ms * 0.9);
        e.set_moving(true);
        e.tick(100.0);
        e.set_moving(false);
        e.tick(e.cfg.stillness_ms * 0.5);
        assert_eq!(e.state(), State::Dim, "the timer restarted");
        assert!(e.dim() > 0.0, "and the dim held rather than clearing");
    }

    #[test]
    fn proximity_going_far_wakes() {
        let mut e = armed_engine();
        e.set_proximity_near(true);
        e.tick(1_000.0);
        e.tick(e.cfg.stillness_ms);
        assert_eq!(e.state(), State::Screensaver);
        e.set_proximity_near(false);
        assert_eq!(e.state(), State::Normal);
        for _ in 0..20 { e.tick(200.0); }
        assert_eq!(e.dim(), 0.0);
    }

    #[test]
    fn a_snap_arriving_while_covered_lights_up_and_re_dims() {
        let mut e = armed_engine();
        e.set_proximity_near(true);
        e.tick(1_000.0);
        e.tick(e.cfg.stillness_ms);
        assert_eq!(e.state(), State::Screensaver);
        e.notify_snap_arrived();
        assert_eq!(e.state(), State::Normal);
        // Still covered, so the cycle starts again.
        e.tick(100.0);
        assert_eq!(e.state(), State::Dim);
    }

    #[test]
    fn waking_is_much_faster_than_dimming() {
        // Someone just asked for the screen; a 30-second fade back would be
        // absurd.
        let cfg = Config::default();
        assert!(cfg.wake_ms < cfg.fade_to_dim1_ms / 10.0);
        assert!(cfg.fade_to_dim2_ms < cfg.fade_to_dim1_ms,
                "deepening is fast too — by then nobody is looking");
    }

    #[test]
    fn disarming_resets_rather_than_freezing_the_overlay() {
        // A page that stops being voice-active must not keep a dim it
        // inherited.
        let mut e = armed_engine();
        e.set_proximity_near(true);
        for _ in 0..10 { e.tick(1_000.0); }
        assert!(e.dim() > 0.0);
        e.set_armed(false);
        let out = e.tick(16.0);
        assert_eq!(out.dim, 0.0);
        assert_eq!(e.state(), State::Normal);
    }

    #[test]
    fn the_master_switch_clears_the_overlay_rather_than_freezing_it() {
        let mut e = armed_engine();
        e.set_proximity_near(true);
        for _ in 0..10 { e.tick(1_000.0); }
        e.cfg.enabled = false;
        assert_eq!(e.tick(16.0).dim, 0.0);
    }

    #[test]
    fn the_talk_flip_is_immediate_and_not_tied_to_the_dim_states() {
        // Putting the phone to your ear switches PTT→gated at once; waiting for
        // a 30-second fade would make the flip useless.
        let mut e = armed_engine();
        e.set_in_call(true);
        assert!(!e.tick(16.0).gated_talk);
        e.set_proximity_near(true);
        assert!(e.tick(16.0).gated_talk, "gated the moment it is near");
        e.set_proximity_near(false);
        assert!(!e.tick(16.0).gated_talk);
    }

    #[test]
    fn the_talk_flip_needs_a_call_as_well_as_a_cheek() {
        // Reading the inbox with the phone face-down is not a reason to change
        // talk mode.
        let mut e = armed_engine();
        e.set_proximity_near(true);
        assert!(!e.tick(16.0).gated_talk);
    }

    #[test]
    fn disarming_releases_the_talk_flip() {
        let mut e = armed_engine();
        e.set_in_call(true);
        e.set_proximity_near(true);
        assert!(e.tick(16.0).gated_talk);
        e.set_armed(false);
        assert!(!e.tick(16.0).gated_talk);
    }

    #[test]
    fn a_live_source_drives_the_whole_cycle() {
        let mut e = armed_engine();
        e.observe(&Sensors { near: true, moving: false });
        e.tick(1_000.0);
        assert_eq!(e.state(), State::Dim);
        e.tick(e.cfg.stillness_ms);
        assert_eq!(e.state(), State::Screensaver);
        e.observe(&Sensors { near: false, moving: true });
        assert_eq!(e.state(), State::Normal);
    }

    #[test]
    fn the_dim_never_leaves_its_range() {
        let mut e = armed_engine();
        e.set_proximity_near(true);
        for _ in 0..500 {
            let out = e.tick(1_000.0);
            assert!((0.0..=1.0).contains(&out.dim), "dim escaped: {}", out.dim);
        }
    }
}
