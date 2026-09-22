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
    /// The lock cover (cover mode for now: swipe dismisses, no auth).
    pub lock: LockScreen,
    /// Power key -> fade -> panel off; triple tap wakes.
    pub sleep: Sleep,
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
            lock: LockScreen::default(),
            sleep: Sleep::default(),
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
        for a in self.lock.tick(dt, now) {
            if a == LockAction::Unlock {
                self.log("cover dismissed");
            }
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
            self.log(format!("auto brightness: slider at {:+.2} of the band", self.ambient.user));
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
                vec![]
            }
            GestureEvent::SwipeUpRelease { target } => {
                self.swipe_up = 0.0;
                self.log(format!("swipe-up -> {target:?}"));
                match target {
                    SwipeUpTarget::Cancel => vec![],
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
            GestureEvent::HoldStart { x, y } => {
                self.swipe_up = 0.0;
                self.hold = Some(HoldMenu::open((x, y), self.screen));
                vec![]
            }
            GestureEvent::HoldMove { x, y } => {
                if let Some(h) = &mut self.hold {
                    h.hover_to(x, y);
                }
                vec![]
            }
            GestureEvent::HoldRelease { x, y } => {
                let pick = self.hold.take().and_then(|h| h.release(x, y));
                self.log(format!("hold release -> {pick:?}"));
                match pick {
                    Some(opt) => self.toggle_mode(opt),
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
            self.drawer.brightness
        }
    }

    pub fn brightness_band(&self) -> crate::ambient::Band {
        self.ambient.band(self.now, self.clock_minute)
    }

    /// Local minute of the day (0..1440), for the night window.
    pub fn set_clock(&mut self, minute: u32) {
        self.clock_minute = minute % 1440;
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
    /// the right edge leaves (`reserve_right` px), so they move out from
    /// under the bar. Call on every window update while the bar is mapped;
    /// it only acts on windows that are actually wrong.
    ///
    /// Why: cosmic-comp epoch-1.8.0 does not re-lay-out an ALREADY maximized
    /// window when a new exclusive zone appears (observed: the window's own
    /// COSMIC geometry stayed 1280 wide with the 64 px bar up; one maximized
    /// after the bar appeared got 1216; removing the bar re-grew it). A
    /// one-shot reflow when the bar mapped was tried first and found nothing:
    /// the window list and its maximized state arrive over several polls.
    pub fn reflow_for_zone(&mut self, reserve_right: f32) -> Vec<Action> {
        let limit = self.screen.0 - reserve_right + 1.0;
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
        let menu = sh.hold.clone().expect("hold menu open");
        let (kx, ky) = menu.centers[1];
        sh.on_edge_touch(TouchEvent::Move { slot: 0, x: kx, y: ky, t: 600 });
        assert_eq!(sh.hold.as_ref().unwrap().hover, Some(HoldOption::Keyboard));
        let out = sh.on_edge_touch(TouchEvent::Up { slot: 0, t: 700 });
        assert_eq!(out, vec![Action::ShowKeyboard(true)]);
        assert!(sh.hold.is_none());
        assert!(sh.quick_state(QuickKind::Keyboard));
    }

    #[test]
    fn hold_released_in_place_does_nothing() {
        let mut sh = shell_with_three();
        sh.on_edge_touch(TouchEvent::Down { slot: 0, x: 204.0, y: 895.0, t: 0 });
        sh.tick(500);
        assert!(sh.on_edge_touch(TouchEvent::Up { slot: 0, t: 600 }).is_empty());
        assert_eq!(sh.modes, Modes::default());
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
        let out = sh.tap_quick(QuickKind::Carousel);
        assert_eq!(out.len(), 2, "restores both windows it maximized: {out:?}");
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
        let out = sh.toggle_mode(HoldOption::Trackpad);
        assert!(out.contains(&Action::Pointer(crate::trackpad::PointerOut::Button {
            button: crate::trackpad::Button::Left,
            pressed: false
        })));
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
