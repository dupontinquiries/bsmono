//! palette_ui — Ctrl+P. Commands are data; ranking is uikit-egui's fuzzy
//! scorer (the same one gitmind's pickers use), so palette feel is consistent
//! across the suite. Four modes: command list, Ctrl+O's file picker,
//! Ctrl+Shift+F's project search, and Ctrl+T's workspace symbol jump —
//! `file_finder` walks the workspace root once per open and fuzzy-ranks
//! against the typed query; project search reuses the same walked pool but
//! greps file *contents* instead of ranking *paths*
//! (`aperture_core::project_search`); workspace symbol reuses it again to
//! build a name index from every file's tree-sitter outline
//! (`aperture_core::workspace_symbols`), fuzzy-ranked the same way file
//! paths are. Typed-path entry survives as the file-picker fallback when
//! nothing fuzzy-matches (a path outside the walked root, or a huge repo
//! past the walk cap): honest, and still works over ssh with zero UI.

use crate::state::AppState;
use snapclone_theme::PaletteId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Commands,
    OpenFile,
    ProjectSearch,
    WorkspaceSymbol,
}

#[derive(Clone, Debug)]
pub struct PaletteState {
    pub mode: Mode,
    pub input: String,
    pub selected: usize,
    /// Ctrl+O's and Ctrl+Shift+F's shared candidate pool: walked ONCE on
    /// entering the mode (not per-keystroke — a repo walk is cheap once,
    /// wasteful sixty times a second while typing), then fuzzy-ranked or
    /// grepped fresh every render against whatever's currently typed.
    pub root: std::path::PathBuf,
    pub files: Vec<std::path::PathBuf>,
    /// Workspace-symbol mode's index: built ONCE on entering the mode (a
    /// parse-per-file pass, unlike `files`' plain directory walk — doing it
    /// every keystroke would make Ctrl+T laggy in anything but a tiny repo),
    /// then fuzzy-ranked fresh every render against whatever's typed. Empty
    /// and unused in every other mode.
    pub symbols: Vec<aperture_core::workspace_symbols::Symbol>,
    pub focus_placed: bool,
}

impl PaletteState {
    pub fn commands() -> Self {
        PaletteState { mode: Mode::Commands, input: String::new(), selected: 0, focus_placed: false, root: std::path::PathBuf::new(), files: Vec::new(), symbols: Vec::new() }
    }
    pub fn open_file(root: std::path::PathBuf, files: Vec<std::path::PathBuf>) -> Self {
        PaletteState { mode: Mode::OpenFile, input: String::new(), selected: 0, focus_placed: false, root, files, symbols: Vec::new() }
    }
    pub fn project_search(root: std::path::PathBuf, files: Vec<std::path::PathBuf>) -> Self {
        PaletteState { mode: Mode::ProjectSearch, input: String::new(), selected: 0, focus_placed: false, root, files, symbols: Vec::new() }
    }
    pub fn workspace_symbol(root: std::path::PathBuf, files: Vec<std::path::PathBuf>) -> Self {
        // Capped at 1500 files, well past the walk cap's typical hit rate
        // for a Rust-only index in a mixed-language monorepo, but still a
        // real bound — see workspace_symbols::index_files's own doc.
        let symbols = aperture_core::workspace_symbols::index_files(&root, &files, 1500);
        PaletteState { mode: Mode::WorkspaceSymbol, input: String::new(), selected: 0, focus_placed: false, root, files, symbols }
    }
}

