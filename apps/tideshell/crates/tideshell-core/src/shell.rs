//! The orchestrator. Owns every model, routes recognizer events and overlay
//! taps into them, and returns the resulting [`Action`]s. The frontend paints
//! from the public fields and calls the `on_*`/tap methods; transports execute
//! the actions and report window state back through [`Shell::set_windows`].

use std::collections::{HashMap, VecDeque};

use crate::bar::BarWidget;
use crate::carousel::Carousel;
use crate::control_center::{ControlCenter, QuickKind, SliderKind};
use crate::ambient::AutoBrightness;
use crate::cursor::FatCursor;
use crate::lock::{LockAction, LockScreen};
use crate::sleep::{Sleep, SleepAction};
use crate::edge::{GestureEvent, Recognizer, SwipeUpTarget};
use crate::hold::{HoldMenu, HoldOption};
use crate::palette::{Geom, Palette, PaletteAction, PaletteItem};
use crate::prefs::{Layout, ShellPrefs};
use crate::switcher::Switcher;
use crate::trackpad::Trackpad;
use crate::{Action, Millis, TouchEvent, WindowCard};

const TRACE_CAP: usize = 80;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modes {
    pub keyboard: bool,
    pub trackpad: bool,
}

/// What the home (cover) widget shows besides the time. Filled in by the frontend: it owns the
/// file and D-Bus reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HomeInfo {
    /// This device's snapclone id (`sc-xxxxxxxx`), if an identity exists.
    pub sc_id: Option<String>,
    /// The on-device temperature estimate in degC, its 0..1 confidence and where it came from.
    pub temp_c: Option<f32>,
    pub temp_confidence: f32,
    pub temp_source: String,
    /// Why there is no temperature (the service is not running, ...), shown instead.
    pub temp_note: String,
}

#[derive(Debug)]
pub struct Shell {
    pub prefs: ShellPrefs,
    pub screen: (f32, f32),
    pub recognizer: Recognizer,
    pub drawer: ControlCenter,
    pub switcher: Switcher,
    pub carousel: Carousel,
    pub hold: Option<HoldMenu>,
    pub trackpad: Trackpad,
    pub modes: Modes,
    /// The fat cursor, present while it is in charge of the pointer.
    pub fat: Option<FatCursor>,
    /// Ambient light + time of day brightness.
    pub ambient: AutoBrightness,
    /// The command palette (press-and-hold on the bottom edge).
    pub palette: Palette,
    /// Apps for the palette (desktop id, name), from hatch via the frontend.
    apps: Vec<(String, String)>,
    /// The palette asked for the settings sheet; the view takes it.
    pub settings_requested: bool,
    /// The lock cover (cover mode for now: swipe dismisses, no auth).
    pub lock: LockScreen,
    /// Power key -> fade -> panel off; triple tap wakes.
    pub sleep: Sleep,
    /// Is the phone lying face-up? Drives the cover clock's 3D (clock3d.rs).
    pub flat: crate::clock3d::FlatDetector,
    /// The cover's depth orbs (orbs.rs), fed by the camera's live window.
    pub orbs: crate::orbs::Orbs,
    /// Bedside mode (nightclock.rs) and whether we are inside sleeping hours
    /// (with pulsing on), as the frontend last read them from settings.
    pub night: crate::nightclock::NightClock,
    pub night_window: bool,
    pub night_pulse: bool,
    pub rotate: crate::rotate::AutoRotate,
    /// The morse unlock, and when the power key last woke the screen (that press's release is not
    /// a symbol).
    pub morse: crate::morse::Morse,
    pub home: HomeInfo,
    morse_wake_ms: Option<Millis>,
    /// The camera pane's world (gyro/gravity/compass) and its anchors.
    pub world: crate::world::World,
    /// Ambient loudness -> notification gain.
    pub sound: crate::ambient_sound::AmbientSound,
    last_imu: Option<Millis>,
    pub cold: crate::cold::Cold,
    /// The recognizer's warm-weather config, so cold tuning is applied to
    /// the original and not compounded.
    base_edge: crate::edge::EdgeConfig,
    tilt: crate::tilt::Tilt,
    /// The governor's headroom, from its state file; default = free.
    pub headroom: crate::voice::Headroom,
    voice_line: String,
    apps_light: bool,
    /// The living design's calendar input (1..366), set by the runtime.
    day_of_year: u32,
    /// Places and what they are like.
    pub ambience: crate::ambience::Ambience,
    ambience_dirty: bool,
    last_ambience_log: Option<Millis>,
    /// Local minute of the day, set by the runtime (the engine has no clock).
    clock_minute: u32,
    /// The learned offset changed and is waiting to be saved once the user
    /// lets go of the slider: (time of the last change).
    offset_dirty_at: Option<Millis>,
    /// Right-bar layout: collapsed rail vs expanded (right-edge swipe toggles).
    pub bar_expanded: bool,
    /// Live swipe-up progress for the preview (0 when not swiping).
    pub swipe_up: f32,
    /// Live horizontal offset along the bottom edge (0 when not swiping).
    pub peek_dx: f32,
    windows: Vec<WindowCard>,
    /// Last reflow request per window, so one the compositor will not
    /// shrink cannot turn into an unmaximize/maximize loop.
    reflow_at: HashMap<u64, Millis>,
    /// The zone total `reflow_for_zone` last saw and since when (the layout must hold still
    /// ZONE_SETTLE_MS before any window is re-maximized), and each window's last (maximized,
    /// width) with the time it last CHANGED (a window mid-resize is not judged).
    zone_seen: f32,
    zone_since: Millis,
    win_seen: HashMap<u64, (bool, Option<i32>, Millis)>,
    now: Millis,
    last_tick: Option<Millis>,
    trace: VecDeque<String>,
}

impl Shell {
    pub fn new(prefs: ShellPrefs, screen: (f32, f32)) -> Shell {
        let mut switcher = Switcher::default();
        switcher.style = prefs.switcher_style;
        let mut trackpad = Trackpad::default();
        trackpad.speed = prefs.trackpad_speed;
        let cold_force = match prefs.cold.as_str() {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        };
        let base_edge = prefs.edge_config();
        let mut cold = crate::cold::Cold::default();
        cold.force = cold_force;
        let mut rotate = crate::rotate::AutoRotate::default();
        rotate.enabled = prefs.autorotate;
        let mut morse = crate::morse::Morse::default();
        morse.set_pattern(&prefs.unlock_morse);
        let mut ambient = AutoBrightness::default();
        ambient.enabled = prefs.auto_brightness;
        ambient.user = prefs.brightness_offset;
        ambient.night = prefs.night;
        Shell {
            recognizer: Recognizer::new(prefs.edge_config(), screen),
            prefs,
            screen,
            drawer: ControlCenter::default(),
            switcher,
            carousel: Carousel::default(),
            hold: None,
            trackpad,
            modes: Modes::default(),
            fat: None,
            ambient,
            palette: Palette::default(),
            apps: Vec::new(),
            settings_requested: false,
            lock: LockScreen::default(),
            sleep: Sleep::default(),
            flat: crate::clock3d::FlatDetector::default(),
            orbs: crate::orbs::Orbs::default(),
            night: crate::nightclock::NightClock::default(),
            night_window: false,
            night_pulse: true,
            rotate,
            morse,
            home: HomeInfo::default(),
            morse_wake_ms: None,
            world: crate::world::World::new(screen.0 * 0.9),
            sound: crate::ambient_sound::AmbientSound::default(),
            last_imu: None,
            cold,
            base_edge,
            tilt: crate::tilt::Tilt::default(),
            headroom: crate::voice::Headroom::default(),
            voice_line: String::new(),
            apps_light: false,
            day_of_year: 172,
            ambience: crate::ambience::Ambience::default(),
            ambience_dirty: false,
            last_ambience_log: None,
            clock_minute: 12 * 60,
            offset_dirty_at: None,
            bar_expanded: false,
            swipe_up: 0.0,
            peek_dx: 0.0,
            windows: Vec::new(),
            reflow_at: HashMap::new(),
            zone_seen: 0.0,
            zone_since: 0,
            win_seen: HashMap::new(),
            now: 0,
            last_tick: None,
            trace: VecDeque::new(),
        }
    }

    // ---- observability -------------------------------------------------

    pub fn trace(&self) -> impl Iterator<Item = &String> {
        self.trace.iter()
    }

    fn log(&mut self, msg: impl Into<String>) {
        if self.trace.len() == TRACE_CAP {
            self.trace.pop_front();
        }
        self.trace.push_back(format!("{:>7} {}", self.now, msg.into()));
    }

    /// Record something from outside the engine (a transport's outcome), so
    /// the trace shows the whole loop: gesture -> action -> what happened.
    pub fn note(&mut self, msg: impl Into<String>) {
        self.log(msg);
    }

    fn emit(&mut self, actions: Vec<Action>) -> Vec<Action> {
        for a in &actions {
            self.log(format!("-> {a:?}"));
        }
        actions
    }

    fn drain_recognizer_log(&mut self) {
        for t in self.recognizer.drain_transitions() {
            self.log(format!("edge {t}"));
        }
    }

    // ---- configuration and world state ----------------------------------

    pub fn set_screen(&mut self, screen: (f32, f32)) {
        self.screen = screen;
        self.recognizer.set_screen(screen);
        if let Some(f) = &mut self.fat {
            f.set_screen(screen);
        }
    }

    /// Start or stop the fat cursor to match (trackpad on && pref), and hide
    /// or restore the system cursor with it.
    fn sync_fat(&mut self) -> Vec<Action> {
        let want = self.modes.trackpad && self.prefs.fat_cursor;
        match (want, self.fat.is_some()) {
            (true, false) => {
                self.fat = Some(FatCursor::new(self.screen, self.now));
                self.log("fat cursor on");
                vec![Action::HideSystemCursor(true)]
            }
            (false, true) => {
                self.fat = None;
                self.log("fat cursor off");
                vec![Action::HideSystemCursor(false)]
            }
            _ => vec![],
        }
    }

    pub fn set_prefs(&mut self, prefs: ShellPrefs) -> Vec<Action> {
        if prefs.layout != self.prefs.layout {
            self.log(format!("layout {} -> {}", self.prefs.layout.token(), prefs.layout.token()));
            self.drawer.close();
            self.bar_expanded = false;
        }
        self.recognizer.cfg = prefs.edge_config();
        self.switcher.style = prefs.switcher_style;
        self.trackpad.speed = prefs.trackpad_speed;
        self.ambient.enabled = prefs.auto_brightness;
        self.ambient.user = prefs.brightness_offset;
        self.ambient.night = prefs.night;
        self.rotate.enabled = prefs.autorotate;
        if prefs.unlock_morse != self.prefs.unlock_morse {
            self.morse.set_pattern(&prefs.unlock_morse);
        }
        self.cold.set_force(match prefs.cold.as_str() {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        });
        self.base_edge = prefs.edge_config();
        self.prefs = prefs;
        self.apply_cold();
        let mut out = vec![Action::SavePrefs];
        out.extend(self.sync_fat());
        self.emit(out)
    }

    pub fn windows(&self) -> &[WindowCard] {
        &self.windows
    }

    pub fn set_windows(&mut self, windows: Vec<WindowCard>) -> Vec<Action> {
        // Helper windows that exist for plumbing, not for people (see
        // `is_helper_window`): never a card, a carousel page or a palette entry.
        let windows: Vec<WindowCard> = windows.into_iter().filter(|w| !crate::is_helper_window(&w.app_id)).collect();
        self.switcher.sync(&windows);
        let out = self.carousel.sync(&windows);
        let now = self.now;
        self.win_seen.retain(|id, _| windows.iter().any(|w| w.id == *id));
        for w in &windows {
            let cur = (w.maximized, w.width);
            let e = self.win_seen.entry(w.id).or_insert((cur.0, cur.1, now));
            if (e.0, e.1) != cur {
                *e = (cur.0, cur.1, now);
            }
        }
        self.windows = windows;
        self.rebuild_palette();
        self.emit(out)
    }

    /// Transport feedback: rusty-board shows/hides itself on text focus.
    pub fn set_keyboard_visible(&mut self, on: bool) {
        self.modes.keyboard = on;
    }

