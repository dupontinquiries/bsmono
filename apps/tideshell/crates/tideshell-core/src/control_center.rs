//! The swipe-down drawer, shaped after plasma-mobile's action drawer: the first
//! pull reveals a single row of quick toggles ("peek"), pulling further opens
//! the full grid with brightness and volume sliders. Release snaps to the
//! nearest stage the gesture committed to.
//!
//! `progress` is the drawn position in `0..=1` (0 closed, [`PEEK`] one row,
//! 1 full). During a drag it follows the finger 1:1; after release it eases
//! toward `target` in [`ControlCenter::tick`].

use crate::Action;

/// Progress at which exactly the first row is visible.
pub const PEEK: f32 = 0.42;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QuickKind {
    Wifi,
    Bluetooth,
    MobileData,
    Airplane,
    Flashlight,
    Rotation,
    DoNotDisturb,
    /// The next three mirror shell modes rather than system services.
    Keyboard,
    Trackpad,
    Carousel,
    /// Ambient light + time of day brightness (shell-owned, see `ambient`).
    AutoBrightness,
    /// cpupower's scaling governor. A three-way CYCLE, not an on/off toggle --
    /// see `CpuGovernor` -- so it is a mode tile like Keyboard/Trackpad/Carousel:
    /// routed specially in `Shell::tap_quick`, skipped by `toggle_system`.
    CpuGovernor,
    /// A momentary action, not a state: tapping it takes a screenshot (and `ctl screenshot` does the same).
    Screenshot,
}

/// The three governors this shell offers, in cycle order.
///
/// Not the FULL set cpufreq supports -- `schedutil`, `userspace` and `ondemand`
/// exist too -- but the three a person actually reasons about day to day, named
/// the way `cpupower frequency-set -g <name>` and every kernel's
/// `scaling_available_governors` name them. `conservative` is common on
/// mobile/ARM cpufreq drivers but ABSENT on some modern desktop kernels that
/// dropped it for `schedutil`; the executor (tideshell-egui) is what checks
/// the running kernel's `scaling_available_governors` and skips a name it does
/// not actually offer -- this cycle is unconditional and pure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CpuGovernor {
    /// Lowest clocks the driver allows; `powersave`.
    Powersave,
    /// Scales with load more cautiously than `ondemand`; `conservative`.
    Conservative,
    /// Highest clocks the driver allows; `performance`.
    Performance,
}

impl CpuGovernor {
    pub const ALL: [CpuGovernor; 3] = [CpuGovernor::Powersave, CpuGovernor::Conservative, CpuGovernor::Performance];

    /// The exact string a kernel's `scaling_governor` file takes and
    /// `scaling_available_governors` reports.
    pub fn token(self) -> &'static str {
        match self {
            CpuGovernor::Powersave => "powersave",
            CpuGovernor::Conservative => "conservative",
            CpuGovernor::Performance => "performance",
        }
    }
    pub fn from_token(s: &str) -> Option<CpuGovernor> {
        CpuGovernor::ALL.into_iter().find(|g| g.token() == s)
    }
    pub fn label(self) -> &'static str {
        match self {
            CpuGovernor::Powersave => "Power saver",
            CpuGovernor::Conservative => "Balanced",
            CpuGovernor::Performance => "Performance",
        }
    }
    /// The next state in the cycle, wrapping. Unconditional -- whether the
    /// RUNNING kernel actually offers it is the executor's question, not this
    /// pure rotation's.
    pub fn next(self) -> CpuGovernor {
        match self {
            CpuGovernor::Powersave => CpuGovernor::Conservative,
            CpuGovernor::Conservative => CpuGovernor::Performance,
            CpuGovernor::Performance => CpuGovernor::Powersave,
        }
    }
}

