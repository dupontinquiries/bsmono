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
    /// Detent/transition animation feel.
    pub animation: AnimationStyle,
}

impl Default for GesturePrefs {
    fn default() -> Self {
        GesturePrefs { sensitivity: 1.0, commit_fraction: 0.35, flick_velocity: 0.9, animation: AnimationStyle::Smooth }
    }
}

/// Touch ergonomics that layer over the adaptive defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPrefs {
    /// Minimum interactive size in points (0 = let the adaptive layer decide).
    pub min_target: f32,
    /// Extra UI scale multiplier (1.0 = adaptive default).
    pub scale: f32,
    /// Text size multiplier across apps (1.0 = default).
    pub text_scale: f32,
    /// Button/control size multiplier across apps (1.0 = default).
    pub button_scale: f32,
}

impl Default for TouchPrefs {
    fn default() -> Self {
        TouchPrefs { min_target: 0.0, scale: 1.0, text_scale: 1.0, button_scale: 1.0 }
    }
}

/// Detent/transition animation feel — the "snappy vs smooth vs heavy" choice. Maps onto the
/// gesture engine's timing so every app animates the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationStyle {
    /// Fast, minimal settle — immediate response.
    Snappy,
    /// Balanced, framerate-friendly (default).
    Smooth,
    /// Weighty, more inertia and settle.
    Heavy,
    /// Minimal motion (reduce-motion accessibility) — near-instant detents.
    Reduced,
}

impl AnimationStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            AnimationStyle::Snappy => "snappy",
            AnimationStyle::Smooth => "smooth",
            AnimationStyle::Heavy => "heavy",
            AnimationStyle::Reduced => "reduced",
        }
    }
    pub fn parse(s: &str) -> AnimationStyle {
        match s {
            "snappy" => AnimationStyle::Snappy,
            "heavy" => AnimationStyle::Heavy,
            "reduced" => AnimationStyle::Reduced,
            _ => AnimationStyle::Smooth,
        }
    }
    pub fn all() -> [AnimationStyle; 4] {
        [AnimationStyle::Snappy, AnimationStyle::Smooth, AnimationStyle::Heavy, AnimationStyle::Reduced]
    }
    pub fn label(self) -> &'static str {
        match self {
            AnimationStyle::Snappy => "Snappy",
            AnimationStyle::Smooth => "Smooth (fps)",
            AnimationStyle::Heavy => "Heavy",
            AnimationStyle::Reduced => "Reduced motion",
        }
    }
    /// Apply the animation timing to a `GestureConfig` (arm window, rate, takeover lock).
    pub fn apply_to_gesture(self, cfg: &mut GestureConfig) {
        let (arm, rate, lock) = match self {
            AnimationStyle::Snappy => (180.0, 2.5, 60.0),
            AnimationStyle::Smooth => (280.0, 1.5, 120.0),
            AnimationStyle::Heavy => (420.0, 1.0, 220.0),
            AnimationStyle::Reduced => (120.0, 4.0, 0.0),
        };
        cfg.detent_arm_window_ms = arm;
        cfg.detent_rate_per_ms = rate;
        cfg.takeover_lock_ms = lock;
    }

    /// A multiplier on visual (egui) animation durations, matching the same feel as
    /// `apply_to_gesture`: Snappy is quick, Heavy lingers, Reduced is instant. uikit-egui uses this
    /// to scale `Context::animation_time` (and its carved sheen etc.) so every app that draws with
    /// the kit animates the way the picker says.
    pub fn time_scale(self) -> f32 {
        match self {
            AnimationStyle::Snappy => 0.5,
            AnimationStyle::Smooth => 1.0,
            AnimationStyle::Heavy => 1.8,
            AnimationStyle::Reduced => 0.0,
        }
    }
}

/// Which hand the UI favours — controls/handles sit on the dominant side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handedness {
    Right,
    Left,
}