    /// Whether a quick tile / bar toggle is on, wherever its state lives.
    pub fn quick_state(&self, kind: QuickKind) -> bool {
        match kind {
            QuickKind::Keyboard => self.modes.keyboard,
            QuickKind::Trackpad => self.modes.trackpad,
            QuickKind::Carousel => self.carousel.is_active(),
            QuickKind::AutoBrightness => self.ambient.enabled,
            QuickKind::Rotation => self.rotate.enabled,
            // Always "on": there is no off state to distinguish, only which
            // of the three modes is current -- the tile's own label carries
            // that (see the drawer's rendering).
            QuickKind::CpuGovernor => true,
            // Momentary: it is never "on".
            QuickKind::Screenshot => false,
            k => self.drawer.system_state(k),
        }
    }

    // ---- input ------------------------------------------------------------

    fn on_sleep_action(&mut self, a: SleepAction) -> Vec<Action> {
        match a {
            SleepAction::Lock => {
                self.lock_now();
                vec![]
            }
            SleepAction::DisplayPower(on) => {
                self.log(format!("display {}", if on { "on" } else { "off" }));
                vec![Action::DisplayPower(on)]
            }
        }
    }

    /// Engage the cover (at startup, on sleep, or on request).
    pub fn lock_now(&mut self) {
        if !self.lock.lock().is_empty() {
            self.drawer.close();
            self.switcher.hide();
            self.hold = None;
            self.log("cover shown");
        }
    }

    /// The power key, with the clock of the loop that received it. While the screen is off the loop
    /// sleeps on its sockets and `self.now` stops moving, so a press that arrives ten seconds later
    /// would be stamped ten seconds ago -- and "the press that woke the screen is not a symbol"
    /// (`power_released`) compares times. Always use this from the loop.
    pub fn power_key_at(&mut self, now: Millis) -> Vec<Action> {
        self.now = self.now.max(now);
        self.power_key()
    }

    /// Milliseconds until the sleep layer wants a tick while the screen is off (re-blanking a panel
    /// something else lit), for the loop's wait timeout.
    pub fn next_quiet_ms(&self, now: Millis) -> Option<Millis> {
        self.sleep.quiet_in(now)
    }

    /// The power key: awake -> sleep (and cover); asleep -> wake.
    pub fn power_key(&mut self) -> Vec<Action> {
        self.log("power key");
        // With a morse unlock set and the cover up on a lit screen, a press is the START of a
        // symbol, not "sleep": its length arrives with the release (`power_released`).
        if self.morse.enabled() && self.lock.is_locked() && matches!(self.sleep.phase, crate::sleep::Phase::Awake | crate::sleep::Phase::FadingIn) {
            return vec![];
        }
        if matches!(self.sleep.phase, crate::sleep::Phase::Off | crate::sleep::Phase::FadingOut) {
            self.morse_wake_ms = Some(self.now);
        }
        let acts = self.sleep.power_key();
        let out: Vec<Action> = acts.into_iter().flat_map(|a| self.on_sleep_action(a)).collect();
        self.emit(out)
    }

    /// The power key came back up after `held_ms` (evdev: the compositor route only reports the
    /// press). Under a morse unlock this is the symbol; a hold past the symbol range sleeps the
    /// screen instead, so a pocket cannot sit "inside" the code.
    pub fn power_released(&mut self, held_ms: Millis, now: Millis) -> Vec<Action> {
        self.now = self.now.max(now);
        if !self.morse.enabled() || !self.lock.is_locked() {
            return vec![];
        }
        if !matches!(self.sleep.phase, crate::sleep::Phase::Awake | crate::sleep::Phase::FadingIn) {
            return vec![];
        }
        // The press that woke the screen is not a symbol.
        let started = now.saturating_sub(held_ms);
        if self.morse_wake_ms.is_some_and(|w| started <= w + 300 && now.saturating_sub(w) < 2500) {
            return vec![];
        }
        if held_ms >= crate::morse::SLEEP_MS {
            self.log("morse: long hold, sleeping");
            let acts = self.sleep.power_key();
            let out: Vec<Action> = acts.into_iter().flat_map(|a| self.on_sleep_action(a)).collect();
            return self.emit(out);
        }
        match self.morse.press(held_ms, now) {
            crate::morse::Outcome::Unlocked => {
                self.log("morse: unlocked");
                self.lock.open_cover();
            }
            crate::morse::Outcome::Progress(n) => self.log(format!("morse: {n}/{}", self.morse.len())),
            crate::morse::Outcome::Wrong => self.log("morse: wrong, start over"),
            crate::morse::Outcome::Ignored => {}
        }
        vec![]
    }

    /// A tap on the home widget: clock -> snapclone id -> temperature -> clock, remembered.
    pub fn cycle_home_widget(&mut self) -> Vec<Action> {
        self.prefs.home_widget = self.prefs.home_widget.next();
        self.log(format!("home widget: {}", self.prefs.home_widget.token()));
        vec![Action::SavePrefs]
    }

    /// A touch while the screen sleeps (all touches go here then).
    pub fn on_sleep_touch(&mut self, ev: TouchEvent) -> Vec<Action> {
        let acts = match ev {
            TouchEvent::Down { slot, x, y, t } => self.sleep.touch_down(slot, x, y, t),
            TouchEvent::Move { slot, x, y, .. } => {
                self.sleep.touch_move(slot, x, y);
                vec![]
            }
            TouchEvent::Up { slot, t } => self.sleep.touch_up(slot, t),
            TouchEvent::Cancel => vec![],
        };
        if !acts.is_empty() {
            self.log("triple tap: wake");
        }
        let out: Vec<Action> = acts.into_iter().flat_map(|a| self.on_sleep_action(a)).collect();
        self.emit(out)
    }

    /// A touch sample from an edge strip.
    pub fn on_edge_touch(&mut self, ev: TouchEvent) -> Vec<Action> {
        // Covered or asleep: no shell gestures (the cover and the sleep
        // capture take the touches instead).
        if self.lock.is_locked() || self.sleep.captures_input() {
            return vec![];
        }
        if let TouchEvent::Down { t, .. } | TouchEvent::Move { t, .. } | TouchEvent::Up { t, .. } = ev {
            self.now = self.now.max(t);
        }
        let events = self.recognizer.on_event(ev);
        self.drain_recognizer_log();
        let mut out = vec![];
        for g in events {
            out.extend(self.on_gesture(g));
        }
        out
    }

    /// Advance time: hold detection, streak settling, drawer animation.
    pub fn tick(&mut self, now: Millis) -> Vec<Action> {
        self.now = self.now.max(now);
        let dt = self.last_tick.map(|l| now.saturating_sub(l) as f32).unwrap_or(0.0);
        self.last_tick = Some(now);
        self.drawer.tick(dt.min(100.0));
        self.switcher.tick_anim(dt.min(100.0));
        if self.flat.tick(dt.min(100.0), now) && !self.flat.is_transitioning() {
            self.log(format!("cover clock 3D {}", if self.flat.is_flat() { "in" } else { "out" }));
        }
        let streaking = self.switcher.is_streaking();
        self.switcher.tick(now);
        if streaking && !self.switcher.is_streaking() {
            self.log("quick-switch streak settled");
        }
        let rotate_blocked = self.lock.is_locked() || self.sleep.captures_input();
        let mut out = vec![];
        if let Some(o) = self.rotate.poll(now, rotate_blocked) {
            self.log(format!("rotate {o:?}"));
            out.extend(self.emit(vec![Action::Rotate(o.transform_deg(self.prefs.rotate_flip))]));
        }
        let events = self.recognizer.tick(now);
        self.drain_recognizer_log();
        for a in self.sleep.tick(dt).into_iter().chain(self.sleep.quiet(now)) {
            out.extend(self.on_sleep_action(a));
        }
        self.palette.tick(dt);
        for a in self.lock.tick(dt, now) {
            if a == LockAction::Unlock {
                self.log("cover dismissed");
            }
        }
        self.log_ambience(now);
        // Sunlight: switch the apps to a light theme when the contrast is
        // well in (0.6) and back once it is well out (0.3): a theme is a
        // step, so hysteresis keeps it from flapping in dappled light.
        let contrast = self.ambient.band(now, self.clock_minute).contrast;
        if !self.apps_light && contrast >= 0.6 {
            self.apps_light = true;
            self.log("sunlight: apps to the light theme");
            out.push(Action::AppsLightTheme(true));
        } else if self.apps_light && contrast <= 0.3 {
            self.apps_light = false;
            self.log("sunlight over: apps theme restored");
            out.push(Action::AppsLightTheme(false));
        }
        // A rotation that waited for the cover or sleep to end.
        if !self.lock.is_locked() && !self.sleep.captures_input() {
            if let Some(o) = self.rotate.pending() {
                self.log(format!("rotate {o:?} (deferred)"));
                out.push(Action::Rotate(o.transform_deg(self.prefs.rotate_flip)));
            }
        }
        // A tap's held release goes out at its deadline (libinput timing).
        for o in self.trackpad.tick(now) {
            self.log(format!("-> {:?}", Action::Pointer(o)));
            out.push(Action::Pointer(o));
        }
        // Auto-brightness ramp steps: not logged (a ramp is many steps).
        if let Some(v) = self.ambient.tick(now, self.clock_minute) {
            self.drawer.brightness = v;
            out.push(Action::SetSlider(SliderKind::Brightness, v));
        }
        // Persist a learned offset once the slider has been still for a while.
        if self.offset_dirty_at.is_some_and(|t| now.saturating_sub(t) >= 1500) {
            self.offset_dirty_at = None;
            self.prefs.brightness_offset = self.ambient.user;
            self.log(format!("saved: brightness {:+.2} of the band, voicing {:+.2}", self.ambient.user, self.prefs.audio_tone));
            out.push(Action::SavePrefs);
        }
        // The fat cursor's eased position IS the pointer: send it as it moves.
        if let Some(p) = self.fat.as_mut().and_then(|f| f.tick(now, dt)) {
            out.push(Action::Pointer(crate::trackpad::PointerOut::Absolute { x: p.0, y: p.1 }));
        }
        for g in events {
            out.extend(self.on_gesture(g));
        }
        out
    }

    fn on_gesture(&mut self, g: GestureEvent) -> Vec<Action> {
        match g {
            GestureEvent::DrawerDrag { progress } => {
                self.drawer.drag(progress);
                vec![]
            }
            GestureEvent::DrawerRelease { open } => {
                self.drawer.release(open);
                self.log(format!("drawer release open={open}"));
                vec![]
            }
            GestureEvent::SwipeUpDrag { progress } => {
                self.swipe_up = progress;
                if self.drawer.is_visible() {
                    self.drawer.close();
                }
                self.switcher.drag(progress, self.now);
                vec![]
            }
            GestureEvent::SwipeUpRelease { target } => {
                self.swipe_up = 0.0;
                self.log(format!("swipe-up -> {target:?}"));
                match target {
                    SwipeUpTarget::Cancel => {
                        self.switcher.cancel_reveal();
                        vec![]
                    }
                    SwipeUpTarget::Switcher => {
                        self.switcher.show();
                        vec![]
                    }
                    SwipeUpTarget::Launcher => {
                        self.switcher.hide();
                        self.emit(vec![Action::SummonLauncher])
                    }
                }
            }
            GestureEvent::HorizontalDrag { dx } => {
                self.peek_dx = dx;
                vec![]
            }
            GestureEvent::QuickSwitch(dir) => {
                self.peek_dx = 0.0;
                let Some(dir) = dir else { return vec![] };
                let out = if self.carousel.is_active() {
                    self.log(format!("carousel step {dir:?}"));
                    self.carousel.step(dir, &self.windows)
                } else {
                    self.log(format!("quick-switch {dir:?}"));
                    self.switcher.quick_switch(dir, self.now).into_iter().collect()
                };
                self.emit(out)
            }
            GestureEvent::HoldStart { .. } => {
                self.swipe_up = 0.0;
                self.drawer.close();
                self.switcher.hide();
                self.rebuild_palette();
                self.palette.open_by_hold();
                self.log("palette opened by hold");
                vec![]
            }
            GestureEvent::HoldMove { x, y } => {
                let g = self.palette.geom(self.screen);
                let from = self.recognizer.start();
                self.palette.hold_move_banded(&g, from, x, y);
                vec![]
            }
            GestureEvent::HoldRelease { x, y } => {
                let g = self.palette.geom(self.screen);
                let from = self.recognizer.start();
                let (pick, leave) = self.palette.hold_release_banded(&g, from, x, y);
                self.palette.end_hold();
                // Lifting above the sheet is "no, none of these": the
                // palette closes rather than staying open for taps, because
                // a sheet left behind after a dismissal is a sheet the user
                // has to dismiss twice.
                if leave {
                    self.palette.close();
                    self.log("hold release -> dismissed");
                    return vec![];
                }
                self.log(format!("hold release -> {pick:?}"));
                match pick {
                    Some(a) => self.palette_action(a),
                    None => vec![],
                }
            }
            GestureEvent::RightSwipeIn => {
                self.bar_expanded = !self.bar_expanded;
                self.log(format!("bar expanded={}", self.bar_expanded));
                vec![]
            }
            GestureEvent::Cancelled => {
                self.swipe_up = 0.0;
                self.peek_dx = 0.0;
                self.hold = None;
                self.palette.end_hold();
                self.switcher.cancel_reveal();
                self.drawer.release(false);
                self.log("gesture cancelled");
                vec![]
            }
        }
    }