impl QuickKind {
    pub const ALL: [QuickKind; 13] = [
        QuickKind::Wifi,
        QuickKind::Bluetooth,
        QuickKind::MobileData,
        QuickKind::Airplane,
        QuickKind::Flashlight,
        QuickKind::Rotation,
        QuickKind::DoNotDisturb,
        QuickKind::Keyboard,
        QuickKind::Trackpad,
        QuickKind::Carousel,
        QuickKind::AutoBrightness,
        QuickKind::CpuGovernor,
        QuickKind::Screenshot,
    ];

    pub fn token(self) -> &'static str {
        match self {
            QuickKind::Wifi => "wifi",
            QuickKind::Bluetooth => "bluetooth",
            QuickKind::MobileData => "mobile-data",
            QuickKind::Airplane => "airplane",
            QuickKind::Flashlight => "flashlight",
            QuickKind::Rotation => "rotation",
            QuickKind::DoNotDisturb => "dnd",
            QuickKind::Keyboard => "keyboard",
            QuickKind::Trackpad => "trackpad",
            QuickKind::Carousel => "carousel",
            QuickKind::AutoBrightness => "auto-brightness",
            QuickKind::CpuGovernor => "cpu-governor",
            QuickKind::Screenshot => "screenshot",
        }
    }

    pub fn from_token(s: &str) -> Option<QuickKind> {
        QuickKind::ALL.into_iter().find(|k| k.token() == s)
    }

    /// The cosmic-settings page (as a desktop-entry id) a long press on this tile opens, for the
    /// tiles that stand for a connection.
    pub fn settings_app(self) -> Option<&'static str> {
        match self {
            QuickKind::Wifi => Some("com.system76.CosmicSettings.Wireless"),
            QuickKind::Bluetooth => Some("com.system76.CosmicSettings.Bluetooth"),
            QuickKind::MobileData => Some("com.system76.CosmicSettings.Network"),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            QuickKind::Wifi => "Wi-Fi",
            QuickKind::Bluetooth => "Bluetooth",
            QuickKind::MobileData => "Mobile data",
            QuickKind::Airplane => "Airplane",
            QuickKind::Flashlight => "Flashlight",
            QuickKind::Rotation => "Auto-rotate",
            QuickKind::DoNotDisturb => "Do not disturb",
            QuickKind::Keyboard => "Keyboard",
            QuickKind::Trackpad => "Trackpad",
            QuickKind::Carousel => "Carousel",
            QuickKind::AutoBrightness => "Auto bright",
            QuickKind::CpuGovernor => "CPU mode",
            QuickKind::Screenshot => "Screenshot",
        }
    }

    /// Shell-mode tiles are owned by `Shell::modes`, not by this drawer.
    pub fn is_mode(self) -> bool {
        matches!(self, QuickKind::Keyboard | QuickKind::Trackpad | QuickKind::Carousel
                     | QuickKind::AutoBrightness | QuickKind::CpuGovernor | QuickKind::Rotation | QuickKind::Screenshot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SliderKind {
    Brightness,
    Volume,
    /// How hard the speaker chain works to fake bass: harmonics first, then
    /// compression (audio::warmth). Not volume — this is what the speaker
    /// does with whatever volume it is given.
    Warmth,
}

impl SliderKind {
    pub fn token(self) -> &'static str {
        match self {
            SliderKind::Brightness => "brightness",
            SliderKind::Volume => "volume",
            SliderKind::Warmth => "warmth",
        }
    }
    pub fn from_token(s: &str) -> Option<SliderKind> {
        match s {
            "brightness" => Some(SliderKind::Brightness),
            "volume" => Some(SliderKind::Volume),
            "warmth" => Some(SliderKind::Warmth),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            SliderKind::Brightness => "Brightness",
            SliderKind::Volume => "Volume",
            SliderKind::Warmth => "Warmth",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Closed,
    Peek,
    Full,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ControlCenter {
    pub progress: f32,
    target: f32,
    dragging: bool,
    /// Where the current pull started from, latched on its first sample.
    drag_base: f32,
    /// Tile order; the first `peek_count` show in the peek row.
    pub tiles: Vec<QuickKind>,
    pub peek_count: usize,
    system: Vec<(QuickKind, bool)>,
    pub brightness: f32,
    pub volume: f32,
    /// The DISPLAYED governor. A placeholder at startup -- the executor
    /// corrects it once the running kernel's actual value is read back (see
    /// `set_governor_state`), the same "reported by a backend" pattern
    /// `set_system_state` documents for the boolean tiles.
    pub governor: CpuGovernor,
    /// Speaker warmth. Lives beside the others so one slider widget serves
    /// all three, but it is only offered where a speaker chain exists.
    pub warmth: f32,
}

impl Default for ControlCenter {
    fn default() -> Self {
        ControlCenter {
            progress: 0.0,
            target: 0.0,
            dragging: false,
            drag_base: 0.0,
            tiles: QuickKind::ALL.to_vec(),
            peek_count: 5,
            system: Vec::new(),
            brightness: 0.6,
            volume: 0.5,
            governor: CpuGovernor::Powersave,
            // Off by default: the speaker chain may not exist, and warmth
            // should be something a listener turns up deliberately rather
            // than a colouration they find already applied.
            warmth: 0.0,
        }
    }
}

impl ControlCenter {
    pub fn stage(&self) -> Stage {
        if self.progress < 0.02 {
            Stage::Closed
        } else if self.progress < (PEEK + 1.0) / 2.0 {
            Stage::Peek
        } else {
            Stage::Full
        }
    }

    pub fn is_visible(&self) -> bool {
        self.progress > 0.001 || self.target > 0.0
    }

    /// Finger position during a pull. A pull that starts on an already
    /// peeking (or full) drawer continues from the peek row, so a second pull
    /// expands rather than restarting from closed.
    pub fn drag(&mut self, progress: f32) {
        if !self.dragging {
            self.dragging = true;
            self.drag_base = if self.target >= PEEK - 1e-3 { PEEK } else { 0.0 };
        }
        self.progress = (self.drag_base + progress).clamp(0.0, 1.0);
        self.target = self.progress;
    }

    /// Snap after release: open means FULL. There used to be a half-open "peek" stop (one row of
    /// tiles) that a pull could land on; it never reached the sliders or the settings entry and was
    /// the single thing standing between a thumb and half the controls, worse in landscape where the
    /// row is a sliver of a short screen. A pull is now either dismissed or the whole sheet.
    pub fn release(&mut self, open: bool) {
        self.dragging = false;
        self.target = if open { 1.0 } else { 0.0 };
    }

    pub fn close(&mut self) {
        self.dragging = false;
        self.target = 0.0;
    }

    pub fn expand(&mut self) {
        self.dragging = false;
        self.target = 1.0;
    }

    /// Ease toward target. Critically-damped-ish exponential: frame-rate
    /// independent because it is a function of `dt`, not of frame count.
    pub fn tick(&mut self, dt_ms: f32) {
        if self.dragging {
            return;
        }
        let k = 1.0 - (-dt_ms / 70.0).exp();
        self.progress += (self.target - self.progress) * k;
        if (self.progress - self.target).abs() < 0.002 {
            self.progress = self.target;
        }
    }

    pub fn is_animating(&self) -> bool {
        !self.dragging && self.progress != self.target
    }

    pub fn system_state(&self, kind: QuickKind) -> bool {
        self.system.iter().find(|(k, _)| *k == kind).map(|(_, v)| *v).unwrap_or(false)
    }

    /// Record state reported by a backend (so the tile reflects reality, not
    /// just the last tap).
    pub fn set_system_state(&mut self, kind: QuickKind, on: bool) {
        match self.system.iter_mut().find(|(k, _)| *k == kind) {
            Some(e) => e.1 = on,
            None => self.system.push((kind, on)),
        }
    }

    /// The governor as read back from the kernel -- see `governor`'s doc.
    pub fn set_governor_state(&mut self, g: CpuGovernor) {
        self.governor = g;
    }

    /// Advance the displayed governor and emit the action; `Shell::tap_quick`
    /// routes `CpuGovernor` here instead of `toggle_system`, the same way it
    /// routes the other mode tiles.
    pub fn cycle_governor(&mut self) -> Action {
        self.governor = self.governor.next();
        Action::SetGovernor(self.governor)
    }

    /// Tap on a system tile. Mode tiles are routed by the shell instead.
    pub fn toggle_system(&mut self, kind: QuickKind) -> Option<Action> {
        if kind.is_mode() {
            return None;
        }
        let on = !self.system_state(kind);
        self.set_system_state(kind, on);
        Some(Action::SetQuick(kind, on))
    }

    pub fn set_slider(&mut self, kind: SliderKind, v: f32) -> Action {
        let v = v.clamp(0.0, 1.0);
        match kind {
            SliderKind::Brightness => self.brightness = v,
            SliderKind::Volume => self.volume = v,
            SliderKind::Warmth => self.warmth = v,
        }
        Action::SetSlider(kind, v)
    }

    pub fn slider(&self, kind: SliderKind) -> f32 {
        match kind {
            SliderKind::Brightness => self.brightness,
            SliderKind::Volume => self.volume,
            SliderKind::Warmth => self.warmth,
        }
    }

    /// Tiles visible at the current stage.
    pub fn visible_tiles(&self) -> &[QuickKind] {
        match self.stage() {
            Stage::Closed => &[],
            Stage::Peek => &self.tiles[..self.peek_count.min(self.tiles.len())],
            Stage::Full => &self.tiles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cycle_visits_all_three_and_wraps() {
        let g = CpuGovernor::Powersave;
        let a = g.next();
        let b = a.next();
        let c = b.next();
        assert_eq!([g, a, b, c], [CpuGovernor::Powersave, CpuGovernor::Conservative,
                                   CpuGovernor::Performance, CpuGovernor::Powersave]);
    }

    #[test]
    fn tokens_round_trip_through_from_token() {
        for g in CpuGovernor::ALL {
            assert_eq!(CpuGovernor::from_token(g.token()), Some(g));
        }
        assert_eq!(CpuGovernor::from_token("schedutil"), None, "not one of the three offered");
    }

    #[test]
    fn cycle_governor_advances_and_reports_the_action() {
        let mut cc = ControlCenter::default();
        assert_eq!(cc.governor, CpuGovernor::Powersave, "default");
        assert_eq!(cc.cycle_governor(), Action::SetGovernor(CpuGovernor::Conservative));
        assert_eq!(cc.governor, CpuGovernor::Conservative);
        assert_eq!(cc.cycle_governor(), Action::SetGovernor(CpuGovernor::Performance));
        assert_eq!(cc.cycle_governor(), Action::SetGovernor(CpuGovernor::Powersave), "wraps");
    }

    /// A tile the SHELL routes specially, not the drawer's own boolean toggle.
    #[test]
    fn cpu_governor_is_a_mode_tile_not_a_system_toggle() {
        assert!(QuickKind::CpuGovernor.is_mode());
        let mut cc = ControlCenter::default();
        assert_eq!(cc.toggle_system(QuickKind::CpuGovernor), None, "mode tiles are routed by the shell");
    }

    #[test]
    fn a_backend_reported_governor_updates_the_display() {
        let mut cc = ControlCenter::default();
        cc.set_governor_state(CpuGovernor::Performance);
        assert_eq!(cc.governor, CpuGovernor::Performance);
    }

    fn settle(cc: &mut ControlCenter) {
        for _ in 0..200 {
            cc.tick(16.0);
        }
    }

    #[test]
    fn any_opening_pull_lands_on_the_full_sheet_with_no_half_stop() {
        for pull in [0.05, 0.2, PEEK, 0.6, 0.95] {
            let mut cc = ControlCenter::default();
            cc.drag(pull);
            cc.release(true);
            settle(&mut cc);
            assert_eq!(cc.stage(), Stage::Full, "a pull of {pull} must not stop at a peek");
            assert_eq!(cc.visible_tiles().len(), QuickKind::ALL.len());
        }
        let mut cc = ControlCenter::default();
        cc.drag(0.3);
        cc.release(false);
        settle(&mut cc);
        assert_eq!(cc.stage(), Stage::Closed, "and a pull released as a dismissal closes");
    }

    #[test]
    fn second_pull_from_peek_continues_to_full() {
        let mut cc = ControlCenter::default();
        cc.drag(0.3);
        cc.release(true);
        settle(&mut cc);
        // A pull of 0.4 from peek reaches 0.82 — past the peek/full midpoint.
        cc.drag(0.4);
        assert!((cc.progress - (PEEK + 0.4)).abs() < 1e-6);
        cc.release(true);
        settle(&mut cc);
        assert_eq!(cc.stage(), Stage::Full);
    }

    #[test]
    fn continuous_pull_tracks_the_finger_without_jumping() {
        let mut cc = ControlCenter::default();
        let mut prev = 0.0;
        for i in 1..=20 {
            let p = i as f32 * 0.04;
            cc.drag(p);
            assert!((cc.progress - p.min(1.0)).abs() < 1e-6, "sample {i}: {} vs {p}", cc.progress);
            assert!(cc.progress >= prev);
            prev = cc.progress;
        }
    }

    #[test]
    fn release_closed_goes_to_zero_and_hides() {
        let mut cc = ControlCenter::default();
        cc.drag(0.8);
        cc.release(false);
        assert!(cc.is_animating());
        settle(&mut cc);
        assert_eq!(cc.progress, 0.0);
        assert!(!cc.is_visible());
        assert_eq!(cc.stage(), Stage::Closed);
    }

    #[test]
    fn tick_is_frame_rate_independent() {
        let mut a = ControlCenter::default();
        let mut b = ControlCenter::default();
        a.expand();
        b.expand();
        for _ in 0..6 {
            a.tick(16.0);
        }
        for _ in 0..12 {
            b.tick(8.0);
        }
        assert!((a.progress - b.progress).abs() < 1e-4, "{} vs {}", a.progress, b.progress);
        assert!(a.progress > 0.5 && a.progress < 1.0);
    }

    #[test]
    fn system_toggle_flips_and_emits_but_mode_tiles_do_not() {
        let mut cc = ControlCenter::default();
        assert_eq!(cc.toggle_system(QuickKind::Wifi), Some(Action::SetQuick(QuickKind::Wifi, true)));
        assert!(cc.system_state(QuickKind::Wifi));
        assert_eq!(cc.toggle_system(QuickKind::Wifi), Some(Action::SetQuick(QuickKind::Wifi, false)));
        assert_eq!(cc.toggle_system(QuickKind::Keyboard), None);
    }

    #[test]
    fn rotation_is_a_mode_tile_the_shell_owns_not_a_system_toggle() {
        // The Auto-rotate tile used to go through toggle_system -> SetQuick(Rotation), which the executor
        // answers 'no system backend' and which never touched the shell's AutoRotate: the tile could
        // flip its own light and nothing else. The setting lives in the shell (AutoRotate + prefs).
        assert!(QuickKind::Rotation.is_mode());
        let mut cc = ControlCenter::default();
        assert_eq!(cc.toggle_system(QuickKind::Rotation), None, "the drawer must not claim it");
    }

    #[test]
    fn slider_clamps() {
        let mut cc = ControlCenter::default();
        assert_eq!(cc.set_slider(SliderKind::Volume, 1.7), Action::SetSlider(SliderKind::Volume, 1.0));
        assert_eq!(cc.volume, 1.0);
    }

    #[test]
    fn tokens_round_trip() {
        for k in QuickKind::ALL {
            assert_eq!(QuickKind::from_token(k.token()), Some(k));
        }
        assert_eq!(QuickKind::from_token("nope"), None);
    }
}