impl Handedness {
    pub fn as_str(self) -> &'static str {
        match self {
            Handedness::Right => "right",
            Handedness::Left => "left",
        }
    }
    pub fn parse(s: &str) -> Handedness {
        if s == "left" { Handedness::Left } else { Handedness::Right }
    }
    pub fn all() -> [Handedness; 2] {
        [Handedness::Right, Handedness::Left]
    }
    pub fn label(self) -> &'static str {
        match self {
            Handedness::Right => "Right-handed",
            Handedness::Left => "Left-handed",
        }
    }
    pub fn flipped(self) -> Handedness {
        match self {
            Handedness::Right => Handedness::Left,
            Handedness::Left => Handedness::Right,
        }
    }
}

/// Input/handedness prefs, incl. the toggleable double-tilt gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPrefs {
    pub handedness: Handedness,
    /// Whether a double physical tilt of the device flips handedness (on-device gesture).
    pub double_tilt: bool,
    /// Movement keybind layout — one shared setting every game reads (see [`MovementLayout`]).
    pub movement: MovementLayout,
}

impl Default for InputPrefs {
    fn default() -> Self {
        InputPrefs { handedness: Handedness::Right, double_tilt: false, movement: MovementLayout::default() }
    }
}

/// Movement keybind layout — the WASD cluster and its counterparts on other keyboard layouts. Shared
/// so every game reads *one* setting and the settings app offers *one* picker; each game maps the
/// abstract key labels to its own key type (egui `Key`, winit `KeyCode`, …) via [`MovementLayout::keys`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MovementLayout {
    /// QWERTY: W A S D.
    #[default]
    Wasd,
    /// Dvorak: `.` O E U — the WASD inverted-T reproduced by physical key position.
    Dvorak,
    /// QWERTY ESDF — one column right of WASD (frees a column of keys under the left hand).
    Esdf,
    /// Arrow keys.
    Arrows,
}

impl MovementLayout {
    pub fn as_str(self) -> &'static str {
        match self {
            MovementLayout::Wasd => "wasd",
            MovementLayout::Dvorak => "dvorak",
            MovementLayout::Esdf => "esdf",
            MovementLayout::Arrows => "arrows",
        }
    }
    pub fn parse(s: &str) -> MovementLayout {
        match s {
            "dvorak" => MovementLayout::Dvorak,
            "esdf" => MovementLayout::Esdf,
            "arrows" => MovementLayout::Arrows,
            _ => MovementLayout::Wasd,
        }
    }
    pub fn all() -> [MovementLayout; 4] {
        [MovementLayout::Wasd, MovementLayout::Dvorak, MovementLayout::Esdf, MovementLayout::Arrows]
    }
    pub fn label(self) -> &'static str {
        match self {
            MovementLayout::Wasd => "WASD (QWERTY)",
            MovementLayout::Dvorak => ".oeu (Dvorak)",
            MovementLayout::Esdf => "ESDF (QWERTY)",
            MovementLayout::Arrows => "Arrow keys",
        }
    }
    /// `[forward, left, back, right]` as lowercase key labels a game maps to its own key enum — the
    /// inverted-T movement cluster on the chosen layout ("up"/"left"/"down"/"right" for `Arrows`).
    pub fn keys(self) -> [&'static str; 4] {
        match self {
            MovementLayout::Wasd => ["w", "a", "s", "d"],
            MovementLayout::Dvorak => [".", "o", "e", "u"],
            MovementLayout::Esdf => ["e", "s", "d", "f"],
            MovementLayout::Arrows => ["up", "left", "down", "right"],
        }
    }
}

/// The action a double-tilt performs when enabled: flip handedness. Pure so the on-device
/// accelerometer handler (which detects the tilt) can call it and publish the new value.
pub fn double_tilt_action(prefs: &InputPrefs) -> Option<Handedness> {
    if prefs.double_tilt { Some(prefs.handedness.flipped()) } else { None }
}

/// How aggressively to filter voice input (noise / whisper hallucinations / non-speech).
/// The levels map onto voice-kit's filtering, which voiced applies to each transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceFilter {
    /// Accept transcripts as-is.
    Off,
    /// Trim trailing artifacts only.
    Light,
    /// Also drop obvious whisper hallucinations (default).
    Standard,
    /// Also require the text to look like real speech.
    Strict,
}

