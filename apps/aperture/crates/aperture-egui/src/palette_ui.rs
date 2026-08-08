//! palette_ui — Ctrl+P. Commands are data; ranking is uikit-egui's fuzzy
//! scorer (the same one gitmind's pickers use), so palette feel is consistent
//! across the suite. Two modes: command list, and free-text path entry for
//! Ctrl+O (a real file picker is a follow-up — see TODO.md; typing a path is
//! honest and works over ssh).

use crate::state::AppState;
use snapclone_theme::PaletteId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Commands,
    OpenFile,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaletteState {
    pub mode: Mode,
    pub input: String,
    pub selected: usize,
    pub focus_placed: bool,
}

impl PaletteState {
    pub fn commands() -> Self {
        PaletteState { mode: Mode::Commands, input: String::new(), selected: 0, focus_placed: false }
    }
    pub fn open_file() -> Self {
        PaletteState { mode: Mode::OpenFile, input: String::new(), selected: 0, focus_placed: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Save,
    OpenFileMode,
    ToggleAi,
    CompleteNow,
    ToggleLoopPane,
    Theme(PaletteId),
    GotoLine(usize),
}

pub fn command_list() -> Vec<(String, Cmd)> {
    let mut v = vec![
        ("File: Save".to_string(), Cmd::Save),
        ("File: Open…".to_string(), Cmd::OpenFileMode),
        ("AI: Toggle ghost completions".to_string(), Cmd::ToggleAi),
        ("AI: Complete now".to_string(), Cmd::CompleteNow),
        ("View: Toggle loop pane".to_string(), Cmd::ToggleLoopPane),
    ];
    for id in PaletteId::all() {
        v.push((format!("Theme: {id:?}"), Cmd::Theme(*id)));
    }
    v
}

/// Rank commands for `input` — `:N`/`N` (digits) becomes "Go to line N".
pub fn matches(input: &str) -> Vec<(String, Cmd)> {
    let trimmed = input.trim().trim_start_matches(':');
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(n) = trimmed.parse::<usize>() {
            return vec![(format!("Go to line {n}"), Cmd::GotoLine(n.saturating_sub(1)))];
        }
    }
    let mut scored: Vec<(i32, String, Cmd)> = command_list()
        .into_iter()
        .filter_map(|(label, cmd)| uikit_egui::fuzzy::score(input, &label).map(|s| (s, label, cmd)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, l, c)| (l, c)).collect()
}

pub fn apply(cmd: Cmd, s: &mut AppState, now_ms: u64) {
    match cmd {
        Cmd::Save => s.save(now_ms),
        Cmd::OpenFileMode => {
            s.palette = Some(PaletteState::open_file());
            return; // keep the palette open, in its new mode
        }
        Cmd::ToggleAi => {
            s.ai_enabled = !s.ai_enabled;
            if !s.ai_enabled {
                s.ghost.dismiss();
                s.debounce.cancel();
            }
            let msg = if s.ai_enabled { "ghost completions on" } else { "ghost completions off" };
            s.set_status(msg, now_ms);
        }
        Cmd::CompleteNow => s.trigger_completion_now(now_ms),
        Cmd::ToggleLoopPane => s.show_loop_pane = !s.show_loop_pane,
        Cmd::Theme(id) => {
            s.theme_id = id;
            s.set_status(&format!("theme: {id:?}"), now_ms);
        }
        Cmd::GotoLine(n) => {
            s.editor.goto_line(n);
            s.follow_caret = true;
        }
    }
    s.palette = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_ranks_and_digits_become_goto() {
        let m = matches("thnight");
        assert!(m.first().unwrap().0.contains("Night"), "{:?}", &m[..3.min(m.len())]);
        let g = matches(":42");
        assert_eq!(g, vec![("Go to line 42".to_string(), Cmd::GotoLine(41))]);
        assert!(matches("zzzzqqq").is_empty());
    }
}
