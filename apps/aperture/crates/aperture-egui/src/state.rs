//! state — the shell's brain, with egui kept out.
//!
//! The view layer translates `egui::Event`s into this module's [`Ev`] and
//! calls [`AppState::handle`]; everything a keystroke *means* — bindings,
//! ghost interplay, undo grouping, when to fire a completion — lives here
//! behind plain types with `now_ms` as a parameter. The payoff is the test
//! block at the bottom: a full type-→-ghost-→-Tab-accept session runs
//! headless in milliseconds, no window, no kittest harness. The view can only
//! get drawing wrong.

use crate::camera::Cam;
use crate::ghost::Ghost;
use aperture_complete::engine::Engine;
use aperture_complete::providers::{CommandProvider, OllamaConfig, OllamaProvider, StaticProvider};
use aperture_complete::{make_request, CompletionOutcome, CompletionRequest, Debounce, Provider};
use aperture_core::gitspace::{self, GitRunner, Runner as _};
use aperture_core::syntax::{Language, Syntax};
use aperture_core::{Editor, Selection};
use std::collections::VecDeque;
use std::path::PathBuf;

pub const PREFIX_BUDGET: usize = 2000;
pub const SUFFIX_BUDGET: usize = 600;

/// Keys the editor binds. The view maps `egui::Key` onto these; tests build
/// them directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum K {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Backspace,
    Delete,
    Enter,
    Tab,
    Escape,
    A,
    O,
    P,
    S,
    Y,
    Z,
    E,
    F,
    F3,
    Space,
    Comma,
    Period,
    B,
    M,
    H,
    T,
    G,
}

/// Incremental search. Matches are absolute char ranges, kept sorted; the
/// query is case-smart in the ASCII sense (any uppercase letter → sensitive)
/// so byte offsets survive the fold — Unicode case folding can change
/// lengths, and a find bar that misdraws its own highlights is worse than
/// one that is case-exact for non-ASCII.
#[derive(Clone, Debug, Default)]
pub struct FindState {
    pub query: String,
    pub matches: Vec<(usize, usize)>,
    pub cur: usize,
    last_version: u64,
    last_query: String,
    /// Ctrl+H's addition: `None` = find-only (Ctrl+F's original behavior,
    /// byte-for-byte unchanged). `Some(text)` = replace mode; `focus` says
    /// which field Text/Backspace events edit.
    pub replace: Option<String>,
    pub focus: FindFocus,
    /// Where in `AppState::find_history` Ctrl+Up/Ctrl+Down currently sits.
    /// `None` = the live typed query, not browsing history. Typing resets
    /// this to `None` — cycling shows past queries, but the moment someone
    /// types, they're composing a new one, not editing an old one in place.
    history_ix: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FindFocus {
    #[default]
    Query,
    Replace,
}

impl FindState {
    pub fn recompute(&mut self, buffer: &aperture_core::Buffer, version: u64) {
        if version == self.last_version && self.query == self.last_query {
            return;
        }
        self.last_version = version;
        self.last_query = self.query.clone();
        self.matches.clear();
        self.cur = 0;
        if self.query.is_empty() {
            return;
        }
        let sensitive = self.query.chars().any(|c| c.is_ascii_uppercase());
        let needle = if sensitive { self.query.clone() } else { self.query.to_ascii_lowercase() };
        for li in 0..buffer.len_lines() {
            // `buffer.line()` trims the line terminator; the buffer's real
            // char-offset scheme (`pos_to_char`/`char_to_pos`, used
            // everywhere else including `on_line` just below) counts it.
            // Manually accumulating trimmed line lengths silently drops
            // that terminator every line — asking the buffer directly for
            // each line's true start avoids the whole bug class rather
            // than patching the arithmetic.
            let line_start_char = buffer.pos_to_char(aperture_core::Pos::new(li, 0));
            let line = buffer.line(li);
            let hay = if sensitive { line.to_string() } else { line.to_ascii_lowercase() };
            for (b, _) in hay.match_indices(&needle) {
                let c0 = hay[..b].chars().count();
                let c1 = c0 + needle.chars().count();
                self.matches.push((line_start_char + c0, line_start_char + c1));
            }
        }
    }

    /// Column ranges of matches on `line` (for the view's highlight slabs),
    /// plus whether each is the current one.
    pub fn on_line(&self, buffer: &aperture_core::Buffer, line: usize) -> Vec<(usize, usize, bool)> {
        let start = buffer.pos_to_char(aperture_core::Pos::new(line, 0));
        let end = start + buffer.line(line).chars().count();
        self.matches
            .iter()
            .enumerate()
            .filter(|(_, &(a, _))| a >= start && a < end)
            .map(|(i, &(a, b))| (a - start, b.min(end) - start, i == self.cur))
            .collect()
    }
}

/// The symbol popup: instant, local, LSP-SHAPED. Items come from
/// `AppState::gather_symbols` today (the outline, parsed in-process); a
/// language server plugs in as another producer of the same `PopupItem`s —
/// the popup itself never knows where names come from. That's the "via LSP
/// standards" seam: label/insert/detail is deliberately the shape of an LSP
/// CompletionItem's label/insertText/detail.
#[derive(Clone, Debug)]
pub struct PopupItem {
    pub label: String,
    pub insert: String,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct Popup {
    pub items: Vec<PopupItem>,
    pub sel: usize,
    /// Char offset where the word being completed starts.
    pub prefix_start: usize,
}

/// An instruct-edit request in flight or awaiting review (see
/// `AppState::pending_edit`'s doc for why acceptance is always explicit).
#[derive(Clone, Debug)]
pub struct PendingEdit {
    pub start: usize,
    pub end: usize,
    pub original: String,
    pub instruction: String,
    /// `None` while `edit_engine` hasn't answered yet.
    pub proposed: Option<String>,
}

pub(crate) fn sel_range(a: usize, b: usize) -> aperture_core::Selection {
    aperture_core::Selection { anchor: a, head: b }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ev {
    Text(String),
    Key { key: K, ctrl: bool, shift: bool, alt: bool },
    Paste(String),
    Copy,
    Cut,
    /// IME composition candidate text — not yet committed. `egui::Event::
    /// Ime(ImeEvent::Preedit(s))` maps here directly; `ImeEvent::Commit(s)`
    /// maps to plain `Text(s)` instead, since committing IS "insert this
    /// text now," the same thing a keystroke does. An empty string clears
    /// whatever's currently shown — the same signal `ImeEvent::Preedit("")`
    /// and `ImeEvent::Commit("")` both use (different platforms emit
    /// different ones of the pair on backspace-during-composition, per
    /// egui's own `TextEdit` — handling `Text("")` as a no-op and
    /// `Preedit("")` as "clear" covers both without caring which fired).
    Preedit(String),
}

/// What the state asks the view to do with the platform.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    pub clipboard: Option<String>,
}

/// Which provider the loop pane has selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Ollama,
    Command,
    Demo,
}

/// One row of the loop pane's history — the engine's traffic as values.
#[derive(Clone, Debug)]
pub struct LoopRow {
    pub generation: u64,
    pub provider: String,
    pub latency_ms: u64,
    pub ok: bool,
    pub brief: String,
}

pub struct AppState {
    pub editor: Editor,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub lang: Language,
    pub syntax: Syntax,
    pub find: Option<FindState>,
    /// impakt-cam mode: the camera keeps the caret centered (dead zone +
    /// snap horizon live in `camera::Follow`). The view applies it only when
    /// the caret LINE changed, so wheel-browsing is free until you type.
    pub follow_mode: bool,
    pub last_follow_line: usize,
    // ── gitspace ────────────────────────────────────────────────────────────
    git: Option<(GitRunner, String)>,
    /// Worktree lines changed vs. the newest commit, refreshed on load and
    /// after every save (edits between saves don't move the gutter — it
    /// answers "what's on disk that isn't committed", the same question the
    /// physical viewer's worktree marks answer).
    pub git_changed: Vec<(usize, usize)>,
    /// Past find queries, most-recent-last, deduped against immediate
    /// repeats. Committed on close (Escape) — a query typed and abandoned
    /// mid-search doesn't clutter history the way committing per-keystroke
    /// would.
    pub find_history: Vec<String>,
    /// IME composition candidate text, shown at the caret but never part
    /// of the buffer — overlay-rendered, exactly like the ghost and the
    /// popup, so it needs no interaction with undo/history/version at all.
    /// Empty = nothing being composed.
    pub preedit: String,
    /// The file's timeline: commits + gitmind checkpoints, newest first.
    pub revs: Vec<gitspace::Rev>,
    /// None = live and editable; Some(i) = READ-ONLY preview of revs[i].
    /// Mirrors aperture-view's rev_ix exactly — same timeline, same index,
    /// same "past-the-newest steps home" rule — so the two shells agree on
    /// what "revision 3" means if you ever compare notes between them.
    pub rev_ix: Option<usize>,
    /// The live buffer's text, snapshotted on entering a revision preview.
    /// Returning home restores it EXACTLY — unsaved edits survive browsing
    /// history unchanged. Honest caveat: the undo STACK does not survive the
    /// round trip (Editor's history isn't cloneable, and cloning it just for
    /// this felt like the wrong invasiveness to add to aperture-core for a
    /// read-only side trip); content is preserved, undo depth resets.
    live_text: Option<String>,
    /// The SAME margin-note idea as aperture-view: a dedicated engine (own
    /// APERTURE_CMD provider, independent of the ghost-completion engine —
    /// a "describe this diff" prompt and a fill-in-middle prompt are
    /// different jobs) that describes whatever revision is on screen.
    /// Newest-request-wins is exactly right here too: scrub through history
    /// and only wherever you settle gets described.
    describe: Option<Engine>,
    pub rev_note: String,
    note_generation: u64,
    /// "Go to via route"'s AI tier: a THIRD independent engine (own
    /// generation counter, own `Option<Engine>`) alongside `engine` (ghost
    /// completion) and `describe` (revision notes) — same reasoning as
    /// `describe`'s doc comment: a route-search prompt and a FIM prompt are
    /// different jobs, and sharing one Engine's generation counter across
    /// two independently-incrementing call sites would break "coalesce to
    /// the newest" (a route request racing a describe request on the same
    /// counter isn't actually newer just because its own local counter says
    /// so). `aperture_complete::route_ai`'s pending/fulfill cache is the
    /// seam this drains — this field IS the worker `route_ai`'s own doc
    /// says the crate can't supply itself.
    route_engine: Option<Engine>,
    route_generation: u64,
    /// The key `route_ai::request_route_summary` returned for the request
    /// currently in flight on `route_engine`, if any — what `poll_route_ai`
    /// calls `route_ai::fulfill` against when the outcome lands.
    route_pending_key: Option<u64>,
    /// "AI: Edit Selection…"'s engine — a FOURTH independent instance,
    /// same reasoning as `route_engine`'s doc: an instruct-edit prompt and
    /// a route-search prompt are different jobs, and sharing a generation
    /// counter across independently-firing features breaks "coalesce to
    /// the newest." Same `APERTURE_CMD` provider construction as `describe`
    /// and `route_engine` — all three are "ask the assistant a one-off
    /// text question," differing only in what the question is.
    edit_engine: Option<Engine>,
    edit_generation: u64,
    /// A proposed edit in flight or awaiting review. `proposed: None` means
    /// still waiting on `edit_engine`; once it lands, the shell shows
    /// original vs. proposed and applies via `Editor::replace_range` (one
    /// undo step) only on explicit accept — an instruct-edit result is a
    /// SUGGESTION, never auto-applied, unlike ghost completion's Tab-to-
    /// accept (a FIM completion is "what comes next," low-stakes to accept
    /// blind; a rewritten selection is a bigger, easier-to-get-wrong bet).
    pub pending_edit: Option<PendingEdit>,
    fim_ctx: String,
    fim_ctx_version: u64,
    pub popup: Option<Popup>,
    symbols: Vec<(String, String)>,
    symbols_version: u64,

    pub cam: Cam,
    /// View sets this after any caret-moving command so the camera follows.
    pub follow_caret: bool,

    pub ai_enabled: bool,
    pub ghost: Ghost,
    pub debounce: Debounce,
    pub engine: Option<Engine>,
    pub loop_log: VecDeque<LoopRow>,
    pub show_loop_pane: bool,
    pub provider_kind: ProviderKind,
    pub ollama_cfg: OllamaConfig,
    pub command_line: String,

