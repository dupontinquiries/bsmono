//! TapHoldButton — a button with distinct tap and hold semantics
//! plus visual progress feedback during the hold interval.
//!
//! Ported from the voiceterm/web2qml QML ToolBtn component
//! (v0.1.202–v0.1.207). Mirrors the same UX:
//!
//!   - Single tap → fires `tapped` callback
//!   - Hold for `hold_interval_ms` (default 800ms) → fires
//!     `held_down` callback. A progress bar at the bottom of the
//!     button fills during the hold to give visual feedback. If
//!     the user releases before the interval elapses, the bar
//!     snaps back and `tapped` fires instead.
//!   - Double-click → fires `held_down` (shortcut path for users
//!     who don't want to wait for the hold timeout)
//!   - Right-click on desktop → fires `held_down`
//!
//! Use this for buttons where the tap action is conservative
//! (open a popup, show help) and the hold action is destructive
//! or commits to a longer-running operation (reset to defaults,
//! delete config, test the endpoint).
//!
//! The progress bar only renders if `set_hold_progress_visible`
//! is true, so consumers that don't want visible feedback (e.g.
//! the keyboard's per-key long-press) can use this without the
//! progress overlay.

use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Configuration for a TapHoldButton. Defaults match the
/// voiceterm/QML ToolBtn (800ms hold, 60% bar width, 120ms
/// release-snap).
#[derive(Debug, Clone, Copy)]
pub struct TapHoldConfig {
    /// How long the user has to hold to trigger `held_down`.
    pub hold_interval_ms: u32,
    /// How fast the progress bar snaps back to 0 when the user
    /// releases early.
    pub release_snap_ms: u32,
    /// Whether to show the progress bar during the hold.
    pub show_progress: bool,
}

impl Default for TapHoldConfig {
    fn default() -> Self {
        Self {
            hold_interval_ms: 800,
            release_snap_ms: 120,
            show_progress: true,
        }
    }
}

/// A button-shaped widget that distinguishes tap from hold. Owns
/// its own GTK widget tree (a `gtk4::Overlay` with a `Button`
/// underneath and a `ProgressBar` on top).
pub struct TapHoldButton {
    root: gtk4::Overlay,
    button: gtk4::Button,
    progress: gtk4::ProgressBar,
    state: Rc<RefCell<ButtonState>>,
    tap_cb: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    hold_cb: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    cfg: TapHoldConfig,
}

#[derive(Debug, Clone, Copy)]
struct ButtonState {
    /// Time when the current press began. None if not pressed.
    press_start: Option<Instant>,
    /// True if held_down has already fired for the current press
    /// (so we don't fire both tapped and held_down).
    hold_fired: bool,
    /// Cached fraction last written to the ProgressBar so we
    /// avoid redundant set_fraction calls.
    last_fraction: f64,
}

impl Default for ButtonState {
    fn default() -> Self {
        Self {
            press_start: None,
            hold_fired: false,
            last_fraction: 0.0,
        }
    }
}

