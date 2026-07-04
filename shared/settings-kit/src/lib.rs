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

/// A whole-screen colour filter for accessibility (colour-vision assistance / grayscale /
/// warmth). Applied as a matrix over each colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorFilter {
    None,
    Grayscale,
    Protanopia,
    Deuteranopia,
    Tritanopia,
    /// Reduce blue light (warm / night).
    Warm,
}

impl ColorFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            ColorFilter::None => "none",
            ColorFilter::Grayscale => "grayscale",
            ColorFilter::Protanopia => "protanopia",
            ColorFilter::Deuteranopia => "deuteranopia",
            ColorFilter::Tritanopia => "tritanopia",
            ColorFilter::Warm => "warm",
        }
    }
    pub fn parse(s: &str) -> ColorFilter {
        match s {
            "grayscale" => ColorFilter::Grayscale,
            "protanopia" => ColorFilter::Protanopia,
            "deuteranopia" => ColorFilter::Deuteranopia,
            "tritanopia" => ColorFilter::Tritanopia,
            "warm" => ColorFilter::Warm,
            _ => ColorFilter::None,
        }
    }
    pub fn all() -> [ColorFilter; 6] {
        [
            ColorFilter::None,
            ColorFilter::Grayscale,
            ColorFilter::Protanopia,
            ColorFilter::Deuteranopia,
            ColorFilter::Tritanopia,
            ColorFilter::Warm,
        ]
    }
    /// A human label for a settings picker.
    pub fn label(self) -> &'static str {
        match self {
            ColorFilter::None => "Off",
            ColorFilter::Grayscale => "Grayscale",
            ColorFilter::Protanopia => "Red–green (protanopia)",
            ColorFilter::Deuteranopia => "Red–green (deuteranopia)",
            ColorFilter::Tritanopia => "Blue–yellow (tritanopia)",
            ColorFilter::Warm => "Warm (reduce blue)",
        }
    }
}

/// Display accessibility: lower the white point (dim bright whites for light sensitivity) and
/// apply a colour filter. These are cross-app so the whole fleet dims/filters together.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayPrefs {
    /// 0.4 (strongly dimmed whites) … 1.0 (normal). Scales luminance so white → grey.
    pub white_point: f32,
    pub color_filter: ColorFilter,
}

impl Default for DisplayPrefs {
    fn default() -> Self {
        DisplayPrefs { white_point: 1.0, color_filter: ColorFilter::None }
    }
}

/// Lower the white point: scale each channel toward black (white → grey). `wp` in [0,1].
pub fn apply_white_point(rgb: (f32, f32, f32), wp: f32) -> (f32, f32, f32) {
    let wp = wp.clamp(0.0, 1.0);
    (rgb.0 * wp, rgb.1 * wp, rgb.2 * wp)
}

/// Apply a colour filter to a linear-ish 0..1 RGB triple (standard colour-vision matrices).
pub fn apply_color_filter(rgb: (f32, f32, f32), f: ColorFilter) -> (f32, f32, f32) {
    let (r, g, b) = rgb;
    let out = match f {
        ColorFilter::None => (r, g, b),
        ColorFilter::Grayscale => {
            let l = 0.299 * r + 0.587 * g + 0.114 * b;
            (l, l, l)
        }
        ColorFilter::Protanopia => (
            0.567 * r + 0.433 * g,
            0.558 * r + 0.442 * g,
            0.242 * g + 0.758 * b,
        ),
        ColorFilter::Deuteranopia => (
            0.625 * r + 0.375 * g,
            0.70 * r + 0.30 * g,
            0.30 * g + 0.70 * b,
        ),
        ColorFilter::Tritanopia => (
            0.95 * r + 0.05 * g,
            0.433 * g + 0.567 * b,
            0.475 * g + 0.525 * b,
        ),
        ColorFilter::Warm => (r, g * 0.95, b * 0.75),
    };
    (out.0.clamp(0.0, 1.0), out.1.clamp(0.0, 1.0), out.2.clamp(0.0, 1.0))
}

impl DisplayPrefs {
    /// Apply the full display transform (filter, then white-point) to an 0..1 RGB triple.
    pub fn apply(&self, rgb: (f32, f32, f32)) -> (f32, f32, f32) {
        apply_white_point(apply_color_filter(rgb, self.color_filter), self.white_point)
    }
    /// Is any display adjustment active (worth doing the per-colour work)?
    pub fn is_active(&self) -> bool {
        self.white_point < 0.999 || self.color_filter != ColorFilter::None
    }
}