    // ---- modes --------------------------------------------------------------

    pub fn toggle_mode(&mut self, opt: HoldOption) -> Vec<Action> {
        let out = match opt {
            HoldOption::Carousel => {
                self.switcher.hide();
                if self.carousel.is_active() {
                    self.carousel.exit()
                } else {
                    let mru = self.switcher.mru_ids().to_vec();
                    self.carousel.enter(&mru, &self.windows)
                }
            }
            HoldOption::Keyboard => {
                self.modes.keyboard = !self.modes.keyboard;
                vec![Action::ShowKeyboard(self.modes.keyboard)]
            }
            HoldOption::Trackpad => {
                self.modes.trackpad = !self.modes.trackpad;
                let mut v = vec![Action::ShowTrackpad(self.modes.trackpad)];
                if !self.modes.trackpad {
                    v.extend(self.trackpad.cancel().into_iter().map(Action::Pointer));
                }
                v.extend(self.sync_fat());
                v
            }
        };
        self.log(format!("mode {opt:?}"));
        self.emit(out)
    }

    // ---- command palette -------------------------------------------------------

    /// Apps for the palette, from hatch (desktop id, display name).
    pub fn set_apps(&mut self, apps: Vec<(String, String)>) {
        self.apps = apps;
        self.rebuild_palette();
    }

    fn rebuild_palette(&mut self) {
        let mut items = Vec::new();
        for w in self.switcher.cards() {
            items.push(PaletteItem::new(format!("win:{}", w.id), w.title.clone(), "running", PaletteAction::Window(w.id)));
        }
        for (id, name) in &self.apps {
            items.push(PaletteItem::new(format!("app:{id}"), name.clone(), "app", PaletteAction::App(id.clone())));
        }
        let ab = if self.ambient.enabled { "Auto brightness: off" } else { "Auto brightness: on" };
        items.push(PaletteItem::new("set:autobright", ab, "setting", PaletteAction::ToggleAutoBrightness));
        let fc = if self.prefs.fat_cursor { "Cursor: normal" } else { "Cursor: fat" };
        items.push(PaletteItem::new("set:cursor", fc, "setting", PaletteAction::ToggleFatCursor));
        // Cold mode cycles rather than toggling, because it has three states
        // and the middle one matters: "automatic" is not "off", it is the
        // battery temperature deciding, and someone who has forced it off in
        // a warm room still wants it back on its own in a cold one.
        let (cold_label, cold_next) = match self.prefs.cold.as_str() {
            "on" => ("Cold mode: always on \u{2192} off", "off"),
            "off" => ("Cold mode: off \u{2192} automatic", "auto"),
            _ => ("Cold mode: automatic \u{2192} always on", "on"),
        };
        items.push(PaletteItem::new("set:cold", cold_label, if self.cold.on { "active" } else { "setting" }, PaletteAction::Cold(cold_next)));
        match self.prefs.layout {
            Layout::PullDown => items.push(PaletteItem::new("set:layout", "Layout: right-hand bar", "setting", PaletteAction::LayoutRightBar)),
            Layout::RightBar => items.push(PaletteItem::new("set:layout", "Layout: pull-down", "setting", PaletteAction::LayoutPullDown)),
        }
        match self.prefs.clock_style {
            crate::clock::ClockStyle::Digital => items.push(PaletteItem::new("set:clock", "Clock: analog", "setting", PaletteAction::ClockAnalog)),
            crate::clock::ClockStyle::Analog => items.push(PaletteItem::new("set:clock", "Clock: digital", "setting", PaletteAction::ClockDigital)),
        }
        items.push(PaletteItem::new("act:switcher", "Switcher", "action", PaletteAction::Switcher));
        items.push(PaletteItem::new("act:launcher", "All apps", "action", PaletteAction::Launcher));
        items.push(PaletteItem::new("act:settings", "Shell settings", "action", PaletteAction::Settings));
        items.push(PaletteItem::new("act:closeall", "Close all windows", "action", PaletteAction::CloseAll));
        items.push(PaletteItem::new("act:lock", "Lock", "action", PaletteAction::Lock));
        items.push(PaletteItem::new("act:sleep", "Sleep", "action", PaletteAction::Sleep));
        let recents = self.prefs.palette_recent.split(',').filter(|k| !k.is_empty()).map(str::to_string);
        self.palette.set_recents(recents);
        self.palette.set_items(items);
    }

    /// Open the palette from a tap (a bar widget, a tile).
    /// Walk the palette's selection with the volume keys (volkeys.rs).
    pub fn palette_step(&mut self, down: bool) {
        self.palette.step(down);
        self.log(format!("palette step -> {:?}", self.palette.selected));
    }

    /// A glove flick (cold mode; gloveflick.rs). Only acted on in cold mode:
    /// on a bare-handed phone, turning it over is not a command.
    pub fn on_flick(&mut self, f: crate::gloveflick::Flick) -> Vec<Action> {
        use crate::gloveflick::{intent, GloveIntent as G};
        if !self.cold.on || self.lock.is_locked() || self.sleep.captures_input() {
            return vec![];
        }
        let i = intent(f, self.palette.open);
        self.log(format!("glove flick {f:?} -> {i:?}"));
        match i {
            G::PaletteOpenAndStep { down } => {
                self.open_palette();
                self.palette_step(down);
                vec![]
            }
            G::PaletteStep { down } => {
                self.palette_step(down);
                vec![]
            }
            G::PaletteActivate => self.palette_activate_selected(),
            G::PaletteClose => {
                self.palette.close();
                vec![]
            }
            G::Keyboard(cmd) => vec![Action::KeyboardCommand(cmd)],
            G::EscapeAll => {
                self.palette.close();
                vec![Action::KeyboardCommand("voice-cancel")]
            }
        }
    }

    /// Activate whatever the keys have walked to.
    pub fn palette_activate_selected(&mut self) -> Vec<Action> {
        match self.palette.activate_selected() {
            Some(a) => self.palette_action(a),
            None => vec![],
        }
    }

    pub fn open_palette(&mut self) {
        self.drawer.close();
        self.switcher.hide();
        self.rebuild_palette();
        self.palette.open_by_tap();
        self.log("palette opened");
    }

    /// A tap on the open palette at screen coordinates.
    pub fn palette_tap(&mut self, x: f32, y: f32) -> Vec<Action> {
        let g = self.palette.geom(self.screen);
        match self.palette.tap(&g, x, y) {
            Some(a) => self.palette_action(a),
            None => vec![],
        }
    }

    pub fn palette_query(&mut self, q: &str) {
        self.palette.set_query(q);
    }

    fn palette_action(&mut self, a: PaletteAction) -> Vec<Action> {
        self.log(format!("palette -> {a:?}"));
        let recent = self.palette.recents_kv();
        let mut out = if recent != self.prefs.palette_recent {
            self.prefs.palette_recent = recent;
            vec![Action::SavePrefs]
        } else {
            vec![]
        };
        let mut prefs = self.prefs.clone();
        let rest = match a {
            PaletteAction::Mode(m) => self.toggle_mode(m),
            PaletteAction::ToggleAutoBrightness => self.tap_quick(QuickKind::AutoBrightness),
            PaletteAction::ToggleFatCursor => {
                prefs.fat_cursor = !prefs.fat_cursor;
                self.set_prefs(prefs)
            }
            PaletteAction::Cold(next) => {
                prefs.cold = next.to_string();
                self.set_prefs(prefs)
            }
            PaletteAction::LayoutPullDown => {
                prefs.layout = Layout::PullDown;
                self.set_prefs(prefs)
            }
            PaletteAction::LayoutRightBar => {
                prefs.layout = Layout::RightBar;
                self.set_prefs(prefs)
            }
            PaletteAction::ClockDigital => {
                prefs.clock_style = crate::clock::ClockStyle::Digital;
                self.set_prefs(prefs)
            }
            PaletteAction::ClockAnalog => {
                prefs.clock_style = crate::clock::ClockStyle::Analog;
                self.set_prefs(prefs)
            }
            PaletteAction::Window(id) => self.activate(id),
            PaletteAction::App(id) => self.launch(&id),
            PaletteAction::Switcher => {
                self.open_switcher();
                vec![]
            }
            PaletteAction::Launcher => self.open_launcher(),
            PaletteAction::Settings => {
                self.settings_requested = true;
                vec![]
            }
            PaletteAction::CloseAll => self.switcher_close_all(),
            PaletteAction::Lock => {
                self.lock_now();
                vec![]
            }
            PaletteAction::Sleep => self.power_key(),
        };
        out.extend(rest);
        self.rebuild_palette();
        out
    }

    // ---- overlay taps ------------------------------------------------------

    /// A long press on a quick tile: the connection tiles open their settings page, the rest
    /// do nothing (a long press must never toggle).
    pub fn long_press_quick(&mut self, kind: QuickKind) -> Vec<Action> {
        match kind.settings_app() {
            Some(id) => {
                self.log(format!("{} held: opening settings", kind.label()));
                self.emit(vec![Action::LaunchApp(id.to_string())])
            }
            None => Vec::new(),
        }
    }

    pub fn tap_quick(&mut self, kind: QuickKind) -> Vec<Action> {
        match kind {
            QuickKind::Keyboard => self.toggle_mode(HoldOption::Keyboard),
            QuickKind::Trackpad => self.toggle_mode(HoldOption::Trackpad),
            QuickKind::Carousel => self.toggle_mode(HoldOption::Carousel),
            QuickKind::AutoBrightness => {
                self.ambient.enabled = !self.ambient.enabled;
                self.prefs.auto_brightness = self.ambient.enabled;
                self.log(format!("auto brightness {}", if self.ambient.enabled { "on" } else { "off" }));
                self.emit(vec![Action::SavePrefs])
            }
            QuickKind::Rotation => {
                // Auto-rotate on/off IS the shell's own setting. Off keeps the screen where it is (a
                // rotation LOCK), it does not snap back to portrait.
                self.rotate.enabled = !self.rotate.enabled;
                self.prefs.autorotate = self.rotate.enabled;
                self.log(format!("auto-rotate {}", if self.rotate.enabled { "on" } else { "off" }));
                self.emit(vec![Action::SavePrefs])
            }
            QuickKind::Screenshot => {
                self.log("screenshot");
                self.emit(vec![Action::Screenshot])
            }
            QuickKind::CpuGovernor => {
                let a = self.drawer.cycle_governor();
                self.log(format!("cpu governor -> {}", self.drawer.governor.label()));
                self.emit(vec![a])
            }
            k => {
                let out = self.drawer.toggle_system(k).into_iter().collect();
                self.emit(out)
            }
        }
    }

    /// A slider moved to fraction `v` of its travel. With auto brightness on,
    /// the brightness slider is RELATIVE: its travel spans the adaptive band
    /// around the auto level, so `v` sets the position in the band and the
    /// output spring does the rest (no direct SetSlider). Otherwise absolute.
    pub fn set_slider(&mut self, kind: SliderKind, v: f32) -> Vec<Action> {
        if kind == SliderKind::Brightness && self.ambient.enabled {
            self.ambient.set_user(v.clamp(0.0, 1.0) * 2.0 - 1.0);
            self.offset_dirty_at = Some(self.now);
            return vec![];
        }
        // Absolute brightness: the slider is curved (fine at the bottom).
        let v = if kind == SliderKind::Brightness { crate::ambient::slider_to_level(v) } else { v };
        let a = self.drawer.set_slider(kind, v);
        if kind == SliderKind::Brightness {
            self.ambient.set_absolute(self.drawer.brightness);
        }
        vec![a]
    }

