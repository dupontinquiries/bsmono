//! The command palette, minus the drawing: what can be asked for by name.
//!
//! Every action the toolbar, the keys and the context menu offer is also
//! here under a name, so a user who does not know the key can type three
//! letters of it. The kit's `fuzzy::score` ranks; `command_bar` shapes the
//! surface. A few forms take an argument — `tempo 128`, `zoom 60`, `grid
//! 1/2`, `go 17` — and are parsed before matching so the number is not
//! fuzzed against the names.

use super::model::Grid;
use super::tools::Tool;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Tool(Tool),
    Undo,
    Redo,
    Delete,
    Duplicate,
    SplitAtPlayhead,
    ToggleLoop,
    SelectAll,
    ClearSelection,
    AddTrack,
    CaptureBeat,
    RecordTake,
    Export,
    Play,
    SeekStart,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    SetGrid(Grid),
    SetTempo(f64),
    /// Beat to seek to.
    Go(f64),
    /// Pixels per beat.
    Zoom(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    pub name: &'static str,
    pub key: &'static str,
    pub action: Action,
}

/// Everything with a name. Order is the tie-break for equal scores, so the
/// common ones come first.
pub const ENTRIES: &[Entry] = &[
    Entry { name: "play / pause", key: "space", action: Action::Play },
    Entry { name: "select tool", key: "1", action: Action::Tool(Tool::Select) },
    Entry { name: "draw tool", key: "2", action: Action::Tool(Tool::Draw) },
    Entry { name: "split tool", key: "3", action: Action::Tool(Tool::Split) },
    Entry { name: "erase tool", key: "4", action: Action::Tool(Tool::Erase) },
    Entry { name: "loop tool", key: "5", action: Action::Tool(Tool::Loop) },
    Entry { name: "undo", key: "ctrl-z", action: Action::Undo },
    Entry { name: "redo", key: "ctrl-y", action: Action::Redo },
    Entry { name: "delete selection", key: "del", action: Action::Delete },
    Entry { name: "duplicate selection", key: "ctrl-d", action: Action::Duplicate },
    Entry { name: "split at playhead", key: "s", action: Action::SplitAtPlayhead },
    Entry { name: "toggle loop", key: "l", action: Action::ToggleLoop },
    Entry { name: "select all", key: "ctrl-a", action: Action::SelectAll },
    Entry { name: "clear selection", key: "esc", action: Action::ClearSelection },
    Entry { name: "add track", key: "", action: Action::AddTrack },
    Entry { name: "capture playing beat", key: "", action: Action::CaptureBeat },
    Entry { name: "record take from the stream", key: "r", action: Action::RecordTake },
    Entry { name: "export wav", key: "", action: Action::Export },
    Entry { name: "go to start", key: "home", action: Action::SeekStart },
    Entry { name: "zoom in", key: "ctrl-wheel", action: Action::ZoomIn },
    Entry { name: "zoom out", key: "ctrl-wheel", action: Action::ZoomOut },
    Entry { name: "zoom to fit", key: "", action: Action::ZoomFit },
    Entry { name: "snap to bar", key: "", action: Action::SetGrid(Grid::BAR) },
    Entry { name: "snap to beat", key: "", action: Action::SetGrid(Grid::BEAT) },
    Entry { name: "snap to half beat", key: "", action: Action::SetGrid(Grid::HALF) },
    Entry { name: "snap to quarter beat", key: "", action: Action::SetGrid(Grid::QUARTER) },
    Entry { name: "snap off", key: "", action: Action::SetGrid(Grid::OFF) },
];

/// A ranked match.
#[derive(Debug, Clone, PartialEq)]
pub struct Match {
    pub label: String,
    pub key: &'static str,
    pub action: Action,
    pub score: i32,
}

/// Subsequence score, the same rules as uikit-egui's `fuzzy::score` (start
/// bonus, contiguity bonus, gap penalty) — copied rather than depended on,
/// because this core has no egui in it and the kit is an egui crate.
fn score(needle: &str, hay: &str) -> Option<i32> {
    let needle: Vec<char> = needle.to_lowercase().chars().filter(|c| *c != ' ').collect();
    if needle.is_empty() {
        return Some(0);
    }
    let hay: Vec<char> = hay.to_lowercase().chars().collect();
    let mut hi = 0usize;
    let mut s = 0i32;
    let mut last: Option<usize> = None;
    for &nc in &needle {
        let rel = hay.get(hi..)?.iter().position(|&h| h == nc)?;
        let p = hi + rel;
        if p == 0 {
            s += 12;
        }
        if last == Some(p.wrapping_sub(1)) {
            s += 6;
        }
        s -= (rel as i32) / 3;
        last = Some(p);
        hi = p + 1;
    }
    Some(s)
}

