//! The right-hand bar: a vertical, thumb-reachable column of user-chosen
//! widgets, any mix, any order. It is the alternative to the pull-down drawer
//! (`prefs::Layout::RightBar`), not an addition to it.
//!
//! The widget list is stored as one flat string so it rides settingsd's
//! key/value namespace unchanged:
//!
//! ```text
//! shell.bar.widgets = clock,battery,toggle:wifi,slider:brightness,spacer,favorites:4,hatch
//! ```
//!
//! Unknown tokens are reported, not silently dropped, so a settings editor
//! can show "1 widget from a newer version was ignored" instead of eating it.

use crate::control_center::{QuickKind, SliderKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BarWidget {
    Clock,
    Battery,
    /// One quick toggle (system or shell mode).
    Toggle(QuickKind),
    /// Vertical slider.
    Slider(SliderKind),
    /// One pinned app, by desktop-entry id (hatch-core's id).
    App(String),
    /// The first `n` hatch favorites.
    Favorites(u8),
    /// Running windows, most recent first; takes whatever room is left.
    Running,
    /// Opens hatch's full grid.
    Hatch,
    /// Opens the switcher (for a bar user who prefers a button to the swipe).
    Switcher,
    /// Soaks up spare height, pushing later widgets down.
    Spacer,
}

impl BarWidget {
    pub fn token(&self) -> String {
        match self {
            BarWidget::Clock => "clock".into(),
            BarWidget::Battery => "battery".into(),
            BarWidget::Toggle(k) => format!("toggle:{}", k.token()),
            BarWidget::Slider(k) => format!("slider:{}", k.token()),
            BarWidget::App(id) => format!("app:{id}"),
            BarWidget::Favorites(n) => format!("favorites:{n}"),
            BarWidget::Running => "running".into(),
            BarWidget::Hatch => "hatch".into(),
            BarWidget::Switcher => "switcher".into(),
            BarWidget::Spacer => "spacer".into(),
        }
    }

    pub fn from_token(tok: &str) -> Result<BarWidget, String> {
        let tok = tok.trim();
        let (kind, arg) = match tok.split_once(':') {
            Some((k, a)) => (k, Some(a)),
            None => (tok, None),
        };
        let w = match (kind, arg) {
            ("clock", None) => BarWidget::Clock,
            ("battery", None) => BarWidget::Battery,
            ("running", None) => BarWidget::Running,
            ("hatch", None) => BarWidget::Hatch,
            ("switcher", None) => BarWidget::Switcher,
            ("spacer", None) => BarWidget::Spacer,
            ("toggle", Some(a)) => BarWidget::Toggle(QuickKind::from_token(a).ok_or_else(|| format!("unknown toggle '{a}'"))?),
            ("slider", Some(a)) => BarWidget::Slider(SliderKind::from_token(a).ok_or_else(|| format!("unknown slider '{a}'"))?),
            // Desktop ids may contain ':'? They may not (spec: [A-Za-z0-9-_.]),
            // but split_once keeps everything after the first ':' regardless.
            ("app", Some(a)) if !a.is_empty() => BarWidget::App(a.to_string()),
            ("favorites", Some(a)) => {
                let n: u8 = a.parse().map_err(|_| format!("favorites count '{a}' is not a number"))?;
                BarWidget::Favorites(n.clamp(1, 12))
            }
            _ => return Err(format!("unknown bar widget '{tok}'")),
        };
        Ok(w)
    }

    /// Natural height in bar units (one unit ≈ one thumb-sized cell).
    /// `None` = flexible: gets a share of the leftover.
    pub fn units(&self) -> Option<u32> {
        match self {
            BarWidget::Clock => Some(2),
            BarWidget::Battery => Some(1),
            BarWidget::Toggle(_) => Some(1),
            BarWidget::Slider(_) => Some(4),
            BarWidget::App(_) => Some(1),
            BarWidget::Favorites(n) => Some(*n as u32),
            BarWidget::Hatch | BarWidget::Switcher => Some(1),
            BarWidget::Running | BarWidget::Spacer => None,
        }
    }

    /// Flexible widgets still need a floor, or `running` could vanish.
    fn min_units(&self) -> u32 {
        match self {
            BarWidget::Running => 2,
            _ => 0,
        }
    }
}

pub const DEFAULT_WIDGETS: &str =
    "clock,battery,toggle:wifi,toggle:bluetooth,slider:brightness,spacer,running,favorites:3,hatch";

/// Parse a widget list. Returns what parsed plus one message per bad token.
pub fn parse_widgets(s: &str) -> (Vec<BarWidget>, Vec<String>) {
    let mut ok = Vec::new();
    let mut errs = Vec::new();
    for tok in s.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        match BarWidget::from_token(tok) {
            Ok(w) => ok.push(w),
            Err(e) => errs.push(e),
        }
    }
    (ok, errs)
}