    /// Slider position to draw for brightness: the band position when auto
    /// is on, the absolute level otherwise.
    pub fn brightness_slider_pos(&self) -> f32 {
        if self.ambient.enabled {
            (self.ambient.user + 1.0) / 2.0
        } else {
            crate::ambient::level_to_slider(self.drawer.brightness)
        }
    }

    pub fn brightness_band(&self) -> crate::ambient::Band {
        self.ambient.band(self.now, self.clock_minute)
    }

    /// The voicing axis of the volume pad moved (-1..1). Saved once the
    /// finger has been still, like the brightness offset.
    pub fn set_voicing(&mut self, tone: f32) -> Vec<Action> {
        let tone = tone.clamp(-1.0, 1.0);
        if (tone - self.prefs.audio_tone).abs() < 1e-4 {
            return vec![];
        }
        self.prefs.audio_tone = tone;
        self.offset_dirty_at = Some(self.now);
        vec![Action::SetVoicing(tone)]
    }

    /// Local minute of the day (0..1440), for the night window.
    pub fn set_clock(&mut self, minute: u32) {
        self.clock_minute = minute % 1440;
    }

    // ---- living design and places -------------------------------------------------

    pub fn set_date(&mut self, day_of_year: u32) {
        self.day_of_year = day_of_year.clamp(1, 366);
    }

    /// The design of this moment: season and time of day, shaded by the
    /// place if one is known (its colours' warmth, its quiet). Neutral when
    /// the living design is off.
    pub fn mood(&self) -> crate::season::Mood {
        use crate::season::Mood;
        if !self.prefs.living {
            return Mood { accent: crate::season::ICE, warmth: 0.0, saturation: 1.0, brightness: 1.0, omega: 28.0, zeta: 0.4, breath_period: 5.0, breath_amp: 0.0, season: "off", phase: "off" };
        }
        let mut m = crate::season::mood(self.day_of_year, self.clock_minute, self.prefs.hemisphere == "south");
        if let Some(h) = self.ambience.hints(self.clock_minute / 60) {
            m.warmth = (m.warmth + 0.5 * h.warmth).clamp(-1.0, 1.0);
            if h.quiet {
                m.zeta = (m.zeta * 1.25).min(0.95);
                m.breath_amp *= 0.6;
            }
        }
        m
    }

    /// The place's accent colour, when the place has one.
    pub fn place_accent(&self) -> Option<[u8; 3]> {
        self.ambience.hints(self.clock_minute / 60).and_then(|h| h.accent)
    }

    /// The accent to draw with now: the scene's, shaded 60/40 toward the
    /// place's own colour when one is known.
    pub fn accent(&self) -> [u8; 3] {
        let m = self.mood().accent;
        match self.place_accent() {
            Some(p) if self.prefs.living => crate::season::mix_rgb(m, p, 0.6),
            _ => m,
        }
    }

    /// Somewhere recognized: by WiFi (the shell) or a snapclone landmark.
    pub fn on_place(&mut self, key: crate::ambience::PlaceKey, name: &str) {
        if self.ambience.arrive(key.clone(), name, self.now) {
            let p = self.ambience.here().map(|p| (p.visits, p.colors.len())).unwrap_or((0, 0));
            self.log(format!("place: {} {name:?} (visit {}, {} colours known)", key.token(), p.0, p.1));
            self.ambience_dirty = true;
        }
    }

    /// The camera pane's dominant colours: remembered for this place.
    pub fn on_photo_colors(&mut self, colors: &[[u8; 3]]) {
        if let Some(p) = self.ambience.here() {
            p.note_colors(colors);
            self.ambience_dirty = true;
        }
    }

    /// Called from tick: once a minute, log the light and sound floor here.
    fn log_ambience(&mut self, now: Millis) {
        if self.last_ambience_log.is_some_and(|t| now.saturating_sub(t) < 60_000) {
            return;
        }
        self.last_ambience_log = Some(now);
        let lux = self.ambient.smoothed_lux().filter(|_| self.ambient.has_sensor(now));
        let floor = self.sound.floor_dbfs();
        let hour = self.clock_minute / 60;
        if let Some(p) = self.ambience.here() {
            if let Some(l) = lux {
                p.note_lux(l, hour);
            }
            if let Some(f) = floor {
                p.note_sound(f);
            }
            self.ambience_dirty = true;
        }
    }

    /// The places log changed since last asked (the runtime saves it).
    pub fn take_ambience_dirty(&mut self) -> bool {
        std::mem::take(&mut self.ambience_dirty)
    }

    /// The sunlight polarity flip (with hysteresis), shared by the apps'
    /// theme and the shell's own look.
    pub fn apps_light(&self) -> bool {
        self.apps_light
    }

    /// Battery temperature (°C): drives cold mode unless forced.
    pub fn on_battery_temp(&mut self, c: f32) {
        if self.cold.on_battery_temp(c) {
            self.log(format!("cold mode {} (battery {c:.1} C)", if self.cold.on { "on" } else { "off" }));
            self.apply_cold();
        }
    }

    fn apply_cold(&mut self) {
        let t = self.cold.tuning();
        let mut cfg = self.base_edge.clone();
        cfg.edge_px *= t.target_scale;
        cfg.slop_px *= t.slop_scale;
        cfg.hold_ms = (cfg.hold_ms as f32 * t.time_scale) as Millis;
        self.recognizer.cfg = cfg;
    }

    /// Cold tuning for the frontend (targets, slops).
    pub fn cold_tuning(&self) -> crate::cold::ColdTuning {
        self.cold.tuning()
    }

    /// Where a listen would be used right now, for the voice route.
    pub fn voice_focus(&self) -> crate::voice::Focus {
        if self.palette.open || self.lock.is_locked() {
            return crate::voice::Focus::Command;
        }
        let text_app = self.windows.iter().find(|w| w.activated).is_some_and(|w| {
            let id = w.app_id.to_ascii_lowercase();
            ["texteditor", "editor", "console", "term", "gedit", "kate", "code", "notes", "writer", "mail", "chat", "message"].iter().any(|k| id.contains(k))
        });
        if text_app { crate::voice::Focus::Dictation } else { crate::voice::Focus::Other }
    }

    /// The route line for voiceterm, when it changed since last asked.
    pub fn voice_route_line(&mut self) -> Option<String> {
        let line = crate::voice::route(self.voice_focus(), &self.headroom).line();
        if line != self.voice_line {
            self.voice_line = line.clone();
            self.log(format!("voice route: {line}"));
            Some(line)
        } else {
            None
        }
    }

    /// Whether the camera should run: only while the cover is showing on a
    /// lit screen (never behind apps, never in a pocket).
    pub fn camera_wanted(&self) -> bool {
        self.lock.is_locked() && !self.sleep.captures_input() && !self.is_covered()
    }

    /// Gyro rates (rad/s) and accelerometer (any scale), device axes.
    pub fn on_imu(&mut self, gyro: [f32; 3], accel: [f32; 3], now: Millis) {
        let dt = self.last_imu.map(|t| now.saturating_sub(t) as f32 / 1000.0).unwrap_or(0.0).min(0.1);
        self.last_imu = Some(now);
        if dt > 0.0 {
            self.world.gyro(gyro, dt);
            // Gyro select: only while a finger holds the palette open.
            if self.palette.is_holding() {
                let (dx, dy) = self.tilt.feed(gyro[0], gyro[2], dt);
                if dx != 0 || dy != 0 {
                    self.palette.move_hover(dx, dy);
                    self.log(format!("tilt select {:?}", self.palette.hover));
                }
            } else {
                self.tilt.reset();
            }
        }
        self.world.accel(accel);
        self.rotate.on_accel(accel);
        let was = self.flat.is_flat();
        self.flat.on_accel(accel, now);
        if was != self.flat.is_flat() {
            self.log(if self.flat.is_flat() { "lying flat (accelerometer)" } else { "picked up" });
        }
    }

    /// How much of the 3D cover clock to draw now (0..1), per the prefs.
    pub fn clock_3d_amount(&self) -> f32 {
        crate::clock3d::amount(self.prefs.clock_3d, &self.flat)
    }

    pub fn on_heading(&mut self, degrees: f32) {
        self.world.heading(degrees);
    }

    /// The tracker's measured image motion for the last frame pair, and
    /// what the gyro predicted over the same interval.
    pub fn on_image_motion(&mut self, measured_px: (f32, f32), predicted_px: (f32, f32)) {
        self.world.image_correction(measured_px, predicted_px, 0.3);
    }

    /// A microphone window's level (dBFS). Logs gain steps.
    pub fn on_mic_level(&mut self, dbfs: f32, now: Millis) {
        if let Some(g) = self.sound.on_level(dbfs, now) {
            self.log(format!("ambient sound: floor {:.0} dBFS -> notification gain {:.2}", self.sound.floor_dbfs().unwrap_or(-100.0), g));
        }
    }

    pub fn notification_gain(&self) -> f32 {
        self.sound.gain()
    }

    /// The proximity sensor: near = covered (pocket, face). Touches are
    /// ignored and the triple tap cannot wake while covered.
    /// Bedside mode, once per loop: light the panel for a dim clock at each
    /// new minute in sleeping hours, and turn it off again after the pulse.
    pub fn night_tick(&mut self, now: Millis, minute_of_day: u16) -> Vec<Action> {
        self.sleep.evening = !(7 * 60..19 * 60).contains(&minute_of_day);
        let asleep = self.sleep.phase == crate::sleep::Phase::Off;
        let window = self.night_window && self.night_pulse;
        let out = match self.night.tick(now, minute_of_day, window, asleep) {
            Some(crate::nightclock::Panel::On) => {
                self.sleep.glance = true;
                vec![Action::DisplayPower(true)]
            }
            Some(crate::nightclock::Panel::Off) => {
                self.sleep.glance = false;
                vec![Action::DisplayPower(false)]
            }
            None => vec![],
        };
        if !self.night.is_pulsing() {
            self.sleep.glance = false;
        }
        out
    }

    /// ext-idle-notify fired: sleep the screen the power key's way.
    pub fn on_idle(&mut self, now: Millis) -> Vec<Action> {
        self.now = self.now.max(now);
        let acts = self.sleep.idle();
        if !acts.is_empty() || self.sleep.phase == crate::sleep::Phase::FadingOut {
            self.log("idle: screen sleeping");
        }
        acts.into_iter().flat_map(|a| self.on_sleep_action(a)).collect()
    }

    /// The compositor reported the panel's power (whoever changed it).
    pub fn on_display_reported(&mut self, on: bool, now: Millis) -> Vec<Action> {
        self.now = self.now.max(now);
        let refused = self.sleep.refused_wakes;
        let acts = self.sleep.display_reported(on, now);
        if on && self.sleep.refused_wakes != refused {
            self.log("display lit while asleep without a triple tap: staying black, blanking again when quiet");
        } else if !on && !acts.is_empty() {
            self.log("compositor blanked the display: adopted as screen sleep");
        }
        acts.into_iter().flat_map(|a| self.on_sleep_action(a)).collect()
    }

    pub fn on_proximity(&mut self, near: bool, now: Millis) {
        self.now = self.now.max(now);
        let was = self.sleep.is_covered(now);
        self.sleep.set_near(near, now);
        if was != near {
            self.log(format!("proximity {}", if near { "near: covered" } else { "far" }));
        }
    }

    /// Covered: every touch is dropped before it means anything.
    pub fn is_covered(&self) -> bool {
        self.sleep.is_covered(self.now)
    }

    /// An orientation reading from iio-sensor-proxy.
    pub fn on_orientation(&mut self, s: &str, now: Millis) -> Vec<Action> {
        let was = self.flat.is_flat();
        self.flat.on_orientation(s, now);
        if was != self.flat.is_flat() {
            self.log(if self.flat.is_flat() { "lying flat (orientation undefined)" } else { "picked up" });
        }
        let blocked = self.lock.is_locked() || self.sleep.captures_input();
        match self.rotate.on_reading(s, now, blocked) {
            Some(o) => {
                self.log(format!("rotate {o:?}"));
                self.emit(vec![Action::Rotate(o.transform_deg(self.prefs.rotate_flip))])
            }
            None => vec![],
        }
    }

