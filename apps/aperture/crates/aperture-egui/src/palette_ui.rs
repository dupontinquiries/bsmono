//! palette_ui — Ctrl+P. Commands are data; ranking is uikit-egui's fuzzy
//! scorer (the same one gitmind's pickers use), so palette feel is consistent
//! across the suite. Five modes: command list, Ctrl+O's file picker,
//! Ctrl+Shift+F's project search, Ctrl+T's workspace symbol jump, and
//! Ctrl+G's "go to via route" natural-language search — `file_finder`
//! walks the workspace root once per open and fuzzy-ranks against the
//! typed query; project search reuses the same walked pool but greps file
//! *contents* instead of ranking *paths* (`aperture_core::project_search`);
//! workspace symbol reuses it again to build a name index from every
//! file's tree-sitter outline (`aperture_core::workspace_symbols`),
//! fuzzy-ranked the same way file paths are; go-to-route scores that same
//! symbol index plus file content against a multi-word natural-language
//! query's keywords (`aperture_core::route`) — "finding the button on the
//! wizard page for account setup" rather than a single name or substring.
//! Typed-path entry survives as the file-picker fallback when nothing
//! fuzzy-matches (a path outside the walked root, or a huge repo past the
//! walk cap): honest, and still works over ssh with zero UI.

use crate::state::AppState;
use snapclone_theme::PaletteId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Commands,
    OpenFile,
    ProjectSearch,
    WorkspaceSymbol,
    GoToRoute,
    AiEditPrompt,
    VoiceCommand,
    /// Shift+F12's answer: who calls (or mentions) the symbol at the caret.
    /// The list is fixed when the mode opens; typing filters it.
    Callers,
    /// Ctrl+K: buckets — named packs of places (`aperture_core::bucket`).
    /// Lists the buckets; entering one lists its entries.
    Bucket,
}