pub fn serialize_widgets(ws: &[BarWidget]) -> String {
    ws.iter().map(BarWidget::token).collect::<Vec<_>>().join(",")
}

/// A widget's slot in the laid-out bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// Index into the widget list.
    pub index: usize,
    pub y: f32,
    pub h: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BarLayout {
    pub placed: Vec<Placed>,
    /// Widgets that did not fit, in list order. The frontend can show a
    /// "more" affordance; the settings editor warns about it.
    pub overflow: Vec<usize>,
}

/// Lay widgets out top to bottom in `height` px with `unit` px per unit.
///
/// Fixed widgets are placed in list order while they fit; once one does not,
/// it and every later fixed widget overflow (skipping ahead to smaller ones
/// would scramble the order the user chose). Flexible widgets then split the
/// remaining height evenly above their floors.
pub fn layout(ws: &[BarWidget], height: f32, unit: f32) -> BarLayout {
    let unit = unit.max(1.0);
    let floor_px: f32 = ws.iter().map(|w| w.min_units() as f32 * unit).sum();
    let mut budget = (height - floor_px).max(0.0);
    let mut fits = vec![false; ws.len()];
    let mut overflowing = false;
    for (i, w) in ws.iter().enumerate() {
        match w.units() {
            Some(u) => {
                let need = u as f32 * unit;
                if !overflowing && need <= budget + 0.5 {
                    budget -= need;
                    fits[i] = true;
                } else {
                    overflowing = true;
                }
            }
            None => fits[i] = true,
        }
    }
    let flex_count = ws.iter().enumerate().filter(|(i, w)| fits[*i] && w.units().is_none()).count();
    let share = if flex_count > 0 { budget / flex_count as f32 } else { 0.0 };

    let mut y = 0.0;
    let mut placed = Vec::new();
    let mut overflow = Vec::new();
    for (i, w) in ws.iter().enumerate() {
        if !fits[i] {
            overflow.push(i);
            continue;
        }
        let h = match w.units() {
            Some(u) => u as f32 * unit,
            None => w.min_units() as f32 * unit + share,
        };
        placed.push(Placed { index: i, y, h });
        y += h;
    }
    BarLayout { placed, overflow }
}

/// Editing operations for the settings screen. All bounds-checked no-ops
/// rather than panics: the editor is driven by taps that can race a reload.
pub fn move_widget(ws: &mut [BarWidget], from: usize, to: usize) {
    if from >= ws.len() || to >= ws.len() || from == to {
        return;
    }
    if from < to {
        ws[from..=to].rotate_left(1);
    } else {
        ws[to..=from].rotate_right(1);
    }
}

pub fn remove_widget(ws: &mut Vec<BarWidget>, at: usize) {
    if at < ws.len() {
        ws.remove(at);
    }
}