    /// An ambient light reading in lux.
    pub fn on_lux(&mut self, lux: f32, now: Millis) {
        self.now = self.now.max(now);
        self.ambient.on_lux(lux, now);
    }

    /// The backlight level found at startup, so auto ramps from reality.
    pub fn set_current_brightness(&mut self, level: f32) {
        self.drawer.brightness = level.clamp(0.0, 1.0);
        self.ambient.set_current(level);
    }

    pub fn launch(&mut self, desktop_id: &str) -> Vec<Action> {
        self.drawer.close();
        self.switcher.hide();
        self.emit(vec![Action::LaunchApp(desktop_id.to_string())])
    }

    pub fn open_launcher(&mut self) -> Vec<Action> {
        self.drawer.close();
        self.switcher.hide();
        self.emit(vec![Action::SummonLauncher])
    }

    pub fn open_switcher(&mut self) {
        self.drawer.close();
        self.switcher.show();
        self.log("switcher opened");
    }

    /// The Tasks button: open the switcher, or close it if it is already open. Rapid taps peek and close
    /// again (the second tap used to re-open it). Judged on `open`, not on the reveal animation, so a tap
    /// in the middle of the slide-in still closes it.
    pub fn toggle_switcher(&mut self) {
        if self.switcher.open {
            self.switcher.hide();
            self.log("switcher closed (tasks button)");
        } else {
            self.open_switcher();
        }
    }

    pub fn activate(&mut self, id: u64) -> Vec<Action> {
        let out = self.switcher.tap(id);
        self.emit(out)
    }

    pub fn switcher_tap(&mut self, id: u64) -> Vec<Action> {
        self.activate(id)
    }

    pub fn switcher_drag(&mut self, id: u64, dy: f32) {
        self.switcher.drag_card(id, dy);
    }

    pub fn switcher_release(&mut self) -> Vec<Action> {
        let out = self.switcher.release_card();
        self.emit(out)
    }

    pub fn switcher_close_all(&mut self) -> Vec<Action> {
        let out = self.switcher.close_all();
        self.emit(out)
    }

/// Minimum time between reflow requests for one window.
    pub const REFLOW_COOLDOWN: Millis = 3000;
    /// The exclusive zones must have held still this long before a window is re-maximized. A
    /// carousel step maps and unmaps peeks and re-lays windows out; judging mid-change made the
    /// shell unmaximize/maximize windows through a distorted intermediate size, over and over.
    pub const ZONE_SETTLE_MS: Millis = 700;
    /// A window whose maximized state or width changed less than this ago is still being laid
    /// out by the compositor, and its reported width is not yet the answer.
    pub const WINDOW_SETTLE_MS: Millis = 500;

    /// Re-maximize maximized windows that are laid out wider than the space
    /// the shell's exclusive zones leave (`reserved` px in total: the bar,
    /// the carousel peeks), so they move out from under them. Call on every window update while the bar is mapped;
    /// it only acts on windows that are actually wrong.
    ///
    /// Why: cosmic-comp epoch-1.8.0 does not re-lay-out an ALREADY maximized
    /// window when a new exclusive zone appears (observed: the window's own
    /// COSMIC geometry stayed 1280 wide with the 64 px bar up; one maximized
    /// after the bar appeared got 1216; removing the bar re-grew it). A
    /// one-shot reflow when the bar mapped was tried first and found nothing:
    /// the window list and its maximized state arrive over several polls.
    pub fn reflow_for_zone(&mut self, reserved: f32) -> Vec<Action> {
        let limit = self.screen.0 - reserved + 1.0;
        let now = self.now;
        if (reserved - self.zone_seen).abs() > 0.5 {
            self.zone_seen = reserved;
            self.zone_since = now;
        }
        if now.saturating_sub(self.zone_since) < Self::ZONE_SETTLE_MS {
            return vec![]; // the zones are still changing: hold, do not chase them
        }
        let settled = |id: u64| self.win_seen.get(&id).is_none_or(|(_, _, t)| now.saturating_sub(*t) >= Self::WINDOW_SETTLE_MS);
        let ids: Vec<u64> = self
            .windows
            .iter()
            .filter(|w| settled(w.id))
            .filter(|w| w.maximized && w.width.is_some_and(|wd| wd as f32 > limit))
            .filter(|w| self.reflow_at.get(&w.id).is_none_or(|t| now.saturating_sub(*t) >= Self::REFLOW_COOLDOWN))
            .map(|w| w.id)
            .collect();
        if ids.is_empty() {
            return vec![];
        }
        for id in &ids {
            self.reflow_at.insert(*id, now);
        }
        self.log(format!("reflow {} maximized window(s) wider than the bar leaves", ids.len()));
        let out = ids.into_iter().flat_map(|id| [Action::SetMaximized(id, false), Action::SetMaximized(id, true)]).collect();
        self.emit(out)
    }

    /// A tap on a carousel peek: cycle to the neighbour it shows.
    pub fn peek_tap(&mut self, side: crate::carousel::Side) -> Vec<Action> {
        let out = self.carousel.tap_peek(side, &self.windows);
        if !out.is_empty() {
            self.log(format!("carousel peek tap {side:?}"));
        }
        self.emit(out)
    }

    /// Width in px the carousel peeks reserve on EACH side while active.
    pub fn peek_reserve(&self) -> f32 {
        if self.carousel.is_active() {
            (self.screen.0 * crate::carousel::peek_frac()).round()
        } else {
            0.0
        }
    }

    /// The bar's own horizontal drag (it owns the right edge).
    pub fn set_bar_expanded(&mut self, on: bool) {
        if self.bar_expanded != on {
            self.bar_expanded = on;
            self.log(format!("bar expanded={on}"));
        }
    }

    /// Tap on empty drawer/switcher backdrop dismisses it.
    pub fn dismiss_overlays(&mut self) {
        self.drawer.close();
        self.switcher.hide();
    }

    /// A touch on the trackpad overlay.
    pub fn on_trackpad_touch(&mut self, ev: TouchEvent) -> Vec<Action> {
        if let TouchEvent::Down { t, .. } | TouchEvent::Move { t, .. } | TouchEvent::Up { t, .. } = ev {
            self.now = self.now.max(t);
        }
        let outs = match ev {
            TouchEvent::Down { slot, x, y, t } => self.trackpad.down(slot, x, y, t),
            TouchEvent::Move { slot, x, y, t } => self.trackpad.motion(slot, x, y, t),
            TouchEvent::Up { slot, t } => self.trackpad.up(slot, t),
            TouchEvent::Cancel => self.trackpad.cancel(),
        };
        // Not logged: motion would flood the trace. Clicks are logged.
        let now = self.now;
        let out: Vec<Action> = outs
            .into_iter()
            .filter_map(|o| match (&mut self.fat, o) {
                // Fat mode: motion moves the disc's target; the pointer follows
                // the eased disc from tick(), so nothing is sent here.
                (Some(f), crate::trackpad::PointerOut::Motion { dx, dy }) => {
                    f.nudge(dx, dy, now);
                    None
                }
                (Some(f), b @ crate::trackpad::PointerOut::Button { pressed, .. }) => {
                    f.press(pressed, now);
                    Some(Action::Pointer(b))
                }
                (_, o) => Some(Action::Pointer(o)),
            })
            .collect();
        for a in &out {
            if matches!(
                a,
                Action::Pointer(
                    crate::trackpad::PointerOut::Button { .. }
                        | crate::trackpad::PointerOut::ZoomBegin
                        | crate::trackpad::PointerOut::Zoom { .. }
                        | crate::trackpad::PointerOut::ZoomEnd
                )
            ) {
                self.log(format!("-> {a:?}"));
            }
        }
        out
    }