    pub ghost_since: Option<u64>,
    pub palette: Option<crate::palette_ui::PaletteState>,
    pub theme_id: snapclone_theme::PaletteId,
    pub status: Option<(String, u64)>, // message, expires_at_ms
}

impl AppState {
    pub fn new(path: Option<PathBuf>, with_engine: bool) -> Self {
        let (text, lang) = match &path {
            Some(p) => (
                std::fs::read_to_string(p).unwrap_or_default(),
                Language::from_path(p),
            ),
            None => (String::new(), Language::Rust),
        };
        let mut s = AppState {
            editor: Editor::from_str(&text),
            path,
            dirty: false,
            lang,
            syntax: Syntax::new(lang),
            find: None,
            follow_mode: false,
            last_follow_line: usize::MAX,
            git: None,
            git_changed: Vec::new(),
            find_history: Vec::new(),
            preedit: String::new(),
            revs: Vec::new(),
            rev_ix: None,
            live_text: None,
            describe: std::env::var("APERTURE_CMD").ok().map(|cmd| {
                let argv = cmd.split_whitespace().map(str::to_string).collect();
                Engine::new(Box::new(CommandProvider::new(argv)))
            }),
            rev_note: String::new(),
            note_generation: 0,
            // Same APERTURE_CMD provider as `describe` — both are "ask the
            // assistant a one-off text question" jobs; what makes them
            // independent is the generation counter and the Engine
            // INSTANCE, not the provider construction.
            route_engine: std::env::var("APERTURE_CMD").ok().map(|cmd| {
                let argv = cmd.split_whitespace().map(str::to_string).collect();
                Engine::new(Box::new(CommandProvider::new(argv)))
            }),
            route_generation: 0,
            route_pending_key: None,
            edit_engine: std::env::var("APERTURE_CMD").ok().map(|cmd| {
                let argv = cmd.split_whitespace().map(str::to_string).collect();
                Engine::new(Box::new(CommandProvider::new(argv)))
            }),
            edit_generation: 0,
            pending_edit: None,
            fim_ctx: String::new(),
            fim_ctx_version: u64::MAX,
            popup: None,
            symbols: Vec::new(),
            symbols_version: u64::MAX,
            cam: Cam::new(15.0),
            follow_caret: false,
            ai_enabled: true,
            ghost: Ghost::Idle,
            debounce: Debounce::new(280),
            engine: None,
            loop_log: VecDeque::new(),
            show_loop_pane: false,
            provider_kind: ProviderKind::Ollama,
            ollama_cfg: OllamaConfig::default(),
            command_line: String::new(),
            ghost_since: None,
            palette: None,
            theme_id: snapclone_theme::PaletteId::Night,
            status: None,
        };
        if with_engine {
            s.apply_env_provider();
            s.engine = Some(Engine::new(s.build_provider()));
        }
        s.git = s.path.as_deref().and_then(gitspace::discover);
        if let Some((r, rel)) = &s.git {
            s.revs = gitspace::history(r, rel, 48);
        }
        s.refresh_git();
        s
    }

    /// Where Ctrl+O's file picker walks from: the git repo root when there
    /// is one (the natural "workspace" — same root gitspace already
    /// discovered), else the open file's directory, else cwd. Never fails
    /// outright; worst case is "walk from wherever we happen to be."
    pub fn workspace_root(&self) -> std::path::PathBuf {
        if let Some((r, _)) = &self.git {
            return r.root.clone();
        }
        if let Some(p) = &self.path {
            if let Some(dir) = p.parent() {
                return dir.to_path_buf();
            }
        }
        std::env::current_dir().unwrap_or_default()
    }

    /// Recompute the gutter's diff marks: worktree vs. the newest commit.
    /// Called on load and after every successful save — never mid-edit, so
    /// typing doesn't churn a `git diff` on every keystroke. Only meaningful
    /// while live; a shown revision computes its own diff in `rev_step`.
    fn refresh_git(&mut self) {
        if self.rev_ix.is_some() {
            return;
        }
        self.git_changed = self
            .git
            .as_ref()
            .and_then(|(r, rel)| {
                self.revs
                    .iter()
                    .find(|v| v.kind == gitspace::RevKind::Commit)
                    .map(|base| gitspace::worktree_changed(r, &base.oid, rel))
            })
            .unwrap_or_default();
    }

    /// Step through the file's timeline: `d = +1` older, `-1` newer; newer
    /// past the newest comes home to the live worktree — exactly
    /// aperture-view's `jump`/`rev_step` contract, so the two shells' idea of
    /// "revision 3" is the same index into the same merged history. The view
    /// is READ-ONLY while previewing (`type_text` and friends check
    /// `rev_ix`); Escape or stepping home hands editing back.
    pub fn rev_step(&mut self, d: isize, now_ms: u64) {
        if self.revs.is_empty() || self.git.is_none() {
            return;
        }
        let cur = self.rev_ix.map(|i| i as isize).unwrap_or(-1);
        let next = (cur + d).clamp(-1, self.revs.len() as isize - 1);
        if next == cur {
            return;
        }
        if self.live_text.is_none() {
            self.live_text = Some(self.editor.buffer.to_string());
        }
        let (r, rel) = self.git.as_ref().unwrap();
        if next < 0 {
            let text = self.live_text.take().unwrap_or_default();
            self.rev_ix = None;
            self.set_preview_text(&text);
            self.refresh_git();
            self.rev_note.clear();
            self.set_status("back to the live file", now_ms);
        } else {
            let i = next as usize;
            let Some(text) = gitspace::file_at(r, &self.revs[i].oid, rel) else {
                // The file didn't exist at this revision: step past it in
                // the same direction, same convention as aperture-view.
                self.rev_ix = Some(i);
                return self.rev_step(d.signum(), now_ms);
            };
            self.git_changed = gitspace::changed_ranges(r, &self.revs[i].oid, rel);
            self.rev_ix = Some(i);
            self.set_preview_text(&text);
            let mark = if matches!(self.revs[i].kind, gitspace::RevKind::Checkpoint(_)) { "◈ " } else { "" };
            self.set_status(
                &format!("viewing {}rev {}/{} — read-only ({})", mark, i + 1, self.revs.len(), self.revs[i].subject),
                now_ms,
            );
            self.request_describe();
        }
    }

    /// Promote the revision currently on screen to a real branch (gitmind's
    /// own pattern: "promoting any checkpoint to a real branch is one
    /// `branch <n> <ckpt-oid>`" — this is that command, reachable from
    /// wherever you're LOOKING). Only meaningful while previewing; a no-op
    /// otherwise rather than branching HEAD, which `Ctrl+B` on the live file
    /// would too easily fire by accident.
    pub fn branch_from_here(&mut self, now_ms: u64) {
        let (Some((r, _)), Some(i)) = (&self.git, self.rev_ix) else {
            self.set_status("branch: preview a revision first", now_ms);
            return;
        };
        let rev = &self.revs[i];
        let name = gitspace::suggest_branch_name(rev);
        match gitspace::branch_from(r, &rev.oid, &name) {
            Ok(()) => self.set_status(&format!("branched '{name}' from {}", &rev.oid[..8]), now_ms),
            Err(e) => self.set_status(&format!("branch failed: {}", e.trim().lines().next().unwrap_or(&e)), now_ms),
        }
    }

    /// Check the previewed revision out into a fresh side-by-side worktree
    /// (gitmind's `materialize`): "stand in this version, in a real
    /// directory, without disturbing the one I'm in." Same guard as
    /// `branch_from_here` — a no-op message while live, never checking out
    /// HEAD into a stray directory by accident.
    pub fn materialize_here(&mut self, now_ms: u64) {
        let (Some((r, _)), Some(i)) = (&self.git, self.rev_ix) else {
            self.set_status("materialize: preview a revision first", now_ms);
            return;
        };
        let rev = &self.revs[i];
        let dest = gitspace::suggest_worktree_path(&r.root, rev);
        match gitspace::materialize(r, &rev.oid, &dest.to_string_lossy()) {
            Ok(()) => self.set_status(&format!("materialized at {}", dest.display()), now_ms),
            Err(e) => self.set_status(&format!("materialize failed: {}", e.trim().lines().next().unwrap_or(&e)), now_ms),
        }
    }

    /// Ask the assistant for one sentence on the shown revision: what it
    /// changed and when you'd want it. No `APERTURE_CMD`, no note, zero
    /// cost — same contract as aperture-view's `request_describe`.
    fn request_describe(&mut self) {
        self.rev_note.clear();
        let (Some(engine), Some(i), Some((r, rel))) = (&self.describe, self.rev_ix, &self.git) else {
            return;
        };
        let rev = &self.revs[i];
        self.note_generation += 1;
        let stat = r.run(&["show", "--stat", "--format=%s", &rev.oid, "--", rel.as_str()]).unwrap_or_default();
        let prompt = format!(
            "One sentence, plain words: what did this change do to this file, and when would someone want this version?\nCommit: {}\n{}\nAnswer:",
            rev.subject,
            stat.chars().take(600).collect::<String>()
        );
        engine.request(CompletionRequest { generation: self.note_generation, prefix: prompt, suffix: String::new(), lang: "text".into() });
    }

    /// Drain the describe engine; call once per frame.
    pub fn poll_note(&mut self) {
        let Some(engine) = &self.describe else { return };
        while let Some(out) = engine.poll() {
            if out.generation == self.note_generation {
                if let Ok(text) = out.result {
                    // Same budget, same reasoning, as aperture-view's margin
                    // note (see viewer.rs) — a literal 60 here rather than a
                    // scene-kit dependency, since the flat editor has no 3D
                    // rendering and shouldn't gain that dependency chain
                    // just to name a constant. `scene_kit::textsurface::
                    // Surface::StreetSign.char_budget()` is where the
                    // number actually comes from: a small legible sign in
                    // the world, budgeted at 60 characters — the same shape
                    // of constraint whether the text sits on a rendered
                    // object or drawn chrome, which is what this status
                    // line is. Keep the two in sync if that budget changes.
                    const MARGIN_NOTE_CHARS: usize = 60;
                    self.rev_note = text.trim().lines().next().unwrap_or("").chars().take(MARGIN_NOTE_CHARS).collect();
                }
            }
        }
    }

    /// The AI tier of "go to via route": ask whatever `route_engine` is
    /// configured to pick and describe the best of `cands` (tier one's
    /// deterministic output). Idempotent per `aperture_complete::route_ai`'s
    /// own content-hash dedup — calling this again with the same query and
    /// candidate set while a request is already queued/in-flight is a
    /// no-op, so the palette can call it on every render without spamming
    /// the provider on every frame. No-op entirely if no `route_engine` is
    /// configured (no `APERTURE_CMD`) — tier one still works standalone.
    pub fn request_route_ai(&mut self, query: &str, cands: &[aperture_core::route::RouteCandidate]) {
        let Some(engine) = &self.route_engine else { return };
        if query.trim().is_empty() || cands.is_empty() {
            return;
        }
        let briefs: Vec<aperture_complete::route_ai::CandidateBrief> = cands
            .iter()
            .map(|c| aperture_complete::route_ai::CandidateBrief {
                label: c.label.clone(),
                rel: c.rel.clone(),
                line: c.line,
                snippet: c.snippet.clone(),
            })
            .collect();
        let key = aperture_complete::route_ai::request_route_summary(query, &briefs);
        if self.route_pending_key == Some(key) {
            return; // already the in-flight/landed request for this exact ask
        }
        let Some((taken_key, prompt)) = aperture_complete::route_ai::take_pending() else {
            // Nothing new was queued (a duplicate request, or the answer
            // already landed in a prior frame) — still track the key so a
            // landed answer's route_summary(key) gets picked up below.
            self.route_pending_key = Some(key);
            return;
        };
        self.route_generation += 1;
        self.route_pending_key = Some(taken_key);
        engine.request(CompletionRequest { generation: self.route_generation, prefix: prompt, suffix: String::new(), lang: "text".into() });
    }

    /// Drain the route-AI engine; call once per frame, same as `poll_note`.
    pub fn poll_route_ai(&mut self) {
        let Some(engine) = &self.route_engine else { return };
        while let Some(out) = engine.poll() {
            if out.generation == self.route_generation {
                if let (Ok(text), Some(key)) = (out.result, self.route_pending_key) {
                    aperture_complete::route_ai::fulfill(key, text);
                }
            }
        }
    }

