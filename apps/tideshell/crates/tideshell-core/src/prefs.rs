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
        }
    }
}

pub const KEYS: [&str; 7] = [
    "shell.layout",
    "shell.bar.widgets",
    "shell.bar.width",
    "shell.switcher.style",
    "shell.hold_ms",
    "shell.trackpad.speed",
    "shell.cursor",
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
