//! Animation helpers built on `adw::Animation`.
//!
//! GTK4 alone doesn't have a friendly animation API, but libadwaita's
//! `TimedAnimation` and `SpringAnimation` make it trivial. We wrap them in
//! conveniences for the keyboard's most common cases: key-press flash,
//! suggestion-bar slide, and recording-pulse.
//!
//! API notes for libadwaita 0.7:
//! - `TimedAnimation::new` and `SpringAnimation::new` take the target by
//!   value (`impl IsA<AnimationTarget>`), not by reference.
//! - `SpringParams` is passed by value too.

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

pub struct Animator;

impl Animator {
    /// Brief opacity dip on key press, then back. CSS-class-driven so
    /// the keyframe runs on the GTK compositor — no main-loop ticks.
    ///
    /// Previous impl used an adw::SpringAnimation with a
    /// CallbackAnimationTarget that fired ~13 main-loop callbacks per
    /// invocation (60Hz × 220ms), each setting widget.opacity. At a
    /// typical 5 cps typing rate that was ~65 callbacks/sec just for
    /// press feedback, and animations overlapped into a queue when the
    /// user typed faster than they completed. The CSS path is free in
    /// comparison (paint happens on the compositor; Rust just toggles
    /// the class).
    pub fn key_press_pulse(widget: &impl IsA<gtk4::Widget>) {
        let w = widget.clone().upcast::<gtk4::Widget>();
        // Remove first in case a previous press is still mid-animation
        // — re-adding restarts the keyframe (matches the commit_flash
        // pattern).
        w.remove_css_class("press-pulse");
        w.add_css_class("press-pulse");
        let w_clone = w.clone();
        // 240ms = the 220ms keyframe + small safety buffer so the
        // class removal happens after the animation completes. Without
        // a buffer, fast-typing users could land the removal mid-frame
        // and see a visible jump.
        glib::timeout_add_local_once(std::time::Duration::from_millis(240), move || {
            w_clone.remove_css_class("press-pulse");
        });
    }

    /// Slide a widget into view from below. Used when the keyboard appears.
    pub fn slide_in_from_bottom(widget: &impl IsA<gtk4::Widget>, distance: f64) {
        let target = adw::CallbackAnimationTarget::new({
            let w = widget.clone().upcast::<gtk4::Widget>();
            move |value| {
                w.set_margin_top(value as i32);
            }
        });

        let anim = adw::TimedAnimation::new(widget, distance, 0.0, 220, target);
        anim.set_easing(adw::Easing::EaseOutCubic);
        anim.play();
    }

    /// Pulse the opacity for a "still recording" indicator. The animation
    /// alternates and repeats forever; the caller should hold the returned
    /// handle and call `pause()` to stop it.
    pub fn recording_pulse(widget: &impl IsA<gtk4::Widget>) -> adw::TimedAnimation {
        let target = adw::CallbackAnimationTarget::new({
            let w = widget.clone().upcast::<gtk4::Widget>();
            move |value| {
                w.set_opacity(value);
            }
        });

        let anim = adw::TimedAnimation::new(widget, 0.5, 1.0, 800, target);
        anim.set_easing(adw::Easing::EaseInOutSine);
        anim.set_repeat_count(0); // 0 = infinite
        anim.set_alternate(true);
        anim.play();
        anim
    }

    /// Spring animation on opacity change — physical-feeling state transitions.
    pub fn spring_fade(widget: &impl IsA<gtk4::Widget>, from: f64, to: f64) {
        let target = adw::CallbackAnimationTarget::new({
            let w = widget.clone().upcast::<gtk4::Widget>();
            move |value| {
                w.set_opacity(value);
            }
        });

        let spring_params = adw::SpringParams::new(0.85, 1.0, 250.0);
        let anim = adw::SpringAnimation::new(widget, from, to, spring_params, target);
        anim.set_clamp(true);
        anim.play();
    }

    /// One-shot "commit flash" — a brief accent-tinted glow on a key,
    /// used as positive feedback when a word commits (e.g. the
    /// spacebar pulses when a swipe-word lands, teaching the
    /// springboard mechanic). Driven by a CSS keyframe via the
    /// `.commit-flash` class rather than a per-frame adw callback, so
    /// the animation runs on the GTK compositor and costs no main-loop
    /// ticks. The class is removed after the keyframe duration so it
    /// can re-trigger on the next commit.
    pub fn commit_flash(widget: &impl IsA<gtk4::Widget>) {
        let w = widget.clone().upcast::<gtk4::Widget>();
        // Remove first in case a previous flash is still mid-animation
        // — re-adding restarts the keyframe.
        w.remove_css_class("commit-flash");
        w.add_css_class("commit-flash");
        let w_clone = w.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(380), move || {
            w_clone.remove_css_class("commit-flash");
        });
    }
}