    /// "AI: Edit Selection…" — ask the assistant to rewrite the current
    /// selection given `instruction`. No-op (with a status message) if
    /// there's no `edit_engine` configured or no real selection: an
    /// instruct-edit on a bare caret has nothing to rewrite, unlike ghost
    /// completion which is exactly for a caret.
    pub fn request_edit(&mut self, instruction: &str, now_ms: u64) {
        let Some(engine) = &self.edit_engine else {
            self.set_status("no AI provider configured (set APERTURE_CMD)", now_ms);
            return;
        };
        if self.editor.sel.is_caret() {
            self.set_status("select text first to ask for an edit", now_ms);
            return;
        }
        let start = self.editor.sel.min();
        let end = self.editor.sel.max();
        let original = self.editor.selected_text();
        let lang = match self.lang {
            Language::Rust => "rust",
            Language::Plain => "text",
        };
        self.edit_generation += 1;
        let req = aperture_complete::edit::make_edit_request(instruction, &original, lang, self.edit_generation, 4000);
        engine.request(req);
        self.pending_edit = Some(PendingEdit { start, end, original, instruction: instruction.to_string(), proposed: None });
    }

    /// Drain the edit engine; call once per frame, same as `poll_note`.
    pub fn poll_edit_ai(&mut self, now_ms: u64) {
        let Some(engine) = &self.edit_engine else { return };
        let mut failure: Option<String> = None;
        while let Some(out) = engine.poll() {
            if out.generation != self.edit_generation {
                continue; // a stale answer for a since-superseded request
            }
            let Some(pending) = &mut self.pending_edit else { continue };
            match out.result {
                Ok(text) => pending.proposed = Some(text),
                Err(e) => failure = Some(e),
            }
        }
        if let Some(e) = failure {
            self.pending_edit = None;
            self.set_status(&format!("edit request failed: {}", e.trim().lines().next().unwrap_or(&e)), now_ms);
        }
    }

    /// Apply the proposed edit as one undo step, exactly at the ORIGINAL
    /// selection range — not wherever the caret happens to be now, since
    /// typing could have moved on while the request was in flight.
    pub fn accept_pending_edit(&mut self) {
        let Some(pending) = self.pending_edit.take() else { return };
        let Some(proposed) = pending.proposed else { return };
        let end = pending.end.min(self.editor.buffer.len_chars());
        let start = pending.start.min(end);
        self.editor.replace_range(start, end, &proposed);
        self.dirty = true;
    }

    /// Discard a proposed (or still-in-flight) edit without touching the
    /// buffer.
    pub fn reject_pending_edit(&mut self) {
        self.pending_edit = None;
    }

    /// Swap the buffer's content without touching `path`/`lang`/`git`;
    /// fresh parse, caret home, undo stack reset (see `live_text`'s doc).
    fn set_preview_text(&mut self, text: &str) {
        self.editor = Editor::from_str(text);
        self.syntax = Syntax::new(self.lang);
        self.dirty = false;
        self.popup = None;
        self.ghost.dismiss();
        self.fim_ctx_version = u64::MAX;
        self.symbols_version = u64::MAX;
    }

    fn apply_env_provider(&mut self) {
        if let Ok(host) = std::env::var("APERTURE_OLLAMA") {
            self.ollama_cfg.host = host;
        }
        if let Ok(model) = std::env::var("APERTURE_MODEL") {
            self.ollama_cfg.model = model;
        }
        if let Ok(cmd) = std::env::var("APERTURE_CMD") {
            self.command_line = cmd;
            self.provider_kind = ProviderKind::Command;
        }
    }

    pub fn build_provider(&self) -> Box<dyn Provider> {
        match self.provider_kind {
            ProviderKind::Ollama => Box::new(OllamaProvider::new(self.ollama_cfg.clone())),
            ProviderKind::Command => Box::new(CommandProvider::new(
                self.command_line.split_whitespace().map(str::to_string).collect(),
            )),
            ProviderKind::Demo => Box::new(StaticProvider::new(&[
                ("fn main", "() {\n    println!(\"hello from aperture\");\n}"),
                ("println!", "(\"{}\", value);"),
                ("for i in ", "0..10 {\n        \n    }"),
                ("let mut ", "buf = String::new();"),
                ("// TODO", ": wire this into the loop pane"),
            ])),
        }
    }

    pub fn swap_provider(&mut self) {
        let p = self.build_provider();
        if let Some(engine) = &self.engine {
            engine.swap_provider(p);
        }
        self.ghost.dismiss();
        self.set_status("provider swapped", u64::MAX);
    }

    pub fn set_status(&mut self, msg: &str, now_ms: u64) {
        let until = now_ms.saturating_add(2500);
        self.status = Some((msg.to_string(), until));
    }

    /// The context filler: the file's outline, as commented signatures, cached
    /// by buffer version. This is what turns "complete this cursor" into
    /// "complete this cursor IN THIS FILE" — the model sees every fn/type
    /// name and signature for ~one screen of budget, assembled in
    /// microseconds from the already-parsed tree (no LSP round trip; the
    /// same seam serves an LSP source later, but a local parse beats a
    /// server hop for latency at this job).
    pub fn fim_context(&mut self) -> String {
        if self.fim_ctx_version == self.editor.version {
            return self.fim_ctx.clone();
        }
        let outline = self.syntax.outline(&self.editor.buffer);
        let mut ctx = String::new();
        if !outline.is_empty() {
            ctx.push_str("// file outline:\n");
            for it in outline.iter().take(60) {
                ctx.push_str("//   ");
                ctx.push_str(&it.sig);
                ctx.push('\n');
            }
        }
        self.fim_ctx_version = self.editor.version;
        self.fim_ctx = ctx.clone();
        ctx
    }

    pub fn toggle_follow(&mut self, now_ms: u64) {
        self.follow_mode = !self.follow_mode;
        self.last_follow_line = usize::MAX; // re-center immediately on the next frame
        let msg = if self.follow_mode { "follow camera: on" } else { "follow camera: off" };
        self.set_status(msg, now_ms);
    }

    // ── symbol popup ────────────────────────────────────────────────────────

    fn gather_symbols(&mut self) -> &[(String, String)] {
        if self.symbols_version != self.editor.version {
            self.symbols = self
                .syntax
                .outline(&self.editor.buffer)
                .into_iter()
                .map(|it| (it.name, it.sig))
                .collect();
            self.symbols_version = self.editor.version;
        }
        &self.symbols
    }

    /// The word (ident chars, `::` included) ending at the caret.
    fn word_before_caret(&self) -> (usize, String) {
        let caret = self.editor.sel.head;
        let mut start = caret;
        let mut chars: Vec<char> = Vec::new();
        while start > 0 {
            let c = self.editor.buffer.char_at(start - 1);
            match c {
                Some(c) if c.is_alphanumeric() || c == '_' || c == ':' => {
                    chars.push(c);
                    start -= 1;
                }
                _ => break,
            }
        }
        chars.reverse();
        (start, chars.into_iter().collect())
    }

    /// (Re)build the popup for the word at the caret; closes it when the word
    /// is too short or nothing matches. `min_len` lets Ctrl+Space open on
    /// anything while auto-trigger waits for two chars.
    fn refresh_popup(&mut self, min_len: usize) {
        if !self.editor.sel.is_caret() {
            self.popup = None;
            return;
        }
        let (start, word) = self.word_before_caret();
        if word.chars().count() < min_len {
            self.popup = None;
            return;
        }
        self.gather_symbols();
        let mut scored: Vec<(i32, PopupItem)> = self
            .symbols
            .iter()
            .filter(|(name, _)| name != &word)
            .filter_map(|(name, sig)| {
                uikit_egui::fuzzy::score(&word, name).map(|s| {
                    (s, PopupItem { label: name.clone(), insert: name.clone(), detail: sig.clone() })
                })
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.label.len().cmp(&b.1.label.len())));
        let items: Vec<PopupItem> = scored.into_iter().map(|(_, i)| i).take(24).collect();
        if items.is_empty() {
            self.popup = None;
        } else {
            self.ghost.dismiss();
            self.popup = Some(Popup { items, sel: 0, prefix_start: start });
        }
    }

    fn popup_accept(&mut self, now_ms: u64) {
        let Some(p) = self.popup.take() else { return };
        let Some(item) = p.items.get(p.sel) else { return };
        let caret = self.editor.sel.head;
        self.editor.replace_range(p.prefix_start, caret, &item.insert);
        self.after_edit(now_ms);
        self.arm_completion(now_ms);
    }

    /// Popup key handling; true = consumed. Runs BEFORE the ghost so Tab
    /// prefers the explicit list over the speculative ghost.
    fn popup_key(&mut self, key: K, shift: bool, now_ms: u64) -> bool {
        if self.popup.is_none() {
            return false;
        }
        match key {
            K::Down => {
                if let Some(p) = &mut self.popup {
                    p.sel = (p.sel + 1) % p.items.len();
                }
            }
            K::Up => {
                if let Some(p) = &mut self.popup {
                    p.sel = (p.sel + p.items.len() - 1) % p.items.len();
                }
            }
            K::Tab | K::Enter => {
                let _ = shift;
                self.popup_accept(now_ms);
            }
            K::Escape => self.popup = None,
            K::Backspace => {
                self.popup = None; // the backspace itself falls through to the editor
                return false;
            }
            _ => return false,
        }
        true
    }

    // ── auto-pairs ──────────────────────────────────────────────────────────
    //
    // Three moves, all invertible through the ordinary op history:
    //  wrap    — selection + opener types the pair AROUND it, selection kept;
    //  pair    — opener at a caret inserts both, caret lands between;
    //  skip    — typing a closer that's already under the caret steps over it
    //            instead of doubling it (unconditional: predictable beats
    //            clever bookkeeping about who inserted it).
    // Quotes only pair when they don't touch a word (so `don't` and Rust
    // lifetimes never fight the editor). Backspace inside an empty pair
    // removes both (one gesture).

    fn pair_of(open: char) -> Option<char> {
        match open {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '\'' => Some('\''),
            _ => None,
        }
    }

    fn try_auto_pair(&mut self, t: &str, now_ms: u64) -> bool {
        let mut chars = t.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else { return false };
        let caret = self.editor.sel.head;
        let next = self.editor.buffer.char_at(caret);
        let is_quote = ch == '"' || ch == '\'';

        // skip-over: the closer is already there.
        if matches!(ch, ')' | ']' | '}') || is_quote {
            if next == Some(ch) && self.editor.sel.is_caret() {
                self.ghost.dismiss();
                self.debounce.cancel();
                self.editor.move_right(false);
                return true;
            }
            if !is_quote {
                return false; // a bare closer with nothing under the caret: type it plainly
            }
        }

        let Some(close) = Self::pair_of(ch) else { return false };

        // wrap: opener with a selection surrounds it and keeps it selected.
        if !self.editor.sel.is_caret() {
            let (a, b) = (self.editor.sel.min(), self.editor.sel.max());
            let inner = self.editor.buffer.slice_to_string(a, b);
            self.ghost.dismiss();
            self.editor.replace_range(a, b, &format!("{ch}{inner}{close}"));
            self.editor.sel = sel_range(a + 1, b + 1);
            self.after_edit(now_ms);
            return true;
        }

        // quotes: never pair against a word (apostrophes, lifetimes).
        if is_quote {
            let prev = if caret > 0 { self.editor.buffer.char_at(caret - 1) } else { None };
            let word = |c: Option<char>| c.map_or(false, |c| c.is_alphanumeric() || c == '_');
            if word(prev) || word(next) {
                return false;
            }
        }

        // pair: both chars, caret between — ONE history gesture, so a single
        // undo removes the pair the way a single keystroke created it.
        self.ghost.dismiss();
        self.debounce.cancel();
        self.editor.insert_text(&format!("{ch}{close}"), now_ms);
        self.editor.move_left(false);
        self.after_edit(now_ms);
        true
    }

    /// Backspace between an empty pair deletes both halves.
    fn backspace_pair(&mut self, now_ms: u64) -> bool {
        if !self.editor.sel.is_caret() {
            return false;
        }
        let caret = self.editor.sel.head;
        if caret == 0 {
            return false;
        }
        let (prev, next) = (self.editor.buffer.char_at(caret - 1), self.editor.buffer.char_at(caret));
        let paired = matches!((prev, next),
            (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}'))
            | (Some('"'), Some('"')) | (Some('\''), Some('\'')));
        if paired {
            self.editor.replace_range(caret - 1, caret + 1, "");
            self.after_edit(now_ms);
            return true;
        }
        false
    }