impl VoiceFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            VoiceFilter::Off => "off",
            VoiceFilter::Light => "light",
            VoiceFilter::Standard => "standard",
            VoiceFilter::Strict => "strict",
        }
    }
    pub fn parse(s: &str) -> VoiceFilter {
        match s {
            "off" => VoiceFilter::Off,
            "light" => VoiceFilter::Light,
            "strict" => VoiceFilter::Strict,
            _ => VoiceFilter::Standard,
        }
    }
    pub fn all() -> [VoiceFilter; 4] {
        [VoiceFilter::Off, VoiceFilter::Light, VoiceFilter::Standard, VoiceFilter::Strict]
    }
    pub fn label(self) -> &'static str {
        match self {
            VoiceFilter::Off => "Off",
            VoiceFilter::Light => "Light (trim artifacts)",
            VoiceFilter::Standard => "Standard (drop hallucinations)",
            VoiceFilter::Strict => "Strict (speech only)",
        }
    }
    /// Should trailing whisper artifacts be stripped?
    pub fn strips_artifacts(self) -> bool {
        self != VoiceFilter::Off
    }
    /// Should obvious hallucinations be dropped?
    pub fn filters_hallucinations(self) -> bool {
        matches!(self, VoiceFilter::Standard | VoiceFilter::Strict)
    }
    /// Should the text be required to look like real speech?
    pub fn requires_speech_like(self) -> bool {
        self == VoiceFilter::Strict
    }
}

/// Accessibility toggles (see uikit-egui::a11y).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct A11yPrefs {
    pub voice: bool,
    pub tts: bool,
    /// Voice input filtering strength.
    pub voice_filter: VoiceFilter,
}