/// Fuzzy-rank `symbols` by name against `query`, capped at `limit`. Same
/// tie-break convention as `file_finder::rank` (shorter first, then
/// lexical) so all the pickers feel alike.
pub fn rank_symbols<'a>(symbols: &'a [aperture_core::workspace_symbols::Symbol], query: &str, limit: usize) -> Vec<&'a aperture_core::workspace_symbols::Symbol> {
    let mut scored: Vec<(i32, &aperture_core::workspace_symbols::Symbol)> =
        symbols.iter().filter_map(|s| uikit_egui::fuzzy::score(query, &s.item.name).map(|sc| (sc, s))).collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.item.name.len().cmp(&b.1.item.name.len())).then(a.1.item.name.cmp(&b.1.item.name)));
    scored.into_iter().take(limit).map(|(_, s)| s).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Save,
    OpenFileMode,
    ProjectSearchMode,
    WorkspaceSymbolMode,
    ToggleAi,
    CompleteNow,
    ToggleLoopPane,
    ToggleFollow,
    OlderRevision,
    NewerRevision,
    BranchFromHere,
    MaterializeHere,
    OpenFindReplace,
    Theme(PaletteId),
    GotoLine(usize),
}

pub fn command_list() -> Vec<(String, Cmd)> {
    let mut v = vec![
        ("File: Save".to_string(), Cmd::Save),
        ("File: Open…".to_string(), Cmd::OpenFileMode),
        ("Find: In Project… (Ctrl+Shift+F)".to_string(), Cmd::ProjectSearchMode),
        ("Go to Symbol in Workspace… (Ctrl+T)".to_string(), Cmd::WorkspaceSymbolMode),
        ("AI: Toggle ghost completions".to_string(), Cmd::ToggleAi),
        ("AI: Complete now".to_string(), Cmd::CompleteNow),
        ("View: Toggle loop pane".to_string(), Cmd::ToggleLoopPane),
        ("View: Toggle follow camera (Ctrl+E)".to_string(), Cmd::ToggleFollow),
        ("Git: Older revision (Ctrl+,)".to_string(), Cmd::OlderRevision),
        ("Git: Newer revision (Ctrl+.)".to_string(), Cmd::NewerRevision),
        ("Git: Branch from previewed revision (Ctrl+B)".to_string(), Cmd::BranchFromHere),
        ("Git: Materialize previewed revision as a worktree (Ctrl+M)".to_string(), Cmd::MaterializeHere),
        ("Find & Replace (Ctrl+H)".to_string(), Cmd::OpenFindReplace),
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
            let root = s.workspace_root();
            let files = crate::file_finder::walk_files(&root, 4000);
            s.palette = Some(PaletteState::open_file(root, files));
            return; // keep the palette open, in its new mode
        }
        Cmd::ProjectSearchMode => {
            let root = s.workspace_root();
            let files = crate::file_finder::walk_files(&root, 4000);
            s.palette = Some(PaletteState::project_search(root, files));
            return; // keep the palette open, in its new mode
        }
        Cmd::WorkspaceSymbolMode => {
            let root = s.workspace_root();
            let files = crate::file_finder::walk_files(&root, 4000);
            s.palette = Some(PaletteState::workspace_symbol(root, files));
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
        Cmd::ToggleFollow => s.toggle_follow(now_ms),
        Cmd::OlderRevision => s.rev_step(1, now_ms),
        Cmd::NewerRevision => s.rev_step(-1, now_ms),
        Cmd::BranchFromHere => s.branch_from_here(now_ms),
        Cmd::MaterializeHere => s.materialize_here(now_ms),
        Cmd::OpenFindReplace => s.open_find_with_replace(),
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

    #[test]
    fn rank_symbols_fuzzy_matches_by_name() {
        use aperture_core::syntax::{OutlineItem, OutlineKind};
        use aperture_core::workspace_symbols::Symbol;
        let mk = |name: &str, path: &str| Symbol {
            item: OutlineItem { kind: OutlineKind::Fn, name: name.to_string(), sig: String::new(), line: 0, end_line: 0 },
            path: std::path::PathBuf::from(path),
            rel: path.to_string(),
        };
        let symbols = vec![mk("parse_line", "a.rs"), mk("parse_header", "b.rs"), mk("unrelated", "c.rs")];
        let ranked = rank_symbols(&symbols, "parseline", 10);
        let names: Vec<&str> = ranked.iter().map(|s| s.item.name.as_str()).collect();
        assert!(names.contains(&"parse_line"), "{names:?}");
        assert!(!names.contains(&"unrelated"), "{names:?}");
    }
}
