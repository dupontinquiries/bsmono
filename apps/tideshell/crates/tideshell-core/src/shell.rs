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
    pub rotate: crate::rotate::AutoRotate,
    /// The camera pane's world (gyro/gravity/compass) and its anchors.
    pub world: crate::world::World,
    /// Ambient loudness -> notification gain.
    pub sound: crate::ambient_sound::AmbientSound,
    last_imu: Option<Millis>,
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
        let mut rotate = crate::rotate::AutoRotate::default();
        rotate.enabled = prefs.autorotate;
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
            rotate,
            world: crate::world::World::new(screen.0 * 0.9),
            sound: crate::ambient_sound::AmbientSound::default(),
            last_imu: None,
            clock_minute: 12 * 60,
            offset_dirty_at: None,
            bar_expanded: false,
            swipe_up: 0.0,
            peek_dx: 0.0,
            windows: Vec::new(),
            reflow_at: HashMap::new(),
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
        self.prefs = prefs;
        let mut out = vec![Action::SavePrefs];
        out.extend(self.sync_fat());
        self.emit(out)
    }

    pub fn windows(&self) -> &[WindowCard] {
        &self.windows
    }

    pub fn set_windows(&mut self, windows: Vec<WindowCard>) -> Vec<Action> {
        self.switcher.sync(&windows);
        let out = self.carousel.sync(&windows);
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

    /// The power key: awake -> sleep (and cover); asleep -> wake.
    pub fn power_key(&mut self) -> Vec<Action> {
        self.log("power key");
        let acts = self.sleep.power_key();
        let out: Vec<Action> = acts.into_iter().flat_map(|a| self.on_sleep_action(a)).collect();
        self.emit(out)
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
        let streaking = self.switcher.is_streaking();
        self.switcher.tick(now);
        if streaking && !self.switcher.is_streaking() {
            self.log("quick-switch streak settled");
        }
        let events = self.recognizer.tick(now);
        self.drain_recognizer_log();
        let mut out = vec![];
        for a in self.sleep.tick(dt) {
            out.extend(self.on_sleep_action(a));
        }
        self.palette.tick(dt);
        for a in self.lock.tick(dt, now) {
            if a == LockAction::Unlock {
                self.log("cover dismissed");
            }
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
                let g = Geom::new(self.screen);
                self.palette.hold_move(&g, x, y);
                vec![]
            }
            GestureEvent::HoldRelease { x, y } => {
                let g = Geom::new(self.screen);
                let pick = self.palette.hold_release(&g, x, y);
                self.palette.end_hold();
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
    pub fn open_palette(&mut self) {
        self.drawer.close();
        self.switcher.hide();
        self.rebuild_palette();
        self.palette.open_by_tap();
        self.log("palette opened");
    }

    /// A tap on the open palette at screen coordinates.
    pub fn palette_tap(&mut self, x: f32, y: f32) -> Vec<Action> {
        let g = Geom::new(self.screen);
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
        }
        self.world.accel(accel);
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
        let ids: Vec<u64> = self
            .windows
            .iter()
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
    fn hold_then_slide_to_keyboard_toggles_it() {
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        assert!(sh.tick(500).is_empty());
        assert!(sh.palette.open, "the hold opens the palette");
        let g = Geom::new(S);
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
    fn hold_released_in_place_leaves_the_palette_open_for_taps() {
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        sh.tick(500);
        assert!(sh.on_edge_touch(TouchEvent::Up { slot: 0, t: 600 }).is_empty());
        assert_eq!(sh.modes, Modes::default());
        assert!(sh.palette.open);
        // Palette lists the running windows; tapping one activates it.
        let g = Geom::new(S);
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
    fn right_bar_layout_disables_drawer_and_enables_bar_swipe() {
        let mut sh = shell_with_three();
        let mut p = sh.prefs.clone();
        p.layout = Layout::RightBar;
        assert_eq!(sh.set_prefs(p), vec![Action::SavePrefs]);
        edge_drag(&mut sh, (200.0, 3.0), (200.0, 400.0), 20, 0);
        assert!(!sh.drawer.is_visible(), "top edge inert in right-bar layout");
        assert!(!sh.bar_widgets().is_empty());
        // The right edge is the bar's, not the recognizer's.
        let out = edge_drag(&mut sh, (406.0, 450.0), (300.0, 450.0), 10, 2000);
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
    fn reflow_only_touches_maximized_windows_too_wide_and_respects_cooldown() {
        let mut sh = Shell::new(ShellPrefs::default(), (1280.0, 800.0));
        let card = |id, maximized, width| WindowCard { maximized, width, ..win(id, false) };
        sh.set_windows(vec![
            card(1, true, Some(1280)),  // under the bar: reflow
            card(2, true, Some(1216)),  // already beside it: leave
            card(3, false, Some(1280)), // floating: not ours to resize
            card(4, true, None),        // geometry unknown: wait
        ]);
        assert_eq!(sh.reflow_for_zone(64.0), vec![Action::SetMaximized(1, false), Action::SetMaximized(1, true)]);
        // Compositor has not shrunk it yet: no repeat within the cooldown...
        sh.tick(1000);
        assert!(sh.reflow_for_zone(64.0).is_empty());
        // ...but it is retried after.
        sh.tick(Shell::REFLOW_COOLDOWN + 10);
        assert_eq!(sh.reflow_for_zone(64.0).len(), 2);
        // Once it is laid out right, nothing happens.
        sh.set_windows(vec![card(1, true, Some(1216))]);
        sh.tick(2 * Shell::REFLOW_COOLDOWN + 20);
        assert!(sh.reflow_for_zone(64.0).is_empty());
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
        for _ in 0..30 {
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
}