/// The typed forms. `tempo 128`, `bpm 128`, `zoom 60`, `go 17`, `grid 1/2`.
pub fn parse(query: &str) -> Option<Match> {
    let q = query.trim().to_ascii_lowercase();
    let mut it = q.split_whitespace();
    let head = it.next()?;
    let arg = it.next()?;
    let num = arg.parse::<f64>().ok();
    let (label, action) = match head {
        "tempo" | "bpm" => {
            let v = num.filter(|v| (20.0..=400.0).contains(v))?;
            (format!("tempo {v:.0}"), Action::SetTempo(v))
        }
        "zoom" => {
            let v = num.filter(|v| (4.0..=400.0).contains(v))?;
            (format!("zoom {v:.0} px/beat"), Action::Zoom(v as f32))
        }
        "go" | "goto" | "seek" => {
            let v = num.filter(|v| *v >= 0.0)?;
            (format!("go to beat {v:.2}"), Action::Go(v))
        }
        "grid" | "snap" => {
            let g = match arg {
                "bar" | "4" => Grid::BAR,
                "beat" | "1" => Grid::BEAT,
                "1/2" | "half" | "0.5" => Grid::HALF,
                "1/4" | "quarter" | "0.25" => Grid::QUARTER,
                "off" | "free" | "0" => Grid::OFF,
                _ => return None,
            };
            (format!("snap {}", g.label()), Action::SetGrid(g))
        }
        _ => return None,
    };
    Some(Match { label, key: "", action, score: i32::MAX })
}

/// Rank the entries against `query`. Empty query lists them all in table
/// order; a parsed form comes first.
pub fn search(query: &str) -> Vec<Match> {
    let q = query.trim();
    let mut out: Vec<Match> = Vec::new();
    if let Some(p) = parse(q) {
        out.push(p);
    }
    if q.is_empty() {
        out.extend(ENTRIES.iter().map(|e| Match { label: e.name.to_string(), key: e.key, action: e.action, score: 0 }));
        return out;
    }
    let mut scored: Vec<(usize, i32)> = ENTRIES
        .iter()
        .enumerate()
        .filter_map(|(i, e)| score(q, e.name).map(|s| (i, s)))
        .collect();
    // Higher score first; table order breaks ties.
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.extend(scored.into_iter().map(|(i, s)| {
        let e = &ENTRIES[i];
        Match { label: e.name.to_string(), key: e.key, action: e.action, score: s }
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_query_lists_everything_in_table_order() {
        let m = search("");
        assert_eq!(m.len(), ENTRIES.len());
        assert_eq!(m[0].action, Action::Play);
    }

    #[test]
    fn a_few_letters_find_the_command_and_rank_the_tight_match_first() {
        let m = search("spl");
        assert!(!m.is_empty());
        assert!(matches!(m[0].action, Action::Tool(Tool::Split) | Action::SplitAtPlayhead), "{:?}", m[0]);
        let m = search("undo");
        assert_eq!(m[0].action, Action::Undo);
        let m = search("zzzqqq");
        assert!(m.is_empty());
    }

    #[test]
    fn typed_forms_parse_and_come_first() {
        assert_eq!(parse("tempo 128").unwrap().action, Action::SetTempo(128.0));
        assert_eq!(parse("BPM 90").unwrap().action, Action::SetTempo(90.0));
        assert!(parse("tempo 900").is_none(), "outside the range");
        assert_eq!(parse("zoom 60").unwrap().action, Action::Zoom(60.0));
        assert_eq!(parse("go 17").unwrap().action, Action::Go(17.0));
        assert_eq!(parse("grid 1/2").unwrap().action, Action::SetGrid(Grid::HALF));
        assert_eq!(parse("snap off").unwrap().action, Action::SetGrid(Grid::OFF));
        assert!(parse("tempo").is_none(), "no argument");
        assert!(parse("frobnicate 3").is_none());
        let m = search("go 4");
        assert_eq!(m[0].action, Action::Go(4.0));
        assert_eq!(m[0].score, i32::MAX);
    }
}