/// The palette a settings editor offers for "add widget".
pub fn catalog() -> Vec<BarWidget> {
    let mut v = vec![BarWidget::Clock, BarWidget::Battery, BarWidget::Running, BarWidget::Hatch, BarWidget::Switcher, BarWidget::Spacer, BarWidget::Favorites(4)];
    v.extend(QuickKind::ALL.into_iter().map(BarWidget::Toggle));
    v.push(BarWidget::Slider(SliderKind::Brightness));
    v.push(BarWidget::Slider(SliderKind::Volume));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_list_parses_clean_and_round_trips() {
        let (ws, errs) = parse_widgets(DEFAULT_WIDGETS);
        assert!(errs.is_empty(), "{errs:?}");
        assert!(ws.len() >= 8, "scan must find the widgets: {}", ws.len());
        assert_eq!(serialize_widgets(&ws), DEFAULT_WIDGETS);
    }

    #[test]
    fn mixed_list_with_bad_tokens_keeps_good_ones_and_reports_bad() {
        let (ws, errs) = parse_widgets("clock, app:org.gnome.Console ,toggle:warp,slider:volume,,favorites:x,hatch");
        assert_eq!(
            ws,
            vec![BarWidget::Clock, BarWidget::App("org.gnome.Console".into()), BarWidget::Slider(SliderKind::Volume), BarWidget::Hatch]
        );
        assert_eq!(errs.len(), 2, "{errs:?}");
        assert!(errs[0].contains("warp"));
    }

    #[test]
    fn favorites_count_is_clamped() {
        assert_eq!(BarWidget::from_token("favorites:0"), Ok(BarWidget::Favorites(1)));
        assert_eq!(BarWidget::from_token("favorites:99"), Ok(BarWidget::Favorites(12)));
    }

    #[test]
    fn layout_stacks_fixed_and_splits_leftover_between_flex() {
        let (ws, _) = parse_widgets("clock,spacer,battery,spacer,hatch");
        // units: 2 + 1 + 1 = 4 fixed; 10 units of height → 6 spare → 3 each.
        let l = layout(&ws, 1000.0, 100.0);
        assert!(l.overflow.is_empty());
        let hs: Vec<f32> = l.placed.iter().map(|p| p.h).collect();
        assert_eq!(hs, vec![200.0, 300.0, 100.0, 300.0, 100.0]);
        let ys: Vec<f32> = l.placed.iter().map(|p| p.y).collect();
        assert_eq!(ys, vec![0.0, 200.0, 500.0, 600.0, 900.0]);
    }

    #[test]
    fn overflow_preserves_order_and_keeps_flex_floor() {
        // 2 + 4 + 4 fixed = 10 units, but only 8 fit after running's 2-unit floor.
        let (ws, _) = parse_widgets("clock,slider:brightness,running,slider:volume,battery");
        let l = layout(&ws, 800.0, 80.0);
        // clock(2)+slider(4) = 6 of 8 usable; slider:volume (4) does not fit, so
        // it AND the later battery (which would) overflow, in order.
        assert_eq!(l.overflow, vec![3, 4]);
        let running = l.placed.iter().find(|p| p.index == 2).unwrap();
        assert!(running.h >= 160.0, "running kept its floor: {}", running.h);
        let total: f32 = l.placed.iter().map(|p| p.h).sum();
        assert!(total <= 800.0 + 0.5, "never taller than the bar: {total}");
    }

    #[test]
    fn move_and_remove_are_safe() {
        let (mut ws, _) = parse_widgets("clock,battery,hatch");
        move_widget(&mut ws, 0, 2);
        assert_eq!(serialize_widgets(&ws), "battery,hatch,clock");
        move_widget(&mut ws, 2, 0);
        assert_eq!(serialize_widgets(&ws), "clock,battery,hatch");
        move_widget(&mut ws, 0, 9);
        remove_widget(&mut ws, 9);
        assert_eq!(ws.len(), 3);
        remove_widget(&mut ws, 1);
        assert_eq!(serialize_widgets(&ws), "clock,hatch");
    }

    #[test]
    fn catalog_tokens_all_parse() {
        let cat = catalog();
        assert!(cat.len() > 15);
        for w in cat {
            assert_eq!(BarWidget::from_token(&w.token()), Ok(w));
        }
    }
}