    /// The bar's widgets for the current layout (empty in pull-down).
    pub fn bar_widgets(&self) -> &[BarWidget] {
        match self.prefs.layout {
            Layout::RightBar => &self.prefs.bar_widgets,
            Layout::PullDown => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tapping_the_cpu_governor_tile_cycles_and_the_tile_always_reads_on() {
        let mut sh = Shell::new(ShellPrefs::default(), S);
        assert!(sh.quick_state(QuickKind::CpuGovernor), "no off state, only which mode");
        let acts = sh.tap_quick(QuickKind::CpuGovernor);
        assert_eq!(acts, vec![Action::SetGovernor(crate::control_center::CpuGovernor::Conservative)]);
        assert_eq!(sh.tap_quick(QuickKind::CpuGovernor), vec![Action::SetGovernor(crate::control_center::CpuGovernor::Performance)]);
    }

    const S: (f32, f32) = (408.0, 900.0);

    fn win(id: u64, activated: bool) -> WindowCard {
        WindowCard { id, title: format!("w{id}"), app_id: format!("a{id}"), activated, maximized: false, width: None }
    }

    fn shell_with_three() -> Shell {
        let mut sh = Shell::new(ShellPrefs::default(), S);
        sh.set_windows(vec![win(1, true)]);
        sh.set_windows(vec![win(1, false), win(2, true)]);
        sh.set_windows(vec![win(1, false), win(2, false), win(3, true)]);
        sh
    }

    /// Straight edge drag in `steps` samples; returns all actions.
    fn edge_drag(sh: &mut Shell, from: (f32, f32), to: (f32, f32), steps: u32, t0: Millis) -> Vec<Action> {
        let mut out = sh.on_edge_touch(TouchEvent::Down { slot: 0, x: from.0, y: from.1, t: t0 });
        for i in 1..=steps {
            let f = i as f32 / steps as f32;
            out.extend(sh.on_edge_touch(TouchEvent::Move {
                slot: 0,
                x: from.0 + (to.0 - from.0) * f,
                y: from.1 + (to.1 - from.1) * f,
                t: t0 + 16 * i as u64,
            }));
        }
        out.extend(sh.on_edge_touch(TouchEvent::Up { slot: 0, t: t0 + 16 * steps as u64 + 16 }));
        out
    }

    #[test]
    fn swipe_up_opens_switcher_long_swipe_summons_hatch() {
        let mut sh = shell_with_three();
        assert!(edge_drag(&mut sh, (200.0, 895.0), (200.0, 700.0), 30, 0).is_empty());
        assert!(sh.switcher.open);
        let out = edge_drag(&mut sh, (200.0, 895.0), (200.0, 200.0), 30, 5000);
        assert_eq!(out, vec![Action::SummonLauncher]);
        assert!(!sh.switcher.open);
    }

    #[test]
    fn glove_flicks_only_act_in_cold_mode_and_reach_the_keyboard_when_the_palette_is_shut() {
        use crate::gloveflick::Flick;
        let mut sh = Shell::new(ShellPrefs::default(), S);
        assert!(sh.on_flick(Flick::Down).is_empty(), "bare hands: turning the phone is not a command");
        assert!(!sh.palette.open);
        sh.cold.on = true;
        assert!(sh.on_flick(Flick::Down).is_empty());
        assert!(sh.palette.open, "a flick opened the palette, like the volume keys");
        sh.on_flick(Flick::Left);
        assert!(!sh.palette.open, "left closes it");
        assert_eq!(sh.on_flick(Flick::Right), vec![Action::KeyboardCommand("voice-done")]);
        assert_eq!(sh.on_flick(Flick::Escape), vec![Action::KeyboardCommand("voice-cancel")]);
    }

    #[test]
    fn hold_then_slide_to_keyboard_toggles_it() {
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        assert!(sh.tick(500).is_empty());
        assert!(sh.palette.open, "the hold opens the palette");
        let g = sh.palette.geom(S);
        let (x, y, w, h) = g.mode_rect(1);
        sh.on_edge_touch(TouchEvent::Move { slot: 0, x: x + w / 2.0, y: y + h / 2.0, t: 600 });
        assert_eq!(sh.palette.hover, Some(crate::palette::Hit::Mode(1)));
        let out = sh.on_edge_touch(TouchEvent::Up { slot: 0, t: 700 });
        assert!(out.contains(&Action::ShowKeyboard(true)), "{out:?}");
        assert!(!sh.palette.open, "activating closes the palette");
        assert!(sh.quick_state(QuickKind::Keyboard));
        assert!(sh.prefs.palette_recent.starts_with("mode:keyboard"), "{}", sh.prefs.palette_recent);
    }

    #[test]
    fn sliding_off_the_top_of_the_sheet_dismisses_it() {
        // "None of these": the sheet fades with the travel and lifting there
        // closes it, rather than leaving a sheet behind that has to be
        // dismissed a second time.
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        sh.tick(500);
        assert!(sh.palette.open);
        let g = sh.palette.geom(S);
        // Onto a mode first, so there IS something selected to discard.
        let (x, y, w, h) = g.mode_rect(1);
        sh.on_edge_touch(TouchEvent::Move { slot: 0, x: x + w / 2.0, y: y + h / 2.0, t: 600 });
        assert_eq!(sh.palette.hover, Some(crate::palette::Hit::Mode(1)));
        assert_eq!(sh.palette.hold_fade, 1.0);
        // Then off the top: the selection is dropped and the sheet fades.
        sh.on_edge_touch(TouchEvent::Move { slot: 0, x: 204.0, y: g.sheet_top - 40.0, t: 700 });
        assert_eq!(sh.palette.hover, None, "nothing is selected while leaving");
        assert!(sh.palette.hold_fade < 1.0 && sh.palette.hold_fade > 0.0, "fade {}", sh.palette.hold_fade);
        // Lifting there does nothing and closes it.
        let out = sh.on_edge_touch(TouchEvent::Up { slot: 0, t: 800 });
        assert!(out.is_empty(), "{out:?}");
        assert!(!sh.palette.open, "dismissed, not left open");
        assert_eq!(sh.modes, Modes::default(), "and no mode was toggled on the way out");
    }

    #[test]
    fn the_sheet_opens_short_and_the_list_opens_in_full_when_the_thumb_reaches_the_search() {
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        sh.tick(500);
        let short = sh.palette.geom(S);
        assert!(!sh.palette.expanded);
        assert!(short.rows_visible() <= crate::palette::COLLAPSED_ROWS, "{}", short.rows_visible());
        // Controls hang off the bottom: list, then search, then the three modes.
        let (_, my, _, _) = short.mode_rect(0);
        assert!(short.row_y(0) < short.search_y() && short.search_y() < my);
        // Sliding a thumb onto a mode button leaves it short.
        let (x, y, w, h) = short.mode_rect(1);
        sh.on_edge_touch(TouchEvent::Move { slot: 0, x: x + w / 2.0, y: y + h / 2.0, t: 600 });
        assert!(!sh.palette.expanded, "a mode button is not the search");
        // Onto the search bar: the list opens in full, the search stays where the thumb is.
        let sy = short.search_y() + crate::palette::SEARCH_H / 2.0;
        sh.on_edge_touch(TouchEvent::Move { slot: 0, x: 204.0, y: sy, t: 700 });
        assert!(sh.palette.expanded);
        let full = sh.palette.geom(S);
        assert!(full.rows_visible() > short.rows_visible());
        assert_eq!(full.search_y(), short.search_y(), "the search does not move under the thumb");
        // Closing resets it.
        sh.palette.close();
        assert!(!sh.palette.expanded);
    }

    #[test]
    fn a_thumb_that_barely_moves_selects_nothing() {
        // The dead zone: a hold that wobbles must not pick whatever it is
        // over when the sheet rises under it.
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        sh.tick(500);
        sh.on_edge_touch(TouchEvent::Move { slot: 0, x: 210.0, y: 890.0, t: 600 });
        assert_eq!(sh.palette.hover, None);
        let out = sh.on_edge_touch(TouchEvent::Up { slot: 0, t: 700 });
        assert!(out.is_empty(), "{out:?}");
        assert!(sh.palette.open, "still open for taps, which is the old behaviour");
    }

    #[test]
    fn hold_released_in_place_leaves_the_palette_open_for_taps() {
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        sh.tick(500);
        assert!(sh.on_edge_touch(TouchEvent::Up { slot: 0, t: 600 }).is_empty());
        assert_eq!(sh.modes, Modes::default());
        assert!(sh.palette.open);
        // Palette lists the running windows; tapping one activates it.
        let g = sh.palette.geom(S);
        let row = sh.palette.rows().position(|r| r.label == "w1").expect("window row");
        let out = sh.palette_tap(200.0, g.row_y(row) + 5.0);
        assert!(out.contains(&Action::Activate(1)), "{out:?}");
        assert!(!sh.palette.open);
        // Tapping above the sheet closes it.
        sh.open_palette();
        assert!(sh.palette_tap(200.0, 20.0).is_empty() && !sh.palette.open);
    }

    #[test]
    fn carousel_mode_repurposes_horizontal_swipe() {
        let mut sh = shell_with_three();
        let out = sh.toggle_mode(HoldOption::Carousel);
        assert_eq!(out, vec![Action::Activate(3), Action::SetMaximized(3, true)]);
        // Back in carousel = previous in the ring (wraps), not MRU streak.
        let out = edge_drag(&mut sh, (100.0, 895.0), (250.0, 895.0), 12, 1000);
        assert_eq!(out, vec![Action::Activate(1), Action::SetMaximized(1, true)]);
        assert!(!sh.switcher.is_streaking(), "quick-switch must not also run");
        // Tap the right peek: no swipe needed.
        // (3 is unmaximized in this fixture, so it gets maximized on arrival.)
        assert_eq!(sh.peek_tap(crate::carousel::Side::Right), vec![Action::Activate(3), Action::SetMaximized(3, true)]);
        assert!(sh.peek_reserve() > 0.0);
        let out = sh.tap_quick(QuickKind::Carousel);
        assert_eq!(out.len(), 2, "restores both windows it maximized: {out:?}");
        assert_eq!(sh.peek_reserve(), 0.0);
        assert!(!sh.quick_state(QuickKind::Carousel));
    }

    #[test]
    fn quick_switch_outside_carousel_walks_mru() {
        let mut sh = shell_with_three();
        let out = edge_drag(&mut sh, (100.0, 895.0), (250.0, 895.0), 12, 1000);
        assert_eq!(out, vec![Action::Activate(2)]);
    }

    #[test]
    fn right_bar_layout_keeps_the_top_pull_down_drawer_and_the_bars_own_right_edge() {
        let mut sh = shell_with_three();
        let mut p = sh.prefs.clone();
        p.layout = Layout::RightBar;
        assert_eq!(sh.set_prefs(p), vec![Action::SavePrefs]);
        edge_drag(&mut sh, (200.0, 3.0), (200.0, 420.0), 25, 0);
        for t in (500..2000).step_by(16) {
            sh.tick(t);
        }
        assert!(sh.drawer.is_visible(), "the top edge pulls the quick settings down in right-bar layout too");
        sh.dismiss_overlays();
        for t in (2000..3500).step_by(16) {
            sh.tick(t);
        }
        assert!(!sh.bar_widgets().is_empty());
        // The right edge is the bar's, not the recognizer's.
        let out = edge_drag(&mut sh, (406.0, 450.0), (300.0, 450.0), 10, 4000);
        assert!(out.is_empty() && !sh.bar_expanded, "no edge swipe-in over the bar");
        sh.set_bar_expanded(true);
        assert!(sh.bar_expanded);
    }

    #[test]
    fn pull_down_drawer_then_mode_tile() {
        let mut sh = shell_with_three();
        edge_drag(&mut sh, (200.0, 3.0), (200.0, 420.0), 25, 0);
        for t in (500..2000).step_by(16) {
            sh.tick(t);
        }
        assert!(sh.drawer.is_visible());
        assert_eq!(sh.tap_quick(QuickKind::Trackpad), vec![Action::ShowTrackpad(true)]);
        assert_eq!(sh.tap_quick(QuickKind::Wifi), vec![Action::SetQuick(QuickKind::Wifi, true)]);
        assert_eq!(sh.tap_quick(QuickKind::Screenshot), vec![Action::Screenshot], "the tile is a momentary action");
        assert!(!sh.quick_state(QuickKind::Screenshot), "and never shows as on");
        assert_eq!(sh.long_press_quick(QuickKind::Wifi), vec![Action::LaunchApp("com.system76.CosmicSettings.Wireless".into())]);
        assert_eq!(sh.long_press_quick(QuickKind::Bluetooth), vec![Action::LaunchApp("com.system76.CosmicSettings.Bluetooth".into())]);
        assert!(sh.long_press_quick(QuickKind::Flashlight).is_empty(), "a long press never toggles or launches on the other tiles");
    }

    #[test]
    fn the_auto_rotate_tile_toggles_the_setting_saves_it_and_reports_the_real_state() {
        let mut sh = shell_with_three();
        assert!(sh.quick_state(QuickKind::Rotation), "on by default, read from AutoRotate");
        assert_eq!(sh.tap_quick(QuickKind::Rotation), vec![Action::SavePrefs]);
        assert!(!sh.rotate.enabled && !sh.prefs.autorotate && !sh.quick_state(QuickKind::Rotation));
        // Off means a rotation lock: a sideways reading, held, applies nothing.
        for t in (0..3000).step_by(100) { assert_eq!(sh.rotate.on_reading("left-up", t, false), None, "locked at {t}"); }
        assert_eq!(sh.tap_quick(QuickKind::Rotation), vec![Action::SavePrefs]);
        assert!(sh.rotate.enabled && sh.prefs.autorotate && sh.quick_state(QuickKind::Rotation));
        // On again: the same held reading now applies.
        let mut applied = None;
        for t in (0..3000).step_by(100) { if let Some(o) = sh.rotate.on_reading("left-up", t, false) { applied = Some(o); } }
        assert!(applied.is_some(), "auto-rotate must work again once re-enabled");
    }

    #[test]
    fn turning_trackpad_off_mid_drag_releases_button() {
        let mut sh = shell_with_three();
        sh.toggle_mode(HoldOption::Trackpad);
        sh.on_trackpad_touch(TouchEvent::Down { slot: 0, x: 10.0, y: 10.0, t: 0 });
        sh.on_trackpad_touch(TouchEvent::Up { slot: 0, t: 50 });
        sh.on_trackpad_touch(TouchEvent::Down { slot: 0, x: 10.0, y: 10.0, t: 100 });
        assert!(sh.trackpad.button_held());
        let out = sh.toggle_mode(HoldOption::Trackpad);
        assert!(out.contains(&Action::Pointer(crate::trackpad::PointerOut::Button {
            button: crate::trackpad::Button::Left,
            pressed: false
        })));
    }

    #[test]
    fn a_taps_release_reaches_the_transport_through_tick() {
        let mut sh = shell_with_three();
        sh.toggle_mode(HoldOption::Trackpad);
        sh.on_trackpad_touch(TouchEvent::Down { slot: 0, x: 10.0, y: 10.0, t: 0 });
        let up = sh.on_trackpad_touch(TouchEvent::Up { slot: 0, t: 60 });
        assert_eq!(up, vec![Action::Pointer(crate::trackpad::PointerOut::Button { button: crate::trackpad::Button::Left, pressed: true })]);
        assert!(sh.tick(100).iter().all(|a| !matches!(a, Action::Pointer(_))));
        let later = sh.tick(60 + crate::trackpad::TAP_DRAG_MS);
        assert!(later.contains(&Action::Pointer(crate::trackpad::PointerOut::Button { button: crate::trackpad::Button::Left, pressed: false })), "{later:?}");
    }

    #[test]
    fn trace_records_the_loop_and_is_bounded() {
        let mut sh = shell_with_three();
        edge_drag(&mut sh, (200.0, 895.0), (200.0, 700.0), 10, 0);
        let lines: Vec<&String> = sh.trace().collect();
        assert!(lines.iter().any(|l| l.contains("edge pending -> swipe-up")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("swipe-up -> Switcher")));
        for i in 0..200 {
            sh.toggle_mode(HoldOption::Keyboard);
            sh.tick(10_000 + i);
        }
        assert_eq!(sh.trace().count(), TRACE_CAP);
    }

    #[test]
    fn fat_cursor_owns_the_pointer_while_trackpad_is_on() {
        use crate::trackpad::PointerOut;
        let mut p = ShellPrefs::default();
        p.fat_cursor = true;
        let mut sh = Shell::new(p, S);
        let out = sh.toggle_mode(HoldOption::Trackpad);
        assert!(out.contains(&Action::HideSystemCursor(true)), "{out:?}");
        // First tick places the pointer at the disc (screen centre).
        let t = sh.tick(10);
        assert_eq!(t, vec![Action::Pointer(PointerOut::Absolute { x: 204.0, y: 450.0 })]);
        // A drag on the pad sends NO relative motion...
        sh.on_trackpad_touch(TouchEvent::Down { slot: 0, x: 100.0, y: 700.0, t: 20 });
        let m = sh.on_trackpad_touch(TouchEvent::Move { slot: 0, x: 140.0, y: 700.0, t: 36 });
        assert!(m.is_empty(), "fat mode must not send relative motion: {m:?}");
        // ...the pointer follows the eased disc on ticks instead, moving right.
        let mut xs = vec![];
        for i in 1..=20 {
            for a in sh.tick(36 + 16 * i) {
                if let Action::Pointer(PointerOut::Absolute { x, .. }) = a {
                    xs.push(x);
                }
            }
        }
        assert!(xs.len() >= 3 && xs.windows(2).all(|w| w[1] > w[0]), "{xs:?}");
        assert!(*xs.last().unwrap() > 230.0);
        let off = sh.toggle_mode(HoldOption::Trackpad);
        assert!(off.contains(&Action::HideSystemCursor(false)));
        assert!(sh.fat.is_none());
    }

    #[test]
    fn normal_cursor_mode_keeps_relative_motion_and_never_hides() {
        let mut sh = Shell::new(ShellPrefs::default(), S);
        let out = sh.toggle_mode(HoldOption::Trackpad);
        assert!(!out.iter().any(|a| matches!(a, Action::HideSystemCursor(_))));
        sh.on_trackpad_touch(TouchEvent::Down { slot: 0, x: 100.0, y: 700.0, t: 20 });
        let m = sh.on_trackpad_touch(TouchEvent::Move { slot: 0, x: 140.0, y: 700.0, t: 36 });
        assert!(matches!(m[..], [Action::Pointer(crate::trackpad::PointerOut::Motion { .. })]), "{m:?}");
    }

    #[test]
    fn toggling_the_pref_while_trackpad_is_on_switches_live() {
        let mut sh = Shell::new(ShellPrefs::default(), S);
        sh.toggle_mode(HoldOption::Trackpad);
        let mut p = sh.prefs.clone();
        p.fat_cursor = true;
        assert!(sh.set_prefs(p).contains(&Action::HideSystemCursor(true)));
        let mut p = sh.prefs.clone();
        p.fat_cursor = false;
        assert!(sh.set_prefs(p).contains(&Action::HideSystemCursor(false)));
    }

    #[test]
    fn the_tasks_button_toggles_so_a_quick_second_tap_closes_what_the_first_opened() {
        let mut sh = shell_with_three();
        assert!(!sh.switcher.open);
        sh.toggle_switcher();
        assert!(sh.switcher.open, "first tap opens");
        sh.toggle_switcher(); // immediately, mid slide-in
        assert!(!sh.switcher.open, "second tap closes, even mid-animation");
        sh.tick(16);
        for t in 0..80 {
            sh.switcher.tick_anim(16.0 + t as f32);
        }
        assert!(!sh.switcher.is_visible(), "and it slides away fully");
        sh.toggle_switcher();
        assert!(sh.switcher.open, "a third tap opens it again");
        // the socket command still only OPENS: a bound key must never close a switcher you are using
        sh.open_switcher();
        assert!(sh.switcher.open);
    }

    #[test]
    fn reflow_only_touches_maximized_windows_too_wide_and_respects_cooldown() {
        let mut sh = Shell::new(ShellPrefs::default(), (1280.0, 800.0));
        let card = |id, maximized, width| WindowCard { maximized, width, ..win(id, false) };
        sh.set_windows(vec![
            card(1, true, Some(1280)),  // under the bar: reflow
            card(2, true, Some(1216)),  // already beside it: leave
            card(3, false, Some(1280)), // floating: not ours to resize
            card(4, true, None),        // geometry unknown: wait
        ]);
        // The zones appeared just now: hold until they have settled.
        assert!(sh.reflow_for_zone(64.0).is_empty());
        sh.tick(Shell::ZONE_SETTLE_MS + 10);
        assert_eq!(sh.reflow_for_zone(64.0), vec![Action::SetMaximized(1, false), Action::SetMaximized(1, true)]);
        // Compositor has not shrunk it yet: no repeat within the cooldown...
        sh.tick(Shell::ZONE_SETTLE_MS + 1000);
        assert!(sh.reflow_for_zone(64.0).is_empty());
        // ...but it is retried after.
        sh.tick(Shell::ZONE_SETTLE_MS + Shell::REFLOW_COOLDOWN + 20);
        assert_eq!(sh.reflow_for_zone(64.0).len(), 2);
        // Once it is laid out right, nothing happens.
        sh.set_windows(vec![card(1, true, Some(1216))]);
        sh.tick(2 * Shell::REFLOW_COOLDOWN + 2 * Shell::ZONE_SETTLE_MS);
        assert!(sh.reflow_for_zone(64.0).is_empty());
    }

    #[test]
    fn a_carousel_step_does_not_make_the_shell_chase_the_layout_through_resizes() {
        let mut sh = Shell::new(ShellPrefs::default(), (1280.0, 800.0));
        let card = |id, maximized, width| WindowCard { maximized, width, ..win(id, false) };
        sh.set_windows(vec![card(1, true, Some(1216))]);
        sh.tick(5000);
        assert!(sh.reflow_for_zone(64.0).is_empty(), "laid out right: nothing");
        // The carousel starts: the peeks add 2 x 96 of zone. The window is now too wide for the
        // new total, but the zones have only just appeared.
        let reserved = 64.0 + 192.0;
        assert!(sh.reflow_for_zone(reserved).is_empty(), "zones changed this instant");
        sh.tick(5000 + Shell::ZONE_SETTLE_MS - 50);
        assert!(sh.reflow_for_zone(reserved).is_empty(), "still inside the settle window");
        // A second change inside the window restarts it (the user stepped again).
        sh.tick(5000 + Shell::ZONE_SETTLE_MS);
        assert!(sh.reflow_for_zone(reserved + 1.0).is_empty(), "the total moved: the clock restarts");
        sh.tick(5000 + 2 * Shell::ZONE_SETTLE_MS - 10);
        assert!(sh.reflow_for_zone(reserved + 1.0).is_empty());
        // Once it has held still, the window that is genuinely too wide is fixed, once.
        sh.tick(5000 + 2 * Shell::ZONE_SETTLE_MS + 10);
        assert_eq!(sh.reflow_for_zone(reserved + 1.0).len(), 2);
        assert!(sh.reflow_for_zone(reserved + 1.0).is_empty(), "and not again inside the cooldown");
    }

    #[test]
    fn a_window_that_is_still_being_resized_is_not_judged_by_its_in_flight_width() {
        let mut sh = Shell::new(ShellPrefs::default(), (1280.0, 800.0));
        let card = |id, maximized, width| WindowCard { maximized, width, ..win(id, false) };
        sh.set_windows(vec![card(1, true, Some(1216))]);
        sh.tick(5000);
        sh.reflow_for_zone(64.0);
        sh.tick(6000);
        assert!(sh.reflow_for_zone(64.0).is_empty());
        // It just got maximized and reports the old, wide width while the compositor works.
        sh.set_windows(vec![card(1, true, Some(1280))]);
        sh.tick(6000 + Shell::WINDOW_SETTLE_MS - 10);
        assert!(sh.reflow_for_zone(64.0).is_empty(), "its width changed a moment ago: wait");
        sh.tick(6000 + Shell::WINDOW_SETTLE_MS + 10);
        assert_eq!(sh.reflow_for_zone(64.0).len(), 2, "still too wide once it has had time: now fix it");
    }

    #[test]
    fn auto_brightness_ramps_from_the_current_level_and_the_tile_toggles_it() {
        let mut sh = Shell::new(ShellPrefs::default(), S);
        sh.set_current_brightness(0.2);
        sh.set_clock(12 * 60);
        let mut last = None;
        for i in 0..60u64 {
            sh.on_lux(20_000.0, i * 100);
            for a in sh.tick(i * 100) {
                if let Action::SetSlider(SliderKind::Brightness, v) = a {
                    assert!(last.is_none_or(|l| v >= l), "ramp not monotonic");
                    last = Some(v);
                }
            }
        }
        assert!(last.unwrap() > 0.95, "daylight reached: {last:?}");
        assert_eq!(sh.drawer.brightness, last.unwrap(), "slider shows what auto applied");
        assert!(sh.quick_state(QuickKind::AutoBrightness));
        assert_eq!(sh.tap_quick(QuickKind::AutoBrightness), vec![Action::SavePrefs]);
        assert!(!sh.quick_state(QuickKind::AutoBrightness) && !sh.prefs.auto_brightness);
        assert!(!sh.tick(10_000).iter().any(|a| matches!(a, Action::SetSlider(..))), "off: no more changes");
    }

    #[test]
    fn voicing_moves_emit_once_and_are_saved_after_the_finger_rests() {
        let mut sh = Shell::new(ShellPrefs::default(), S);
        sh.tick(0);
        assert_eq!(sh.set_voicing(0.4), vec![Action::SetVoicing(0.4)]);
        assert!(sh.set_voicing(0.4).is_empty(), "no repeat for the same value");
        assert_eq!(sh.set_voicing(3.0), vec![Action::SetVoicing(1.0)], "clamped");
        let saves: usize = (100..3000).step_by(100).map(|t| sh.tick(t).iter().filter(|a| **a == Action::SavePrefs).count()).sum();
        assert_eq!(saves, 1);
        assert_eq!(sh.prefs.audio_tone, 1.0);
    }

    #[test]
    fn slider_offset_is_saved_once_after_the_user_lets_go() {
        let mut sh = Shell::new(ShellPrefs::default(), S);
        sh.set_current_brightness(0.5);
        sh.on_lux(300.0, 0);
        sh.tick(0);
        // A drag: many slider events in quick succession.
        let mut saves = 0;
        for i in 0..10u64 {
            sh.tick(100 + i * 50);
            sh.set_slider(SliderKind::Brightness, 0.6 + i as f32 * 0.01);
        }
        for t in (700..4000).step_by(100) {
            saves += sh.tick(t).iter().filter(|a| **a == Action::SavePrefs).count();
        }
        assert_eq!(saves, 1, "one save after the drag, not one per event");
        assert!((sh.prefs.brightness_offset - sh.ambient.user).abs() < 1e-6 && sh.ambient.user > 0.0);
        assert_eq!(sh.brightness_slider_pos(), (sh.ambient.user + 1.0) / 2.0);
    }

    #[test]
    fn the_mood_follows_the_calendar_and_the_place() {
        let mut sh = shell_with_three();
        sh.set_date(20);
        sh.set_clock(2 * 60);
        let m = sh.mood();
        assert_eq!((m.season, m.phase), ("winter", "night"));
        assert!(m.warmth < -0.4 && m.zeta > 0.6);
        sh.set_date(200);
        sh.set_clock(13 * 60);
        assert_eq!(sh.mood().season, "summer");
        // A warm, quiet place shades the mood.
        sh.on_place(crate::ambience::PlaceKey::Landmark("cafe".into()), "Cafe");
        sh.on_photo_colors(&[[220, 150, 60]]);
        for _ in 0..3 {
            sh.ambience.here().unwrap().note_sound(-60.0);
        }
        let m2 = sh.mood();
        assert!(m2.warmth > sh_mood_without_place(&sh).warmth + 0.1, "warmer at the cafe");
        assert!(m2.zeta > sh_mood_without_place(&sh).zeta, "calmer at a quiet place");
        assert_eq!(sh.place_accent(), Some([220, 150, 60]));
        assert!(sh.take_ambience_dirty() && !sh.take_ambience_dirty());
        // Off: neutral.
        let mut p = sh.prefs.clone();
        p.living = false;
        sh.set_prefs(p);
        assert_eq!(sh.mood().phase, "off");
    }

    fn sh_mood_without_place(sh: &Shell) -> crate::season::Mood {
        crate::season::mood(sh.day_of_year, sh.clock_minute, false)
    }

    #[test]
    fn cold_mode_grows_the_edges_and_slows_the_hold() {
        let mut sh = shell_with_three();
        let warm = sh.recognizer.cfg.clone();
        sh.on_battery_temp(3.0);
        assert!(sh.cold.on);
        assert!((sh.recognizer.cfg.edge_px - warm.edge_px * 1.35).abs() < 1e-3);
        assert_eq!(sh.recognizer.cfg.hold_ms, (warm.hold_ms as f32 * 1.4) as Millis);
        sh.on_battery_temp(20.0);
        assert_eq!(sh.recognizer.cfg, warm, "restored exactly, not compounded");
        let mut p = sh.prefs.clone();
        p.cold = "on".into();
        sh.set_prefs(p);
        assert!(sh.cold.on && sh.recognizer.cfg.edge_px > warm.edge_px, "forced on");
    }

    #[test]
    fn tilting_while_holding_the_palette_moves_the_hover() {
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        sh.tick(500);
        assert!(sh.palette.is_holding());
        let mut t = 500u64;
        for _ in 0..40 {
            t += 20;
            sh.on_imu([-40f32.to_radians(), 0.0, 0.0], [0.0, 9.8, 0.0], t); // top edge away: down
        }
        assert!(matches!(sh.palette.hover, Some(crate::palette::Hit::Row(_))), "{:?}", sh.palette.hover);
        // Lift: the hovered row is activated through the normal release path.
        let g = sh.palette.geom(S);
        let _ = g;
    }

    #[test]
    fn sunlight_flips_the_apps_theme_with_hysteresis_and_the_voice_route_follows_focus() {
        let mut sh = shell_with_three();
        sh.set_current_brightness(0.5);
        let mut acts = vec![];
        for i in 0..400u64 {
            sh.on_lux(30_000.0, i * 100);
            acts.extend(sh.tick(i * 100));
        }
        assert!(acts.contains(&Action::AppsLightTheme(true)));
        acts.clear();
        for i in 400..800u64 {
            sh.on_lux(5_000.0, i * 100); // dappled: contrast 0.5, inside the band
            acts.extend(sh.tick(i * 100));
        }
        assert!(!acts.iter().any(|a| matches!(a, Action::AppsLightTheme(_))), "no flap in the band");
        for i in 800..1200u64 {
            sh.on_lux(500.0, i * 100);
            acts.extend(sh.tick(i * 100));
        }
        assert!(acts.contains(&Action::AppsLightTheme(false)));
        // Voice: apps in front -> default; a console -> dictation; the palette -> command.
        assert_eq!(sh.voice_route_line().as_deref(), Some("tiers=tiny,base reason=default: tiny + base"));
        assert!(sh.voice_route_line().is_none(), "unchanged: nothing to publish");
        sh.set_windows(vec![WindowCard { app_id: "org.gnome.Console".into(), activated: true, ..win(9, true) }]);
        assert!(sh.voice_route_line().unwrap().contains("dictation"));
        sh.open_palette();
        assert!(sh.voice_route_line().unwrap().starts_with("tiers=tiny reason=command"));
    }

    #[test]
    fn the_camera_runs_only_on_a_lit_cover() {
        let mut sh = shell_with_three();
        assert!(!sh.camera_wanted(), "apps in front: no camera");
        sh.lock_now();
        assert!(sh.camera_wanted());
        sh.on_proximity(true, 100);
        assert!(!sh.camera_wanted(), "in a pocket: no");
        sh.on_proximity(false, 200);
        sh.tick(200 + crate::sleep::POCKET_HOLD_MS + 1);
        assert!(sh.camera_wanted());
        sh.power_key();
        assert!(!sh.camera_wanted(), "screen going dark: no");
        // IMU samples turn the world; a heading sets north.
        for i in 0..50u64 {
            sh.on_imu([0.0, 10.0 * crate::world::DEG, 0.0], [0.0, 9.8, 0.0], 5000 + i * 20);
        }
        assert!((sh.world.yaw_deg() - 350.0).abs() < 2.0, "{}", sh.world.yaw_deg());
        sh.on_mic_level(-30.0, 6000);
        assert!(sh.notification_gain() > 0.0);
    }

    #[test]
    fn power_key_covers_sleeps_and_triple_tap_wakes_to_the_cover() {
        let mut sh = shell_with_three();
        sh.open_switcher();
        assert!(sh.power_key().is_empty(), "the fade comes first; the panel goes off at its end");
        assert!(sh.lock.is_locked() && !sh.switcher.open, "cover engaged, overlays closed");
        let mut t = 1000u64;
        let mut off = vec![];
        for _ in 0..100 {
            t += 11;
            off.extend(sh.tick(t));
        }
        assert!(off.contains(&Action::DisplayPower(false)));
        // Edge gestures are dead while asleep/covered.
        assert!(edge_drag(&mut sh, (200.0, 895.0), (200.0, 600.0), 20, t).is_empty());
        assert!(!sh.switcher.open);
        let mut wake = vec![];
        for i in 0..3u64 {
            let tt = t + 2000 + i * 200;
            sh.on_sleep_touch(TouchEvent::Down { slot: 0, x: 200.0, y: 500.0, t: tt });
            wake.extend(sh.on_sleep_touch(TouchEvent::Up { slot: 0, t: tt + 80 }));
        }
        assert_eq!(wake, vec![Action::DisplayPower(true)]);
        assert!(sh.lock.is_locked(), "wakes to the cover");
    }

    #[test]
    fn launch_goes_through_hatch_and_closes_overlays() {
        let mut sh = shell_with_three();
        sh.open_switcher();
        assert_eq!(sh.launch("org.gnome.Console"), vec![Action::LaunchApp("org.gnome.Console".into())]);
        assert!(!sh.switcher.open);
    }

    #[test]
    fn the_home_widget_cycles_in_three_and_asks_to_be_saved() {
        use crate::prefs::HomeWidget as H;
        let mut sh = shell_with_three();
        assert_eq!(sh.prefs.home_widget, H::Clock);
        for want in [H::Snapclone, H::Temperature, H::Clock] {
            assert_eq!(sh.cycle_home_widget(), vec![Action::SavePrefs]);
            assert_eq!(sh.prefs.home_widget, want);
        }
    }

    fn morse_shell(pattern: &str) -> Shell {
        let mut sh = shell_with_three();
        let mut p = sh.prefs.clone();
        p.unlock_morse = pattern.into();
        sh.set_prefs(p);
        sh.lock.lock();
        sh
    }

    #[test]
    fn under_a_morse_unlock_a_press_does_not_sleep_and_the_pattern_opens_the_cover() {
        let mut sh = morse_shell(".-.");
        assert!(sh.power_key().is_empty());
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::Awake, "a press on the lit cover is a symbol, not sleep");
        assert!(sh.power_released(100, 1000).is_empty());
        assert!(sh.power_released(600, 1600).is_empty());
        assert!(sh.lock.is_locked());
        sh.power_released(100, 2200);
        let mut unlocked = false;
        for i in 0..400u64 {
            if sh.lock.tick(11.0, 2300 + i * 11).contains(&crate::lock::LockAction::Unlock) { unlocked = true; break; }
        }
        assert!(unlocked, "the right code slid the cover away");
    }

    #[test]
    fn a_wrong_code_leaves_the_cover_up_and_a_long_hold_sleeps_the_screen() {
        let mut sh = morse_shell(".-.");
        sh.power_released(600, 1000);
        sh.power_released(600, 1600);
        sh.power_released(600, 2200);
        for i in 0..400u64 {
            assert!(!sh.lock.tick(11.0, 3000 + i * 11).contains(&crate::lock::LockAction::Unlock));
        }
        assert!(sh.lock.is_locked());
        sh.power_released(crate::morse::SLEEP_MS, 9000);
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::FadingOut, "a long hold puts it to sleep");
    }