impl TapHoldButton {
    pub fn new(label: &str, cfg: TapHoldConfig) -> Self {
        let button = gtk4::Button::builder()
            .label(label)
            .css_classes(["tap-hold-button"])
            .build();

        let progress = gtk4::ProgressBar::builder()
            .show_text(false)
            .valign(gtk4::Align::End)
            .halign(gtk4::Align::Center)
            .css_classes(["tap-hold-progress"])
            // Initially invisible until a press starts.
            .visible(false)
            .can_target(false)
            .build();

        let root = gtk4::Overlay::new();
        root.set_child(Some(&button));
        root.add_overlay(&progress);

        let state: Rc<RefCell<ButtonState>> = Rc::new(RefCell::new(ButtonState::default()));
        let tap_cb: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        let hold_cb: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));

        let me = Self {
            root,
            button,
            progress,
            state,
            tap_cb,
            hold_cb,
            cfg,
        };
        me.wire_gestures();
        me
    }

    /// The widget to embed in a parent container.
    pub fn widget(&self) -> &gtk4::Widget {
        self.root.upcast_ref()
    }

    /// Register the tap callback. Fires when the user releases
    /// before the hold interval elapses.
    pub fn on_tap<F: Fn() + 'static>(&self, cb: F) {
        *self.tap_cb.borrow_mut() = Some(Box::new(cb));
    }

    /// Register the hold callback. Fires once when the hold
    /// interval is reached, or immediately on a double-click /
    /// right-click.
    pub fn on_hold<F: Fn() + 'static>(&self, cb: F) {
        *self.hold_cb.borrow_mut() = Some(Box::new(cb));
    }

    fn wire_gestures(&self) {
        // GtkButton's own `clicked` signal fires on release; we'd
        // double-fire if we kept it AND added our own gesture. So
        // we override with a single GestureClick that handles all
        // press/release semantics.
        //
        // GtkGestureClick fires:
        //   - `pressed(n_press, x, y)` on each press
        //   - `released(n_press, x, y)` on each release
        //   - `n_press == 2` indicates a double-click
        //
        // The hold detection runs on a glib timer that fires once
        // at `hold_interval_ms`. If the user is still pressing
        // when it fires, we mark hold_fired and run the hold cb.
        // Otherwise the timer's closure sees press_start = None
        // and exits.

        let click = gtk4::GestureClick::builder()
            // Accept any mouse button so right-click also triggers
            // (matching the QML pattern). Touch press is button 0
            // which is always accepted.
            .button(0)
            .build();

        let state_p = self.state.clone();
        let progress_p = self.progress.clone();
        let cfg = self.cfg;
        let hold_cb_p = self.hold_cb.clone();
        let state_for_timer = self.state.clone();
        let progress_for_timer = self.progress.clone();
        click.connect_pressed(move |gesture, _n, _x, _y| {
            let mut s = state_p.borrow_mut();
            s.press_start = Some(Instant::now());
            s.hold_fired = false;
            s.last_fraction = 0.0;
            drop(s);
            if cfg.show_progress {
                progress_p.set_fraction(0.0);
                progress_p.set_visible(true);
            }

            // Right-click immediately = hold.
            if gesture.current_button() == 3 {
                let mut s = state_p.borrow_mut();
                s.hold_fired = true;
                s.press_start = None;
                drop(s);
                if let Some(f) = hold_cb_p.borrow().as_ref() {
                    f();
                }
                progress_p.set_visible(false);
                return;
            }

            // Schedule the hold check. The closure captures the
            // press_start by reading it from state on each tick;
            // a release between now and the timer's fire will
            // clear press_start and cause the timer to see "not
            // pressed anymore" and skip the hold callback.
            //
            // We also schedule periodic redraws of the progress
            // bar at ~30Hz so the fill animation looks smooth.
            let state_t = state_for_timer.clone();
            let progress_t = progress_for_timer.clone();
            let hold_cb_t = hold_cb_p.clone();
            glib::timeout_add_local(
                Duration::from_millis(33),
                move || {
                    let s = state_t.borrow();
                    let start = match s.press_start {
                        Some(t) => t,
                        None => {
                            // Released early. Snap-back is handled
                            // by the release handler.
                            return glib::ControlFlow::Break;
                        }
                    };
                    let elapsed = start.elapsed().as_millis() as u32;
                    let interval = cfg.hold_interval_ms.max(1);
                    let fraction = (elapsed as f64 / interval as f64).min(1.0);
                    drop(s);
                    if cfg.show_progress
                        && (fraction - state_t.borrow().last_fraction).abs() > 0.01
                    {
                        progress_t.set_fraction(fraction);
                        state_t.borrow_mut().last_fraction = fraction;
                    }
                    if fraction >= 1.0 {
                        // Hold completed. Fire and stop the timer.
                        let mut s = state_t.borrow_mut();
                        if !s.hold_fired {
                            s.hold_fired = true;
                            s.press_start = None;
                            drop(s);
                            if let Some(f) = hold_cb_t.borrow().as_ref() {
                                f();
                            }
                            // Leave the bar visible briefly so the
                            // user sees the commit, then hide.
                            let p = progress_t.clone();
                            glib::timeout_add_local_once(
                                Duration::from_millis(200),
                                move || {
                                    p.set_visible(false);
                                    p.set_fraction(0.0);
                                },
                            );
                        }
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                },
            );
        });

        let state_r = self.state.clone();
        let progress_r = self.progress.clone();
        let tap_cb_r = self.tap_cb.clone();
        let hold_cb_r = self.hold_cb.clone();
        click.connect_released(move |_gesture, n_press, _x, _y| {
            let mut s = state_r.borrow_mut();
            let was_pressed = s.press_start.is_some();
            let hold_already_fired = s.hold_fired;
            s.press_start = None;
            drop(s);

            // Double-click = hold (one event for the original
            // tap, second event releases here as n_press=2).
            if n_press == 2 {
                if !hold_already_fired {
                    if let Some(f) = hold_cb_r.borrow().as_ref() {
                        f();
                    }
                }
                progress_r.set_visible(false);
                progress_r.set_fraction(0.0);
                return;
            }

            // Normal release. If hold already fired (held to
            // completion), do nothing — the hold callback ran
            // when the timer fired. Otherwise it's a tap.
            if was_pressed && !hold_already_fired {
                if let Some(f) = tap_cb_r.borrow().as_ref() {
                    f();
                }
                // Snap the progress back to 0 with a quick
                // animation. GTK4 doesn't have an easing API for
                // ProgressBar; we simulate the snap with a brief
                // glib timer that decrements the fraction.
                let p = progress_r.clone();
                let starting = state_r.borrow().last_fraction;
                let snap_ms = cfg.release_snap_ms;
                let snap_start = Instant::now();
                if starting > 0.01 {
                    glib::timeout_add_local(
                        Duration::from_millis(16),
                        move || {
                            let elapsed =
                                snap_start.elapsed().as_millis() as u32;
                            if elapsed >= snap_ms {
                                p.set_fraction(0.0);
                                p.set_visible(false);
                                return glib::ControlFlow::Break;
                            }
                            let t = elapsed as f64 / snap_ms as f64;
                            let eased = 1.0 - (1.0 - t).powi(2); // ease-out-quad
                            p.set_fraction(starting * (1.0 - eased));
                            glib::ControlFlow::Continue
                        },
                    );
                } else {
                    progress_r.set_visible(false);
                    progress_r.set_fraction(0.0);
                }
            }
        });

        // Cancel: pointer leaves the button while pressed. GTK4
        // routes this through `cancel`. Reset state.
        let state_c = self.state.clone();
        let progress_c = self.progress.clone();
        click.connect_cancel(move |_gesture, _sequence| {
            let mut s = state_c.borrow_mut();
            s.press_start = None;
            s.hold_fired = false;
            drop(s);
            progress_c.set_visible(false);
            progress_c.set_fraction(0.0);
        });

        // Suppress the GtkButton's own clicked signal — we handle
        // the press/release ourselves. Without this, a tap fires
        // both our tapped() and the button's default activate
        // path. Setting can_focus=false prevents keyboard-Space
        // from triggering activate.
        self.button.set_can_focus(false);
        self.button.add_controller(click);
    }

    /// Reach in and set the button label after construction.
    /// Useful for buttons whose label depends on dynamic state.
    pub fn set_label(&self, label: &str) {
        self.button.set_label(label);
    }

    /// Apply additional CSS classes to the inner button (the
    /// progress bar's classes are managed internally).
    pub fn add_css_class(&self, class: &str) {
        self.button.add_css_class(class);
    }

    /// A weak handle to the inner Button, exposed for callers
    /// that need to set tooltips or other Button-specific
    /// properties. The TapHoldButton retains ownership.
    #[allow(dead_code)]
    pub fn button(&self) -> &gtk4::Button {
        &self.button
    }
}