impl Default for A11yPrefs {
    fn default() -> Self {
        A11yPrefs { voice: true, tts: false, voice_filter: VoiceFilter::Standard }
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
    /// Surface opacity: 1.0 = opaque, down to ~0.6 = translucent panels/cards (lets a live
    /// background or the desktop show through). Applied by uikit-egui to panel/window fills.
    pub opacity: f32,
}

impl Default for DisplayPrefs {
    fn default() -> Self {
        DisplayPrefs { white_point: 1.0, color_filter: ColorFilter::None, opacity: 1.0 }
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
        self.white_point < 0.999 || self.color_filter != ColorFilter::None || self.opacity < 0.999
    }
}

/// The full cross-app preference set.
/// Fleet voice-mesh preferences (surfaced in bsettings): whether this device's mic joins the mesh,
/// whether heavy STT may be offloaded to a trusted host, the preferred host id ("" = auto-pick the
/// best trusted heavy device), and whether multi-user speaker recognition is on.
#[derive(Debug, Clone, PartialEq)]
pub struct VoicePrefs {
    pub mic_enabled: bool,
    pub offload_enabled: bool,
    pub compute_host: String,
    pub speaker_id: bool,
    /// Preference-ordered trusted voice-compute partners (tier list, best first). Managed in
    /// bsettings' voice sub-page; the mesh prefers the highest-ranked reachable Heavy partner.
    pub partners: Vec<String>,
}
impl Default for VoicePrefs {
    fn default() -> Self {
        VoicePrefs { mic_enabled: true, offload_enabled: true, compute_host: String::new(), speaker_id: false, partners: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub gesture: GesturePrefs,
    pub touch: TouchPrefs,
    pub a11y: A11yPrefs,
    pub display: DisplayPrefs,
    pub input: InputPrefs,
    /// snapclone-theme palette id (e.g. "Dark").
    pub palette: String,
    /// Fleet voice-mesh preferences.
    pub voice: VoicePrefs,
    /// Directory of shareable model files for the fabric model-storage pool (empty = none). The
    /// logscope fabric daemon serves this dir; peers pull weights from it over the LAN.
    pub pool_dir: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            gesture: GesturePrefs::default(),
            touch: TouchPrefs::default(),
            a11y: A11yPrefs::default(),
            display: DisplayPrefs::default(),
            input: InputPrefs::default(),
            palette: "Dark".to_string(),
            voice: VoicePrefs::default(),
            pool_dir: String::new(),
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
            ("gesture.animation".into(), self.gesture.animation.as_str().to_string()),
            ("touch.min_target".into(), self.touch.min_target.to_string()),
            ("touch.scale".into(), self.touch.scale.to_string()),
            ("touch.text_scale".into(), self.touch.text_scale.to_string()),
            ("touch.button_scale".into(), self.touch.button_scale.to_string()),
            ("a11y.voice".into(), self.a11y.voice.to_string()),
            ("a11y.tts".into(), self.a11y.tts.to_string()),
            ("a11y.voice_filter".into(), self.a11y.voice_filter.as_str().to_string()),
            ("display.white_point".into(), self.display.white_point.to_string()),
            ("display.color_filter".into(), self.display.color_filter.as_str().to_string()),
            ("display.opacity".into(), self.display.opacity.to_string()),
            ("input.handedness".into(), self.input.handedness.as_str().to_string()),
            ("input.double_tilt".into(), self.input.double_tilt.to_string()),
            ("input.movement".into(), self.input.movement.as_str().to_string()),
            ("theme.palette".into(), self.palette.clone()),
            ("voice.mic_enabled".into(), self.voice.mic_enabled.to_string()),
            ("voice.offload_enabled".into(), self.voice.offload_enabled.to_string()),
            ("voice.compute_host".into(), self.voice.compute_host.clone()),
            ("voice.speaker_id".into(), self.voice.speaker_id.to_string()),
            ("voice.partners".into(), self.voice.partners.join(",")),
            ("model.pool_dir".into(), self.pool_dir.clone()),
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
                "gesture.animation" => self.gesture.animation = AnimationStyle::parse(v),
                "touch.min_target" => if let Ok(x) = v.parse() { self.touch.min_target = x },
                "touch.scale" => if let Ok(x) = v.parse() { self.touch.scale = x },
                "touch.text_scale" => if let Ok(x) = v.parse() { self.touch.text_scale = x },
                "touch.button_scale" => if let Ok(x) = v.parse() { self.touch.button_scale = x },
                "a11y.voice" => if let Ok(x) = v.parse() { self.a11y.voice = x },
                "a11y.tts" => if let Ok(x) = v.parse() { self.a11y.tts = x },
                "a11y.voice_filter" => self.a11y.voice_filter = VoiceFilter::parse(v),
                "voice.mic_enabled" => if let Ok(x) = v.parse() { self.voice.mic_enabled = x },
                "voice.offload_enabled" => if let Ok(x) = v.parse() { self.voice.offload_enabled = x },
                "voice.compute_host" => self.voice.compute_host = v.to_string(),
                "voice.speaker_id" => if let Ok(x) = v.parse() { self.voice.speaker_id = x },
                "voice.partners" => self.voice.partners = v.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                "model.pool_dir" => self.pool_dir = v.clone(),
                "display.white_point" => if let Ok(x) = v.parse() { self.display.white_point = x },
                "display.color_filter" => self.display.color_filter = ColorFilter::parse(v),
                "display.opacity" => if let Ok(x) = v.parse::<f32>() { self.display.opacity = x.clamp(0.3, 1.0) },
                "input.handedness" => self.input.handedness = Handedness::parse(v),
                "input.double_tilt" => if let Ok(x) = v.parse() { self.input.double_tilt = x },
                "input.movement" => self.input.movement = MovementLayout::parse(v),
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
        self.gesture.animation.apply_to_gesture(cfg);
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
    fn swipe_and_interaction_prefs_roundtrip() {
        let mut p = Prefs::default();
        p.gesture.animation = AnimationStyle::Heavy;
        p.touch.text_scale = 1.3;
        p.touch.button_scale = 1.5;
        p.input.handedness = Handedness::Left;
        p.input.double_tilt = true;
        let back = Prefs::from_kv(&p.to_kv());
        assert_eq!(back.gesture.animation, AnimationStyle::Heavy);
        assert_eq!(back.touch.text_scale, 1.3);
        assert_eq!(back.touch.button_scale, 1.5);
        assert_eq!(back.input.handedness, Handedness::Left);
        assert!(back.input.double_tilt);
    }

    #[test]
    fn animation_styles_change_gesture_timing() {
        let mut sp = Prefs::default();
        sp.gesture.animation = AnimationStyle::Snappy;
        let snappy = sp.gesture_config(320.0);
        let mut hp = Prefs::default();
        hp.gesture.animation = AnimationStyle::Heavy;
        let heavy = hp.gesture_config(320.0);
        assert!(heavy.takeover_lock_ms > snappy.takeover_lock_ms);
        assert!(heavy.detent_rate_per_ms < snappy.detent_rate_per_ms);
        let mut rp = Prefs::default();
        rp.gesture.animation = AnimationStyle::Reduced;
        assert_eq!(rp.gesture_config(320.0).takeover_lock_ms, 0.0);
    }

    #[test]
    fn movement_layout_presets_and_persist() {
        // the .oeu Dvorak preset the request asked for, plus WASD
        assert_eq!(MovementLayout::Wasd.keys(), ["w", "a", "s", "d"]);
        assert_eq!(MovementLayout::Dvorak.keys(), [".", "o", "e", "u"]);
        assert_eq!(MovementLayout::parse("dvorak"), MovementLayout::Dvorak);
        assert_eq!(MovementLayout::parse("nonsense"), MovementLayout::Wasd); // forward-compatible default
        assert_eq!(MovementLayout::default(), MovementLayout::Wasd);
        // survives the shared KV round-trip (so every app + the settings app agree)
        let mut p = Prefs::default();
        p.input.movement = MovementLayout::Dvorak;
        let back = Prefs::from_kv(&p.to_kv());
        assert_eq!(back.input.movement, MovementLayout::Dvorak);
    }

    #[test]
    fn double_tilt_flips_handedness_only_when_enabled() {
        let mut input = InputPrefs { handedness: Handedness::Right, double_tilt: false, movement: MovementLayout::default() };
        assert_eq!(double_tilt_action(&input), None, "disabled → no effect");
        input.double_tilt = true;
        assert_eq!(double_tilt_action(&input), Some(Handedness::Left), "flips when enabled");
    }

    #[test]
    fn voice_filter_roundtrip_and_levels() {
        let mut p = Prefs::default();
        p.a11y.voice_filter = VoiceFilter::Strict;
        assert_eq!(Prefs::from_kv(&p.to_kv()).a11y.voice_filter, VoiceFilter::Strict);
        // level semantics voiced uses to pick voice-kit filters
        assert!(!VoiceFilter::Off.strips_artifacts());
        assert!(VoiceFilter::Light.strips_artifacts() && !VoiceFilter::Light.filters_hallucinations());
        assert!(VoiceFilter::Standard.filters_hallucinations() && !VoiceFilter::Standard.requires_speech_like());
        assert!(VoiceFilter::Strict.requires_speech_like());
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
        let d = DisplayPrefs { white_point: 0.5, color_filter: ColorFilter::Grayscale, opacity: 1.0 };
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

#[cfg(test)]
mod anim_scale_tests {
    use super::AnimationStyle;
    #[test]
    fn time_scale_orders_and_reduced_is_instant() {
        assert_eq!(AnimationStyle::Reduced.time_scale(), 0.0);
        assert_eq!(AnimationStyle::Smooth.time_scale(), 1.0);
        assert!(AnimationStyle::Snappy.time_scale() < AnimationStyle::Smooth.time_scale());
        assert!(AnimationStyle::Smooth.time_scale() < AnimationStyle::Heavy.time_scale());
    }
}

#[cfg(test)]
mod voice_prefs_tests {
    use super::Prefs;
    #[test]
    fn voice_prefs_round_trip_through_kv() {
        let mut p = Prefs::default();
        p.voice.mic_enabled = false;
        p.voice.offload_enabled = false;
        p.voice.compute_host = "desktop".into();
        p.voice.speaker_id = true;
        p.voice.partners = vec!["desktop".into(), "laptop".into()];
        let back = Prefs::from_kv(&p.to_kv());
        assert_eq!(back.voice, p.voice);
    }
}