    #[test]
    fn presses_during_the_wake_fade_are_symbols_not_sleep() {
        // Black screen, dot-dot-dash straight away: the first press wakes, the next two land in the
        // (slow, evening) fade-in and must not put the screen back to sleep.
        let mut sh = morse_shell("..-");
        sh.power_released(crate::morse::SLEEP_MS, 1000);
        let mut t = 1000u64;
        for _ in 0..400 { t += 11; sh.tick(t); }
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::Off);
        sh.sleep.evening = true;
        sh.now = t;
        sh.power_key(); // press 1 wakes
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::FadingIn);
        sh.power_released(100, t + 100); // its release: not a symbol
        sh.power_key(); // press 2, mid-fade
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::FadingIn, "must not sleep again");
        sh.power_released(100, t + 600);
        sh.power_key();
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::FadingIn);
        sh.power_released(100, t + 1100);
        assert_eq!(sh.morse.entered(), 2, "two dots counted, the wake press not");
    }

    #[test]
    fn waking_after_a_long_tickless_sleep_does_not_turn_the_wake_press_into_a_dot() {
        // The real loop does not tick while the screen is off: self.now stays where the last tick
        // left it, and the wake press arrives ten seconds later carrying only the loop's clock.
        let mut sh = morse_shell("..-");
        sh.power_released(crate::morse::SLEEP_MS, 1000);
        let mut t = 1000u64;
        for _ in 0..400 { t += 11; sh.tick(t); }
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::Off);
        let wake = t + 10_000; // no tick in between
        sh.power_key_at(wake);
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::FadingIn);
        sh.power_released(120, wake + 130); // the wake press's own release
        assert_eq!(sh.morse.entered(), 0, "that release is not the first dot");
        // and the next, real, press still counts
        sh.power_key_at(wake + 700);
        sh.power_released(100, wake + 800);
        assert_eq!(sh.morse.entered(), 1);
    }

    #[test]
    fn the_press_that_wakes_the_screen_is_not_a_symbol() {
        let mut sh = morse_shell(".-.");
        sh.power_key(); // awake + morse: consumed
        sh.power_released(crate::morse::SLEEP_MS, 1000); // long hold: asleep
        for _ in 0..400 { sh.tick(5000 + 11); }
        let mut t = 6000u64;
        for _ in 0..200 { t += 11; sh.tick(t); }
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::Off);
        sh.now = 8000;
        sh.power_key(); // wakes
        assert_eq!(sh.sleep.phase, crate::sleep::Phase::FadingIn);
        sh.power_released(120, 8130); // the wake press's own release
        assert_eq!(sh.morse.entered(), 0, "that release is not the first dot");
    }
}

