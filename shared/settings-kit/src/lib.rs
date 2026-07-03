//! settings-kit — cross-app preferences for the egui-kit apps.
//!
//! Answers "if I update the touch gesture feel, do all apps change?" — with this, yes. There
//! is one [`Prefs`] schema (gesture feel, touch targets, palette, a11y) shared by every
//! egui-kit app. It serializes to flat `key = value` pairs so it rides the sharemd data bus
//! under the `settings` namespace: the central settings app `put`s a change, every app is
//! notified and re-`apply`s it live — while still keeping its own per-app overrides
//! (uniqueness) via [`Prefs::apply_kv`] on top.
//!
//! The schema deliberately maps onto the shared kit knobs, so applying it actually changes
//! behaviour: gesture prefs → `uikit_gesture::GestureConfig`, touch prefs → the adaptive
//! layer, palette → the theme.

use uikit_gesture::GestureConfig;

/// Nominal detent threshold (px) at sensitivity 1.0; higher sensitivity divides it (fires
/// sooner), lower multiplies it.
const BASE_DETENT_PX: f64 = 40.0;

/// How the two-tier swipe should feel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GesturePrefs {
    /// 0.25 (deliberate) … 1.0 (default) … 3.0 (hair-trigger). Scales the detent threshold.
    pub sensitivity: f64,
    /// Fraction of a page a drag must cover to commit a jump (0.1 easy … 0.6 firm).
    pub commit_fraction: f64,
    /// Min flick velocity (px/ms) to page on release.
    pub flick_velocity: f64,
}

impl Default for GesturePrefs {
    fn default() -> Self {
        GesturePrefs { sensitivity: 1.0, commit_fraction: 0.35, flick_velocity: 0.9 }
    }
}

/// Touch ergonomics that layer over the adaptive defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPrefs {
    /// Minimum interactive size in points (0 = let the adaptive layer decide).
    pub min_target: f32,
    /// Extra UI scale multiplier (1.0 = adaptive default).
    pub scale: f32,
}

impl Default for TouchPrefs {
    fn default() -> Self {
        TouchPrefs { min_target: 0.0, scale: 1.0 }
    }
}

/// Accessibility toggles (see uikit-egui::a11y).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct A11yPrefs {
    pub voice: bool,
    pub tts: bool,
}

impl Default for A11yPrefs {
    fn default() -> Self {
        A11yPrefs { voice: true, tts: false }
    }
}

/// The full cross-app preference set.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub gesture: GesturePrefs,
    pub touch: TouchPrefs,
    pub a11y: A11yPrefs,
    /// snapclone-theme palette id (e.g. "Dark").
    pub palette: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { gesture: GesturePrefs::default(), touch: TouchPrefs::default(), a11y: A11yPrefs::default(), palette: "Dark".to_string() }
    }
}

impl Prefs {
    /// Flat key/value form (the wire/bus + config-file form).
    pub fn to_kv(&self) -> Vec<(String, String)> {
        vec![
            ("gesture.sensitivity".into(), self.gesture.sensitivity.to_string()),
            ("gesture.commit_fraction".into(), self.gesture.commit_fraction.to_string()),
            ("gesture.flick_velocity".into(), self.gesture.flick_velocity.to_string()),
            ("touch.min_target".into(), self.touch.min_target.to_string()),
            ("touch.scale".into(), self.touch.scale.to_string()),
            ("a11y.voice".into(), self.a11y.voice.to_string()),
            ("a11y.tts".into(), self.a11y.tts.to_string()),
            ("theme.palette".into(), self.palette.clone()),
        ]
    }

    /// Build from a full or partial key/value set, filling gaps with defaults.
    pub fn from_kv(kv: &[(String, String)]) -> Prefs {
        let mut p = Prefs::default();
        p.apply_kv(kv);
        p
    }

    /// Apply a subset of keys — the mechanism for both live central changes *and* per-app
    /// overrides. Unknown keys and unparseable values are ignored (forward-compatible).
    pub fn apply_kv(&mut self, kv: &[(String, String)]) {
        for (k, v) in kv {
            match k.as_str() {
                "gesture.sensitivity" => if let Ok(x) = v.parse() { self.gesture.sensitivity = x },
                "gesture.commit_fraction" => if let Ok(x) = v.parse() { self.gesture.commit_fraction = x },
                "gesture.flick_velocity" => if let Ok(x) = v.parse() { self.gesture.flick_velocity = x },
                "touch.min_target" => if let Ok(x) = v.parse() { self.touch.min_target = x },
                "touch.scale" => if let Ok(x) = v.parse() { self.touch.scale = x },
                "a11y.voice" => if let Ok(x) = v.parse() { self.a11y.voice = x },
                "a11y.tts" => if let Ok(x) = v.parse() { self.a11y.tts = x },
                "theme.palette" => self.palette = v.clone(),
                _ => {}
            }
        }
    }

    /// Apply gesture prefs onto a `GestureConfig` (preserving its page_size and the fields
    /// prefs don't govern). This is what makes "update the gesture feel" actually change how
    /// every app's swipes behave.
    pub fn apply_to_gesture(&self, cfg: &mut GestureConfig) {
        cfg.commit_fraction = self.gesture.commit_fraction;
        cfg.flick_velocity_px_ms = self.gesture.flick_velocity;
        cfg.detent_threshold = BASE_DETENT_PX / self.gesture.sensitivity.max(0.1);
    }

    /// A `GestureConfig` for `page_size` with these prefs applied — the one-call path apps use.
    pub fn gesture_config(&self, page_size: f64) -> GestureConfig {
        let mut cfg = GestureConfig::for_page_size(page_size);
        self.apply_to_gesture(&mut cfg);
        cfg
    }
}

/// The data-bus namespace cross-app settings live under.
pub const NAMESPACE: &str = "settings";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_roundtrips() {
        let mut p = Prefs::default();
        p.gesture.sensitivity = 1.6;
        p.palette = "Nord".into();
        p.a11y.tts = true;
        let back = Prefs::from_kv(&p.to_kv());
        assert_eq!(back, p);
    }

    #[test]
    fn partial_kv_keeps_defaults_and_overrides_win() {
        // an app applies only its unique override on top of central defaults
        let mut p = Prefs::default();
        p.apply_kv(&[("gesture.commit_fraction".into(), "0.5".into())]);
        assert_eq!(p.gesture.commit_fraction, 0.5, "override applied");
        assert_eq!(p.gesture.sensitivity, 1.0, "other prefs stay at default");
        assert_eq!(p.palette, "Dark");
    }

    #[test]
    fn updating_gesture_feel_changes_every_apps_config() {
        // central publishes a snappier feel
        let central = Prefs::from_kv(&[
            ("gesture.sensitivity".into(), "2.0".into()),
            ("gesture.commit_fraction".into(), "0.2".into()),
        ]);
        // two different apps build their gesture config from the same prefs → identical feel
        let a = central.gesture_config(320.0);
        let b = central.gesture_config(320.0);
        assert_eq!(a.commit_fraction, 0.2);
        assert_eq!(a.commit_fraction, b.commit_fraction);
        // higher sensitivity → smaller detent threshold than default
        let default_cfg = Prefs::default().gesture_config(320.0);
        assert!(a.detent_threshold < default_cfg.detent_threshold, "more sensitive fires sooner");
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let mut p = Prefs::default();
        p.apply_kv(&[("future.thing".into(), "x".into()), ("gesture.sensitivity".into(), "not-a-number".into())]);
        assert_eq!(p.gesture.sensitivity, 1.0, "bad value ignored, default kept");
    }
}
