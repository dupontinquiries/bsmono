//! The orchestrator. Owns every model, routes recognizer events and overlay
//! taps into them, and returns the resulting [`Action`]s. The frontend paints
//! from the public fields and calls the `on_*`/tap methods; transports execute
//! the actions and report window state back through [`Shell::set_windows`].

use std::collections::VecDeque;

use crate::bar::BarWidget;
use crate::carousel::Carousel;
use crate::control_center::{ControlCenter, QuickKind, SliderKind};
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
    /// Right-bar layout: collapsed rail vs expanded (right-edge swipe toggles).
    pub bar_expanded: bool,
    /// Live swipe-up progress for the preview (0 when not swiping).
    pub swipe_up: f32,
    /// Live horizontal offset along the bottom edge (0 when not swiping).
    pub peek_dx: f32,
    windows: Vec<WindowCard>,
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
            bar_expanded: false,
            swipe_up: 0.0,
            peek_dx: 0.0,
            windows: Vec::new(),
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
        self.prefs = prefs;
        self.emit(vec![Action::SavePrefs])
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
            k => self.drawer.system_state(k),
        }
    }

    // ---- input ------------------------------------------------------------

    /// A touch sample from an edge strip.
    pub fn on_edge_touch(&mut self, ev: TouchEvent) -> Vec<Action> {
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
            k => {
                let out = self.drawer.toggle_system(k).into_iter().collect();
                self.emit(out)
            }
        }
    }

    pub fn set_slider(&mut self, kind: SliderKind, v: f32) -> Vec<Action> {
        vec![self.drawer.set_slider(kind, v)]
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

    /// Tap on empty drawer/switcher backdrop dismisses it.
    pub fn dismiss_overlays(&mut self) {
        self.drawer.close();
        self.switcher.hide();
    }

    /// A touch on the trackpad overlay.
    pub fn on_trackpad_touch(&mut self, ev: TouchEvent) -> Vec<Action> {
        let outs = match ev {
            TouchEvent::Down { slot, x, y, t } => self.trackpad.down(slot, x, y, t),
            TouchEvent::Move { slot, x, y, t } => self.trackpad.motion(slot, x, y, t),
            TouchEvent::Up { slot, t } => self.trackpad.up(slot, t),
            TouchEvent::Cancel => self.trackpad.cancel(),
        };
        // Not logged: motion would flood the trace. Clicks are logged.
        let out: Vec<Action> = outs.into_iter().map(Action::Pointer).collect();
        for a in &out {
            if matches!(a, Action::Pointer(crate::trackpad::PointerOut::Button { .. })) {
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
        WindowCard { id, title: format!("w{id}"), app_id: format!("a{id}"), activated, maximized: false }
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
        edge_drag(&mut sh, (406.0, 450.0), (300.0, 450.0), 10, 2000);
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
    fn launch_goes_through_hatch_and_closes_overlays() {
        let mut sh = shell_with_three();
        sh.open_switcher();
        assert_eq!(sh.launch("org.gnome.Console"), vec![Action::LaunchApp("org.gnome.Console".into())]);
        assert!(!sh.switcher.open);
    }
}