/// What a row in [`Mode::Bucket`] does when chosen. Rows are computed by
/// `AppState::bucket_rows` (which can read files and the outline); the
/// view only picks one.
#[derive(Clone, Debug, PartialEq)]
pub enum BucketAct {
    /// Enter bucket `i` (index into the workspace's list).
    Open(usize),
    /// Make a bucket with this name and enter it.
    Create(String),
    /// Back to the list of buckets.
    Back,
    /// Add the item (or file) under the caret to the open bucket.
    AddHere,
    /// Add every place the jump trail visited.
    KeepTrail,
    /// Go to a resolved entry.
    Jump { path: std::path::PathBuf, line: usize },
    /// Remove entry `entry` of the open bucket.
    Remove(usize),
    /// Delete the open bucket (asks once).
    DeleteBucket,
    /// Nothing: an unresolved entry shown for information.
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BucketRow {
    pub label: String,
    pub detail: String,
    pub act: BucketAct,
    /// A remove button on the row (entries only).
    pub removable: Option<usize>,
}

/// One row of [`Mode::Callers`].
#[derive(Clone, Debug, PartialEq)]
pub struct CallerHit {
    pub path: std::path::PathBuf,
    pub line: usize,
    /// Char column (already converted from LSP's UTF-16).
    pub col: usize,
    /// "caller" or the file name.
    pub title: String,
    /// "src/other.rs:7" and the line's text.
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct PaletteState {
    pub mode: Mode,
    pub input: String,
    pub selected: usize,
    /// Whether `selected` moved on THIS frame, i.e. by the keyboard.
    ///
    /// The list has to follow the arrow keys, and it must NOT follow anything
    /// else. Calling `scroll_to_me` on the selected row unconditionally — which
    /// is the obvious way to write it, and how it was written — drags the view
    /// back to the selection on every frame, so a touch drag scrolls and then
    /// snaps home the instant the finger lifts. Reported exactly that way: "the
    /// swipe to scroll works but the focus element staying on 1 fights back to
    /// the top once the gesture stops".
    ///
    /// Set by the arrow handling and cleared after the list is drawn, so it is
    /// true for precisely the frame that needs it.
    pub follow_selection: bool,
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
    /// [`Mode::Callers`]' rows; empty in every other mode.
    pub callers: Vec<CallerHit>,
    /// Whether `callers` are real calls (call hierarchy) or mentions
    /// (references fallback) — the nav step's relation and the heading.
    pub callers_are_calls: bool,
    /// [`Mode::Bucket`]: the bucket being shown, None = the list of them.
    pub bucket_open: Option<usize>,
    pub focus_placed: bool,
}

impl PaletteState {
    pub fn commands() -> Self {
        PaletteState { mode: Mode::Commands, input: String::new(), selected: 0, follow_selection: false, focus_placed: false, root: std::path::PathBuf::new(), files: Vec::new(), symbols: Vec::new(), callers: Vec::new(), callers_are_calls: false, bucket_open: None }
    }
    pub fn open_file(root: std::path::PathBuf, files: Vec<std::path::PathBuf>) -> Self {
        PaletteState { mode: Mode::OpenFile, input: String::new(), selected: 0, follow_selection: false, focus_placed: false, root, files, symbols: Vec::new(), callers: Vec::new(), callers_are_calls: false, bucket_open: None }
    }
    pub fn project_search(root: std::path::PathBuf, files: Vec<std::path::PathBuf>) -> Self {
        PaletteState { mode: Mode::ProjectSearch, input: String::new(), selected: 0, follow_selection: false, focus_placed: false, root, files, symbols: Vec::new(), callers: Vec::new(), callers_are_calls: false, bucket_open: None }
    }
    pub fn workspace_symbol(root: std::path::PathBuf, files: Vec<std::path::PathBuf>) -> Self {
        // Capped at 1500 files, well past the walk cap's typical hit rate
        // for a Rust-only index in a mixed-language monorepo, but still a
        // real bound — see workspace_symbols::index_files's own doc.
        let symbols = aperture_core::workspace_symbols::index_files(&root, &files, 1500);
        PaletteState { mode: Mode::WorkspaceSymbol, input: String::new(), selected: 0, follow_selection: false, focus_placed: false, root, files, symbols, callers: Vec::new(), callers_are_calls: false, bucket_open: None }
    }
    pub fn go_to_route(root: std::path::PathBuf, files: Vec<std::path::PathBuf>) -> Self {
        // Same symbol index workspace_symbol builds — route::candidates
        // scores against it too, so there's no reason to index twice.
        let symbols = aperture_core::workspace_symbols::index_files(&root, &files, 1500);
        PaletteState { mode: Mode::GoToRoute, input: String::new(), selected: 0, follow_selection: false, focus_placed: false, root, files, symbols, callers: Vec::new(), callers_are_calls: false, bucket_open: None }
    }
    /// A plain text-entry mode: the typed line IS the instruction, sent to
    /// `AppState::request_edit` on Enter. No candidate list, so `root`/
    /// `files`/`symbols` stay empty — nothing in this mode reads them.
    pub fn ai_edit_prompt() -> Self {
        PaletteState { mode: Mode::AiEditPrompt, input: String::new(), selected: 0, follow_selection: false, focus_placed: false, root: std::path::PathBuf::new(), files: Vec::new(), symbols: Vec::new(), callers: Vec::new(), callers_are_calls: false, bucket_open: None }
    }

    /// A plain text-entry mode: the typed line IS the utterance, sent to
    /// `AppState::run_voice_command` on Enter. A typed stand-in for what a
    /// real transcript would say — see `aperture_core::voice`'s module doc.
    pub fn callers(hits: Vec<CallerHit>, are_calls: bool) -> Self {
        PaletteState { mode: Mode::Callers, input: String::new(), selected: 0, follow_selection: false, focus_placed: false, root: std::path::PathBuf::new(), files: Vec::new(), symbols: Vec::new(), callers: hits, callers_are_calls: are_calls, bucket_open: None }
    }

    pub fn buckets(open: Option<usize>) -> Self {
        let mut p = Self::commands();
        p.mode = Mode::Bucket;
        p.bucket_open = open;
        p
    }