    // ── revision preview (read-only) ────────────────────────────────────────

    /// While `rev_ix.is_some()`: cursor movement, scrolling, and Ctrl+,/.
    /// pass through normally (browsing a fixed text is exactly what the
    /// editor's motion code already does); anything that would MUTATE the
    /// buffer is swallowed instead of silently no-oping — Escape (or
    /// stepping past the newest revision) is the one documented way home.
    fn handle_preview(&mut self, ev: &Ev, now_ms: u64) -> Option<Effects> {
        match ev {
            Ev::Key { key: K::Escape, .. } => {
                while self.rev_ix.is_some() {
                    self.rev_step(-1, now_ms);
                }
                Some(Effects::default())
            }
            Ev::Key { key: K::Comma, ctrl: true, .. } => {
                self.rev_step(1, now_ms);
                Some(Effects::default())
            }
            Ev::Key { key: K::Period, ctrl: true, .. } => {
                self.rev_step(-1, now_ms);
                Some(Effects::default())
            }
            Ev::Key { key: K::B, ctrl: true, .. } => {
                self.branch_from_here(now_ms);
                Some(Effects::default())
            }
            Ev::Key { key: K::M, ctrl: true, .. } => {
                self.materialize_here(now_ms);
                Some(Effects::default())
            }
            Ev::Text(_) | Ev::Paste(_) | Ev::Cut | Ev::Preedit(_) => Some(Effects::default()), // swallow mutation
            Ev::Key { key: K::Backspace | K::Delete | K::Tab | K::Enter, .. } => Some(Effects::default()),
            _ => None, // motion, copy, scroll, etc. — handle normally
        }
    }

    // ── find (Ctrl+F) ───────────────────────────────────────────────────────

    pub fn open_find(&mut self) {
        self.open_find_ex(false);
    }


    pub fn open_find_with_replace(&mut self) {
        self.open_find_ex(true);
    }

    fn open_find_ex(&mut self, with_replace: bool) {
        let seed = if !self.editor.sel.is_caret() {
            let t = self.editor.selected_text();
            if t.contains('\n') { String::new() } else { t }
        } else {
            String::new()
        };
        let mut f = FindState {
            query: seed,
            matches: Vec::new(),
            cur: 0,
            last_version: u64::MAX,
            last_query: String::new(),
            history_ix: None,
            replace: if with_replace { Some(String::new()) } else { None },
            focus: FindFocus::Query,
        };
        f.recompute(&self.editor.buffer, self.editor.version);
        self.find = Some(f);
        self.find_jump(1, true);
    }

    /// Returns Some(effects) when the event was consumed by find; None when
    /// it closed find and should still be handled normally (never currently).
    fn handle_find(&mut self, ev: &Ev, now_ms: u64) -> Option<Effects> {
        let fx = Effects::default();
        // Which string Text/Backspace edit: the query (find-only's original,
        // unchanged target) or the replace text, once replace mode has
        // focus there. Never both at once.
        let editing_replace = self
            .find
            .as_ref()
            .is_some_and(|f| f.replace.is_some() && f.focus == FindFocus::Replace);
        match ev {
            Ev::Text(t) => {
                if let Some(f) = &mut self.find {
                    if editing_replace {
                        f.replace.as_mut().unwrap().push_str(t);
                    } else {
                        f.query.push_str(t);
                        f.history_ix = None;
                    }
                }
                if !editing_replace {
                    self.find_refresh_jump();
                }
            }
            Ev::Paste(t) => {
                let clean = t.replace('\n', " ");
                if let Some(f) = &mut self.find {
                    if editing_replace {
                        f.replace.as_mut().unwrap().push_str(&clean);
                    } else {
                        f.query.push_str(&clean);
                        f.history_ix = None;
                    }
                }
                if !editing_replace {
                    self.find_refresh_jump();
                }
            }
            Ev::Key { key: K::Backspace, .. } => {
                if let Some(f) = &mut self.find {
                    if editing_replace {
                        f.replace.as_mut().unwrap().pop();
                    } else {
                        f.query.pop();
                        f.history_ix = None;
                    }
                }
                if !editing_replace {
                    self.find_refresh_jump();
                }
            }
            Ev::Key { key: K::Escape, .. } => {
                if let Some(f) = self.find.take() {
                    let q = f.query.trim().to_string();
                    if !q.is_empty() && self.find_history.last() != Some(&q) {
                        self.find_history.push(q);
                    }
                }
            }
            Ev::Key { key: K::Tab, .. } => {
                if let Some(f) = &mut self.find {
                    if f.replace.is_some() {
                        f.focus = if f.focus == FindFocus::Query { FindFocus::Replace } else { FindFocus::Query };
                    }
                }
            }
            Ev::Key { key: K::Enter, shift, ctrl: true, .. } if editing_replace => {
                let _ = shift;
                self.replace_all(now_ms);
            }
            Ev::Key { key: K::Enter, .. } if editing_replace => {
                self.replace_current(now_ms);
            }
            Ev::Key { key: K::Enter, shift, .. } | Ev::Key { key: K::F3, shift, .. } => {
                self.find_jump(if *shift { -1 } else { 1 }, false);
            }
            Ev::Key { key: K::Down, ctrl: true, .. } => self.find_history_step(1),
            Ev::Key { key: K::Up, ctrl: true, .. } => self.find_history_step(-1),
            Ev::Key { key: K::Down, .. } => self.find_jump(1, false),
            Ev::Key { key: K::Up, .. } => self.find_jump(-1, false),
            Ev::Key { key: K::F, ctrl: true, .. } => {} // already open; keep typing
            Ev::Key { key: K::H, ctrl: true, .. } => {
                if let Some(f) = &mut self.find {
                    if f.replace.is_none() {
                        f.replace = Some(String::new());
                    }
                }
            }
            _ => return Some(fx), // swallow everything else while the bar is up
        }
        Some(fx)
    }

    /// Replace the CURRENT match with the replace text, then jump to what is
    /// now the next match — the same "replace, then advance" a person
    /// expects from a Find & Replace "Replace" button. One undo step per
    /// press (`Editor::replace_range`'s existing contract).
    fn replace_current(&mut self, now_ms: u64) {
        let Some(f) = &self.find else { return };
        let (Some(replacement), Some(&(a, b))) = (f.replace.clone(), f.matches.get(f.cur)) else { return };
        self.editor.replace_range(a, b, &replacement);
        self.after_edit(now_ms);
        self.find_refresh_jump();
    }

    /// Replace EVERY match in one `Editor::replace_many` call — one undo
    /// step for the whole operation, not one per match.
    fn replace_all(&mut self, now_ms: u64) {
        let Some(f) = &self.find else { return };
        let Some(replacement) = f.replace.clone() else { return };
        if f.matches.is_empty() {
            return;
        }
        let ranges: Vec<(usize, usize, String)> = f.matches.iter().map(|&(a, b)| (a, b, replacement.clone())).collect();
        let n = ranges.len();
        self.editor.replace_many(ranges, now_ms);
        self.after_edit(now_ms);
        self.set_status(&format!("replaced {n} match{}", if n == 1 { "" } else { "es" }), now_ms);
        self.find_refresh_jump();
    }

    /// Ctrl+Up (`d = -1`, older) / Ctrl+Down (`d = 1`, newer) through
    /// `find_history`. Older from the live-typed state starts at the most
    /// recent entry; newer than the most recent entry returns to an empty
    /// query — the same "one past the newest = blank line" convention a
    /// shell's history gives you.
    fn find_history_step(&mut self, d: isize) {
        if self.find_history.is_empty() {
            return;
        }
        let Some(f) = &mut self.find else { return };
        let n = self.find_history.len() as isize;
        match f.history_ix {
            // Older from live: land on the most recent entry. Newer from
            // live is a no-op — there's nothing more recent than "live."
            None => {
                if d < 0 {
                    f.history_ix = Some((n - 1) as usize);
                    f.query = self.find_history[(n - 1) as usize].clone();
                }
            }
            Some(i) => {
                let next = i as isize + d;
                if next < 0 {
                    // Older than the oldest entry: CLAMP, don't wrap to
                    // blank — that reset is only for the opposite end.
                } else if next >= n {
                    f.history_ix = None;
                    f.query.clear();
                } else {
                    f.history_ix = Some(next as usize);
                    f.query = self.find_history[next as usize].clone();
                }
            }
        }
        self.find_refresh_jump();
    }

    fn find_refresh_jump(&mut self) {
        let version = self.editor.version;
        if let Some(f) = &mut self.find {
            f.recompute(&self.editor.buffer, version);
        }
        self.find_jump(1, true);
    }

    /// Move to the next/prev match. `from_caret` = seek the first match at or
    /// after the caret instead of stepping from the current index.
    fn find_jump(&mut self, dir: isize, from_caret: bool) {
        let caret = self.editor.sel.min();
        let version = self.editor.version;
        let Some(f) = &mut self.find else { return };
        f.recompute(&self.editor.buffer, version);
        if f.matches.is_empty() {
            return;
        }
        let n = f.matches.len();
        f.cur = if from_caret {
            f.matches.iter().position(|&(s, _)| s >= caret).unwrap_or(0)
        } else if dir > 0 {
            (f.cur + 1) % n
        } else {
            (f.cur + n - 1) % n
        };
        let (a, b) = f.matches[f.cur];
        self.editor.sel = crate::state::sel_range(a, b);
    }

    // ── the event loop ──────────────────────────────────────────────────────

    pub fn handle(&mut self, ev: Ev, now_ms: u64) -> Effects {
        let mut fx = Effects::default();
        if self.find.is_some() {
            if let Some(fx) = self.handle_find(&ev, now_ms) {
                return fx;
            }
            // fall through: the event closed find and should also act normally
        }
        if self.rev_ix.is_some() {
            if let Some(fx) = self.handle_preview(&ev, now_ms) {
                return fx;
            }
        }
        match ev {
            Ev::Text(t) => self.type_text(&t, now_ms),
            Ev::Paste(t) => {
                self.ghost.dismiss();
                self.debounce.cancel();
                self.editor.insert_text(&t, now_ms);
                self.after_edit(now_ms);
            }
            Ev::Copy => fx.clipboard = self.copy_selection(),
            Ev::Cut => {
                fx.clipboard = self.copy_selection();
                if !self.editor.sel.is_caret() {
                    let (a, b) = (self.editor.sel.min(), self.editor.sel.max());
                    self.editor.replace_range(a, b, "");
                    self.after_edit(now_ms);
                }
            }
            Ev::Key { key, ctrl, shift, alt } => self.key(key, ctrl, shift, alt, now_ms, &mut fx),
            Ev::Preedit(s) => {
                // egui's own TextEdit ignores a literal "\n"/"\r" arriving
                // as preedit text — a documented platform quirk (some IME
                // integrations emit one on certain composition states) —
                // kept here for the same reason: accidentally inserting a
                // newline mid-composition would be a much worse failure
                // than just not reacting to it.
                if s != "\n" && s != "\r" {
                    self.preedit = s;
                    if !self.preedit.is_empty() {
                        // Composing text and a speculative ghost suggestion
                        // are both "text that isn't real yet" — showing
                        // both at once at the caret is genuinely confusing,
                        // not just visually busy.
                        self.ghost.dismiss();
                        self.popup = None;
                    }
                }
            }
        }
        fx
    }

    fn type_text(&mut self, t: &str, now_ms: u64) {
        self.preedit.clear(); // a real insertion always ends any pending composition
        if self.try_auto_pair(t, now_ms) {
            self.popup = None;
            return;
        }
        let consumed = self.ghost.on_text_inserted(t);
        self.editor.insert_text(t, now_ms);
        self.dirty = true;
        self.syntax.absorb(&mut self.editor.buffer);
        self.follow_caret = true;
        if consumed {
            // Ghost absorbed the keystroke: no re-request, no debounce churn.
            self.debounce.cancel();
        } else {
            self.arm_completion(now_ms);
        }
        // Popup auto-trigger — but never over a SHOWN ghost: the ghost is a
        // concrete suggestion in hand, the popup is for fishing. Two ident
        // chars open it; any non-ident char closes it.
        if !self.ghost.is_shown() {
            let ident = t.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':');
            if ident {
                self.refresh_popup(2);
            } else {
                self.popup = None;
            }
        }
    }

