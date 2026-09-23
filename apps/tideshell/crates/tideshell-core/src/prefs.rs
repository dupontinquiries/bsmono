//! Shell preferences as flat `key = value` pairs, the shape settingsd's
//! `settings` namespace already carries (see `settings-kit`). Keys are
//! prefixed `shell.` so they sit beside the fleet-wide prefs without
//! colliding, and a settings UI can list them generically.
//!
//! `apply_kv` ignores unknown keys and rejects bad values per key, returning
//! the rejections, so one malformed entry never resets the rest.

use crate::bar::{self, BarWidget};
use crate::edge::EdgeConfig;
use crate::switcher::Style;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Swipe down from the top for the control-center drawer.
    PullDown,
    /// A persistent right-hand bar of mixable widgets; the top edge is inert.
    RightBar,
}

impl Layout {
    pub fn token(self) -> &'static str {
        match self {
            Layout::PullDown => "pull-down",
            Layout::RightBar => "right-bar",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShellPrefs {
    pub layout: Layout,
    pub bar_widgets: Vec<BarWidget>,
    /// Bar width in logical px.
    pub bar_width: f32,
    pub switcher_style: Style,
    pub hold_ms: u64,
    pub trackpad_speed: f32,
    /// iPadOS-style eased disc instead of the system cursor while the
    /// trackpad drives the pointer.
    pub fat_cursor: bool,
    /// Ambient light + time of day brightness.
    pub auto_brightness: bool,
    /// The brightness slider's position within the adaptive band, −1..1,
    /// while auto brightness is on (the slider is relative then).
    pub brightness_offset: f32,
    /// Night window for the time-of-day dimming.
    pub night: crate::ambient::NightWindow,
    /// Backlight write rate, Hz. 30 by default: 90 provoked DSI worker
    /// errors on the FP5's panel; lower it further if they persist.
    pub brightness_rate: u32,
    /// uikit palette id for the shell's own surfaces ("night", "slate",
    /// "slate-dusk", ...). Validated by the frontend against the registry.
    pub theme: String,
    /// Command-palette recents, most recent first (item keys, comma-joined).
    pub palette_recent: String,
    /// Cold mode: auto (battery temperature), on, off.
    pub cold: String,
    /// Ambient loudness from the microphone (notification gain). The mic is
    /// only ever reduced to a level, but it is open while this is on.
    pub ambient_mic: bool,
    /// Audio voicing position, -1..1 (see audio.rs).
    pub audio_tone: f32,
    /// Autorotation on/off, and whether landscape is mirrored on this panel.
    pub autorotate: bool,
    pub rotate_flip: bool,
    /// Cover clock: digital or analog, and how fuzzy.
    pub clock_style: crate::clock::ClockStyle,
    pub clock_fuzzy: crate::clock::Fuzzy,
}

impl Default for ShellPrefs {
    fn default() -> Self {
        ShellPrefs {
            layout: Layout::PullDown,
            bar_widgets: bar::parse_widgets(bar::DEFAULT_WIDGETS).0,
            bar_width: 64.0,
            switcher_style: Style::Grid,
            hold_ms: 420,
            trackpad_speed: 1.0,
            fat_cursor: false,
            auto_brightness: true,
            brightness_offset: 0.0,
            night: crate::ambient::NightWindow::default(),
            brightness_rate: 30,
            theme: "night".into(),
            palette_recent: String::new(),
            cold: "auto".into(),
            ambient_mic: true,
            audio_tone: 0.0,
            autorotate: true,
            rotate_flip: false,
            clock_style: crate::clock::ClockStyle::Digital,
            clock_fuzzy: crate::clock::Fuzzy::Exact,
        }
    }
}

pub const KEYS: [&str; 20] = [
    "shell.layout",
    "shell.bar.widgets",
    "shell.bar.width",
    "shell.switcher.style",
    "shell.hold_ms",
    "shell.trackpad.speed",
    "shell.cursor",
    "shell.brightness.auto",
    "shell.brightness.offset",
    "shell.brightness.night",
    "shell.brightness.rate",
    "shell.theme",
    "shell.palette.recent",
    "shell.cold",
    "shell.ambient.mic",
    "shell.audio.tone",
    "shell.rotate.auto",
    "shell.rotate.flip",
    "shell.clock.style",
    "shell.clock.fuzzy",
];

impl ShellPrefs {
    pub fn to_kv(&self) -> Vec<(String, String)> {
        vec![
            ("shell.layout".into(), self.layout.token().into()),
            ("shell.bar.widgets".into(), bar::serialize_widgets(&self.bar_widgets)),
            ("shell.bar.width".into(), format!("{}", self.bar_width)),
            (
                "shell.switcher.style".into(),
                match self.switcher_style {
                    Style::Grid => "grid",
                    Style::Deck => "deck",
                }
                .into(),
            ),
            ("shell.hold_ms".into(), self.hold_ms.to_string()),
            ("shell.trackpad.speed".into(), format!("{}", self.trackpad_speed)),
            ("shell.cursor".into(), if self.fat_cursor { "fat" } else { "normal" }.into()),
            ("shell.brightness.auto".into(), if self.auto_brightness { "on" } else { "off" }.into()),
            ("shell.brightness.offset".into(), format!("{:.3}", self.brightness_offset)),
            ("shell.brightness.night".into(), self.night.format_span()),
            ("shell.brightness.rate".into(), self.brightness_rate.to_string()),
            ("shell.theme".into(), self.theme.clone()),
            ("shell.palette.recent".into(), self.palette_recent.clone()),
            ("shell.cold".into(), self.cold.clone()),
            ("shell.ambient.mic".into(), if self.ambient_mic { "on" } else { "off" }.into()),
            ("shell.audio.tone".into(), format!("{:.3}", self.audio_tone)),
            ("shell.rotate.auto".into(), if self.autorotate { "on" } else { "off" }.into()),
            ("shell.rotate.flip".into(), if self.rotate_flip { "on" } else { "off" }.into()),
            (
                "shell.clock.style".into(),
                match self.clock_style {
                    crate::clock::ClockStyle::Digital => "digital",
                    crate::clock::ClockStyle::Analog => "analog",
                }
                .into(),
            ),
            ("shell.clock.fuzzy".into(), self.clock_fuzzy.token().into()),
        ]
    }

    /// Apply one pair. `Ok(false)` = not a shell key (ignored).
    pub fn apply(&mut self, key: &str, value: &str) -> Result<bool, String> {
        let v = value.trim();
        match key {
            "shell.layout" => {
                self.layout = match v {
                    "pull-down" => Layout::PullDown,
                    "right-bar" => Layout::RightBar,
                    _ => return Err(format!("shell.layout: '{v}' is not pull-down|right-bar")),
                }
            }
            "shell.bar.widgets" => {
                let (ws, errs) = bar::parse_widgets(v);
                // Keep what parsed; report the rest. An all-bad list would
                // leave an empty bar, so that one case is refused outright.
                if ws.is_empty() && !errs.is_empty() {
                    return Err(format!("shell.bar.widgets: nothing usable ({})", errs.join("; ")));
                }
                self.bar_widgets = ws;
                if !errs.is_empty() {
                    return Err(format!("shell.bar.widgets: ignored {}", errs.join("; ")));
                }
            }
            "shell.bar.width" => self.bar_width = parse_range(key, v, 44.0, 160.0)?,
            "shell.switcher.style" => {
                self.switcher_style = match v {
                    "grid" => Style::Grid,
                    "deck" => Style::Deck,
                    _ => return Err(format!("shell.switcher.style: '{v}' is not grid|deck")),
                }
            }
            "shell.hold_ms" => self.hold_ms = parse_range(key, v, 200.0, 1500.0)? as u64,
            "shell.trackpad.speed" => self.trackpad_speed = parse_range(key, v, 0.25, 4.0)?,
            "shell.brightness.auto" => {
                self.auto_brightness = match v {
                    "on" => true,
                    "off" => false,
                    _ => return Err(format!("shell.brightness.auto: '{v}' is not on|off")),
                }
            }
            "shell.brightness.offset" => self.brightness_offset = parse_range(key, v, -1.0, 1.0)?,
            "shell.brightness.rate" => self.brightness_rate = parse_range(key, v, 2.0, 90.0)? as u32,
            "shell.palette.recent" => self.palette_recent = v.to_string(),
            "shell.cold" => {
                if !matches!(v, "auto" | "on" | "off") {
                    return Err(format!("shell.cold: '{v}' is not auto|on|off"));
                }
                self.cold = v.to_string();
            }
            "shell.ambient.mic" => self.ambient_mic = parse_on_off(key, v)?,
            "shell.audio.tone" => self.audio_tone = parse_range(key, v, -1.0, 1.0)?,
            "shell.rotate.auto" => self.autorotate = parse_on_off(key, v)?,
            "shell.rotate.flip" => self.rotate_flip = parse_on_off(key, v)?,
            "shell.theme" => {
                if v.is_empty() || !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                    return Err(format!("shell.theme: '{v}' is not a palette id"));
                }
                self.theme = v.to_string();
            }
            "shell.brightness.night" => {
                let (start, end) = crate::ambient::NightWindow::parse_span(v).ok_or_else(|| format!("shell.brightness.night: '{v}' is not HH:MM-HH:MM"))?;
                self.night.start = start;
                self.night.end = end;
            }
            "shell.clock.style" => {
                self.clock_style = match v {
                    "digital" => crate::clock::ClockStyle::Digital,
                    "analog" => crate::clock::ClockStyle::Analog,
                    _ => return Err(format!("shell.clock.style: '{v}' is not digital|analog")),
                }
            }
            "shell.clock.fuzzy" => {
                self.clock_fuzzy = crate::clock::Fuzzy::from_token(v).ok_or_else(|| format!("shell.clock.fuzzy: '{v}' is not exact|15|30|hour|weekday"))?
            }
            "shell.cursor" => {
                self.fat_cursor = match v {
                    "fat" => true,
                    "normal" => false,
                    _ => return Err(format!("shell.cursor: '{v}' is not normal|fat")),
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn apply_kv<'a>(&mut self, pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
        pairs.into_iter().filter_map(|(k, v)| self.apply(k, v).err()).collect()
    }

    /// The recognizer config this layout implies.
    pub fn edge_config(&self) -> EdgeConfig {
        EdgeConfig {
            hold_ms: self.hold_ms,
            top_enabled: self.layout == Layout::PullDown,
            // Off in every layout today. It was on in right-bar, but the bar
            // covers the right edge, so a swipe-in over it competed with the
            // bar's own widgets (a right-edge swipe dragged its slider, seen
            // live). The bar takes a horizontal drag to expand instead.
            right_enabled: false,
            ..EdgeConfig::default()
        }
    }
}

fn parse_on_off(key: &str, v: &str) -> Result<bool, String> {
    match v {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(format!("{key}: '{v}' is not on|off")),
    }
}

fn parse_range(key: &str, v: &str, lo: f32, hi: f32) -> Result<f32, String> {
    let x: f32 = v.parse().map_err(|_| format!("{key}: '{v}' is not a number"))?;
    if !(lo..=hi).contains(&x) {
        return Err(format!("{key}: {x} outside {lo}..={hi}"));
    }
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_through_kv() {
        let mut p = ShellPrefs::default();
        p.layout = Layout::RightBar;
        p.switcher_style = Style::Deck;
        p.bar_width = 80.0;
        p.fat_cursor = true;
        p.auto_brightness = false;
        p.brightness_offset = 0.125;
        p.night.start = 23 * 60 + 30;
        p.night.end = 6 * 60;
        p.clock_style = crate::clock::ClockStyle::Analog;
        p.brightness_rate = 12;
        p.theme = "slate-dusk".into();
        p.palette_recent = "mode:keyboard,app:x".into();
        p.autorotate = false;
        p.audio_tone = -0.25;
        p.ambient_mic = false;
        p.cold = "on".into();
        p.rotate_flip = true;
        p.clock_fuzzy = crate::clock::Fuzzy::Quarter;
        p.bar_widgets = bar::parse_widgets("clock,toggle:trackpad,app:org.gnome.Console").0;
        let kv = p.to_kv();
        assert_eq!(kv.len(), KEYS.len());
        assert!(kv.iter().zip(KEYS).all(|((k, _), want)| k == want));
        let mut q = ShellPrefs::default();
        let errs = q.apply_kv(kv.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        assert!(errs.is_empty(), "{errs:?}");
        assert_eq!(p, q);
    }

    #[test]
    fn bad_value_rejected_without_touching_others() {
        let mut p = ShellPrefs::default();
        let errs = p.apply_kv([("shell.layout", "sideways"), ("shell.switcher.style", "deck"), ("shell.hold_ms", "5"), ("other.key", "x")]);
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert_eq!(p.layout, Layout::PullDown);
        assert_eq!(p.switcher_style, Style::Deck);
        assert_eq!(p.hold_ms, 420);
    }

    #[test]
    fn partly_bad_widget_list_keeps_good_part_all_bad_is_refused() {
        let mut p = ShellPrefs::default();
        let e = p.apply("shell.bar.widgets", "clock,bogus");
        assert!(e.is_err());
        assert_eq!(p.bar_widgets, vec![BarWidget::Clock]);
        let before = p.bar_widgets.clone();
        assert!(p.apply("shell.bar.widgets", "bogus,worse").is_err());
        assert_eq!(p.bar_widgets, before, "all-bad list must not empty the bar");
    }

    #[test]
    fn layout_drives_which_edges_are_live() {
        let mut p = ShellPrefs::default();
        let c = p.edge_config();
        assert!(c.top_enabled && !c.right_enabled);
        p.layout = Layout::RightBar;
        let c = p.edge_config();
        assert!(!c.top_enabled && !c.right_enabled, "the bar owns the right edge");
    }
}