/// The full cross-app preference set.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub gesture: GesturePrefs,
    pub touch: TouchPrefs,
    pub a11y: A11yPrefs,
    pub display: DisplayPrefs,
    /// snapclone-theme palette id (e.g. "Dark").
    pub palette: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            gesture: GesturePrefs::default(),
            touch: TouchPrefs::default(),
            a11y: A11yPrefs::default(),
            display: DisplayPrefs::default(),
            palette: "Dark".to_string(),
        }
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
            ("display.white_point".into(), self.display.white_point.to_string()),
            ("display.color_filter".into(), self.display.color_filter.as_str().to_string()),
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
                "display.white_point" => if let Ok(x) = v.parse() { self.display.white_point = x },
                "display.color_filter" => self.display.color_filter = ColorFilter::parse(v),
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

/// The changed keys between two Prefs (for publishing only what the user touched).
pub fn diff(old: &Prefs, new: &Prefs) -> Vec<(String, String)> {
    let (o, n) = (old.to_kv(), new.to_kv());
    n.into_iter()
        .zip(o)
        .filter(|((_, nv), (_, ov))| nv != ov)
        .map(|((k, nv), _)| (k, nv))
        .collect()
}

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

    #[test]
    fn display_prefs_roundtrip_and_transform() {
        let mut p = Prefs::default();
        p.display.white_point = 0.6;
        p.display.color_filter = ColorFilter::Deuteranopia;
        let back = Prefs::from_kv(&p.to_kv());
        assert_eq!(back.display.white_point, 0.6);
        assert_eq!(back.display.color_filter, ColorFilter::Deuteranopia);
    }

    #[test]
    fn white_point_dims_whites() {
        // white at wp 0.5 becomes mid-grey; black stays black; wp 1.0 is identity
        assert_eq!(apply_white_point((1.0, 1.0, 1.0), 0.5), (0.5, 0.5, 0.5));
        assert_eq!(apply_white_point((0.0, 0.0, 0.0), 0.5), (0.0, 0.0, 0.0));
        assert_eq!(apply_white_point((0.8, 0.4, 0.2), 1.0), (0.8, 0.4, 0.2));
    }

    #[test]
    fn color_filters_transform_expectedly() {
        // grayscale collapses channels to a single luminance
        let (r, g, b) = apply_color_filter((1.0, 0.0, 0.0), ColorFilter::Grayscale);
        assert!((r - g).abs() < 1e-6 && (g - b).abs() < 1e-6);
        // None is identity
        assert_eq!(apply_color_filter((0.3, 0.6, 0.9), ColorFilter::None), (0.3, 0.6, 0.9));
        // a colour-vision filter actually changes pure red
        assert_ne!(apply_color_filter((1.0, 0.0, 0.0), ColorFilter::Protanopia), (1.0, 0.0, 0.0));
        // warm reduces blue
        let warm = apply_color_filter((1.0, 1.0, 1.0), ColorFilter::Warm);
        assert!(warm.2 < warm.0);
    }

    #[test]
    fn display_apply_combines_filter_and_white_point() {
        let d = DisplayPrefs { white_point: 0.5, color_filter: ColorFilter::Grayscale };
        assert!(d.is_active());
        let (r, g, b) = d.apply((1.0, 1.0, 1.0));
        // grayscale(white)=white, then wp 0.5 → 0.5 grey
        assert!((r - 0.5).abs() < 1e-6 && (g - 0.5).abs() < 1e-6 && (b - 0.5).abs() < 1e-6);
        assert!(!DisplayPrefs::default().is_active());
    }

    #[test]
    fn diff_emits_only_changed_keys() {
        let old = Prefs::default();
        let mut new = old.clone();
        new.gesture.sensitivity = 1.8;
        new.palette = "Nord".into();
        let d = diff(&old, &new);
        assert_eq!(d.len(), 2);
        assert!(d.contains(&("gesture.sensitivity".to_string(), "1.8".to_string())));
        assert!(d.contains(&("theme.palette".to_string(), "Nord".to_string())));
        assert!(diff(&old, &old).is_empty());
    }
}