    fn after_edit(&mut self, now_ms: u64) {
        self.dirty = true;
        self.syntax.absorb(&mut self.editor.buffer);
        self.follow_caret = true;
        let _ = now_ms;
    }

    fn arm_completion(&mut self, now_ms: u64) {
        // No engine check here: the debounce records intent ("an edit
        // happened"); whether a request can actually go out is decided once,
        // at the send site (`trigger_completion_now`).
        if self.ai_enabled {
            self.debounce.note_edit(now_ms);
        }
    }

    fn key(&mut self, key: K, ctrl: bool, shift: bool, alt: bool, now_ms: u64, fx: &mut Effects) {
        use K::*;
        let _ = alt;
        // Popup first: an explicit list outranks the speculative ghost.
        if self.popup_key(key, shift, now_ms) {
            return;
        }
        // Ghost interception next: Tab / Ctrl+→ / Esc while shown.
        if self.ghost.is_shown() {
            match (key, ctrl) {
                (Tab, false) => {
                    if let Some(text) = self.ghost.accept_all() {
                        self.editor.insert_text(&text, now_ms);
                        self.after_edit(now_ms);
                        self.debounce.cancel();
                    }
                    return;
                }
                (Right, true) => {
                    if let Some(word) = self.ghost.accept_word() {
                        self.editor.insert_text(&word, now_ms);
                        self.dirty = true;
                        self.syntax.absorb(&mut self.editor.buffer);
                        self.follow_caret = true;
                        self.debounce.cancel();
                    }
                    return;
                }
                (Escape, _) => {
                    self.ghost.dismiss();
                    self.debounce.cancel();
                    return;
                }
                _ => {}
            }
        }
        match (key, ctrl) {
            (Z, true) if shift => self.with_dismiss(|s| s.editor.redo(), now_ms),
            (Z, true) => self.with_dismiss(|s| s.editor.undo(), now_ms),
            (Y, true) => self.with_dismiss(|s| s.editor.redo(), now_ms),
            (A, true) => self.editor.select_all(),
            (S, true) => self.save(now_ms),
            (O, true) => self.open_palette_in_open_mode(),
            (P, true) => self.open_palette(),
            (F, true) if shift => self.open_project_search(),
            (F, true) => self.open_find(),
            (T, true) => self.open_workspace_symbol(),
            (G, true) => self.open_go_to_route(),
            (H, true) => self.open_find_with_replace(),
            (E, true) => self.toggle_follow(now_ms),
            (Comma, true) => self.rev_step(1, now_ms),
            (Period, true) => self.rev_step(-1, now_ms),
            (B, true) => self.branch_from_here(now_ms),
            (M, true) => self.materialize_here(now_ms),
            (F3, false) => {
                // F3 with the bar closed: reopen on the last query and step.
                if self.find.is_none() {
                    self.open_find();
                } 
            }
            (Space, true) if shift => self.trigger_completion_now(now_ms),
            (Space, true) => self.refresh_popup(1),
            (Left, true) => self.moved(|s, sh| s.editor.move_word(-1, sh), shift),
            (Right, true) => self.moved(|s, sh| s.editor.move_word(1, sh), shift),
            (Home, true) => self.moved(|s, sh| s.editor.move_doc(false, sh), shift),
            (End, true) => self.moved(|s, sh| s.editor.move_doc(true, sh), shift),
            (Left, false) => self.moved(|s, sh| s.editor.move_left(sh), shift),
            (Right, false) => self.moved(|s, sh| s.editor.move_right(sh), shift),
            (Up, false) => self.moved(|s, sh| s.editor.move_vertical(-1, sh), shift),
            (Down, false) => self.moved(|s, sh| s.editor.move_vertical(1, sh), shift),
            (PageUp, false) => self.moved(|s, sh| s.editor.move_vertical(-24, sh), shift),
            (PageDown, false) => self.moved(|s, sh| s.editor.move_vertical(24, sh), shift),
            (Home, false) => self.moved(|s, sh| s.editor.move_home(sh), shift),
            (End, false) => self.moved(|s, sh| s.editor.move_end(sh), shift),
            (Backspace, _) => {
                self.ghost.dismiss();
                if !self.backspace_pair(now_ms) {
                    self.editor.backspace(now_ms);
                    self.after_edit(now_ms);
                }
                self.arm_completion(now_ms);
            }
            (Delete, _) => {
                self.ghost.dismiss();
                self.editor.delete_forward(now_ms);
                self.after_edit(now_ms);
            }
            (Enter, _) => {
                self.ghost.dismiss();
                self.editor.newline_smart(now_ms);
                self.after_edit(now_ms);
                self.arm_completion(now_ms);
            }
            (Tab, false) => {
                self.editor.insert_tab(now_ms);
                self.after_edit(now_ms);
            }
            (Escape, _) => {
                self.ghost.dismiss();
                self.debounce.cancel();
            }
            _ => {}
        }
        let _ = fx;
    }

    fn with_dismiss(&mut self, f: impl FnOnce(&mut Self), _now_ms: u64) {
        self.ghost.dismiss();
        self.debounce.cancel();
        f(self);
        self.dirty = true;
        self.syntax.absorb(&mut self.editor.buffer);
        self.follow_caret = true;
    }

    fn moved(&mut self, f: impl FnOnce(&mut Self, bool), shift: bool) {
        self.ghost.dismiss();
        self.debounce.cancel();
        f(self, shift);
        self.follow_caret = true;
    }

    fn copy_selection(&self) -> Option<String> {
        let s = self.editor.selected_text();
        (!s.is_empty()).then_some(s)
    }

    pub fn save(&mut self, now_ms: u64) {
        match &self.path {
            Some(p) => match self.editor.buffer.save(p) {
                Ok(()) => {
                    self.dirty = false;
                    self.refresh_git();
                    self.set_status("saved", now_ms);
                }
                Err(e) => self.set_status(&format!("save failed: {e}"), now_ms),
            },
            None => self.set_status("no path — open a file first (Ctrl+O)", now_ms),
        }
    }