    pub fn voice_command() -> Self {
        PaletteState { mode: Mode::VoiceCommand, input: String::new(), selected: 0, follow_selection: false, focus_placed: false, root: std::path::PathBuf::new(), files: Vec::new(), symbols: Vec::new(), callers: Vec::new(), callers_are_calls: false, bucket_open: None }
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
    GoToRouteMode,
    FindCallers,
    BucketMode,
    AiEditMode,
    VoiceCommandMode,
    ToggleAi,
    CompleteNow,
    ToggleLoopPane,
    ToggleFollow,
    ToggleTypewriter,
    ToggleCmdStrip,
    SetCaretStyle(crate::camera::CaretStyle),
    SetScope(aperture_complete::scope::Scope),
    OlderRevision,
    NewerRevision,
    BranchFromHere,
    MaterializeHere,
    OpenFindReplace,
    Theme(PaletteId),
    GotoLine(usize),
}

pub fn command_list() -> Vec<(String, Cmd)> {
    let mut v: Vec<(String, Cmd)> = vec![
        ("File: Save".to_string(), Cmd::Save),
        ("File: Open…".to_string(), Cmd::OpenFileMode),
        ("Find: In Project… (Ctrl+Shift+F)".to_string(), Cmd::ProjectSearchMode),
        ("Go to Symbol in Workspace… (Ctrl+T)".to_string(), Cmd::WorkspaceSymbolMode),
        ("Go to via Route… (Ctrl+G)".to_string(), Cmd::GoToRouteMode),
        ("Go to: Callers of this symbol (Shift+F12)".to_string(), Cmd::FindCallers),
        ("Buckets… (Ctrl+K)".to_string(), Cmd::BucketMode),
        ("AI: Edit Selection…".to_string(), Cmd::AiEditMode),
        ("Voice: run command…".to_string(), Cmd::VoiceCommandMode),
        ("AI: Toggle ghost completions".to_string(), Cmd::ToggleAi),
        ("AI: Complete now".to_string(), Cmd::CompleteNow),
        ("View: Toggle loop pane".to_string(), Cmd::ToggleLoopPane),
        ("View: Toggle follow camera (Ctrl+E)".to_string(), Cmd::ToggleFollow),
        ("View: Typewriter camera (center the caret line)".to_string(), Cmd::ToggleTypewriter),
        ("View: Bottom command strip (recents + quick actions)".to_string(), Cmd::ToggleCmdStrip),
    ];
    // One entry per caret style, generated rather than listed, for the same
    // reason `Scope::ALL` is below: a new variant cannot be added and
    // silently have no way to reach it.
    for st in crate::camera::CaretStyle::ALL {
        v.push((format!("View: Caret — {} ({})", st.label(), st.hint()), Cmd::SetCaretStyle(*st)));
    }
    // One entry per scope, generated from Scope::ALL so a new variant
    // cannot be added and silently have no way to reach it.
    for sc in aperture_complete::scope::Scope::ALL {
        v.push((
            format!("AI: Context scope — {} ({})", sc.label(), sc.hint()),
            Cmd::SetScope(sc),
        ));
    }
    v.extend([
        ("Git: Older revision (Ctrl+,)".to_string(), Cmd::OlderRevision),
        ("Git: Newer revision (Ctrl+.)".to_string(), Cmd::NewerRevision),
        ("Git: Branch from previewed revision (Ctrl+B)".to_string(), Cmd::BranchFromHere),
        ("Git: Materialize previewed revision as a worktree (Ctrl+M)".to_string(), Cmd::MaterializeHere),
        ("Find & Replace (Ctrl+H)".to_string(), Cmd::OpenFindReplace),
    ]);
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
        Cmd::GoToRouteMode => {
            let root = s.workspace_root();
            let files = crate::file_finder::walk_files(&root, 4000);
            s.palette = Some(PaletteState::go_to_route(root, files));
            return; // keep the palette open, in its new mode
        }
        Cmd::BucketMode => {
            s.open_buckets(now_ms);
            return;
        }
        Cmd::FindCallers => {
            s.palette = None;
            s.find_callers(now_ms);
            return;
        }
        Cmd::AiEditMode => {
            s.palette = Some(PaletteState::ai_edit_prompt());
            return; // keep the palette open, in its new mode
        }
        Cmd::VoiceCommandMode => {
            s.open_voice_command();
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
        Cmd::ToggleTypewriter => s.toggle_typewriter(now_ms),
        Cmd::ToggleCmdStrip => {
            let on = s.cmd_strip.toggle();
            s.set_status(if on { "command strip: on" } else { "command strip: off" }, now_ms);
        }
        Cmd::SetScope(sc) => s.set_scope(sc, now_ms),
        Cmd::SetCaretStyle(st) => {
            s.caret.style = st;
            s.set_status(&format!("caret: {}", st.label()), now_ms);
        }
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