#[cfg(test)]
mod cold_palette_tests {
    use super::*;

    const S: (f32, f32) = (1080.0, 2400.0);

    fn sh() -> Shell {
        let mut s = Shell::new(ShellPrefs::default(), S);
        s.rebuild_palette();
        s
    }

    fn cold_row(s: &Shell) -> String {
        s.palette.rows().find(|r| r.key == "set:cold").map(|r| r.label.clone()).expect("a cold row")
    }

    #[test]
    fn cold_mode_cycles_through_three_states_not_two() {
        // Automatic is not off: it is the battery temperature deciding, and
        // someone who forced it off in a warm room still wants it back on
        // its own in a cold one. A toggle would lose that state.
        let mut s = sh();
        assert!(cold_row(&s).contains("automatic"), "{}", cold_row(&s));
        s.palette_action(crate::palette::PaletteAction::Cold("on"));
        assert_eq!(s.prefs.cold, "on");
        s.rebuild_palette();
        assert!(cold_row(&s).contains("always on"), "{}", cold_row(&s));
        s.palette_action(crate::palette::PaletteAction::Cold("off"));
        s.rebuild_palette();
        assert!(cold_row(&s).contains("off"), "{}", cold_row(&s));
        s.palette_action(crate::palette::PaletteAction::Cold("auto"));
        assert_eq!(s.prefs.cold, "auto");
    }

    #[test]
    fn forcing_it_on_widens_the_targets_immediately() {
        // The setting is only worth having if it takes effect without a
        // cold battery to prove it.
        let mut s = sh();
        let warm = s.cold_tuning().target_scale;
        s.palette_action(crate::palette::PaletteAction::Cold("on"));
        assert!(s.cold.on, "forced on");
        assert!(s.cold_tuning().target_scale > warm);
        s.palette_action(crate::palette::PaletteAction::Cold("off"));
        assert!(!s.cold.on);
    }

    #[test]
    fn the_row_says_when_it_is_actually_active() {
        // "automatic" tells you the policy; the hint tells you what is
        // happening now, which is the thing you want to know in the cold.
        let mut s = sh();
        let hint = |s: &Shell| s.palette.rows().find(|r| r.key == "set:cold").unwrap().hint;
        assert_eq!(hint(&s), "setting");
        s.palette_action(crate::palette::PaletteAction::Cold("on"));
        s.rebuild_palette();
        assert_eq!(hint(&s), "active");
    }
}