    pub fn open_file(&mut self, path: PathBuf, now_ms: u64) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.lang = Language::from_path(&path);
                self.syntax.set_language(self.lang);
                self.editor = Editor::from_str(&text);
                self.path = Some(path);
                self.dirty = false;
                self.ghost.dismiss();
                self.debounce.cancel();
                self.cam.scroll_y.snap(0.0);
                self.follow_caret = false;
                self.set_status("opened", now_ms);
            }
            Err(e) => self.set_status(&format!("open failed: {e}"), now_ms),
        }
    }

    // ── AI wiring, called from the view's frame tick ────────────────────────

    /// Fire the debounce if due; send at most one request.
    pub fn tick_completion(&mut self, now_ms: u64) {
        if self.debounce.fire(now_ms) {
            self.trigger_completion_now(now_ms);
        }
    }

    /// Ctrl+Space and the palette's "complete now" both land here.
    pub fn trigger_completion_now(&mut self, _now_ms: u64) {
        let ctx = self.fim_context();
        let Some(engine) = &self.engine else { return };
        if !self.ai_enabled {
            return;
        }
        // Only offer at a caret (not over a selection): a suggestion that
        // replaces a selection is a different feature (edit prediction).
        if !self.editor.sel.is_caret() {
            return;
        }
        let head = self.editor.sel.head;
        let before = self.editor.buffer.slice_to_string(0, head);
        let after = self.editor.buffer.slice_to_string(head, self.editor.buffer.len_chars());
        let lang = match self.lang {
            Language::Rust => "rust",
            Language::Plain => "text",
        };
        let req = make_request(&ctx, &before, &after, lang, self.editor.version, PREFIX_BUDGET, SUFFIX_BUDGET);
        self.ghost.request_sent(req.generation);
        engine.request(req);
    }

    /// Drain engine outcomes into the ghost + the loop log.
    pub fn poll_completions(&mut self, now_ms: u64) {
        let mut outs = Vec::new();
        if let Some(engine) = &self.engine {
            while let Some(out) = engine.poll() {
                outs.push(out);
            }
        }
        for out in outs {
            self.note_outcome(&out, now_ms);
        }
    }

    pub fn note_outcome(&mut self, out: &CompletionOutcome, now_ms: u64) {
        let was_shown = self.ghost.is_shown();
        // Ghost text updates on EVERY outcome, partial or final — that's
        // the whole point of streaming: the text grows on screen as it
        // arrives, not just once at the end.
        self.ghost.on_outcome(out.generation, self.editor.version, &out.result);
        if self.ghost.is_shown() && !was_shown {
            self.ghost_since = Some(now_ms);
        }
        // The loop pane is a history of REQUESTS answered, not a firehose
        // of fragments — a 20-delta streamed completion should read as one
        // row, not twenty near-duplicates burying everything before it.
        if !out.is_final {
            return;
        }
        let (ok, brief) = match &out.result {
            Ok(t) => (true, first_line(t, 64)),
            Err(e) => (false, first_line(e, 64)),
        };
        self.loop_log.push_front(LoopRow {
            generation: out.generation,
            provider: out.provider.clone(),
            latency_ms: out.latency_ms,
            ok,
            brief,
        });
        self.loop_log.truncate(40);
    }

    fn open_palette(&mut self) {
        self.palette = Some(crate::palette_ui::PaletteState::commands());
        self.ghost.dismiss();
        self.debounce.cancel();
    }

    fn open_palette_in_open_mode(&mut self) {
        let root = self.workspace_root();
        let files = crate::file_finder::walk_files(&root, 4000);
        self.palette = Some(crate::palette_ui::PaletteState::open_file(root, files));
        self.ghost.dismiss();
        self.debounce.cancel();
    }

    fn open_project_search(&mut self) {
        let root = self.workspace_root();
        let files = crate::file_finder::walk_files(&root, 4000);
        self.palette = Some(crate::palette_ui::PaletteState::project_search(root, files));
        self.ghost.dismiss();
        self.debounce.cancel();
    }

    fn open_workspace_symbol(&mut self) {
        let root = self.workspace_root();
        let files = crate::file_finder::walk_files(&root, 4000);
        self.palette = Some(crate::palette_ui::PaletteState::workspace_symbol(root, files));
        self.ghost.dismiss();
        self.debounce.cancel();
    }

    fn open_go_to_route(&mut self) {
        let root = self.workspace_root();
        let files = crate::file_finder::walk_files(&root, 4000);
        self.palette = Some(crate::palette_ui::PaletteState::go_to_route(root, files));
        self.ghost.dismiss();
        self.debounce.cancel();
    }

    /// Open `path` (same as `open_file`) and land the caret at `line` (a
    /// 0-based line index — project search's own convention, matching
    /// `Pos::line`/`goto_line`), then aim the camera at it. Used by project
    /// search's jump-to-hit; `open_file` alone would land at char 0, losing
    /// exactly the thing a search result promised.
    pub fn open_file_at_line(&mut self, path: PathBuf, line: usize, now_ms: u64) {
        self.open_file(path, now_ms);
        self.editor.goto_line(line);
        self.follow_caret = true;
    }

    /// Caret char index for a click at content-space (x px, y px).
    pub fn caret_from_click(&self, x: f32, y: f32, line_h: f32, char_w: f32) -> usize {
        let line = ((y / line_h).floor().max(0.0)) as usize;
        let line = line.min(self.editor.buffer.len_lines().saturating_sub(1));
        let text = self.editor.buffer.line(line);
        let target_visual = ((x / char_w) + 0.5).floor().max(0.0) as usize;
        let col = aperture_core::width::visual_to_char_col(&text, target_visual);
        self.editor
            .buffer
            .pos_to_char(aperture_core::Pos::new(line, col))
    }

    pub fn click_set_caret(&mut self, ci: usize, extend: bool) {
        self.ghost.dismiss();
        self.debounce.cancel();
        self.editor.sel = if extend {
            Selection { anchor: self.editor.sel.anchor, head: ci }
        } else {
            Selection::caret(ci)
        };
        self.follow_caret = false; // the user aimed; don't yank the camera
    }
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or("");
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max || s.lines().count() > 1 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aperture_complete::CompletionOutcome;

    fn typed(s: &mut AppState, text: &str, t: u64) {
        s.handle(Ev::Text(text.into()), t);
    }
    fn key(s: &mut AppState, k: K, ctrl: bool, t: u64) {
        s.handle(Ev::Key { key: k, ctrl, shift: false, alt: false }, t);
    }

    /// The whole ghost session, headless: type → idle → request → outcome →
    /// Tab → text lands → undo removes it as one gesture.
    #[test]
    fn full_completion_session_without_a_window() {
        let mut s = AppState::new(None, false);
        s.engine = Some(Engine::new(s.build_provider())); // never polled; we inject
        for (i, ch) in "fn main".chars().enumerate() {
            typed(&mut s, &ch.to_string(), i as u64 * 50);
        }
        assert!(s.debounce.armed());
        s.tick_completion(350 + 280); // idle window passed → request sent
        assert!(s.ghost.is_pending());
        let generation = s.editor.version;
        s.note_outcome(&CompletionOutcome {
            generation,
            provider: "test".into(),
            latency_ms: 12,
            result: Ok("() {}".into()),
            is_final: true,
        }, 700);
        assert!(s.ghost.is_shown());
        key(&mut s, K::Tab, false, 1000);
        assert_eq!(s.editor.buffer.to_string(), "fn main() {}");
        key(&mut s, K::Z, true, 1100); // undo the acceptance…
        assert_eq!(s.editor.buffer.to_string(), "fn main");
        key(&mut s, K::Z, true, 1200); // …then the typed word, as its own group
        assert_eq!(s.editor.buffer.to_string(), "fn ");
    }

    #[test]
    fn typing_through_the_ghost_keeps_it_alive() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "pr", 0);
        s.ghost = Ghost::Shown { text: "intln!".into() };
        typed(&mut s, "i", 10);
        assert_eq!(s.ghost, Ghost::Shown { text: "ntln!".into() });
        assert!(!s.debounce.armed(), "consumed keystroke doesn't re-arm");
        typed(&mut s, "x", 20);
        assert_eq!(s.ghost, Ghost::Idle, "divergence dismissed it");
        assert!(s.debounce.armed(), "and armed a fresh request");
    }

    #[test]
    fn stale_outcome_never_becomes_a_ghost() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "a", 0);
        s.ghost.request_sent(s.editor.version);
        let stale_generation = s.editor.version;
        typed(&mut s, "b", 10); // buffer moved on (this also dismissed → re-request path)
        s.ghost.request_sent(s.editor.version);
        s.note_outcome(&CompletionOutcome {
            generation: stale_generation,
            provider: "test".into(),
            latency_ms: 5,
            result: Ok("OLD".into()),
            is_final: true,
        }, 100);
        assert!(!s.ghost.is_shown());
        assert_eq!(s.loop_log.len(), 1, "…but the loop pane still shows it happened");
    }

    #[test]
    fn word_accept_inserts_incrementally() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "x", 0);
        s.ghost = Ghost::Shown { text: " = compute();".into() };
        key(&mut s, K::Right, true, 100);
        assert_eq!(s.editor.buffer.to_string(), "x =");
        key(&mut s, K::Right, true, 200);
        assert_eq!(s.editor.buffer.to_string(), "x = compute");
        assert!(s.ghost.is_shown(), "remainder still offered");
    }

    #[test]
    fn selection_blocks_requests_and_escape_clears() {
        let mut s = AppState::new(None, false);
        s.engine = Some(Engine::new(s.build_provider()));
        typed(&mut s, "abc", 0);
        s.editor.select_all();
        s.trigger_completion_now(0);
        assert!(!s.ghost.is_pending(), "no ghost over a selection");
        s.ghost = Ghost::Shown { text: "zzz".into() };
        key(&mut s, K::Escape, false, 0);
        assert_eq!(s.ghost, Ghost::Idle);
    }

    #[test]
    fn click_maps_pixels_to_caret_with_rounding() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::Editor::from_str("hello\nworld");
        let ci = s.caret_from_click(3.0 * 8.0 + 3.0, 1.0 * 16.0 + 2.0, 16.0, 8.0);
        assert_eq!(s.editor.buffer.char_to_pos(ci), aperture_core::Pos::new(1, 3));
    }
    #[test]
    fn find_is_case_smart_and_cycles() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "Alpha beta\nALPHA gamma\nalpha", 0);
        key(&mut s, K::F, true, 100);
        for ch in "alpha".chars() {
            s.handle(Ev::Text(ch.to_string()), 110);
        }
        let f = s.find.as_ref().unwrap();
        assert_eq!(f.matches.len(), 3, "lowercase query matches all cases");
        // Enter cycles forward and wraps; the selection tracks the match.
        let first = s.editor.sel;
        key(&mut s, K::Enter, false, 120);
        key(&mut s, K::Enter, false, 130);
        key(&mut s, K::Enter, false, 140);
        assert_eq!(s.editor.sel, first, "three steps over three matches wraps home");
        // An uppercase char flips to case-sensitive.
        s.handle(Ev::Text("X".into()), 150);
        for _ in 0..1 {} // (query is now "alphaX": no matches)
        assert_eq!(s.find.as_ref().unwrap().matches.len(), 0);
        key(&mut s, K::Backspace, false, 160);
        key(&mut s, K::Escape, false, 170);
        assert!(s.find.is_none(), "esc closes");
        // Typing goes to the editor again — and since Esc leaves the match
        // selected (jump-then-edit is the workflow), it REPLACES the match.
        let before = s.editor.buffer.len_chars();
        s.handle(Ev::Text("!".into()), 180);
        assert_eq!(s.editor.buffer.len_chars(), before - 5 + 1);
        assert!(s.editor.buffer.to_string().contains('!'));
    }

    #[test]
    fn find_seeds_from_selection_and_swallows_while_open() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "needle hay needle", 0);
        s.editor.sel = crate::state::sel_range(0, 6); // "needle"
        key(&mut s, K::F, true, 100);
        assert_eq!(s.find.as_ref().unwrap().query, "needle");
        assert_eq!(s.find.as_ref().unwrap().matches.len(), 2);
        // Ctrl+S while the bar is up is swallowed, not a save.
        let dirty_before = s.dirty;
        key(&mut s, K::S, true, 110);
        assert_eq!(s.dirty, dirty_before);
        assert!(s.find.is_some());
    }

    #[test]
    fn auto_pairs_pair_skip_wrap_and_unpair() {
        let mut s = AppState::new(None, false);
        s.handle(Ev::Text("(".into()), 0);
        assert_eq!(s.editor.buffer.to_string(), "()");
        assert_eq!(s.editor.sel.head, 1, "caret between");
        s.handle(Ev::Text(")".into()), 10);
        assert_eq!(s.editor.buffer.to_string(), "()", "skip-over, not doubled");
        assert_eq!(s.editor.sel.head, 2);
        key(&mut s, K::Z, true, 20);
        assert_eq!(s.editor.buffer.to_string(), "", "the pair was ONE undo gesture");
        // wrap a selection, selection survives inside
        typed(&mut s, "abc", 100);
        s.editor.sel = crate::state::sel_range(0, 3);
        s.handle(Ev::Text("[".into()), 200);
        assert_eq!(s.editor.buffer.to_string(), "[abc]");
        assert_eq!((s.editor.sel.min(), s.editor.sel.max()), (1, 4));
        key(&mut s, K::Z, true, 300);
        assert_eq!(s.editor.buffer.to_string(), "abc", "wrap undoes whole");
    }

    #[test]
    fn quotes_respect_words_and_backspace_eats_pairs() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "don", 0);
        s.handle(Ev::Text("'".into()), 50);
        assert_eq!(s.editor.buffer.to_string(), "don'", "apostrophe after a word never pairs");
        typed(&mut s, "t ", 60);
        s.handle(Ev::Text("\"".into()), 100);
        assert_eq!(s.editor.buffer.to_string(), "don't \"\"");
        key(&mut s, K::Backspace, false, 200);
        assert_eq!(s.editor.buffer.to_string(), "don't ", "backspace inside an empty pair eats both");
    }

    #[test]
    fn follow_mode_toggles_and_rearms() {
        let mut s = AppState::new(None, false);
        assert!(!s.follow_mode);
        key(&mut s, K::E, true, 0);
        assert!(s.follow_mode);
        assert_eq!(s.last_follow_line, usize::MAX, "toggle re-arms an immediate recenter");
        s.last_follow_line = 5;
        key(&mut s, K::E, true, 100);
        assert!(!s.follow_mode);
        key(&mut s, K::E, true, 200);
        assert_eq!(s.last_follow_line, usize::MAX, "re-enable re-arms again");
    }

    #[test]
    fn popup_suggests_outline_symbols_and_accepts() {
        let mut s = AppState::new(None, false);
        s.lang = aperture_core::syntax::Language::Rust;
        s.syntax.set_language(s.lang);
        typed(&mut s, "fn compute_total() {}\nfn compute_tax() {}\nfn main() { ", 0);
        for ch in "comp".chars() {
            s.handle(Ev::Text(ch.to_string()), 100);
        }
        // typing "comp" keystroke-by-keystroke (2+ ident chars, no ghost) auto-opened it
        let p = s.popup.as_ref().expect("auto-triggered");
        assert!(p.items.iter().any(|i| i.label == "compute_total"), "{:?}",
            p.items.iter().map(|i| &i.label).collect::<Vec<_>>());
        assert!(p.items.iter().any(|i| i.label == "compute_tax"));
        assert!(p.items.iter().all(|i| i.label != "main"), "non-matches filtered");
        // Tab completes the word IN PLACE (replaces the typed prefix).
        let want = p.items[0].label.clone();
        key(&mut s, K::Tab, false, 500);
        assert!(s.popup.is_none());
        assert!(s.editor.buffer.to_string().ends_with(&format!("main() {{ {want}")),
            "{}", s.editor.buffer.to_string());
        // One undo removes the completion as a unit.
        key(&mut s, K::Z, true, 600);
        assert!(s.editor.buffer.to_string().ends_with("main() { comp"));
    }

    #[test]
    fn popup_esc_closes_and_nonident_dismisses() {
        let mut s = AppState::new(None, false);
        s.lang = aperture_core::syntax::Language::Rust;
        s.syntax.set_language(s.lang);
        typed(&mut s, "fn hello_world() {}\n", 0);
        for ch in "hel".chars() {
            s.handle(Ev::Text(ch.to_string()), 50);
        }
        assert!(s.popup.is_some());
        key(&mut s, K::Escape, false, 100);
        assert!(s.popup.is_none());
        typed(&mut s, "l", 200);
        assert!(s.popup.is_some(), "typing more ident chars reopens");
        s.handle(Ev::Text(" ".into()), 300);
        assert!(s.popup.is_none(), "a non-ident char closes it");
    }

    #[test]
    fn fim_context_carries_the_outline_and_caches() {
        let mut s = AppState::new(None, false);
        s.lang = aperture_core::syntax::Language::Rust;
        s.syntax.set_language(s.lang);
        typed(&mut s, "fn alpha() {}\nstruct Beta { x: u32 }\n", 0);
        let ctx = s.fim_context();
        assert!(ctx.contains("fn alpha()"), "{ctx}");
        assert!(ctx.contains("struct Beta"), "{ctx}");
        let again = s.fim_context();
        assert_eq!(ctx, again, "version-cached");
    }


    #[test]
    fn git_gutter_tracks_uncommitted_lines_and_refreshes_on_save() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("g.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        // Uncommitted edit on disk before the editor even opens it.
        std::fs::write(&f, "fn one() {}\nfn two() {}\n").unwrap();

        let mut s = AppState::new(Some(f.clone()), false);
        assert_eq!(s.git_changed, vec![(1, 2)], "picked up on open, before any keystroke");

        // An in-editor edit does NOT move the gutter until save (refresh is
        // save-triggered, not edit-triggered — no diff-per-keystroke).
        typed(&mut s, "fn three() {}\n", 0);
        assert_eq!(s.git_changed, vec![(1, 2)], "unchanged mid-edit");

        // save() just writes the file — it doesn't commit — so the gutter
        // still marks everything uncommitted, now including the typed line.
        s.save(100);
        assert_eq!(
            s.editor.buffer.to_string(),
            "fn three() {}\nfn one() {}\nfn two() {}\n",
        );
        assert_eq!(s.git_changed, vec![(0, 1), (2, 3)], "refreshed against the on-disk content");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_repo_no_git_status_zero_cost() {
        let s = AppState::new(None, false);
        assert!(s.git_changed.is_empty());
    }

    #[test]
    fn rev_step_previews_read_only_and_home_restores_exact_live_text() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-revstep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("r.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        std::fs::write(&f, "fn one() {}\nfn two() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "two"]);
        let head = String::from_utf8(Command::new("git").arg("-C").arg(&dir).args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap();
        git(&["update-ref", "refs/checkpoints/sess", head.trim()]);

        let mut s = AppState::new(Some(f.clone()), false);
        typed(&mut s, "fn three() {}\n", 0); // unsaved live edit, never committed
        let live_now = s.editor.buffer.to_string();
        assert_eq!(s.revs.len(), 3, "2 commits + 1 checkpoint");

        key(&mut s, K::Comma, true, 100); // older
        assert!(s.rev_ix.is_some());
        let ix0 = s.rev_ix.unwrap();

        // The view is READ-ONLY: typing is swallowed, not applied.
        let before = s.editor.buffer.to_string();
        typed(&mut s, "INTRUDER", 150);
        assert_eq!(s.editor.buffer.to_string(), before, "preview text is untouched by typing");

        // Ctrl+, / Ctrl+. still work while previewing (navigate the timeline).
        key(&mut s, K::Comma, true, 200);
        assert_ne!(s.rev_ix.unwrap(), ix0, "moved further back");
        key(&mut s, K::Period, true, 250);
        assert_eq!(s.rev_ix.unwrap(), ix0, "and back to the same spot");

        // Escape jumps all the way home in one gesture, regardless of depth.
        key(&mut s, K::Escape, false, 300);
        assert!(s.rev_ix.is_none());
        assert_eq!(s.editor.buffer.to_string(), live_now, "unsaved edit survived the round trip exactly");

        // Stepping past the newest revision also comes home.
        key(&mut s, K::Comma, true, 400);
        key(&mut s, K::Period, true, 450);
        assert!(s.rev_ix.is_none());
        assert_eq!(s.editor.buffer.to_string(), live_now);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rev_step_is_a_noop_without_a_repo() {
        let mut s = AppState::new(None, false);
        key(&mut s, K::Comma, true, 0);
        assert!(s.rev_ix.is_none());
    }

    #[test]
    fn describe_engine_fires_on_landing_and_clears_on_home() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-note-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("n.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);

        let mut s = AppState::new(Some(f), false);
        assert!(s.describe.is_none(), "no APERTURE_CMD in the test process: zero cost");
        // Inject a deterministic provider directly (avoids mutating process
        // env, which is global and unsafe to touch from a parallel test run).
        s.describe = Some(aperture_complete::engine::Engine::new(Box::new(
            aperture_complete::providers::CommandProvider::new(vec!["echo".into(), "-n".into(), "a tidy note".into()]),
        )));

        s.rev_step(1, 0);
        assert!(s.rev_ix.is_some());
        let mut got = false;
        for _ in 0..200 {
            s.poll_note();
            if s.rev_note == "a tidy note" {
                got = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got, "note arrived within 1s: {:?}", s.rev_note);

        s.rev_step(-1, 100); // home
        assert!(s.rev_ix.is_none());
        assert!(s.rev_note.is_empty(), "note clears on returning home");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn route_ai_fires_and_lands_through_a_real_engine() {
        let mut s = AppState::new(None, false);
        assert!(s.route_engine.is_none(), "no APERTURE_CMD in the test process: zero cost");
        // Same injection pattern as describe_engine_fires_on_landing_and_clears_on_home:
        // a real CommandProvider, avoiding global process-env mutation.
        s.route_engine = Some(aperture_complete::engine::Engine::new(Box::new(
            aperture_complete::providers::CommandProvider::new(vec!["echo".into(), "-n".into(), "1. wizard.rs — the account setup flow".into()]),
        )));

        let cands = vec![aperture_core::route::RouteCandidate {
            path: PathBuf::from("wizard.rs"),
            rel: "wizard.rs".to_string(),
            line: 4,
            kind: aperture_core::route::RouteKind::Symbol,
            label: "account_setup_wizard".to_string(),
            snippet: "fn account_setup_wizard() {}".to_string(),
            score: 5,
        }];
        s.request_route_ai("find the account setup wizard", &cands);
        assert!(s.route_pending_key.is_some(), "a request should be in flight");

        let briefs: Vec<aperture_complete::route_ai::CandidateBrief> = cands
            .iter()
            .map(|c| aperture_complete::route_ai::CandidateBrief { label: c.label.clone(), rel: c.rel.clone(), line: c.line, snippet: c.snippet.clone() })
            .collect();
        let key = aperture_complete::route_ai::route_key("find the account setup wizard", &briefs);
        assert!(aperture_complete::route_ai::in_flight(key), "cache should show the request in flight");

        let mut got = false;
        for _ in 0..200 {
            s.poll_route_ai();
            if aperture_complete::route_ai::route_summary(key).is_some() {
                got = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got, "the AI tier's answer should land within 1s");
        assert_eq!(aperture_complete::route_ai::route_summary(key).as_deref(), Some("1. wizard.rs — the account setup flow"));

        // A second call with the SAME query+candidates is idempotent — no
        // new request queued (would panic-free either way, but this checks
        // the dedup path doesn't error or double-queue).
        s.request_route_ai("find the account setup wizard", &cands);
        assert!(aperture_complete::route_ai::take_pending().is_none(), "no duplicate request queued for the same key");
    }

    #[test]
    fn ai_edit_selection_round_trips_from_request_to_accepted_buffer_change() {
        let mut s = AppState::new(None, false);
        assert!(s.edit_engine.is_none(), "no APERTURE_CMD in the test process: zero cost");
        s.editor = aperture_core::Editor::from_str("fn get(v: &[i32], i: usize) -> i32 { v[i] }\n");

        // No selection yet: request_edit should refuse, not panic or fire.
        s.request_edit("add bounds checking", 0);
        assert!(s.pending_edit.is_none(), "no selection to edit — request must be a no-op");

        // Select the whole function.
        let end = s.editor.buffer.len_chars() - 1; // exclude the trailing newline
        s.editor.sel = aperture_core::Selection { anchor: 0, head: end };

        s.edit_engine = Some(aperture_complete::engine::Engine::new(Box::new(
            aperture_complete::providers::CommandProvider::new(vec![
                "echo".into(), "-n".into(),
                "fn get(v: &[i32], i: usize) -> Option<i32> { v.get(i).copied() }".into(),
            ]),
        )));
        s.request_edit("add bounds checking", 0);
        assert!(s.pending_edit.is_some(), "a request in flight should be recorded");
        assert!(s.pending_edit.as_ref().unwrap().proposed.is_none(), "not landed yet");

        let mut got = false;
        for _ in 0..200 {
            s.poll_edit_ai(0);
            if s.pending_edit.as_ref().is_some_and(|p| p.proposed.is_some()) {
                got = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got, "the proposed edit should land within 1s");
        assert_eq!(s.pending_edit.as_ref().unwrap().proposed.as_deref(), Some("fn get(v: &[i32], i: usize) -> Option<i32> { v.get(i).copied() }"));

        s.accept_pending_edit();
        assert!(s.pending_edit.is_none(), "accepting clears the pending edit");
        assert!(s.editor.buffer.to_string().starts_with("fn get(v: &[i32], i: usize) -> Option<i32> { v.get(i).copied() }"));
        assert!(s.dirty, "accepting an edit marks the buffer dirty");
    }

    #[test]
    fn rejecting_a_pending_edit_never_touches_the_buffer() {
        let mut s = AppState::new(None, false);
        s.editor = aperture_core::Editor::from_str("original text\n");
        s.editor.sel = aperture_core::Selection { anchor: 0, head: 8 };
        s.edit_engine = Some(aperture_complete::engine::Engine::new(Box::new(
            aperture_complete::providers::CommandProvider::new(vec!["echo".into(), "-n".into(), "replaced".into()]),
        )));
        s.request_edit("rewrite this", 0);
        let mut got = false;
        for _ in 0..200 {
            s.poll_edit_ai(0);
            if s.pending_edit.as_ref().is_some_and(|p| p.proposed.is_some()) {
                got = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got);
        let before = s.editor.buffer.to_string();
        s.reject_pending_edit();
        assert!(s.pending_edit.is_none());
        assert_eq!(s.editor.buffer.to_string(), before, "rejecting must not change the buffer");
    }

    #[test]
    fn branch_from_here_only_acts_while_previewing() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-branch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("b.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "keep this idea"]);

        let mut s = AppState::new(Some(f), false);
        let branches_before = || {
            String::from_utf8(Command::new("git").arg("-C").arg(&dir).args(["branch", "--list"]).output().unwrap().stdout).unwrap()
        };
        // Live, not previewing: Ctrl+B must NOT branch HEAD by accident, and
        // must say so rather than silently doing nothing (a prior version of
        // this binding existed ONLY inside the preview-mode gate, so the key
        // was a true no-op while live — this assertion is what catches that).
        key(&mut s, K::B, true, 0);
        assert!(!branches_before().contains("keep-this"), "no branch created while live: {}", branches_before());
        assert!(s.status.as_ref().unwrap().0.contains("preview a revision first"), "the guard must SAY something, not just silently do nothing: {:?}", s.status);

        key(&mut s, K::Comma, true, 50); // enter preview
        key(&mut s, K::B, true, 100);
        let branches = branches_before();
        assert!(branches.contains("from/keep-this-idea-"), "branch created from the previewed revision: {branches}");
        assert!(s.status.as_ref().unwrap().0.starts_with("branched"));

        // Re-running the same branch collides; the failure surfaces plainly.
        key(&mut s, K::B, true, 150);
        assert!(s.status.as_ref().unwrap().0.contains("failed"), "{:?}", s.status);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn materialize_here_guards_live_and_checks_out_while_previewing() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-mat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("m.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "snapshot me"]);

        let mut s = AppState::new(Some(f), false);
        key(&mut s, K::M, true, 0);
        assert!(s.status.as_ref().unwrap().0.contains("preview a revision first"), "guarded while live");

        key(&mut s, K::Comma, true, 50);
        key(&mut s, K::M, true, 100);
        let (msg, _) = s.status.clone().unwrap();
        assert!(msg.starts_with("materialized at"), "{msg}");
        let path = msg.strip_prefix("materialized at ").unwrap();
        assert_eq!(std::fs::read_to_string(format!("{path}/m.rs")).unwrap(), "fn one() {}\n", "a real, separate checkout");
        assert_eq!(std::fs::read_to_string(dir.join("m.rs")).unwrap(), "fn one() {}\n", "the original worktree is untouched");

        let _ = Command::new("git").arg("-C").arg(&dir).args(["worktree", "remove", "--force", path]).output();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn ctrl_o_walks_the_git_root_and_populates_real_candidates() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-open-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("src/lib.rs"), "").unwrap();
        std::fs::write(dir.join("Cargo.toml"), "").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);

        // Open a file two directories below the git root — workspace_root()
        // should still resolve to the REPO root, not the file's own parent,
        // so the picker can find siblings elsewhere in the tree.
        let f = dir.join("src/lib.rs");
        let mut s = AppState::new(Some(f), false);
        assert_eq!(s.workspace_root(), dir, "prefers the git root over the file's own directory");

        key(&mut s, K::O, true, 0);
        let p = s.palette.as_ref().expect("palette opened");
        assert_eq!(p.mode, crate::palette_ui::Mode::OpenFile);
        let rels: Vec<String> = p.files.iter().map(|f| f.strip_prefix(&p.root).unwrap().to_string_lossy().into_owned()).collect();
        assert!(rels.contains(&"src/lib.rs".to_string()), "{rels:?}");
        assert!(rels.contains(&"Cargo.toml".to_string()), "{rels:?}");
        assert!(!rels.iter().any(|r| r.starts_with(".git")), "{rels:?}");

        let ranked = crate::file_finder::rank(&p.root, &p.files, "lib", 10);
        assert!(ranked.iter().any(|(_, rel)| rel == "src/lib.rs"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_root_falls_back_without_a_repo() {
        let s = AppState::new(None, false);
        // No path, no repo: falls back to cwd rather than panicking.
        assert_eq!(s.workspace_root(), std::env::current_dir().unwrap_or_default());
    }

    #[test]
    fn enter_between_auto_paired_braces_splits_through_the_real_key_path() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "fn main() ", 0);
        typed(&mut s, "{", 10); // auto-pairs to "{}"", caret between them
        assert_eq!(s.editor.buffer.to_string(), "fn main() {}");
        key(&mut s, K::Enter, false, 20);
        assert_eq!(s.editor.buffer.to_string(), "fn main() {\n    \n}");
        let pos = s.editor.caret_pos();
        assert_eq!((pos.line, pos.col), (1, 4), "caret on the empty middle line");
    }

    #[test]
    fn enter_is_swallowed_while_previewing_a_revision() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-enter-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("e.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);

        let mut s = AppState::new(Some(f), false);
        key(&mut s, K::Comma, true, 0); // enter preview
        assert!(s.rev_ix.is_some());
        let before = s.editor.buffer.to_string();
        key(&mut s, K::Enter, false, 50);
        assert_eq!(
            s.editor.buffer.to_string(), before,
            "Enter must not mutate a 'read-only' preview buffer"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn caret_from_click_accounts_for_wide_cjk_glyphs() {
        let mut s = AppState::new(None, false);
        // "a漢b" -- a(1 cell) [wide glyph](2 cells) b(1 cell): visual columns 0,1,3,4.
        typed(&mut s, "a漢b", 0);
        let char_w = 8.0;
        let line_h = 20.0;
        assert_eq!(s.caret_from_click(0.0, 0.0, line_h, char_w), 0);
        assert_eq!(s.caret_from_click(char_w * 1.0, 0.0, line_h, char_w), 1);
        assert_eq!(s.caret_from_click(char_w * 2.0, 0.0, line_h, char_w), 2);
        assert_eq!(s.caret_from_click(char_w * 3.0, 0.0, line_h, char_w), 2);
        assert_eq!(s.caret_from_click(char_w * 99.0, 0.0, line_h, char_w), 3);
    }

    #[test]
    fn caret_from_click_is_unchanged_for_plain_ascii() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "let x = 1;", 0);
        let char_w = 8.0;
        for col in 0..=10usize {
            assert_eq!(s.caret_from_click(col as f32 * char_w, 0.0, 20.0, char_w), col, "col {col}");
        }
    }

    #[test]
    fn find_replace_current_replaces_one_and_advances() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "cat cat cat", 0);
        s.editor.sel = Selection::caret(0);
        s.open_find_with_replace();
        s.handle(Ev::Text("cat".into()), 10);
        {
            let f = s.find.as_mut().unwrap();
            f.focus = FindFocus::Replace;
        }
        s.handle(Ev::Text("dog".into()), 20);
        key(&mut s, K::Enter, false, 30); // replace current, jump to next
        assert_eq!(s.editor.buffer.to_string(), "dog cat cat", "only the first match replaced");
        assert_eq!(s.find.as_ref().unwrap().cur, 0, "now pointing at the (new) first remaining match");
    }

    #[test]
    fn find_replace_all_is_one_undo_step() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "cat cat cat", 0);
        s.editor.sel = Selection::caret(0);
        s.open_find_with_replace();
        s.handle(Ev::Text("cat".into()), 10);
        { s.find.as_mut().unwrap().focus = FindFocus::Replace; }
        s.handle(Ev::Text("dog".into()), 20);
        s.handle(Ev::Key { key: K::Enter, ctrl: true, shift: false, alt: false }, 30);
        assert_eq!(s.editor.buffer.to_string(), "dog dog dog");
        assert!(s.status.as_ref().unwrap().0.contains("replaced 3"));
        s.editor.undo();
        assert_eq!(s.editor.buffer.to_string(), "cat cat cat", "one undo restores every replacement");
    }

    #[test]
    fn find_tab_toggles_focus_and_routes_typing_correctly() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "abc", 0);
        s.open_find_with_replace();
        assert_eq!(s.find.as_ref().unwrap().focus, FindFocus::Query);
        s.handle(Ev::Text("a".into()), 10);
        assert_eq!(s.find.as_ref().unwrap().query, "a", "types into query by default");
        key(&mut s, K::Tab, false, 20);
        assert_eq!(s.find.as_ref().unwrap().focus, FindFocus::Replace);
        s.handle(Ev::Text("X".into()), 30);
        assert_eq!(s.find.as_ref().unwrap().query, "a", "query untouched once focus moved");
        assert_eq!(s.find.as_ref().unwrap().replace.as_deref(), Some("X"));
        key(&mut s, K::Tab, false, 40);
        assert_eq!(s.find.as_ref().unwrap().focus, FindFocus::Query);
    }

    #[test]
    fn plain_ctrl_f_still_has_no_replace_field_at_all() {
        // Ctrl+F alone must be byte-for-byte the old find-only behavior.
        let mut s = AppState::new(None, false);
        typed(&mut s, "abc", 0);
        s.open_find();
        assert!(s.find.as_ref().unwrap().replace.is_none());
        key(&mut s, K::Tab, false, 0); // Tab is a no-op without a replace field
        assert_eq!(s.find.as_ref().unwrap().focus, FindFocus::Query);
    }

    #[test]
    fn replace_all_with_no_matches_does_nothing() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "hello", 0);
        s.editor.sel = Selection::caret(0);
        s.open_find_with_replace();
        s.handle(Ev::Text("zzz".into()), 10);
        { s.find.as_mut().unwrap().focus = FindFocus::Replace; }
        s.handle(Ev::Text("q".into()), 20);
        s.handle(Ev::Key { key: K::Enter, ctrl: true, shift: false, alt: false }, 30);
        assert_eq!(s.editor.buffer.to_string(), "hello", "nothing to replace, nothing changes");
    }

    #[test]
    fn find_history_commits_on_close_and_cycles_with_ctrl_arrows() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "alpha beta gamma", 0);
        s.editor.sel = Selection::caret(0);

        s.open_find();
        s.handle(Ev::Text("alpha".into()), 10);
        key(&mut s, K::Escape, false, 20); // commits "alpha" to history
        assert_eq!(s.find_history, vec!["alpha".to_string()]);
        assert!(s.find.is_none());

        // find_jump's own side effect selects the match text; open_find
        // seeds from a non-caret selection on purpose (search-the-selection
        // is an existing, intentional feature). Reset to a caret so THIS
        // session starts from an empty query, same as a user clicking
        // elsewhere before searching again.
        s.editor.sel = Selection::caret(0);
        s.open_find();
        s.handle(Ev::Text("beta".into()), 30);
        key(&mut s, K::Escape, false, 40);
        assert_eq!(s.find_history, vec!["alpha".to_string(), "beta".to_string()]);

        // Fresh session: Ctrl+Up recalls the most recent entry first.
        s.editor.sel = Selection::caret(0);
        s.open_find();
        s.handle(Ev::Key { key: K::Up, ctrl: true, shift: false, alt: false }, 50);
        assert_eq!(s.find.as_ref().unwrap().query, "beta");
        s.handle(Ev::Key { key: K::Up, ctrl: true, shift: false, alt: false }, 60);
        assert_eq!(s.find.as_ref().unwrap().query, "alpha", "older still");
        s.handle(Ev::Key { key: K::Up, ctrl: true, shift: false, alt: false }, 70);
        assert_eq!(s.find.as_ref().unwrap().query, "alpha", "clamped at the oldest entry");
        s.handle(Ev::Key { key: K::Down, ctrl: true, shift: false, alt: false }, 80);
        assert_eq!(s.find.as_ref().unwrap().query, "beta", "newer");
        s.handle(Ev::Key { key: K::Down, ctrl: true, shift: false, alt: false }, 90);
        assert_eq!(s.find.as_ref().unwrap().query, "", "one past the newest: blank, shell-history style");

        // Typing resets history browsing to "live", not editing history in place.
        s.handle(Ev::Key { key: K::Up, ctrl: true, shift: false, alt: false }, 100);
        assert_eq!(s.find.as_ref().unwrap().query, "beta");
        s.handle(Ev::Text("!".into()), 110);
        assert_eq!(s.find.as_ref().unwrap().query, "beta!");
        key(&mut s, K::Escape, false, 120);
        assert_eq!(s.find_history, vec!["alpha".to_string(), "beta".to_string(), "beta!".to_string()]);
    }

    #[test]
    fn find_history_ignores_empty_and_consecutive_duplicate_queries() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "x", 0);
        s.editor.sel = Selection::caret(0);

        s.open_find();
        key(&mut s, K::Escape, false, 10); // never typed anything: no entry
        assert!(s.find_history.is_empty());

        s.open_find();
        s.handle(Ev::Text("x".into()), 20);
        key(&mut s, K::Escape, false, 30);
        s.editor.sel = Selection::caret(0);
        s.open_find();
        s.handle(Ev::Text("x".into()), 40);
        key(&mut s, K::Escape, false, 50);
        assert_eq!(s.find_history, vec!["x".to_string()], "immediate repeat isn't duplicated");
    }

    #[test]
    fn find_history_arrows_are_a_noop_with_no_history() {
        let mut s = AppState::new(None, false);
        s.open_find();
        s.handle(Ev::Key { key: K::Up, ctrl: true, shift: false, alt: false }, 0);
        assert_eq!(s.find.as_ref().unwrap().query, "");
    }

    #[test]
    fn margin_note_truncates_to_the_streetsign_budget_not_an_arbitrary_number() {
        let mut s = AppState::new(None, false);
        let long = "x".repeat(200);
        s.describe = Some(aperture_complete::engine::Engine::new(Box::new(
            aperture_complete::providers::CommandProvider::new(vec!["echo".into(), "-n".into(), long.clone()]),
        )));
        // request_describe needs git+rev_ix context to fire; drive it the
        // same way the other describe test does — a real repo, a real
        // revision landed on.
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-margin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("m.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);
        let mut s2 = AppState::new(Some(f), false);
        s2.describe = s.describe.take();

        s2.rev_step(1, 0);
        let mut got = false;
        for _ in 0..200 {
            s2.poll_note();
            if !s2.rev_note.is_empty() {
                got = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(got, "note arrived");
        assert_eq!(s2.rev_note.chars().count(), 60, "truncated to the StreetSign budget, not 160 or the full 200");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preedit_shows_composing_text_and_dismisses_ghost_and_popup() {
        let mut s = AppState::new(None, false);
        typed(&mut s, "fn hello_world() {}\n", 0);
        for ch in "hel".chars() {
            s.handle(Ev::Text(ch.to_string()), 10);
        }
        assert!(s.popup.is_some(), "auto-triggered before composition starts");
        s.handle(Ev::Preedit("あ".into()), 10); // hiragana 'a'
        assert_eq!(s.preedit, "あ");
        assert!(s.popup.is_none(), "composing text and a suggestion popup don't coexist");
    }

    #[test]
    fn preedit_ignores_literal_newline_and_carriage_return() {
        let mut s = AppState::new(None, false);
        s.handle(Ev::Preedit("\n".into()), 0);
        assert_eq!(s.preedit, "", "a literal newline in preedit text is a platform quirk, not real composition");
        s.handle(Ev::Preedit("\r".into()), 0);
        assert_eq!(s.preedit, "");
        s.handle(Ev::Preedit("real".into()), 0);
        assert_eq!(s.preedit, "real", "ordinary preedit text still works");
    }

    #[test]
    fn preedit_empty_string_clears_it() {
        let mut s = AppState::new(None, false);
        s.handle(Ev::Preedit("composing".into()), 0);
        assert_eq!(s.preedit, "composing");
        s.handle(Ev::Preedit("".into()), 10);
        assert_eq!(s.preedit, "", "an empty preedit clears the overlay (the 'composition cancelled' signal)");
    }

    #[test]
    fn a_real_text_insertion_always_clears_any_pending_preedit() {
        let mut s = AppState::new(None, false);
        s.handle(Ev::Preedit("composing".into()), 0);
        assert_eq!(s.preedit, "composing");
        // The commit itself routes through plain Ev::Text (see view.rs's
        // translation of ImeEvent::Commit) -- the SAME path a keystroke
        // takes, and it must close out the composition either way.
        s.handle(Ev::Text("finished".into()), 10);
        assert_eq!(s.preedit, "", "committing (or just typing) always ends composition");
        assert!(s.editor.buffer.to_string().contains("finished"));
    }

    #[test]
    fn preedit_is_swallowed_while_previewing_a_revision() {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!("aperture-egui-preedit-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        let f = dir.join("p.rs");
        std::fs::write(&f, "fn one() {}\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "one"]);

        let mut s = AppState::new(Some(f), false);
        key(&mut s, K::Comma, true, 0); // enter preview
        assert!(s.rev_ix.is_some());
        s.handle(Ev::Preedit("composing".into()), 50);
        assert_eq!(s.preedit, "", "composing into a read-only buffer is swallowed, same as typing");
        let _ = std::fs::remove_dir_all(&dir);
    }

}
